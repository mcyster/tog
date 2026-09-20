use std::io;
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use futures_util::FutureExt;
use futures_util::future::BoxFuture;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;

use super::ExecutableTool;
use crate::conversation_event::{ToolDefinition, ToolExecutionProblem, ToolName};

pub(crate) const DEFAULT_TIMEOUT_SECONDS: u64 = 30;
pub(crate) const MAXIMUM_RETAINED_OUTPUT_BYTES: usize = 64 * 1024;

pub(crate) struct ShellTool {
    definition: ToolDefinition,
}

impl ShellTool {
    pub(crate) fn new() -> Self {
        Self {
            definition: ToolDefinition::try_new(
                ToolName::try_new("shell".to_owned())
                    .expect("the shell tool name should be valid"),
                "Run a command with /bin/sh -c in an optional working directory and return captured standard output, standard error, and exit status.".to_owned(),
                schema_for!(ShellCommandParameters),
                schema_for!(ShellCommandResult),
            )
            .expect("the shell tool definition should be valid"),
        }
    }
}

impl ExecutableTool for ShellTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute<'execute>(
        &'execute self,
        arguments: Value,
    ) -> BoxFuture<'execute, Result<Value, ToolExecutionProblem>> {
        async move {
            let parameters: ShellCommandParameters = serde_json::from_value(arguments)
                .map_err(|error| invalid_arguments(format!("invalid shell arguments: {error}")))?;
            let result = run_shell_command(parameters).await?;
            serde_json::to_value(result).map_err(|error| {
                execution_failed(format!("the shell result could not be serialized: {error}"))
            })
        }
        .boxed()
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ShellCommandParameters {
    command: String,
    #[serde(default)]
    working_directory: Option<PathBuf>,
    #[serde(default)]
    timeout_seconds: Option<u64>,
}

#[derive(Debug, JsonSchema, Serialize)]
pub(crate) struct ShellCommandResult {
    stdout: String,
    stderr: String,
    exit_status: ShellExitStatus,
    stdout_truncated: bool,
    stderr_truncated: bool,
}

impl ShellCommandResult {
    fn from_exit_status(
        exit_status: ExitStatus,
        stdout: CapturedOutput,
        stderr: CapturedOutput,
    ) -> Self {
        Self {
            stdout: stdout.text,
            stderr: stderr.text,
            exit_status: shell_exit_status(exit_status),
            stdout_truncated: stdout.truncated,
            stderr_truncated: stderr.truncated,
        }
    }
}

#[derive(Debug, JsonSchema, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ShellExitStatus {
    Exited { code: i32 },
    Signaled { signal: i32 },
}

#[derive(Debug, Serialize)]
struct ShellTimeoutDetails {
    timeout_seconds: u64,
    stdout: String,
    stderr: String,
    stdout_truncated: bool,
    stderr_truncated: bool,
}

