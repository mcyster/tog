use std::fmt::{Display, Formatter};
use std::io;
use std::process::{ExitStatus, Stdio};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use uuid::Uuid;

use tog::conversation::{FailureCategory, OperationFailure};

use super::stream::{SPOOL_MEMORY_THRESHOLD_BYTES, SharedStream, StreamRead};
use super::{
    ExecutionState, ExecutionStatus, ShellExecutionError, ShellExecutionErrorKind, ShellExitStatus,
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct ExecutionId(Uuid);

impl ExecutionId {
    pub(super) fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Display for ExecutionId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "exec_{}", self.0.simple())
    }
}

impl FromStr for ExecutionId {
    type Err = ();

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let uuid_text = text.strip_prefix("exec_").ok_or(())?;
        Uuid::parse_str(uuid_text).map(Self).map_err(|_| ())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum WaitPolicy {
    WaitForCompletion,
    WaitForOutputOrCompletion,
}

pub(super) struct Execution {
    standard_output: Arc<SharedStream>,
    standard_error: Arc<SharedStream>,
    combined_output: Arc<SharedStream>,
    process_id: u32,
    termination: watch::Sender<bool>,
    signals: ExecutionSignals,
    completion: Mutex<Completion>,
}

impl Execution {
    pub(super) fn spawn(
        commands: String,
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
        let (termination, termination_receiver) = watch::channel(false);
        let execution = Arc::new(Execution {
            standard_output: Arc::clone(&standard_output),
            standard_error: Arc::clone(&standard_error),
            combined_output: Arc::clone(&combined_output),
            process_id,
            termination,
            signals: ExecutionSignals::default(),
            completion: Mutex::new(Completion::default()),
        });

        let standard_output_drain = tokio::spawn(drain_output(
            stdout_pipe,
            Arc::clone(&standard_output),
            Arc::clone(&combined_output),
            Arc::clone(&execution),
        ));
        let standard_error_drain = tokio::spawn(drain_output(
            stderr_pipe,
            Arc::clone(&standard_error),
            Arc::clone(&combined_output),
            Arc::clone(&execution),
        ));
        tokio::spawn(supervise(
            child,
            termination_receiver,
            standard_output_drain,
            standard_error_drain,
            Arc::clone(&execution),
            timeout,
        ));
        Ok(execution)
    }

    pub(super) fn standard_output(&self) -> Arc<SharedStream> {
        Arc::clone(&self.standard_output)
    }

    pub(super) fn standard_error(&self) -> Arc<SharedStream> {
        Arc::clone(&self.standard_error)
    }

    pub(super) fn combined_output(&self) -> Arc<SharedStream> {
        Arc::clone(&self.combined_output)
    }

    pub(super) fn request_cancel(&self) {
        self.signals.cancelled.store(true, Ordering::SeqCst);
        let _ = self.termination.send(true);
    }

    pub(super) async fn wait_for_state(
        &self,
        offset: u64,
        output_limit: u64,
        wait: Duration,
        policy: WaitPolicy,
    ) -> ExecutionState {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            if self.completion_snapshot().status == ExecutionStatus::Complete {
                break;
            }
            if policy == WaitPolicy::WaitForOutputOrCompletion
                && self.combined_output.available(offset)
            {
                break;
            }
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

    fn wants_termination(&self) -> bool {
        self.signals.cancelled.load(Ordering::SeqCst)
            || self.signals.timed_out.load(Ordering::SeqCst)
            || self.signals.capture_failed.load(Ordering::SeqCst)
    }

    fn note_timed_out(&self) {
        self.signals.timed_out.store(true, Ordering::SeqCst);
    }

    fn note_capture_failure(&self) {
        self.signals.capture_failed.store(true, Ordering::SeqCst);
        let _ = self.termination.send(true);
    }

    fn mark_completed(&self, natural_exit: Option<ExitStatus>) {
        let mut completion = self
            .completion
            .lock()
            .expect("the shell completion should lock");
        completion.status = ExecutionStatus::Complete;
        if self.signals.capture_failed.load(Ordering::SeqCst) {
            completion.error = Some(ShellExecutionError {
                kind: ShellExecutionErrorKind::CaptureFailed,
                message: "the shell output could not be captured".to_owned(),
            });
        } else if self.signals.cancelled.load(Ordering::SeqCst) {
            completion.error = Some(ShellExecutionError {
                kind: ShellExecutionErrorKind::Cancelled,
                message: "the shell execution was cancelled".to_owned(),
            });
        } else if self.signals.timed_out.load(Ordering::SeqCst) {
            completion.error = Some(ShellExecutionError {
                kind: ShellExecutionErrorKind::TimedOut,
                message: "the shell execution exceeded its timeout".to_owned(),
            });
        } else if let Some(exit_status) = natural_exit {
            completion.exit_status = Some(shell_exit_status(exit_status));
        }
    }

    fn release_streams(&self, standard_output: DrainResult, standard_error: DrainResult) {
        if matches!(standard_output, DrainResult::CaptureFailed)
            || matches!(standard_error, DrainResult::CaptureFailed)
        {
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

#[derive(Default)]
struct ExecutionSignals {
    cancelled: AtomicBool,
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
    termination: watch::Receiver<bool>,
    standard_output_drain: JoinHandle<DrainResult>,
    standard_error_drain: JoinHandle<DrainResult>,
    execution: Arc<Execution>,
    timeout: Duration,
) {
    let timeout_deadline = tokio::time::Instant::now() + timeout;
    let mut termination = termination;
    let natural_exit = loop {
        if execution.wants_termination() {
            kill_process_group(execution.process_id);
        }
        let remaining = timeout_deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            execution.note_timed_out();
            kill_process_group(execution.process_id);
            break None;
        }
        tokio::select! {
            status = child.wait() => break status.ok(),
            _ = termination.changed() => {}
            () = tokio::time::sleep(remaining) => execution.note_timed_out(),
        }
    };
    let natural_exit = match natural_exit {
        Some(status) => Some(status),
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
    execution: Arc<Execution>,
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
                    execution.note_capture_failure();
                    return DrainResult::CaptureFailed;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => {
                execution.note_capture_failure();
                return DrainResult::CaptureFailed;
            }
        }
    }
    DrainResult::Finished
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

pub(super) fn invalid_arguments(message: String) -> OperationFailure {
    OperationFailure::try_new(FailureCategory::InvalidArguments, message, None)
        .expect("the invalid-argument message should be valid")
}

pub(super) fn execution_failed(message: String) -> OperationFailure {
    OperationFailure::try_new(FailureCategory::ExecutionFailed, message, None)
        .expect("the execution failure message should be valid")
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::ExecutionId;

    #[test]
    fn execution_identifiers_parse_from_and_display_their_prefix_form() {
        let identifier = ExecutionId::new();

        assert!(identifier.to_string().starts_with("exec_"));
        assert_eq!(
            ExecutionId::from_str(&identifier.to_string()),
            Ok(identifier)
        );
        assert!(ExecutionId::from_str("stream_foo").is_err());
        assert!(ExecutionId::from_str("exec_not-a-uuid").is_err());
        assert!(ExecutionId::from_str("exec_123").is_err());
    }

    #[test]
    fn execution_identifiers_do_not_repeat() {
        let first = ExecutionId::new();
        let second = ExecutionId::new();

        assert_ne!(first, second);
        assert_ne!(first.to_string(), second.to_string());
    }
}
