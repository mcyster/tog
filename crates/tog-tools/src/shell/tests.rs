use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};

use tog::conversation::{FailureCategory, OperationFailure};
use tog_context::toolset::ExecutableTool;
use uuid::Uuid;

use super::service::ShellService;
use super::tools::{ShellCancelTool, ShellExecuteTool, ShellWaitTool, StreamReadTool};

const LARGE_OUTPUT_BYTES: u64 = 200_000;

fn tools() -> (
    ShellExecuteTool,
    ShellWaitTool,
    ShellCancelTool,
    StreamReadTool,
) {
    let service = Arc::new(ShellService::new());
    (
        ShellExecuteTool::new(Arc::clone(&service)),
        ShellWaitTool::new(Arc::clone(&service)),
        ShellCancelTool::new(Arc::clone(&service)),
        StreamReadTool::new(Arc::clone(&service)),
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

fn temporary_path(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("tog-{label}-{}", Uuid::now_v7().simple()))
}

fn wait_for_process_id(path: &Path) -> i32 {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(text) = std::fs::read_to_string(path)
            && let Ok(process_id) = text.trim().parse::<i32>()
        {
            return process_id;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the shell did not report its process identifier"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn process_alive(process_id: i32) -> bool {
    std::process::Command::new("kill")
        .arg("-0")
        .arg(process_id.to_string())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[tokio::test]
async fn a_short_command_completes_with_output_in_one_call() {
    let (execute_tool, _, _, read_tool) = tools();
    let result = execute(&execute_tool, json!({ "commands": "printf hello" }))
        .await
        .expect("the shell should succeed");

    assert!(
        result["execution_id"]
            .as_str()
            .unwrap()
            .starts_with("exec_")
    );
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
async fn commands_can_change_their_directory_within_an_execution() {
    let (execute_tool, _, _, _) = tools();
    let result = execute(&execute_tool, json!({ "commands": "cd /tmp && pwd" }))
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
async fn cancellation_after_completion_preserves_the_result() {
    let (execute_tool, wait_tool, cancel_tool, _) = tools();
    let result = execute(&execute_tool, json!({ "commands": "printf done" }))
        .await
        .expect("the shell should succeed");

    assert_eq!(result["state"]["status"], "complete");
    assert_eq!(result["state"]["exit_status"]["code"], 0);
    let execution_id = result["execution_id"].as_str().unwrap().to_owned();

    let cancelled = cancel_tool
        .execute(json!({ "execution_id": execution_id }))
        .await
        .expect("cancelling a completed execution should be acknowledged");
    assert_eq!(cancelled["cancelled"], true);

    let state = wait(&wait_tool, &execution_id, 0, 5)
        .await
        .expect("the completed execution should still read");
    assert_eq!(state["status"], "complete");
    assert_eq!(state["exit_status"]["type"], "exited");
    assert_eq!(state["exit_status"]["code"], 0);
    assert!(state["error"].is_null());
    assert_eq!(state["output"], "done");
}

#[tokio::test]
async fn cancellation_racing_completion_settles_consistently() {
    let (execute_tool, wait_tool, cancel_tool, _) = tools();
    let result = execute(
        &execute_tool,
        json!({ "commands": "sleep 0.05; exit 7", "wait_time_seconds": 0 }),
    )
    .await
    .expect("the shell should start");
    let execution_id = result["execution_id"].as_str().unwrap().to_owned();

    let _ = cancel_tool
        .execute(json!({ "execution_id": execution_id }))
        .await
        .expect("the cancellation should be acknowledged");

    let state = wait(&wait_tool, &execution_id, 0, 5)
        .await
        .expect("the racing execution should settle");
    assert_eq!(state["status"], "complete");
    let completed_with_result = state["error"].is_null() && state["exit_status"]["code"] == 7;
    let cancelled = state["error"]["kind"] == "cancelled";
    assert!(
        completed_with_result || cancelled,
        "the settled state must be the result or cancellation, was {state}"
    );

    let _ = cancel_tool
        .execute(json!({ "execution_id": execution_id }))
        .await
        .expect("repeated cancellation should be safe");
    let again = wait(&wait_tool, &execution_id, 0, 5)
        .await
        .expect("the settled execution should still read");
    assert_eq!(
        again, state,
        "repeated cancellation leaves the outcome unchanged"
    );
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
async fn identifiers_do_not_collide_across_service_instances() {
    let (first, _, _, _) = tools();
    let (second, _, _, _) = tools();
    let first_result = execute(&first, json!({ "commands": "printf one" }))
        .await
        .expect("the first shell should succeed");
    let second_result = execute(&second, json!({ "commands": "printf two" }))
        .await
        .expect("the second shell should succeed");

    assert!(
        first_result["execution_id"]
            .as_str()
            .unwrap()
            .starts_with("exec_")
    );
    assert_ne!(first_result["execution_id"], second_result["execution_id"]);
    assert_ne!(
        first_result["stdout_stream_id"],
        second_result["stdout_stream_id"]
    );
    assert_ne!(
        first_result["stderr_stream_id"],
        second_result["stderr_stream_id"]
    );
    assert_ne!(
        first_result["output_stream_id"],
        second_result["output_stream_id"]
    );
}

#[tokio::test]
async fn identifiers_from_an_old_service_instance_are_rejected() {
    let (old_execute, old_wait, old_cancel, old_read) = tools();
    let old_result = execute(&old_execute, json!({ "commands": "printf old" }))
        .await
        .expect("the old shell should succeed");
    let execution_id = old_result["execution_id"].as_str().unwrap().to_owned();
    let output_stream_id = old_result["output_stream_id"].as_str().unwrap().to_owned();
    drop((old_execute, old_wait, old_cancel, old_read));

    let (_, new_wait, new_cancel, new_read) = tools();
    let wait_problem = new_wait
        .execute(json!({
            "execution_id": execution_id,
            "offset": 0,
            "wait_time_seconds": 1
        }))
        .await
        .expect_err("an old execution id should be rejected by a new service");
    assert_eq!(wait_problem.category(), FailureCategory::InvalidArguments);

    let cancel_problem = new_cancel
        .execute(json!({ "execution_id": execution_id }))
        .await
        .expect_err("cancelling on a new service should be rejected");
    assert_eq!(cancel_problem.category(), FailureCategory::InvalidArguments);

    let read_problem = new_read
        .execute(json!({ "stream_id": output_stream_id, "timeout_seconds": 1 }))
        .await
        .expect_err("an old stream id should be rejected by a new service");
    assert_eq!(read_problem.category(), FailureCategory::InvalidArguments);
}

#[tokio::test]
async fn releasing_the_service_terminates_running_work() {
    let pid_file = temporary_path("service-shutdown");
    let service = Arc::new(ShellService::new());
    let execute_tool = ShellExecuteTool::new(Arc::clone(&service));
    let command = format!("printf $$ > '{}'; sleep 30", pid_file.display());
    let result = execute(
        &execute_tool,
        json!({ "commands": command, "wait_time_seconds": 0 }),
    )
    .await
    .expect("the shell should start");

    assert_eq!(result["state"]["status"], "running");
    let process_id = wait_for_process_id(&pid_file);
    assert!(
        process_alive(process_id),
        "the command should still be running"
    );

    drop(execute_tool);
    drop(service);
    tokio::time::sleep(Duration::from_millis(500)).await;

    assert!(
        !process_alive(process_id),
        "releasing the service should terminate the running command"
    );
    let _ = std::fs::remove_file(pid_file);
}

#[tokio::test]
async fn unknown_execution_and_stream_identifiers_are_invalid() {
    let (execute_tool, wait_tool, cancel_tool, read_tool) = tools();
    execute(&execute_tool, json!({ "commands": "true" }))
        .await
        .expect("the shell should start");

    let unknown_execution_id = format!("exec_{}", Uuid::now_v7().simple());
    let unknown_stream_id = format!("stream_{}", Uuid::now_v7().simple());

    let unknown_execution = wait(&wait_tool, &unknown_execution_id, 0, 1)
        .await
        .expect_err("an unknown execution id should fail");
    assert_eq!(
        unknown_execution.category(),
        FailureCategory::InvalidArguments
    );
    assert!(unknown_execution.message().contains("unknown execution"));

    let malformed_execution = wait(&wait_tool, "exec_123", 0, 1)
        .await
        .expect_err("a malformed execution id should fail");
    assert_eq!(
        malformed_execution.category(),
        FailureCategory::InvalidArguments
    );
    assert!(malformed_execution.message().contains("invalid execution"));

    let cancelled = cancel_tool
        .execute(json!({ "execution_id": unknown_execution_id }))
        .await
        .expect_err("cancelling an unknown execution should fail");
    assert_eq!(cancelled.category(), FailureCategory::InvalidArguments);

    let unknown_stream = read_stream(&read_tool, &unknown_stream_id, 0, 1)
        .await
        .expect_err("an unknown stream id should fail");
    assert_eq!(unknown_stream.category(), FailureCategory::InvalidArguments);
}
