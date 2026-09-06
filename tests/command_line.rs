use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use serde_json::Value;

fn tog_command() -> Command {
    Command::new(env!("CARGO_BIN_EXE_tog"))
}

fn temporary_data_directory() -> PathBuf {
    std::env::temp_dir().join(format!("tog-command-test-{}", uuid::Uuid::now_v7()))
}

struct MockOpenAiServer {
    base_url: String,
    requests: Arc<Mutex<Vec<Value>>>,
    server_thread: JoinHandle<()>,
}

enum MockResponse {
    Success {
        response_id: &'static str,
        assistant_text: &'static str,
    },
    SuccessWithReasoning {
        response_id: &'static str,
        detailed: &'static str,
        interesting: &'static str,
        important: &'static str,
    },
    Failure {
        status: &'static str,
    },
    Refusal {
        message: &'static str,
    },
    Incremental {
        first_events: &'static str,
        remaining_events: &'static str,
        continue_response: Receiver<()>,
    },
}

impl MockOpenAiServer {
    fn start(responses: Vec<MockResponse>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("the mock server should bind");
        let address = listener
            .local_addr()
            .expect("the mock server address should be available");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let thread_requests = Arc::clone(&requests);
        let server_thread = thread::spawn(move || {
            for response in responses {
                let (mut stream, _) = listener.accept().expect("the mock server should accept");
                let request = read_request(&stream);
                thread_requests
                    .lock()
                    .expect("the request list should lock")
                    .push(request);
                write_response(&mut stream, response);
            }
        });
        Self {
            base_url: format!("http://{address}/v1"),
            requests,
            server_thread,
        }
    }

    fn finish(self) -> Vec<Value> {
        self.server_thread
            .join()
            .expect("the mock server should stop cleanly");
        Arc::try_unwrap(self.requests)
            .expect("the request list should have one owner")
            .into_inner()
            .expect("the request list should unlock")
    }
}

fn read_request(stream: &TcpStream) -> Value {
    let mut reader = BufReader::new(stream.try_clone().expect("the request stream should clone"));
    let mut content_length = None;
    loop {
        let mut header_line = String::new();
        reader
            .read_line(&mut header_line)
            .expect("the request header should read");
        if header_line == "\r\n" {
            break;
        }
        if let Some(length) = header_line
            .to_ascii_lowercase()
            .strip_prefix("content-length:")
        {
            content_length = Some(
                length
                    .trim()
                    .parse::<usize>()
                    .expect("the content length should be numeric"),
            );
        }
    }
    let mut body = vec![0; content_length.expect("the request should have a body")];
    reader
        .read_exact(&mut body)
        .expect("the request body should read");
    serde_json::from_slice(&body).expect("the request body should be JSON")
}

