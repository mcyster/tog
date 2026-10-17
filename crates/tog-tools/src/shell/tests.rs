use std::sync::Arc;

use serde_json::{Value, json};

use tog::conversation::{FailureCategory, OperationFailure};
use tog_context::toolset::ExecutableTool;

use super::registry::ShellState;
use super::tools::{ShellCancelTool, ShellExecuteTool, ShellWaitTool, StreamReadTool};

const LARGE_OUTPUT_BYTES: u64 = 200_000;

fn tools() -> (
    ShellExecuteTool,
    ShellWaitTool,
    ShellCancelTool,
    StreamReadTool,
) {
    let state = Arc::new(ShellState::new());
    (
        ShellExecuteTool::new(Arc::clone(&state)),
        ShellWaitTool::new(Arc::clone(&state)),
        ShellCancelTool::new(Arc::clone(&state)),
        StreamReadTool::new(Arc::clone(&state)),
    )
}

async fn execute(
    execute_tool: &ShellExecuteTool,
    arguments: Value,
) -> Result<Value, OperationFailure> {
    execute_tool.execute(arguments).await
}

async fn wait(
    wait_tool: &ShellWaitTool,
    execution_id: &str,
    offset: u64,
    wait_time_seconds: u64,
) -> Result<Value, OperationFailure> {
    wait_tool
        .execute(json!({
            "execution_id": execution_id,
            "offset": offset,
            "wait_time_seconds": wait_time_seconds
        }))
        .await
}

async fn read_stream(
    read_tool: &StreamReadTool,
    stream_id: &str,
    offset: u64,
    timeout_seconds: u64,
) -> Result<Value, OperationFailure> {
    read_tool
        .execute(json!({
            "stream_id": stream_id,
            "offset": offset,
            "timeout_seconds": timeout_seconds
        }))
        .await
}

#[tokio::test]
async fn a_short_command_completes_with_output_in_one_call() {
    let (execute_tool, _, _, read_tool) = tools();
    let result = execute(&execute_tool, json!({ "commands": "printf hello" }))
        .await
        .expect("the shell should succeed");

    assert_eq!(result["execution_id"].as_str(), Some("exec_1"));
    assert!(
        result["stdout_stream_id"]
            .as_str()
            .unwrap()
            .starts_with("stream_")
    );
    assert!(
        result["stderr_stream_id"]
            .as_str()
            .unwrap()
            .starts_with("stream_")
    );
    assert!(
        result["output_stream_id"]
            .as_str()
            .unwrap()
            .starts_with("stream_")
    );
    assert_eq!(result["state"]["output"], "hello");
    assert_eq!(result["state"]["next_offset"], 5);
    assert_eq!(result["state"]["status"], "complete");
    assert_eq!(result["state"]["exit_status"]["type"], "exited");
    assert_eq!(result["state"]["exit_status"]["code"], 0);
    assert_eq!(result["state"].get("error"), None);

    let read = read_stream(
        &read_tool,
        result["output_stream_id"].as_str().unwrap(),
        0,
        3,
    )
    .await
    .expect("the combined stream should read");
    assert_eq!(read["content"], "hello");
    assert_eq!(read["next_offset"], 5);
    assert_eq!(read["eof"], true);
}

#[tokio::test]
async fn a_nonzero_exit_is_a_completed_result_with_separate_streams() {
    let (execute_tool, _, _, read_tool) = tools();
    let result = execute(
        &execute_tool,
        json!({ "commands": "printf problem >&2; exit 3" }),
    )
    .await
    .expect("a nonzero exit should be a result");

    assert_eq!(result["state"]["status"], "complete");
    assert_eq!(result["state"]["exit_status"]["type"], "exited");
    assert_eq!(result["state"]["exit_status"]["code"], 3);
    assert_eq!(result["state"].get("error"), None);

    let stdout = read_stream(
        &read_tool,
        result["stdout_stream_id"].as_str().unwrap(),
        0,
        3,
    )
    .await
    .expect("stdout should read");
    assert_eq!(stdout["content"], "");
    assert_eq!(stdout["eof"], true);
    let stderr = read_stream(
        &read_tool,
        result["stderr_stream_id"].as_str().unwrap(),
        0,
        3,
    )
    .await
    .expect("stderr should read");
    assert_eq!(stderr["content"], "problem");
    assert_eq!(stderr["eof"], true);
    let combined = read_stream(
        &read_tool,
        result["output_stream_id"].as_str().unwrap(),
        0,
        3,
    )
    .await
    .expect("the combined stream should read");
    assert_eq!(combined["content"], "problem");
}

