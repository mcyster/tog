use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::str::FromStr;
use std::thread;

use futures_util::StreamExt;
use serde_json::{Map, Value};

use super::{OpenRouterResponsesModelDriver, PROVIDER_PAYLOAD_VERSION, REASONING_ITEM_EVENT_TYPE};
use tog::conversation::{
    Conversation, ConversationEvent, ConversationEventId, ConversationEventPayload,
    ConversationEventPersistence, ConversationHistory, ConversationId, ModelId, ModelOutcome,
    ModelRequest, ModelSource, ModelSpecificEvent, ProviderId, TurnStart, User, UserContent,
};
use tog::conversation::{ToolDefinition, ToolName};
use tog::conversation_event_store::ConversationEventRecord;
use tog::model_driver::{ModelDriver, ModelDriverOutput, ModelOutputStream, TurnInput};

pub(super) fn conversation_event(
    conversation_id: ConversationId,
    position: u64,
    content: ConversationEventPayload,
) -> ConversationEventRecord {
    ConversationEventRecord::new(position, ConversationEvent::new(conversation_id, content))
}

pub(super) fn user_event(
    conversation_id: ConversationId,
    position: u64,
    text: &str,
) -> ConversationEventRecord {
    conversation_event(
        conversation_id,
        position,
        ConversationEventPayload::User(
            User::new(vec![UserContent::Text(text.to_owned())])
                .expect("the user event should be valid"),
        ),
    )
}

pub(super) fn turn_and_request(
    conversation_id: ConversationId,
) -> (ConversationEventRecord, ConversationEventRecord) {
    let turn_start = conversation_event(
        conversation_id,
        1,
        ConversationEventPayload::TurnStart(TurnStart::new(None, 0)),
    );
    let model_request = conversation_event(
        conversation_id,
        2,
        ConversationEventPayload::ModelRequest(
            ModelRequest::new(turn_start.event().id(), source(), 1, Vec::new(), None, None)
                .expect("the model request should be valid"),
        ),
    );
    (turn_start, model_request)
}

pub(super) fn source() -> ModelSource {
    ModelSource::new(
        ProviderId::from_str("openrouter").expect("the provider should be valid"),
        ModelId::from_str("deepseek/deepseek-v4.1-flash").expect("the model should be valid"),
    )
}

pub(super) fn tool_definition(name: &str) -> ToolDefinition {
    ToolDefinition::try_new(
        ToolName::try_new(name.to_owned()).expect("the tool name should be valid"),
        "Run a command.".to_owned(),
        schemars::json_schema!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": { "command": { "type": "string" } }
        }),
        schemars::json_schema!({ "type": "object" }),
    )
    .expect("the tool definition should be valid")
}

pub(super) fn reasoning_event(
    request_id: ConversationEventId,
    item_id: &str,
    encrypted_content: Option<&str>,
    summary: Value,
) -> ConversationEventPayload {
    let mut payload = Map::new();
    payload.insert("item_id".to_owned(), Value::String(item_id.to_owned()));
    if let Some(encrypted_content) = encrypted_content {
        payload.insert(
            "encrypted_content".to_owned(),
            Value::String(encrypted_content.to_owned()),
        );
    }
    payload.insert("summary".to_owned(), summary);
    ConversationEventPayload::ModelSpecificEvent(
        ModelSpecificEvent::new(
            request_id,
            REASONING_ITEM_EVENT_TYPE.to_owned(),
            PROVIDER_PAYLOAD_VERSION,
            Value::Object(payload),
            Some("Thinking.".to_owned()),
        )
        .expect("the reasoning event should be valid"),
    )
}

pub(super) fn reasoning_event_with_version(
    request_id: ConversationEventId,
    item_id: &str,
    version: u32,
    summary: Value,
) -> ConversationEventPayload {
    let mut payload = Map::new();
    payload.insert("item_id".to_owned(), Value::String(item_id.to_owned()));
    payload.insert("summary".to_owned(), summary);
    ConversationEventPayload::ModelSpecificEvent(
        ModelSpecificEvent::new(
            request_id,
            REASONING_ITEM_EVENT_TYPE.to_owned(),
            version,
            Value::Object(payload),
            Some("Thinking.".to_owned()),
        )
        .expect("the reasoning event should be valid"),
    )
}

fn test_conversation(text: &str) -> ConversationHistory {
    let conversation_id = ConversationId::new();
    ConversationHistory::from_events(vec![user_event(conversation_id, 0, text)])
        .expect("the conversation should be valid")
}

fn driver_request(conversation: &dyn Conversation) -> TurnInput<'_> {
    let model_request_id = ConversationEventId::new();
    TurnInput::new(conversation, model_request_id, 0)
}