fn write_response(stream: &mut TcpStream, response: MockResponse) {
    if let MockResponse::Incremental {
        first_events,
        remaining_events,
        continue_response,
    } = response
    {
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n{first_events}"
        )
        .expect("the first mock response events should write");
        stream
            .flush()
            .expect("the first mock response events should flush");
        continue_response
            .recv()
            .expect("the test should permit the response to continue");
        stream
            .write_all(remaining_events.as_bytes())
            .expect("the remaining mock response events should write");
        return;
    }

    let (status, content_type, response_body) = match response {
        MockResponse::Success {
            response_id,
            assistant_text,
        } => (
            "200 OK",
            "text/event-stream",
            format!(
                concat!(
                    "data: {{\"type\":\"response.created\",\"response\":{{\"id\":\"{}\"}}}}\n\n",
                    "data: {{\"type\":\"response.output_text.delta\",\"delta\":\"{}\"}}\n\n",
                    "data: {{\"type\":\"response.completed\",\"response\":{{\"id\":\"{}\"}}}}\n\n",
                    "data: [DONE]\n\n"
                ),
                response_id, assistant_text, response_id
            ),
        ),
        MockResponse::SuccessWithReasoning {
            response_id,
            detailed,
            interesting,
            important,
        } => (
            "200 OK",
            "text/event-stream",
            format!(
                concat!(
                    "data: {{\"type\":\"response.created\",\"response\":{{\"id\":\"{}\"}}}}\n\n",
                    "data: {{\"type\":\"response.reasoning_text.delta\",\"delta\":\"{}\"}}\n\n",
                    "data: {{\"type\":\"response.reasoning_summary_text.delta\",\"delta\":\"{}\"}}\n\n",
                    "data: {{\"type\":\"response.output_text.delta\",\"delta\":\"{}\"}}\n\n",
                    "data: {{\"type\":\"response.completed\",\"response\":{{\"id\":\"{}\"}}}}\n\n",
                    "data: [DONE]\n\n"
                ),
                response_id, detailed, interesting, important, response_id
            ),
        ),
        MockResponse::Failure { status } => (
            status,
            "application/json",
            "{\"error\":{\"message\":\"request rejected\"}}".to_owned(),
        ),
        MockResponse::Refusal { message } => (
            "200 OK",
            "text/event-stream",
            format!(
                concat!(
                    "data: {{\"type\":\"response.refusal.done\",\"refusal\":\"{}\"}}\n\n",
                    "data: {{\"type\":\"response.completed\",\"response\":{{}}}}\n\n",
                    "data: [DONE]\n\n"
                ),
                message
            ),
        ),
        MockResponse::Incremental { .. } => unreachable!("handled before the response match"),
    };
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
        response_body.len(),
    )
    .expect("the mock response should write");
}

fn configured_command(server: &MockOpenAiServer, data_directory: &PathBuf) -> Command {
    let mut command = tog_command();
    command
        .env("OPENAI_API_KEY", "test-key")
        .env("TOG_OPENAI_BASE_URL", &server.base_url)
        .env("TOG_DATA_DIR", data_directory);
    command
}

fn reported_conversation_id(standard_error: &[u8]) -> String {
    String::from_utf8(standard_error.to_vec())
        .expect("standard error should be UTF-8")
        .lines()
        .find_map(|line| line.strip_prefix("#> conversation "))
        .expect("standard error should identify the conversation")
        .to_owned()
}