#[tokio::test]
async fn signal_termination_is_reported_explicitly() {
    let (execute_tool, _, _, _) = tools();
    let result = execute(&execute_tool, json!({ "commands": "kill -TERM $$" }))
        .await
        .expect("signal termination should be a result");

    assert_eq!(result["state"]["status"], "complete");
    assert_eq!(result["state"]["exit_status"]["type"], "signaled");
    assert_eq!(result["state"]["exit_status"]["signal"], 15);
    assert_eq!(result["state"].get("error"), None);
}

#[tokio::test]
async fn the_working_directory_is_used_when_provided() {
    let (execute_tool, _, _, _) = tools();
    let result = execute(
        &execute_tool,
        json!({ "commands": "pwd", "directory": "/tmp" }),
    )
    .await
    .expect("the shell should succeed");

    assert_eq!(result["state"]["output"].as_str().unwrap().trim(), "/tmp");
    assert_eq!(result["state"]["status"], "complete");
}

#[tokio::test]
async fn a_long_command_returns_running_and_can_be_waited_on_and_cancelled() {
    let (execute_tool, wait_tool, cancel_tool, _) = tools();
    let result = execute(
        &execute_tool,
        json!({ "commands": "printf start; sleep 30", "wait_time_seconds": 1 }),
    )
    .await
    .expect("the shell should start");

    assert_eq!(result["state"]["status"], "running");
    assert_eq!(result["state"]["output"], "start");
    let execution_id = result["execution_id"].as_str().unwrap().to_owned();
    let next_offset = result["state"]["next_offset"].as_u64().unwrap();

    let waited = wait(&wait_tool, &execution_id, next_offset, 2)
        .await
        .expect("the wait should not fail");
    assert_eq!(waited["status"], "running");

    let cancelled = cancel_tool
        .execute(json!({ "execution_id": execution_id }))
        .await
        .expect("the cancellation should be acknowledged");
    assert_eq!(cancelled["cancelled"], true);
    let cancelled_again = cancel_tool
        .execute(json!({ "execution_id": execution_id }))
        .await
        .expect("repeated cancellation should be safe");
    assert_eq!(cancelled_again["cancelled"], true);

    let final_state = wait(&wait_tool, &execution_id, next_offset, 5)
        .await
        .expect("the final wait should report the cancelled state");
    assert_eq!(final_state["status"], "complete");
    assert_eq!(final_state["error"]["kind"], "cancelled");
}

#[tokio::test]
async fn a_timeout_is_a_completed_state_with_a_timed_out_error() {
    let (execute_tool, wait_tool, _, read_tool) = tools();
    let result = execute(
        &execute_tool,
        json!({ "commands": "sleep 10", "timeout_seconds": 1 }),
    )
    .await
    .expect("the timeout should complete the execution");

    assert_eq!(result["state"]["status"], "complete");
    assert_eq!(result["state"]["error"]["kind"], "timed_out");
    assert!(result["state"].get("exit_status").is_none());

    let final_state = wait(&wait_tool, result["execution_id"].as_str().unwrap(), 0, 5)
        .await
        .expect("the final wait should confirm the timeout");
    assert_eq!(final_state["error"]["kind"], "timed_out");
    let read = read_stream(
        &read_tool,
        result["output_stream_id"].as_str().unwrap(),
        0,
        5,
    )
    .await
    .expect("the combined stream should close");
    assert_eq!(read["content"], "");
    assert_eq!(read["eof"], true);
}