fn read_request(connection: &TcpStream) -> Value {
    let mut reader = BufReader::new(
        connection
            .try_clone()
            .expect("the request connection should clone"),
    );
    let mut content_length = None;
    loop {
        let mut header = String::new();
        reader
            .read_line(&mut header)
            .expect("the request header should read");
        if header == "\r\n" {
            break;
        }
        if let Some(length) = header.to_ascii_lowercase().strip_prefix("content-length:") {
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

fn mock_driver(address: std::net::SocketAddr) -> OpenRouterResponsesModelDriver {
    OpenRouterResponsesModelDriver {
        http_client: reqwest::Client::new(),
        api_key: "test-key".to_owned(),
        responses_url: format!("http://{address}/responses"),
        source: source(),
    }
}

async fn expect_single(stream: &mut ModelOutputStream) -> ModelDriverOutput {
    let batch = stream
        .next()
        .await
        .expect("the stream should yield a batch")
        .expect("the batch should be valid");
    let mut outputs = batch.into_outputs();
    assert_eq!(outputs.len(), 1, "the batch should hold one output");
    outputs.remove(0)
}

#[tokio::test]
async fn invoke_streams_unpersisted_progress_then_the_completed_output_and_usage() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("the mock server should bind");
    let address = listener
        .local_addr()
        .expect("the mock server address should be available");
    let server = thread::spawn(move || {
        let (mut connection, _) = listener.accept().expect("the mock server should accept");
        let request = read_request(&connection);
        assert_eq!(request["model"], "deepseek/deepseek-v4.1-flash");
        assert_eq!(request["stream"], true);
        assert_eq!(request["input"][0]["role"], "user");
        assert_eq!(request["input"][0]["content"][0]["text"], "Hello");
        assert!(
            request.get("store").is_none(),
            "the stateless Responses API must not be asked to store the response"
        );
        let response_body = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"item_id\":\"msg_1\",\"output_index\":0,\"content_index\":0,\"delta\":\"Answer\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"id\":\"msg_1\",\"status\":\"completed\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"Answer\",\"annotations\":[]}]}],\"usage\":{\"input_tokens\":10,\"output_tokens\":5,\"cost\":0.0002}}}\n\n",
            "data: [DONE]\n\n"
        );
        write!(
            connection,
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
            response_body.len()
        )
        .expect("the mock response should write");
    });
    let driver = mock_driver(address);

    let conversation = test_conversation("Hello");
    let mut model_events = driver
        .invoke(driver_request(&conversation))
        .await
        .expect("the invocation should establish its stream");
    let first = expect_single(&mut model_events).await;
    let ModelDriverOutput::ModelSpecificEvent(first_event) = &first else {
        panic!("the first output should be a content delta");
    };
    assert_eq!(first_event.provider_event_type(), "content_delta");
    assert_eq!(
        first_event.persistence(),
        ConversationEventPersistence::Unpersisted,
        "the intermediate content delta must not be persisted"
    );
    let second = expect_single(&mut model_events).await;
    assert!(matches!(
        &second,
        ModelDriverOutput::AssistantResponse(response) if response.content() == "Answer"
    ));
    let terminal = expect_single(&mut model_events).await;
    let ModelDriverOutput::ModelResponse(response) = &terminal else {
        panic!("the final output should be a model response");
    };
    assert!(matches!(response.outcome(), ModelOutcome::Succeeded));
    assert_eq!(
        response.usage().and_then(|usage| usage.cost_usd()),
        Some(0.0002)
    );
    assert!(model_events.next().await.is_none());
    server.join().expect("the mock server should stop");
}

#[tokio::test]
async fn an_interrupted_stream_records_a_failure_without_durable_outputs() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("the mock server should bind");
    let address = listener
        .local_addr()
        .expect("the mock server address should be available");
    let server = thread::spawn(move || {
        let (mut connection, _) = listener.accept().expect("the mock server should accept");
        read_request(&connection);
        let response_body = concat!(
            "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"id\":\"fc_1\",\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"shell\",\"arguments\":\"\"}}\n\n",
            "data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_1\",\"output_index\":0,\"delta\":\"{\\\"command\\\":\\\"pwd\\\"}\"}\n\n"
        );
        write!(
            connection,
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
            response_body.len()
        )
        .expect("the interrupted response should write");
    });
    let driver = mock_driver(address);

    let conversation = test_conversation("Run pwd");
    let mut model_events = driver
        .invoke(driver_request(&conversation))
        .await
        .expect("the invocation should establish its stream");
    let mut outputs = Vec::new();
    while let Some(batch) = model_events.next().await {
        outputs.extend(batch.expect("the batch should be valid").into_outputs());
    }
    assert_eq!(outputs.len(), 2, "unpersisted progress then the failure");
    assert!(matches!(
        &outputs[0],
        ModelDriverOutput::ModelSpecificEvent(event)
            if event.persistence() == ConversationEventPersistence::Unpersisted
    ));
    assert!(matches!(
        &outputs[1],
        ModelDriverOutput::ModelResponse(response)
            if matches!(response.outcome(), ModelOutcome::Failed { .. })
    ));
    assert!(
        !outputs.iter().any(|output| matches!(
            output,
            ModelDriverOutput::ToolRequest(_) | ModelDriverOutput::AssistantResponse(_)
        )),
        "no tool request or answer may survive an interrupted stream, so no tool runs"
    );
    server.join().expect("the mock server should stop");
}

#[tokio::test]
async fn an_early_http_failure_produces_a_failed_terminal() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("the mock server should bind");
    let address = listener
        .local_addr()
        .expect("the mock server address should be available");
    let server = thread::spawn(move || {
        let (mut connection, _) = listener.accept().expect("the mock server should accept");
        read_request(&connection);
        let body = "unauthorized";
        write!(
            connection,
            "HTTP/1.1 401 Unauthorized\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .expect("the mock response should write");
    });
    let driver = mock_driver(address);

    let conversation = test_conversation("Hello");
    let mut model_events = driver
        .invoke(driver_request(&conversation))
        .await
        .expect("the invocation should establish a stream");
    let batch = model_events
        .next()
        .await
        .expect("the stream should yield a failure batch")
        .expect("the failure batch should be valid")
        .into_outputs();
    assert_eq!(batch.len(), 1);
    assert!(matches!(
        &batch[0],
        ModelDriverOutput::ModelResponse(response)
            if matches!(
                response.outcome(),
                ModelOutcome::Failed { failure }
                    if failure.category() == tog::conversation::FailureCategory::Authentication
            )
    ));
    assert!(model_events.next().await.is_none());
    server.join().expect("the mock server should stop");
}