#[test]
fn turn_persists_events_and_prints_semantic_output() {
    let server = MockOpenAiServer::start(vec![MockResponse::Success {
        response_id: "resp_first",
        assistant_text: "Hello",
    }]);
    let data_directory = temporary_data_directory();

    let command_output = configured_command(&server, &data_directory)
        .args(["say", "hi"])
        .output()
        .expect("tog should run");

    assert!(command_output.status.success());
    assert_eq!(
        String::from_utf8(command_output.stdout).expect("standard output should be UTF-8"),
        "Hello\n"
    );
    let standard_error =
        String::from_utf8(command_output.stderr.clone()).expect("standard error should be UTF-8");
    let conversation_id = reported_conversation_id(&command_output.stderr);
    assert!(conversation_id.starts_with("conversation_"));
    assert!(standard_error.contains("## waiting for model gpt-5.6\n"));
    assert!(!data_directory.join("agent-runs").exists());
    let conversation_directory = data_directory
        .join("conversations")
        .join(conversation_id.trim_start_matches("conversation_"));
    assert!(!conversation_directory.join("conversation.json").exists());
    let mut event_paths = fs::read_dir(conversation_directory.join("events"))
        .expect("the persisted events should be readable")
        .map(|entry| entry.expect("the event entry should be readable").path())
        .collect::<Vec<_>>();
    event_paths.sort();
    let first_event: Value = serde_json::from_reader(
        fs::File::open(&event_paths[0]).expect("the persisted user event should open"),
    )
    .expect("the persisted user event should be JSON");
    assert_eq!(
        first_event["conversation_id"]
            .as_str()
            .expect("the event should carry its conversation identifier")
            .replace('-', ""),
        conversation_id.trim_start_matches("conversation_")
    );
    assert_eq!(first_event["schema_version"], 11);
    assert_eq!(first_event["class"], "command");
    assert_eq!(first_event["event"]["type"], "user_message_requested");
    assert_eq!(first_event["event"]["content"][0]["type"], "text");
    assert_eq!(first_event["event"]["content"][0]["value"], "say hi");
    assert!(first_event.get("kind").is_none());
    assert!(first_event.get("model").is_none());
    let turn_request: Value = serde_json::from_reader(
        fs::File::open(&event_paths[1]).expect("the persisted turn request should open"),
    )
    .expect("the persisted turn request should be JSON");
    assert_eq!(turn_request["class"], "command");
    assert_eq!(turn_request["event"]["type"], "turn_requested");
    let user_event: Value = serde_json::from_reader(
        fs::File::open(&event_paths[2]).expect("the persisted user event should open"),
    )
    .expect("the persisted user event should be JSON");
    assert_eq!(user_event["schema_version"], 11);
    assert_eq!(user_event["class"], "fact");
    assert_eq!(user_event["event"]["type"], "user");
    let invocation_event: Value = serde_json::from_reader(
        fs::File::open(&event_paths[3]).expect("the invocation event should open"),
    )
    .expect("the invocation event should be JSON");
    assert_eq!(invocation_event["class"], "command");
    assert_eq!(invocation_event["driver"], "openai");
    assert_eq!(invocation_event["driver_version"], "1");
    assert_eq!(invocation_event["event_type"], "model_invocation_requested");
    assert_eq!(invocation_event["event_schema_version"], 1);
    assert!(invocation_event["description"].is_string());
    assert!(invocation_event["payload"]["invocation_id"].is_string());
    assert_eq!(invocation_event["payload"]["model"]["provider"], "openai");
    assert_eq!(invocation_event["payload"]["model"]["model"], "gpt-5.6");
    let model_event: Value = serde_json::from_reader(
        fs::File::open(&event_paths[4]).expect("the persisted model event should open"),
    )
    .expect("the persisted model event should be JSON");
    assert_eq!(model_event["schema_version"], 11);
    assert_eq!(model_event["class"], "fact");
    assert_eq!(model_event["event"]["type"], "assistant");
    assert_eq!(model_event["event"]["response"]["message"], "Hello");
    assert!(model_event.get("kind").is_none());
    assert!(model_event.get("data").is_none());
    let requests = server.finish();
    assert_eq!(requests[0]["model"], "gpt-5.6");
    assert_eq!(requests[0]["input"][0]["content"], "say hi");
    assert_eq!(requests[0]["reasoning"]["summary"], "auto");
    assert_eq!(requests[0]["stream"], true);
    assert!(requests[0].get("text").is_none());
}

#[test]
fn high_verbosity_prints_all_model_event_messages() {
    let server = MockOpenAiServer::start(vec![MockResponse::SuccessWithReasoning {
        response_id: "resp_verbose",
        detailed: "Detailed thought",
        interesting: "Reasoning summary",
        important: "Final answer",
    }]);
    let data_directory = temporary_data_directory();

    let command_output = configured_command(&server, &data_directory)
        .args(["--verbosity", "high", "Explain ownership"])
        .output()
        .expect("tog should run");

    assert!(command_output.status.success());
    assert_eq!(
        String::from_utf8(command_output.stdout).expect("standard output should be UTF-8"),
        "### Detailed thought\n### Reasoning summary\nFinal answer\n"
    );
    let requests = server.finish();
    assert_eq!(requests.len(), 1);
}

