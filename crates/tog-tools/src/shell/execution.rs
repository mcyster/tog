use std::fmt::{Display, Formatter};
use std::io;
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;

use tog::conversation::{FailureCategory, OperationFailure};

use super::stream::{SPOOL_MEMORY_THRESHOLD_BYTES, SharedStream, StreamRead};
use super::{
    ExecutionState, ExecutionStatus, ShellExecutionError, ShellExecutionErrorKind, ShellExitStatus,
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct ExecutionId(u64);

impl ExecutionId {
    pub(crate) fn new(value: u64) -> Self {
        Self(value)
    }
}

impl Display for ExecutionId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "exec_{}", self.0)
    }
}

impl FromStr for ExecutionId {
    type Err = ();

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let digits = text.strip_prefix("exec_").ok_or(())?;
        let value = digits.parse::<u64>().map_err(|_| ())?;
        Ok(Self(value))
    }
}

pub(crate) struct Execution {
    standard_output: Arc<SharedStream>,
    standard_error: Arc<SharedStream>,
    combined_output: Arc<SharedStream>,
    process_id: u32,
    cancellation: Arc<Cancellation>,
    completion: Mutex<Completion>,
}

impl Execution {
    pub(crate) fn standard_output(&self) -> Arc<SharedStream> {
        Arc::clone(&self.standard_output)
    }

    pub(crate) fn standard_error(&self) -> Arc<SharedStream> {
        Arc::clone(&self.standard_error)
    }

    pub(crate) fn combined_output(&self) -> Arc<SharedStream> {
        Arc::clone(&self.combined_output)
    }

    pub(crate) fn request_cancel(&self) {
        self.cancellation.requested.store(true, Ordering::SeqCst);
        kill_process_group(self.process_id);
    }

    pub(crate) async fn state_at(
        &self,
        offset: u64,
        output_limit: u64,
        wait: Option<Duration>,
        return_on_output: bool,
    ) -> ExecutionState {
        let deadline = wait.map(|duration| tokio::time::Instant::now() + duration);
        loop {
            if self.completion_snapshot().status == ExecutionStatus::Complete {
                break;
            }
            if return_on_output && self.combined_output.available(offset) {
                break;
            }
            let Some(deadline) = deadline else {
                break;
            };
            let now = tokio::time::Instant::now();
            if now >= deadline {
                break;
            }
            if tokio::time::timeout(
                deadline.saturating_duration_since(now),
                self.combined_output.follow(),
            )
            .await
            .is_err()
            {
                break;
            }
        }
        let read = self.combined_output.read(offset, output_limit).await;
        build_state(self, offset, read, self.completion_snapshot())
    }

    fn completion_snapshot(&self) -> CompletionSnapshot {
        let completion = self
            .completion
            .lock()
            .expect("the shell completion should lock");
        CompletionSnapshot {
            status: completion.status,
            exit_status: completion.exit_status,
            error: completion.error.clone(),
        }
    }

    fn mark_completed(&self, natural_exit: Option<ExitStatus>) {
        let mut completion = self
            .completion
            .lock()
            .expect("the shell completion should lock");
        completion.status = ExecutionStatus::Complete;
        if self.cancellation.requested.load(Ordering::SeqCst) {
            completion.error = Some(ShellExecutionError {
                kind: ShellExecutionErrorKind::Cancelled,
                message: "the shell execution was cancelled".to_owned(),
            });
        } else if self.cancellation.timed_out.load(Ordering::SeqCst) {
            completion.error = Some(ShellExecutionError {
                kind: ShellExecutionErrorKind::TimedOut,
                message: "the shell execution exceeded its timeout".to_owned(),
            });
        } else if let Some(exit_status) = natural_exit {
            completion.exit_status = Some(shell_exit_status(exit_status));
        }
    }

    fn release_streams(
        &self,
        standard_output_result: DrainResult,
        standard_error_result: DrainResult,
    ) {
        let capture_failed = matches!(standard_output_result, DrainResult::CaptureFailed)
            || matches!(standard_error_result, DrainResult::CaptureFailed);
        if capture_failed {
            let mut completion = self
                .completion
                .lock()
                .expect("the shell completion should lock");
            completion.exit_status = None;
            completion.error = Some(ShellExecutionError {
                kind: ShellExecutionErrorKind::CaptureFailed,
                message: "the shell output could not be captured".to_owned(),
            });
        }
        self.standard_output.close();
        self.standard_error.close();
        self.combined_output.close();
    }
}

pub(crate) async fn spawn(
    commands: String,
    directory: Option<PathBuf>,
    timeout: Duration,
) -> Result<Arc<Execution>, OperationFailure> {
    let mut command = Command::new("/bin/sh");
    command
        .arg("-c")
        .arg(commands)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    if let Some(directory) = directory {
        command.current_dir(directory);
    }
    let mut child = command.spawn().map_err(|error| {
        execution_failed(format!("the shell process could not be started: {error}"))
    })?;
    let process_id = child.id().ok_or_else(|| {
        execution_failed("the shell process reported no identifier after starting".to_owned())
    })?;
    let stdout_pipe = take_pipe(child.stdout.take(), "standard output")?;
    let stderr_pipe = take_pipe(child.stderr.take(), "standard error")?;

    let standard_output = Arc::new(SharedStream::new(SPOOL_MEMORY_THRESHOLD_BYTES));
    let standard_error = Arc::new(SharedStream::new(SPOOL_MEMORY_THRESHOLD_BYTES));
    let combined_output = Arc::new(SharedStream::new(SPOOL_MEMORY_THRESHOLD_BYTES));
    let cancellation = Arc::new(Cancellation::default());
    let execution = Arc::new(Execution {
        standard_output: Arc::clone(&standard_output),
        standard_error: Arc::clone(&standard_error),
        combined_output: Arc::clone(&combined_output),
        process_id,
        cancellation: Arc::clone(&cancellation),
        completion: Mutex::new(Completion::default()),
    });

    let standard_output_drain = tokio::spawn(drain_output(
        stdout_pipe,
        Arc::clone(&standard_output),
        Arc::clone(&combined_output),
        Arc::clone(&cancellation),
        process_id,
    ));
    let standard_error_drain = tokio::spawn(drain_output(
        stderr_pipe,
        Arc::clone(&standard_error),
        Arc::clone(&combined_output),
        Arc::clone(&cancellation),
        process_id,
    ));
    tokio::spawn(supervise(
        child,
        standard_output_drain,
        standard_error_drain,
        Arc::clone(&execution),
        timeout,
    ));
    Ok(execution)
}