async fn run_shell_command(
    parameters: ShellCommandParameters,
) -> Result<ShellCommandResult, ToolExecutionProblem> {
    if parameters.command.trim().is_empty() {
        return Err(invalid_arguments(
            "the shell command must not be blank".to_owned(),
        ));
    }
    let timeout_seconds = parameters
        .timeout_seconds
        .unwrap_or(DEFAULT_TIMEOUT_SECONDS);
    let mut command = Command::new("/bin/sh");
    command
        .arg("-c")
        .arg(&parameters.command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    if let Some(working_directory) = &parameters.working_directory {
        command.current_dir(working_directory);
    }
    let mut child = command.spawn().map_err(|error| {
        execution_failed(format!("the shell process could not be started: {error}"))
    })?;
    let process_id = child.id();
    let stdout = take_pipe(child.stdout.take(), "standard output")?;
    let stderr = take_pipe(child.stderr.take(), "standard error")?;
    let stdout_capture = tokio::spawn(capture_stream(stdout));
    let stderr_capture = tokio::spawn(capture_stream(stderr));

    let exit_status = tokio::select! {
        status = child.wait() => Some(status),
        () = tokio::time::sleep(Duration::from_secs(timeout_seconds)) => None,
    };

    match exit_status {
        Some(status) => {
            let status = status.map_err(|error| {
                execution_failed(format!("the shell process could not be awaited: {error}"))
            })?;
            let stdout = join_capture(stdout_capture, "standard output").await?;
            let stderr = join_capture(stderr_capture, "standard error").await?;
            Ok(ShellCommandResult::from_exit_status(status, stdout, stderr))
        }
        None => {
            terminate_process_group(process_id, &mut child);
            let stdout = join_capture(stdout_capture, "standard output").await?;
            let stderr = join_capture(stderr_capture, "standard error").await?;
            let _ = child.wait().await;
            Err(shell_timed_out(timeout_seconds, stdout, stderr))
        }
    }
}

fn shell_timed_out(
    timeout_seconds: u64,
    stdout: CapturedOutput,
    stderr: CapturedOutput,
) -> ToolExecutionProblem {
    let details = ShellTimeoutDetails {
        timeout_seconds,
        stdout: stdout.text,
        stderr: stderr.text,
        stdout_truncated: stdout.truncated,
        stderr_truncated: stderr.truncated,
    };
    ToolExecutionProblem::try_timed_out(
        format!("the shell command timed out after {timeout_seconds} seconds"),
        Some(serde_json::to_value(details).expect("the shell timeout details should serialize")),
    )
    .expect("the shell timeout problem should be valid")
}

fn take_pipe<Stream>(
    stream: Option<Stream>,
    stream_name: &str,
) -> Result<Stream, ToolExecutionProblem> {
    stream.ok_or_else(|| {
        execution_failed(format!(
            "the shell {stream_name} pipe was not captured after spawning"
        ))
    })
}

fn terminate_process_group(process_id: Option<u32>, child: &mut Child) {
    match process_id {
        Some(process_id) => {
            let process_group = nix::unistd::Pid::from_raw(process_id as i32);
            let _ = nix::sys::signal::killpg(process_group, nix::sys::signal::Signal::SIGKILL);
        }
        None => {
            let _ = child.start_kill();
        }
    }
}

async fn join_capture(
    capture: JoinHandle<CapturedOutput>,
    stream_name: &str,
) -> Result<CapturedOutput, ToolExecutionProblem> {
    capture.await.map_err(|error| {
        execution_failed(format!("the shell {stream_name} reader failed: {error}"))
    })
}

#[derive(Default)]
struct CapturedOutput {
    text: String,
    truncated: bool,
}

async fn capture_stream<Reader>(mut reader: Reader) -> CapturedOutput
where
    Reader: AsyncRead + Unpin,
{
    let mut retained = Vec::new();
    let mut truncated = false;
    let mut buffer = [0_u8; 8192];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) => break,
            Ok(count) => {
                let available = MAXIMUM_RETAINED_OUTPUT_BYTES.saturating_sub(retained.len());
                let retained_count = count.min(available);
                retained.extend_from_slice(&buffer[..retained_count]);
                if retained_count < count {
                    truncated = true;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => {
                truncated = true;
                break;
            }
        }
    }
    CapturedOutput {
        text: String::from_utf8_lossy(&retained).into_owned(),
        truncated,
    }
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

fn invalid_arguments(message: String) -> ToolExecutionProblem {
    ToolExecutionProblem::try_invalid_arguments(message)
        .expect("the invalid-argument message should be valid")
}

fn execution_failed(message: String) -> ToolExecutionProblem {
    ToolExecutionProblem::try_execution_failed(message)
        .expect("the execution failure message should be valid")
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{ExecutableTool, MAXIMUM_RETAINED_OUTPUT_BYTES, ShellTool};
    use crate::conversation_event::{ToolExecutionProblem, ToolExecutionProblemKind};

    async fn execute(arguments: Value) -> Result<Value, ToolExecutionProblem> {
        ShellTool::new().execute(arguments).await
    }

    #[tokio::test]
    async fn a_zero_exit_status_is_a_successful_result() {
        let result = execute(json!({ "command": "printf hello" }))
            .await
            .expect("the shell should succeed");

        assert_eq!(result["stdout"], "hello");
        assert_eq!(result["stderr"], "");
        assert_eq!(
            result["exit_status"],
            json!({ "type": "exited", "code": 0 })
        );
        assert_eq!(result["stdout_truncated"], false);
        assert_eq!(result["stderr_truncated"], false);
    }

    #[tokio::test]
    async fn a_nonzero_exit_status_is_still_a_result() {
        let result = execute(json!({ "command": "printf problem >&2; exit 3" }))
            .await
            .expect("a nonzero exit should be a result");

        assert_eq!(result["stdout"], "");
        assert_eq!(result["stderr"], "problem");
        assert_eq!(
            result["exit_status"],
            json!({ "type": "exited", "code": 3 })
        );
    }

    #[tokio::test]
    async fn signal_termination_is_reported_explicitly() {
        let result = execute(json!({ "command": "kill -TERM $$" }))
            .await
            .expect("signal termination should be a result");

        assert_eq!(
            result["exit_status"],
            json!({ "type": "signaled", "signal": 15 })
        );
    }

    #[tokio::test]
    async fn the_working_directory_is_used_when_provided() {
        let result = execute(json!({ "command": "pwd", "working_directory": "/tmp" }))
            .await
            .expect("the shell should succeed");

        assert_eq!(result["stdout"], "/tmp\n");
    }

    #[tokio::test]
    async fn a_timeout_is_a_problem_with_partial_output_details() {
        let problem = execute(json!({
            "command": "printf partial; printf warning >&2; sleep 5",
            "timeout_seconds": 1
        }))
        .await
        .expect_err("the timeout should be a problem");

        assert_eq!(problem.kind(), ToolExecutionProblemKind::TimedOut);
        assert_eq!(
            problem.message(),
            "the shell command timed out after 1 seconds"
        );
        assert_eq!(
            problem.details(),
            Some(&json!({
                "timeout_seconds": 1,
                "stdout": "partial",
                "stderr": "warning",
                "stdout_truncated": false,
                "stderr_truncated": false
            }))
        );
    }

    #[tokio::test]
    async fn timeout_details_report_truncated_partial_output() {
        let problem = execute(json!({
            "command": "i=0; while [ \"$i\" -lt 20000 ]; do printf aaaa; i=$((i+1)); done; sleep 5",
            "timeout_seconds": 1
        }))
        .await
        .expect_err("the timeout should be a problem");

        assert_eq!(problem.kind(), ToolExecutionProblemKind::TimedOut);
        let details = problem.details().expect("the timeout should carry details");
        assert_eq!(details["timeout_seconds"], 1);
        assert_eq!(
            details["stdout"].as_str().map(str::len),
            Some(MAXIMUM_RETAINED_OUTPUT_BYTES)
        );
        assert_eq!(details["stderr"], "");
        assert_eq!(details["stdout_truncated"], true);
        assert_eq!(details["stderr_truncated"], false);
    }

    #[tokio::test]
    async fn output_beyond_the_retained_bound_is_truncated_visibly() {
        let result = execute(json!({
            "command": "i=0; while [ \"$i\" -lt 20000 ]; do printf aaaa; i=$((i+1)); done"
        }))
        .await
        .expect("the shell should succeed");

        let stdout = result["stdout"]
            .as_str()
            .expect("stdout should be a string");
        assert_eq!(stdout.len(), MAXIMUM_RETAINED_OUTPUT_BYTES);
        assert_eq!(result["stdout_truncated"], true);
        assert_eq!(result["stderr_truncated"], false);
        assert_eq!(
            result["exit_status"],
            json!({ "type": "exited", "code": 0 })
        );
    }

    #[tokio::test]
    async fn missing_or_blank_arguments_are_invalid() {
        let missing = execute(json!({}))
            .await
            .expect_err("missing arguments should fail");
        let blank = execute(json!({ "command": "  " }))
            .await
            .expect_err("a blank command should fail");

        assert_eq!(missing.kind(), ToolExecutionProblemKind::InvalidArguments);
        assert_eq!(blank.kind(), ToolExecutionProblemKind::InvalidArguments);
    }

    #[tokio::test]
    async fn an_unusable_working_directory_is_an_execution_failure() {
        let problem = execute(json!({
            "command": "pwd",
            "working_directory": "/nonexistent/tog-shell-tool-test"
        }))
        .await
        .expect_err("an unusable working directory should fail to start the process");

        assert_eq!(problem.kind(), ToolExecutionProblemKind::ExecutionFailed);
        assert!(
            problem.message().contains("could not be started"),
            "the message should explain that the process could not be started"
        );
    }

    #[test]
    fn the_declared_result_schema_does_not_mention_timeouts() {
        let definition = ShellTool::new();
        let serialized = serde_json::to_value(&definition.definition)
            .expect("the tool definition should serialize");
        let result_schema = serialized["result"]
            .as_object()
            .expect("the result schema should be an object");
        let properties = result_schema["properties"]
            .as_object()
            .expect("the result schema should describe properties");

        assert!(!properties.contains_key("timed_out"));
        assert!(properties.contains_key("stdout"));
        assert!(properties.contains_key("stderr"));
        assert!(properties.contains_key("exit_status"));
        assert!(properties.contains_key("stdout_truncated"));
        assert!(properties.contains_key("stderr_truncated"));
        assert_eq!(
            serialized["parameters"]["properties"]["command"]["type"],
            "string"
        );
        assert_eq!(
            serialized["parameters"]["properties"]["working_directory"]["type"],
            json!(["string", "null"])
        );
        assert_eq!(
            serialized["parameters"]["properties"]["timeout_seconds"]["type"],
            json!(["integer", "null"])
        );
    }
}