#[test]
fn cli_output_is_incremental_and_not_duplicated() {
    let (continue_response, response_permission) = mpsc::channel();
    let server = MockOpenAiServer::start(vec![MockResponse::Incremental {
        first_events: concat!(
            "data: {\"type\":\"response.reasoning_text.delta\",\"delta\":\"Detailed thought\"}\n\n",
            "data: {\"type\":\"response.reasoning_text.done\",\"text\":\"Detailed thought\"}\n\n"
        ),
        remaining_events: concat!(
            "data: {\"type\":\"response.output_text.done\",\"text\":\"Final answer\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{}}\n\n",
            "data: [DONE]\n\n"
        ),
        continue_response: response_permission,
    }]);
    let data_directory = temporary_data_directory();
    let mut command = configured_command(&server, &data_directory);
    let mut child = command
        .args(["--verbosity", "high", "Explain ownership"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("tog should start");
    let mut standard_output = BufReader::new(
        child
            .stdout
            .take()
            .expect("tog standard output should be captured"),
    );
    let mut first_line = String::new();

    standard_output
        .read_line(&mut first_line)
        .expect("the first semantic event should be readable");

    assert_eq!(first_line, "### Detailed thought\n");
    continue_response
        .send(())
        .expect("the mock response should continue");
    let mut remaining_output = String::new();
    standard_output
        .read_to_string(&mut remaining_output)
        .expect("the remaining semantic events should be readable");
    let output = child.wait_with_output().expect("tog should stop");

    assert!(output.status.success());
    assert_eq!(remaining_output, "Final answer\n");
    let requests = server.finish();
    assert_eq!(requests.len(), 1);
}

#[test]
fn model_issue_is_rendered_and_persisted_as_a_top_level_problem() {
    let server = MockOpenAiServer::start(vec![MockResponse::Refusal {
        message: "I cannot comply.",
    }]);
    let data_directory = temporary_data_directory();

    let command_output = configured_command(&server, &data_directory)
        .args(["Question"])
        .output()
        .expect("tog should run");

    assert!(command_output.status.success());
    assert_eq!(
        String::from_utf8(command_output.stdout).expect("standard output should be UTF-8"),
        "### I cannot comply.\n"
    );
    let conversation_id = reported_conversation_id(&command_output.stderr);
    let events_directory = data_directory
        .join("conversations")
        .join(conversation_id.trim_start_matches("conversation_"))
        .join("events");
    let mut event_paths = fs::read_dir(events_directory)
        .expect("the persisted events should be readable")
        .map(|entry| entry.expect("the event entry should be readable").path())
        .collect::<Vec<_>>();
    event_paths.sort();
    let problem: Value = serde_json::from_reader(
        fs::File::open(&event_paths[4]).expect("the problem event should open"),
    )
    .expect("the problem event should be JSON");
    assert_eq!(problem["class"], "fact");
    assert_eq!(problem["event"]["type"], "problem");
    assert_eq!(problem["event"]["problem"]["category"], "issue");
    assert_eq!(problem["event"]["problem"]["detail"]["type"], "refusal");
    assert_eq!(
        problem["event"]["problem"]["detail"]["message"],
        "I cannot comply."
    );
    assert!(problem["event"]["invocation_id"].is_string());
    assert!(problem.get("message").is_none());
    assert!(problem.get("severity").is_none());
    assert_eq!(server.finish().len(), 1);
}

#[test]
fn low_verbosity_prints_only_the_assistant_response() {
    let server = MockOpenAiServer::start(vec![MockResponse::SuccessWithReasoning {
        response_id: "resp_low",
        detailed: "Detailed thought",
        interesting: "Reasoning summary",
        important: "Final answer",
    }]);
    let data_directory = temporary_data_directory();

    let command_output = configured_command(&server, &data_directory)
        .args(["--verbosity", "low", "Explain ownership"])
        .output()
        .expect("tog should run");

    assert!(command_output.status.success());
    assert_eq!(
        String::from_utf8(command_output.stdout).expect("standard output should be UTF-8"),
        "Final answer\n"
    );
    server.finish();
}

#[test]
fn reasoning_events_are_persisted_and_printed_but_not_replayed_as_assistant_messages() {
    let server = MockOpenAiServer::start(vec![
        MockResponse::SuccessWithReasoning {
            response_id: "resp_reasoning",
            detailed: "Detailed thought",
            interesting: "Reasoning summary",
            important: "Final answer",
        },
        MockResponse::Success {
            response_id: "resp_follow_up",
            assistant_text: "Follow-up answer",
        },
    ]);
    let data_directory = temporary_data_directory();

    let first_output = configured_command(&server, &data_directory)
        .args(["--verbosity", "high", "First question"])
        .output()
        .expect("the first turn should run");

    assert!(first_output.status.success());
    assert_eq!(
        String::from_utf8(first_output.stdout).expect("standard output should be UTF-8"),
        "### Detailed thought\n### Reasoning summary\nFinal answer\n"
    );
    let conversation_id = reported_conversation_id(&first_output.stderr);
    let events_directory = data_directory
        .join("conversations")
        .join(conversation_id.trim_start_matches("conversation_"))
        .join("events");
    let mut event_paths = fs::read_dir(events_directory)
        .expect("the persisted events should be readable")
        .map(|entry| entry.expect("the event entry should be readable").path())
        .collect::<Vec<_>>();
    event_paths.sort();
    let persisted_events = event_paths
        .iter()
        .map(|path| {
            serde_json::from_reader::<_, Value>(
                fs::File::open(path).expect("the persisted event should open"),
            )
            .expect("the persisted event should be JSON")
        })
        .collect::<Vec<_>>();
    assert_eq!(persisted_events[4]["class"], "fact");
    assert_eq!(persisted_events[4]["event"]["type"], "communication");
    assert_eq!(
        persisted_events[4]["event"]["communication"]["subtype"],
        "reasoning"
    );
    assert_eq!(
        persisted_events[4]["event"]["communication"]["message"],
        "Detailed thought"
    );
    assert_eq!(persisted_events[5]["class"], "fact");
    assert_eq!(persisted_events[5]["event"]["type"], "communication");
    assert_eq!(
        persisted_events[5]["event"]["communication"]["subtype"],
        "reasoning_summary"
    );
    assert_eq!(
        persisted_events[5]["event"]["communication"]["message"],
        "Reasoning summary"
    );

    let second_output = configured_command(&server, &data_directory)
        .args([
            ":turn",
            "--conversation",
            &conversation_id,
            "Second question",
        ])
        .output()
        .expect("the second turn should run");

    assert!(second_output.status.success());
    let requests = server.finish();
    assert_eq!(requests[1]["input"].as_array().map(Vec::len), Some(3));
    assert_eq!(requests[1]["input"][0]["content"], "First question");
    assert_eq!(requests[1]["input"][1]["content"], "Final answer");
    assert_eq!(requests[1]["input"][2]["content"], "Second question");
}

#[test]
fn medium_verbosity_hides_detailed_model_event_messages() {
    let server = MockOpenAiServer::start(vec![MockResponse::SuccessWithReasoning {
        response_id: "resp_verbose",
        detailed: "Detailed thought",
        interesting: "Reasoning summary",
        important: "Final answer",
    }]);
    let data_directory = temporary_data_directory();

    let command_output = configured_command(&server, &data_directory)
        .args(["--verbosity", "medium", "Explain ownership"])
        .output()
        .expect("tog should run");

    assert!(command_output.status.success());
    assert_eq!(
        String::from_utf8(command_output.stdout).expect("standard output should be UTF-8"),
        "### Reasoning summary\nFinal answer\n"
    );
    server.finish();
}

#[test]
fn subsequent_turn_reconstructs_semantic_conversation() {
    let server = MockOpenAiServer::start(vec![
        MockResponse::Success {
            response_id: "resp_first",
            assistant_text: "First answer",
        },
        MockResponse::Success {
            response_id: "resp_second",
            assistant_text: "Second answer",
        },
    ]);
    let data_directory = temporary_data_directory();
    let first_output = configured_command(&server, &data_directory)
        .args([":turn", "First question"])
        .output()
        .expect("the first turn should run");
    assert!(first_output.status.success());
    let conversation_id = reported_conversation_id(&first_output.stderr);

    let second_output = configured_command(&server, &data_directory)
        .args([
            ":turn",
            "--conversation",
            &conversation_id,
            "Second question",
        ])
        .output()
        .expect("the second turn should run");

    assert!(second_output.status.success());
    assert_eq!(
        String::from_utf8(second_output.stdout).expect("standard output should be UTF-8"),
        "Second answer\n"
    );
    let requests = server.finish();
    assert!(requests[1].get("previous_response_id").is_none());
    assert_eq!(requests[1]["input"].as_array().map(Vec::len), Some(3));
    assert_eq!(requests[1]["input"][0]["content"], "First question");
    assert_eq!(requests[1]["input"][1]["content"], "First answer");
    assert_eq!(requests[1]["input"][2]["content"], "Second question");
}

#[test]
fn failed_user_turn_is_included_in_the_next_local_reconstruction() {
    let server = MockOpenAiServer::start(vec![
        MockResponse::Success {
            response_id: "resp_first",
            assistant_text: "First answer",
        },
        MockResponse::Failure {
            status: "500 Internal Server Error",
        },
        MockResponse::Success {
            response_id: "resp_after_failure",
            assistant_text: "Recovered answer",
        },
    ]);
    let data_directory = temporary_data_directory();
    let first_output = configured_command(&server, &data_directory)
        .args([":turn", "First question"])
        .output()
        .expect("the first turn should run");
    let conversation_id = reported_conversation_id(&first_output.stderr);
    let failed_output = configured_command(&server, &data_directory)
        .args([
            ":turn",
            "--conversation",
            &conversation_id,
            "Failed question",
        ])
        .output()
        .expect("the failed turn should run");
    assert!(failed_output.status.success());
    assert_eq!(
        reported_conversation_id(&failed_output.stderr),
        conversation_id
    );
    let events_directory = data_directory
        .join("conversations")
        .join(conversation_id.trim_start_matches("conversation_"))
        .join("events");
    let mut event_paths = fs::read_dir(events_directory)
        .expect("the persisted events should be readable")
        .map(|entry| entry.expect("the event entry should be readable").path())
        .collect::<Vec<_>>();
    event_paths.sort();
    let invocation_problem = event_paths
        .iter()
        .map(|path| {
            serde_json::from_reader::<_, Value>(
                fs::File::open(path).expect("the persisted event should open"),
            )
            .expect("the persisted event should be JSON")
        })
        .find(|event| event["class"] == "fact" && event["event"]["type"] == "problem")
        .expect("the invocation problem should be persisted");
    assert_eq!(invocation_problem["class"], "fact");
    assert_eq!(invocation_problem["event"]["type"], "problem");
    assert_eq!(
        invocation_problem["event"]["problem"]["category"],
        "invocation"
    );
    assert_eq!(
        invocation_problem["event"]["problem"]["detail"]["type"],
        "provider_failure"
    );
    assert_eq!(
        invocation_problem["event"]["problem"]["detail"]["message"],
        "The model provider failed the invocation."
    );
    assert!(invocation_problem.get("message").is_none());
    assert!(invocation_problem.get("severity").is_none());
    assert!(!invocation_problem.to_string().contains("request rejected"));

    let recovered_output = configured_command(&server, &data_directory)
        .args([
            ":turn",
            "--conversation",
            &conversation_id,
            "Recovery question",
        ])
        .output()
        .expect("the recovery turn should run");

    assert!(recovered_output.status.success());
    let requests = server.finish();
    assert!(requests[2].get("previous_response_id").is_none());
    assert_eq!(requests[2]["input"].as_array().map(Vec::len), Some(4));
    assert_eq!(requests[2]["input"][2]["content"], "Failed question");
}

#[test]
fn turn_rejects_a_missing_user_prompt() {
    let command_output = tog_command().output().expect("tog should run");

    assert!(!command_output.status.success());
    let standard_error =
        String::from_utf8(command_output.stderr).expect("standard error should be UTF-8");
    assert!(standard_error.contains("required"));
    assert!(standard_error.contains("Usage: tog [:turn] [OPTIONS] <USER_PROMPT>..."));
}

#[test]
fn help_lists_the_colon_prefixed_turn_command() {
    let command_output = tog_command()
        .arg("--help")
        .output()
        .expect("tog should run");

    assert!(command_output.status.success());
    let standard_output =
        String::from_utf8(command_output.stdout).expect("standard output should be UTF-8");
    assert!(standard_output.contains(":turn"));
    assert!(!standard_output.contains("\n  help"));
}