#[tokio::test]
async fn a_live_stream_read_accumulates_output_until_eof() {
    let (execute_tool, _, _, read_tool) = tools();
    let result = execute(
        &execute_tool,
        json!({ "commands": "printf hi; sleep 1; printf there", "wait_time_seconds": 0 }),
    )
    .await
    .expect("the shell should start");

    assert_eq!(result["state"]["status"], "running");
    let read = read_stream(
        &read_tool,
        result["output_stream_id"].as_str().unwrap(),
        0,
        5,
    )
    .await
    .expect("the live stream should accumulate to its end");
    assert_eq!(read["content"], "hithere");
    assert_eq!(read["eof"], true);
}

#[tokio::test]
async fn large_output_spills_and_stays_fully_readable_across_offsets() {
    let (execute_tool, _, _, read_tool) = tools();
    let command = "i=0; while [ \"$i\" -lt 50000 ]; do printf aaaa; i=$((i+1)); done";
    let result = execute(
        &execute_tool,
        json!({ "commands": command, "output_limit_bytes": 4096 }),
    )
    .await
    .expect("the shell should succeed");

    assert_eq!(result["state"]["status"], "complete");
    assert_eq!(result["state"]["output"].as_str().unwrap().len(), 4096);
    let stream_id = result["output_stream_id"].as_str().unwrap().to_owned();
    let mut next_offset = result["state"]["next_offset"].as_u64().unwrap();
    let mut content = result["state"]["output"].as_str().unwrap().to_owned();
    loop {
        let read = read_stream(&read_tool, &stream_id, next_offset, 5)
            .await
            .expect("the stream should keep reading");
        content.push_str(read["content"].as_str().unwrap());
        next_offset = read["next_offset"].as_u64().unwrap();
        if read["eof"].as_bool().unwrap() {
            break;
        }
    }
    assert_eq!(content.len() as u64, LARGE_OUTPUT_BYTES);
    assert_eq!(content, "a".repeat(LARGE_OUTPUT_BYTES as usize));
    assert_eq!(next_offset, LARGE_OUTPUT_BYTES);
}

#[tokio::test]
async fn blank_commands_and_supplied_contexts_are_invalid() {
    let (execute_tool, _, _, _) = tools();
    let blank = execute(&execute_tool, json!({ "commands": "  " }))
        .await
        .expect_err("a blank command should fail");
    assert_eq!(blank.category(), FailureCategory::InvalidArguments);

    let context = execute(
        &execute_tool,
        json!({ "commands": "true", "shell_context_id": "ctx_1" }),
    )
    .await
    .expect_err("a shell context should be rejected");
    assert_eq!(context.category(), FailureCategory::InvalidArguments);
}

#[tokio::test]
async fn an_unusable_working_directory_is_an_execution_failure() {
    let (execute_tool, _, _, _) = tools();
    let problem = execute(
        &execute_tool,
        json!({ "commands": "pwd", "directory": "/nonexistent/tog-shell-tool-test" }),
    )
    .await
    .expect_err("an unusable working directory should fail to start the process");

    assert_eq!(problem.category(), FailureCategory::ExecutionFailed);
    assert!(
        problem.message().contains("could not be started"),
        "the message should explain that the process could not be started"
    );
}

#[tokio::test]
async fn unknown_execution_and_stream_identifiers_are_invalid() {
    let (execute_tool, wait_tool, cancel_tool, read_tool) = tools();
    execute(&execute_tool, json!({ "commands": "true" }))
        .await
        .expect("the shell should start");

    let unknown_execution = wait(&wait_tool, "exec_999", 0, 1)
        .await
        .expect_err("an unknown execution id should fail");
    assert_eq!(
        unknown_execution.category(),
        FailureCategory::InvalidArguments
    );
    let malformed_execution = wait(&wait_tool, "not-a-id", 0, 1)
        .await
        .expect_err("a malformed execution id should fail");
    assert_eq!(
        malformed_execution.category(),
        FailureCategory::InvalidArguments
    );

    let cancelled = cancel_tool
        .execute(json!({ "execution_id": "exec_999" }))
        .await
        .expect_err("cancelling an unknown execution should fail");
    assert_eq!(cancelled.category(), FailureCategory::InvalidArguments);

    let unknown_stream = read_stream(&read_tool, "stream_999", 0, 1)
        .await
        .expect_err("an unknown stream id should fail");
    assert_eq!(unknown_stream.category(), FailureCategory::InvalidArguments);
}