#[derive(Default)]
struct Cancellation {
    requested: AtomicBool,
    timed_out: AtomicBool,
    capture_failed: AtomicBool,
}

#[derive(Default)]
struct Completion {
    status: ExecutionStatus,
    exit_status: Option<ShellExitStatus>,
    error: Option<ShellExecutionError>,
}

#[derive(Clone)]
struct CompletionSnapshot {
    status: ExecutionStatus,
    exit_status: Option<ShellExitStatus>,
    error: Option<ShellExecutionError>,
}

enum DrainResult {
    Finished,
    CaptureFailed,
}

async fn supervise(
    mut child: Child,
    standard_output_drain: JoinHandle<DrainResult>,
    standard_error_drain: JoinHandle<DrainResult>,
    execution: Arc<Execution>,
    timeout: Duration,
) {
    let natural_exit = tokio::select! {
        status = child.wait() => Some(status),
        () = tokio::time::sleep(timeout) => {
            execution.cancellation.timed_out.store(true, Ordering::SeqCst);
            kill_process_group(execution.process_id);
            None
        }
    };
    let natural_exit = match natural_exit {
        Some(status) => status.ok(),
        None => {
            let _ = child.wait().await;
            None
        }
    };
    execution.mark_completed(natural_exit);
    let standard_output = standard_output_drain
        .await
        .unwrap_or(DrainResult::CaptureFailed);
    let standard_error = standard_error_drain
        .await
        .unwrap_or(DrainResult::CaptureFailed);
    execution.release_streams(standard_output, standard_error);
}

async fn drain_output<Reader>(
    mut reader: Reader,
    target: Arc<SharedStream>,
    combined: Arc<SharedStream>,
    cancellation: Arc<Cancellation>,
    process_id: u32,
) -> DrainResult
where
    Reader: AsyncRead + Unpin,
{
    let mut buffer = [0_u8; 8192];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) => break,
            Ok(count) => {
                let chunk = buffer[..count].to_vec();
                if combined.append(chunk.clone()).await.is_err()
                    || target.append(chunk).await.is_err()
                {
                    return fail_capture(&cancellation, process_id);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return fail_capture(&cancellation, process_id),
        }
    }
    DrainResult::Finished
}

fn fail_capture(cancellation: &Cancellation, process_id: u32) -> DrainResult {
    cancellation.capture_failed.store(true, Ordering::SeqCst);
    kill_process_group(process_id);
    DrainResult::CaptureFailed
}

fn build_state(
    execution: &Execution,
    offset: u64,
    read: StreamRead,
    completion: CompletionSnapshot,
) -> ExecutionState {
    ExecutionState {
        output: String::from_utf8_lossy(&read.bytes).into_owned(),
        next_offset: offset + read.bytes.len() as u64,
        output_eof: execution.combined_output.is_closed() && !read.more,
        status: completion.status,
        exit_status: completion.exit_status,
        error: completion.error,
    }
}

fn take_pipe<Reader>(pipe: Option<Reader>, stream_name: &str) -> Result<Reader, OperationFailure> {
    pipe.ok_or_else(|| {
        execution_failed(format!(
            "the shell {stream_name} pipe was not captured after spawning"
        ))
    })
}

fn kill_process_group(process_id: u32) {
    let process_group = nix::unistd::Pid::from_raw(process_id as i32);
    let _ = nix::sys::signal::killpg(process_group, nix::sys::signal::Signal::SIGKILL);
}

fn shell_exit_status(exit_status: ExitStatus) -> ShellExitStatus {
    use std::os::unix::process::ExitStatusExt;

    match exit_status.code() {
        Some(code) => ShellExitStatus::Exited { code },
        None => ShellExitStatus::Signaled {
            signal: exit_status
                .signal()
                .expect("a Unix exit status without an exit code was signal termination"),
        },
    }
}

pub(crate) fn invalid_arguments(message: String) -> OperationFailure {
    OperationFailure::try_new(FailureCategory::InvalidArguments, message, None)
        .expect("the invalid-argument message should be valid")
}

pub(crate) fn execution_failed(message: String) -> OperationFailure {
    OperationFailure::try_new(FailureCategory::ExecutionFailed, message, None)
        .expect("the execution failure message should be valid")
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::ExecutionId;

    #[test]
    fn execution_identifiers_parse_from_and_display_their_prefix_form() {
        let identifier = ExecutionId::new(2);

        assert_eq!(identifier.to_string(), "exec_2");
        assert_eq!(ExecutionId::from_str("exec_2"), Ok(identifier));
        assert!(ExecutionId::from_str("stream_2").is_err());
        assert!(ExecutionId::from_str("exec_abc").is_err());
    }
}
