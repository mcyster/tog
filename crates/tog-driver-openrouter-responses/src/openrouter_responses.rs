use std::collections::HashMap;
use std::collections::VecDeque;
use std::fmt::{Display, Formatter};
use std::str::FromStr;

use futures_util::future::BoxFuture;
use futures_util::stream::{self, BoxStream};
use futures_util::{FutureExt, StreamExt};
use reqwest::Client;
use reqwest::StatusCode;
use schemars::Schema;
use serde_json::{Map, Value, json};

use tog::conversation::{
    AssistantResponse, ConversationEventId, ConversationEventPayload, ConversationEventPersistence,
    FailureCategory, InvalidAssistantResponse, InvalidModelResponse, InvalidModelSpecificEvent,
    InvalidToolRequest, ModelContinuation, ModelData, ModelId, ModelOutcome, ModelResponse,
    ModelSource, ModelSpecificEvent, OperationFailure, ProviderId, ToolRequest, Usage, UserContent,
};
use tog::conversation::{ToolAvailability, ToolName};
use tog::conversation_event_store::ConversationEventRecord;
use tog::model_driver::{
    ModelDriver, ModelDriverError, ModelDriverOutput, ModelDriverOutputBatch, ModelOutputStream,
    TurnInput,
};

type ResponseByteStream = BoxStream<'static, Result<Vec<u8>, OpenRouterResponsesError>>;
type ProviderOutputStream = BoxStream<'static, Result<ModelDriverEvent, OpenRouterResponsesError>>;

const PROVIDER_PAYLOAD_VERSION: u32 = 1;
const CONTENT_DELTA_EVENT_TYPE: &str = "content_delta";
const REASONING_DELTA_EVENT_TYPE: &str = "reasoning_delta";
const REASONING_SUMMARY_DELTA_EVENT_TYPE: &str = "reasoning_summary_delta";
const TOOL_CALL_ARGUMENTS_DELTA_EVENT_TYPE: &str = "tool_call_arguments_delta";
const REASONING_ITEM_EVENT_TYPE: &str = "response_reasoning_item";

#[derive(Debug, Eq, PartialEq)]
pub enum OpenRouterResponsesError {
    Authentication(String),
    RateLimited(String),
    Transport(String),
    InvalidRequest(String),
    InvalidResponse(String),
    StreamInterrupted(String),
    Provider(String),
}

impl Display for OpenRouterResponsesError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Authentication(message) => write!(formatter, "authentication failed: {message}"),
            Self::RateLimited(message) => write!(formatter, "rate limited: {message}"),
            Self::Transport(message) => write!(formatter, "model transport failed: {message}"),
            Self::InvalidRequest(message) => write!(formatter, "invalid model request: {message}"),
            Self::InvalidResponse(message) => {
                write!(formatter, "invalid model response: {message}")
            }
            Self::StreamInterrupted(message) => write!(
                formatter,
                "model response stream was interrupted: {message}"
            ),
            Self::Provider(message) => write!(formatter, "model provider failed: {message}"),
        }
    }
}

impl std::error::Error for OpenRouterResponsesError {}

#[derive(Clone)]
enum ModelDriverEvent {
    AssistantResponse {
        content: String,
    },
    ModelSpecificEvent {
        event_type: String,
        payload: Value,
        message: Option<String>,
        persistence: ConversationEventPersistence,
    },
    ToolRequest {
        tool_name: ToolName,
        arguments: Value,
        provider_call_id: Option<String>,
    },
    Terminal {
        outcome: ModelOutcome,
        continuation: ModelContinuation,
        usage: Option<Usage>,
    },
}

pub struct OpenRouterResponsesModelDriver {
    http_client: Client,
    api_key: String,
    responses_url: String,
    source: ModelSource,
}

impl OpenRouterResponsesModelDriver {
    pub fn from_environment(model: ModelId) -> Result<Self, OpenRouterResponsesError> {
        let api_key = std::env::var("OPENROUTER_API_KEY").map_err(|_| {
            OpenRouterResponsesError::Authentication("OPENROUTER_API_KEY must be set".to_owned())
        })?;
        let base_url = std::env::var("TOG_OPENROUTER_BASE_URL")
            .unwrap_or_else(|_| "https://openrouter.ai/api/v1".to_owned());
        Ok(Self {
            http_client: Client::new(),
            api_key,
            responses_url: format!("{}/responses", base_url.trim_end_matches('/')),
            source: ModelSource::new(
                ProviderId::from_str("openrouter")
                    .expect("the OpenRouter provider identifier should be valid"),
                model,
            ),
        })
    }
}

impl ModelDriver for OpenRouterResponsesModelDriver {
    fn source(&self) -> &ModelSource {
        &self.source
    }

    fn invoke<'invoke>(
        &'invoke self,
        input: TurnInput<'invoke>,
    ) -> BoxFuture<'invoke, Result<ModelOutputStream, ModelDriverError>> {
        let model_request_id = input.model_request_id();
        let events = input.events();
        let mut request_body = Map::new();
        request_body.insert(
            "model".to_owned(),
            Value::String(self.source.model().as_str().to_owned()),
        );
        request_body.insert("input".to_owned(), responses_input(events));
        let immediate_tools = input
            .tools()
            .map(|tools| {
                tools
                    .tools()
                    .iter()
                    .filter(|tool| tool.availability() == ToolAvailability::Immediate)
                    .map(|tool| provider_tool(tool.definition()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if !immediate_tools.is_empty() {
            request_body.insert("tools".to_owned(), Value::Array(immediate_tools));
        }
        request_body.insert("reasoning".to_owned(), json!({ "summary": "auto" }));
        request_body.insert("stream".to_owned(), Value::Bool(true));

        let request = self
            .http_client
            .post(&self.responses_url)
            .bearer_auth(&self.api_key)
            .json(&request_body)
            .build();
        let http_client = self.http_client.clone();
        async move {
            let request = match request {
                Ok(request) => request,
                Err(error) => {
                    let error = OpenRouterResponsesError::InvalidRequest(error.to_string());
                    return Ok(failed_terminal_stream(
                        model_request_id,
                        FailureStage::BeforeStream,
                        error,
                    ));
                }
            };
            let response = match http_client.execute(request).await {
                Ok(response) => response,
                Err(error) => {
                    return Ok(failed_terminal_stream(
                        model_request_id,
                        FailureStage::BeforeStream,
                        OpenRouterResponsesError::Transport(error.to_string()),
                    ));
                }
            };
            let response_status = response.status();
            if !response_status.is_success() {
                let response_body = match response.text().await {
                    Ok(response_body) => response_body,
                    Err(error) => {
                        return Ok(failed_terminal_stream(
                            model_request_id,
                            FailureStage::BeforeStream,
                            OpenRouterResponsesError::Transport(error.to_string()),
                        ));
                    }
                };
                return match classify_response_failure(response_status, response_body) {
                    Ok(()) => Ok(context_limit_stream(model_request_id)),
                    Err(error) => Ok(failed_terminal_stream(
                        model_request_id,
                        FailureStage::BeforeStream,
                        error,
                    )),
                };
            }

            let response_bytes = response
                .bytes_stream()
                .map(|result| {
                    result
                        .map(|bytes| bytes.to_vec())
                        .map_err(|error| OpenRouterResponsesError::Transport(error.to_string()))
                })
                .boxed();
            Ok(conversation_event_stream(
                model_output_stream(response_bytes),
                model_request_id,
            ))
        }
        .boxed()
    }
}

fn responses_input(events: &[ConversationEventRecord]) -> Value {
    let provider_call_ids = provider_tool_call_ids(events);
    let input = events
        .iter()
        .filter_map(|conversation_event| {
            let event = conversation_event.event();
            match event.payload() {
                ConversationEventPayload::User(user) => {
                    let text = user
                        .content()
                        .iter()
                        .map(|content| match content {
                            UserContent::Text(text) => text.as_str(),
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    Some(json!({
                        "type": "message",
                        "role": "user",
                        "content": [{ "type": "input_text", "text": text }],
                    }))
                }
                ConversationEventPayload::ModelSpecificEvent(specific_event) => {
                    replay_reasoning_item(specific_event)
                }
                ConversationEventPayload::AssistantResponse(response) => Some(json!({
                    "type": "message",
                    "role": "assistant",
                    "id": event.id().to_string(),
                    "status": "completed",
                    "content": [{
                        "type": "output_text",
                        "text": response.content(),
                        "annotations": [],
                    }],
                })),
                ConversationEventPayload::ToolRequest(request) => {
                    let call_id = provider_call_ids
                        .get(&event.id())
                        .cloned()
                        .unwrap_or_else(|| event.id().to_string());
                    Some(json!({
                        "type": "function_call",
                        "id": event.id().to_string(),
                        "call_id": call_id,
                        "name": request.tool_name().as_str(),
                        "arguments": serde_json::to_string(request.arguments())
                            .expect("the tool request arguments should serialize"),
                    }))
                }
                ConversationEventPayload::ToolResponse(response) => {
                    let call_id = provider_call_ids
                        .get(&response.tool_request_id())
                        .cloned()
                        .unwrap_or_else(|| response.tool_request_id().to_string());
                    Some(json!({
                        "type": "function_call_output",
                        "id": event.id().to_string(),
                        "call_id": call_id,
                        "output": serde_json::to_string(response.outcome())
                            .expect("the tool response outcome should serialize"),
                    }))
                }
                ConversationEventPayload::TurnStart(_)
                | ConversationEventPayload::TurnEnd(_)
                | ConversationEventPayload::ModelRequest(_)
                | ConversationEventPayload::ModelResponse(_)
                | ConversationEventPayload::Automation(_)
                | ConversationEventPayload::Context(_)
                | ConversationEventPayload::Tools(_) => None,
            }
        })
        .collect::<Vec<_>>();
    Value::Array(input)
}

fn replay_reasoning_item(specific_event: &ModelSpecificEvent) -> Option<Value> {
    if specific_event.provider_event_type() != REASONING_ITEM_EVENT_TYPE
        || specific_event.provider_payload_version() != PROVIDER_PAYLOAD_VERSION
    {
        return None;
    }
    let payload = specific_event.payload();
    let item_id = payload.get("item_id").and_then(Value::as_str)?;
    let summary = payload.get("summary").and_then(Value::as_array)?;
    if !summary.iter().all(is_summary_text) {
        return None;
    }
    let mut item = json!({ "type": "reasoning", "id": item_id });
    item["summary"] = Value::Array(summary.clone());
    if let Some(encrypted_content) = payload.get("encrypted_content").and_then(Value::as_str) {
        item["encrypted_content"] = Value::String(encrypted_content.to_owned());
    }
    Some(item)
}

fn is_summary_text(value: &Value) -> bool {
    value.get("type").and_then(Value::as_str) == Some("summary_text") && value.get("text").is_some()
}

fn provider_tool(definition: &tog::conversation::ToolDefinition) -> Value {
    json!({
        "type": "function",
        "name": definition.name().as_str(),
        "description": definition.description(),
        "parameters": provider_schema(definition.parameters()),
    })
}

fn provider_schema(schema: &Schema) -> Value {
    let mut schema = schema.as_value().clone();
    if let Value::Object(schema_object) = &mut schema {
        schema_object.remove("$schema");
    }
    schema
}

fn provider_tool_call_ids(
    events: &[ConversationEventRecord],
) -> HashMap<ConversationEventId, String> {
    events
        .iter()
        .filter_map(
            |conversation_event| match conversation_event.event().payload() {
                ConversationEventPayload::ToolRequest(request) => Some((
                    conversation_event.event().id(),
                    provider_tool_call_id(request),
                )),
                _ => None,
            },
        )
        .collect()
}

fn provider_tool_call_id(request: &ToolRequest) -> String {
    request
        .data()
        .and_then(|data| data.content().get("call_id"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_default()
}

fn classify_response_failure(
    status: StatusCode,
    body: String,
) -> Result<(), OpenRouterResponsesError> {
    if status == StatusCode::BAD_REQUEST && is_context_limit_error(&body) {
        return Ok(());
    }

    Err(match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            OpenRouterResponsesError::Authentication(body)
        }
        StatusCode::TOO_MANY_REQUESTS => OpenRouterResponsesError::RateLimited(body),
        StatusCode::BAD_REQUEST | StatusCode::NOT_FOUND | StatusCode::UNPROCESSABLE_ENTITY => {
            OpenRouterResponsesError::InvalidRequest(body)
        }
        _ => OpenRouterResponsesError::Provider(format!(
            "OpenRouter Responses returned {status}: {body}"
        )),
    })
}

fn is_context_limit_error(body: &str) -> bool {
    serde_json::from_str::<Value>(body)
        .ok()
        .is_some_and(|payload| is_context_limit_payload(&payload))
}

fn is_context_limit_payload(payload: &Value) -> bool {
    let error_code = payload
        .pointer("/code")
        .or_else(|| payload.pointer("/error/code"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let error_message = payload
        .pointer("/error/message")
        .or_else(|| payload.pointer("/message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    [
        "context_length",
        "context_window",
        "maximum context",
        "too many tokens",
        "context_length_exceeded",
        "context_window_exceeded",
    ]
    .into_iter()
    .any(|needle| error_code.contains(needle) || error_message.contains(needle))
}

fn context_limit_stream(model_request_id: ConversationEventId) -> ModelOutputStream {
    terminal_model_output(
        stream::once(async move {
            Ok(ModelDriverEvent::Terminal {
                outcome: failed_outcome(
                    FailureCategory::ContextLimitExceeded,
                    "The model context limit was exceeded.",
                ),
                continuation: ModelContinuation::Complete,
                usage: None,
            })
        })
        .boxed(),
        model_request_id,
    )
}

#[derive(Clone, Copy)]
enum FailureStage {
    BeforeStream,
    DuringStream,
}

fn failed_terminal_stream(
    model_request_id: ConversationEventId,
    failure_stage: FailureStage,
    error: OpenRouterResponsesError,
) -> ModelOutputStream {
    let (category, message) = provider_failure(&error, failure_stage);
    terminal_model_output(
        stream::once(async move {
            Ok(ModelDriverEvent::Terminal {
                outcome: failed_outcome(category, &message),
                continuation: ModelContinuation::Complete,
                usage: None,
            })
        })
        .boxed(),
        model_request_id,
    )
}

fn failed_outcome(category: FailureCategory, message: &str) -> ModelOutcome {
    ModelOutcome::Failed {
        failure: OperationFailure::try_new(category, message.to_owned(), None)
            .expect("the terminal failure should be valid"),
    }
}

fn provider_failure(
    error: &OpenRouterResponsesError,
    failure_stage: FailureStage,
) -> (FailureCategory, String) {
    match error {
        OpenRouterResponsesError::Authentication(_) => (
            FailureCategory::Authentication,
            "The model provider could not authenticate the invocation.".to_owned(),
        ),
        OpenRouterResponsesError::RateLimited(_) => (
            FailureCategory::RateLimited,
            "The model provider rate-limited the invocation.".to_owned(),
        ),
        OpenRouterResponsesError::Transport(_)
            if matches!(failure_stage, FailureStage::DuringStream) =>
        {
            (
                FailureCategory::StreamInterrupted,
                "The model response stream was interrupted.".to_owned(),
            )
        }
        OpenRouterResponsesError::Transport(_) => (
            FailureCategory::Transport,
            "The model provider could not be reached.".to_owned(),
        ),
        OpenRouterResponsesError::InvalidRequest(_) => (
            FailureCategory::InvalidRequest,
            "The model invocation request was invalid.".to_owned(),
        ),
        OpenRouterResponsesError::InvalidResponse(_) => (
            FailureCategory::InvalidProviderResponse,
            "The model provider returned an invalid response.".to_owned(),
        ),
        OpenRouterResponsesError::StreamInterrupted(_) => (
            FailureCategory::StreamInterrupted,
            "The model response stream was interrupted.".to_owned(),
        ),
        OpenRouterResponsesError::Provider(_) => (
            FailureCategory::ProviderFailure,
            "The model provider failed the invocation.".to_owned(),
        ),
    }
}

struct ConversationEventStreamState {
    provider_events: ProviderOutputStream,
    model_request_id: ConversationEventId,
    pending_batches: VecDeque<ModelDriverOutputBatch>,
    terminated: bool,
}

fn conversation_event_stream(
    provider_events: ProviderOutputStream,
    model_request_id: ConversationEventId,
) -> ModelOutputStream {
    stream::unfold(
        ConversationEventStreamState {
            provider_events,
            model_request_id,
            pending_batches: VecDeque::new(),
            terminated: false,
        },
        |mut state| async move {
            if let Some(batch) = state.pending_batches.pop_front() {
                return Some((Ok(batch), state));
            }
            if state.terminated {
                return None;
            }
            match state.provider_events.next().await {
                Some(Ok(driver_event)) => {
                    if let Err(error) = translate_model_driver_event(
                        driver_event,
                        state.model_request_id,
                        &mut state,
                    ) {
                        fail_stream(&mut state, error);
                    }
                    state
                        .pending_batches
                        .pop_front()
                        .map(|batch| (Ok(batch), state))
                }
                Some(Err(error)) => {
                    fail_stream(&mut state, error);
                    state
                        .pending_batches
                        .pop_front()
                        .map(|batch| (Ok(batch), state))
                }
                None => None,
            }
        },
    )
    .boxed()
}

fn terminal_model_output(
    provider_events: ProviderOutputStream,
    model_request_id: ConversationEventId,
) -> ModelOutputStream {
    conversation_event_stream(provider_events, model_request_id)
}

fn fail_stream(state: &mut ConversationEventStreamState, error: OpenRouterResponsesError) {
    let (category, message) = provider_failure(&error, FailureStage::DuringStream);
    let response = terminal_model_response(
        state.model_request_id,
        failed_outcome(category, &message),
        ModelContinuation::Complete,
        None,
    )
    .expect("the failed terminal model response should be valid");
    state.pending_batches = VecDeque::from([ModelDriverOutputBatch::from(
        ModelDriverOutput::ModelResponse(response),
    )]);
    state.terminated = true;
}

fn terminal_model_response(
    model_request_id: ConversationEventId,
    outcome: ModelOutcome,
    continuation: ModelContinuation,
    usage: Option<Usage>,
) -> Result<ModelResponse, InvalidModelResponse> {
    ModelResponse::new(model_request_id, Vec::new(), outcome, continuation, usage)
}

fn translate_model_driver_event(
    driver_event: ModelDriverEvent,
    model_request_id: ConversationEventId,
    state: &mut ConversationEventStreamState,
) -> Result<(), OpenRouterResponsesError> {
    let output = match driver_event {
        ModelDriverEvent::AssistantResponse { content } => ModelDriverOutput::AssistantResponse(
            AssistantResponse::new(model_request_id, content)
                .map_err(invalid_assistant_response)?,
        ),
        ModelDriverEvent::ModelSpecificEvent {
            event_type,
            payload,
            message,
            persistence,
        } => {
            let specific_event = match persistence {
                ConversationEventPersistence::Durable => ModelSpecificEvent::new(
                    model_request_id,
                    event_type,
                    PROVIDER_PAYLOAD_VERSION,
                    payload,
                    message,
                ),
                ConversationEventPersistence::Unpersisted => ModelSpecificEvent::new_unpersisted(
                    model_request_id,
                    event_type,
                    PROVIDER_PAYLOAD_VERSION,
                    payload,
                    message,
                ),
            }
            .map_err(invalid_model_specific_event)?;
            ModelDriverOutput::ModelSpecificEvent(specific_event)
        }
        ModelDriverEvent::ToolRequest {
            tool_name,
            arguments,
            provider_call_id,
        } => {
            let data = provider_call_id
                .map(|call_id| {
                    ModelData::new(Map::from_iter([(
                        "call_id".to_owned(),
                        Value::String(call_id),
                    )]))
                })
                .transpose()
                .map_err(|error| OpenRouterResponsesError::InvalidResponse(error.to_string()))?;
            ModelDriverOutput::ToolRequest(
                ToolRequest::try_new(model_request_id, tool_name, arguments, data)
                    .map_err(invalid_tool_request)?,
            )
        }
        ModelDriverEvent::Terminal {
            outcome,
            continuation,
            usage,
        } => {
            let response = terminal_model_response(model_request_id, outcome, continuation, usage)
                .map_err(invalid_model_response)?;
            state.pending_batches = VecDeque::from([ModelDriverOutputBatch::from(
                ModelDriverOutput::ModelResponse(response),
            )]);
            state.terminated = true;
            return Ok(());
        }
    };
    state
        .pending_batches
        .push_back(ModelDriverOutputBatch::from(output));
    Ok(())
}

fn invalid_assistant_response(error: InvalidAssistantResponse) -> OpenRouterResponsesError {
    OpenRouterResponsesError::InvalidResponse(error.to_string())
}

fn invalid_model_specific_event(error: InvalidModelSpecificEvent) -> OpenRouterResponsesError {
    OpenRouterResponsesError::InvalidResponse(error.to_string())
}

fn invalid_tool_request(error: InvalidToolRequest) -> OpenRouterResponsesError {
    OpenRouterResponsesError::InvalidResponse(error.to_string())
}

fn invalid_model_response(error: InvalidModelResponse) -> OpenRouterResponsesError {
    OpenRouterResponsesError::InvalidResponse(error.to_string())
}

#[derive(Default)]
struct ResponseState {
    assistant_outputs: Vec<AccumulatedText>,
    function_calls: Vec<AccumulatedFunctionCall>,
    reasoning_items: Vec<AccumulatedReasoningItem>,
    refusal: Option<String>,
    usage: Option<Usage>,
    completed: bool,
}

struct AccumulatedFunctionCall {
    key: String,
    call_id: Option<String>,
    name: Option<String>,
    completed_arguments: Option<String>,
    emitted: bool,
}

struct AccumulatedText {
    key: String,
    streamed_text: String,
    completed_text: Option<String>,
    emitted: bool,
}

struct AccumulatedReasoningItem {
    key: String,
    item_id: Option<String>,
    encrypted_content: Option<String>,
    summary: Vec<Value>,
    provider_summary: bool,
    emitted: bool,
}

struct ServerSentEvent {
    name: Option<String>,
    data: String,
}

#[derive(Default)]
struct ServerSentEventDecoder {
    bytes: Vec<u8>,
    event_name: Option<String>,
    data_lines: Vec<String>,
    events: VecDeque<ServerSentEvent>,
}

impl ServerSentEventDecoder {
    fn push(&mut self, bytes: &[u8]) -> Result<(), OpenRouterResponsesError> {
        self.bytes.extend_from_slice(bytes);
        while let Some(newline_position) = self.bytes.iter().position(|byte| *byte == b'\n') {
            let mut line = self.bytes.drain(..=newline_position).collect::<Vec<_>>();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            self.process_line(line)?;
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<(), OpenRouterResponsesError> {
        if !self.bytes.is_empty() {
            let mut line = std::mem::take(&mut self.bytes);
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            self.process_line(line)?;
        }
        self.dispatch_event();
        Ok(())
    }

    fn process_line(&mut self, line: Vec<u8>) -> Result<(), OpenRouterResponsesError> {
        let line = String::from_utf8(line)
            .map_err(|error| OpenRouterResponsesError::InvalidResponse(error.to_string()))?;
        if line.is_empty() {
            self.dispatch_event();
            return Ok(());
        }
        if line.starts_with(':') {
            return Ok(());
        }

        let (field, value) = line
            .split_once(':')
            .map_or((line.as_str(), ""), |(field, value)| {
                (field, value.strip_prefix(' ').unwrap_or(value))
            });
        match field {
            "event" => self.event_name = Some(value.to_owned()),
            "data" => self.data_lines.push(value.to_owned()),
            _ => {}
        }
        Ok(())
    }

    fn dispatch_event(&mut self) {
        if !self.data_lines.is_empty() {
            self.events.push_back(ServerSentEvent {
                name: self.event_name.take(),
                data: self.data_lines.join("\n"),
            });
            self.data_lines.clear();
        } else {
            self.event_name = None;
        }
    }
}

struct OpenRouterResponsesStreamState {
    response_bytes: ResponseByteStream,
    decoder: ServerSentEventDecoder,
    response: ResponseState,
    model_outputs: VecDeque<ModelDriverEvent>,
    response_end: Option<ResponseEnd>,
    terminated: bool,
}

#[derive(Clone, Copy)]
enum ResponseEnd {
    BodyEnded,
    DoneSentinel,
}

fn model_output_stream(response_bytes: ResponseByteStream) -> ProviderOutputStream {
    let state = OpenRouterResponsesStreamState {
        response_bytes,
        decoder: ServerSentEventDecoder::default(),
        response: ResponseState::default(),
        model_outputs: VecDeque::new(),
        response_end: None,
        terminated: false,
    };

    stream::unfold(state, |mut state| async move {
        loop {
            if let Some(model_output) = state.model_outputs.pop_front() {
                return Some((Ok(model_output), state));
            }
            if state.terminated {
                return None;
            }
            if let Some(server_sent_event) = state.decoder.events.pop_front() {
                match process_event(server_sent_event, &mut state.response) {
                    Ok(ProcessEventResult::Outputs(model_outputs)) => {
                        if state.response.completed {
                            state.terminated = true;
                        }
                        state.model_outputs.extend(model_outputs);
                    }
                    Ok(ProcessEventResult::Done) => {
                        state.decoder.events.clear();
                        state.response_end = Some(ResponseEnd::DoneSentinel);
                    }
                    Err(error) => {
                        state.terminated = true;
                        return Some((Err(error), state));
                    }
                }
                continue;
            }
            if let Some(response_end) = state.response_end {
                state.terminated = true;
                if state.response.completed {
                    return None;
                }
                let error = match response_end {
                    ResponseEnd::BodyEnded => OpenRouterResponsesError::StreamInterrupted(
                        "the response body ended before a terminal response event".to_owned(),
                    ),
                    ResponseEnd::DoneSentinel => OpenRouterResponsesError::InvalidResponse(
                        "the stream ended before a terminal response event".to_owned(),
                    ),
                };
                return Some((Err(error), state));
            }

            match state.response_bytes.next().await {
                Some(Ok(bytes)) => {
                    if let Err(error) = state.decoder.push(&bytes) {
                        state.terminated = true;
                        return Some((Err(error), state));
                    }
                }
                Some(Err(error)) => {
                    state.terminated = true;
                    return Some((Err(error), state));
                }
                None => {
                    if let Err(error) = state.decoder.finish() {
                        state.terminated = true;
                        return Some((Err(error), state));
                    }
                    state.response_end = Some(ResponseEnd::BodyEnded);
                }
            }
        }
    })
    .boxed()
}

enum ProcessEventResult {
    Outputs(Vec<ModelDriverEvent>),
    Done,
}

fn process_event(
    server_sent_event: ServerSentEvent,
    response_state: &mut ResponseState,
) -> Result<ProcessEventResult, OpenRouterResponsesError> {
    if server_sent_event.data == "[DONE]" {
        return Ok(ProcessEventResult::Done);
    }

    let payload: Value = serde_json::from_str(&server_sent_event.data)
        .map_err(|error| OpenRouterResponsesError::InvalidResponse(error.to_string()))?;
    let payload_event_type = match payload.get("type") {
        Some(Value::String(event_type)) => Some(event_type.clone()),
        Some(_) => {
            return Err(OpenRouterResponsesError::InvalidResponse(
                "an OpenRouter stream event contained a non-string type".to_owned(),
            ));
        }
        None => None,
    };
    if let (Some(payload_event_type), Some(server_sent_event_name)) =
        (&payload_event_type, &server_sent_event.name)
        && payload_event_type != server_sent_event_name
    {
        return Err(OpenRouterResponsesError::InvalidResponse(format!(
            "OpenRouter stream event type {server_sent_event_name} did not match payload type {payload_event_type}"
        )));
    }
    let event_type = payload_event_type
        .or(server_sent_event.name)
        .unwrap_or_else(|| "unknown".to_owned());

    if response_state.completed {
        return Err(OpenRouterResponsesError::InvalidResponse(format!(
            "OpenRouter stream emitted {event_type} after the response completed"
        )));
    }

    let model_events = match event_type.as_str() {
        "response.output_text.delta" => {
            let key = semantic_output_key(&payload, &["output_index", "content_index"])?;
            let delta = append_text_delta(
                &payload,
                accumulated_text(&mut response_state.assistant_outputs, key)?,
            )?;
            content_delta_events(&delta)
        }
        "response.content_part.added" | "response.content_part.delta" => {
            output_text_part_delta(&payload, response_state)?
        }
        "response.output_text.done" => {
            let key = semantic_output_key(&payload, &["output_index", "content_index"])?;
            complete_text(
                &payload,
                "text",
                accumulated_text(&mut response_state.assistant_outputs, key.clone())?,
            )?;
            emit_assistant_response(&mut response_state.assistant_outputs, &key)?
                .into_iter()
                .collect()
        }
        "response.content_part.done" => output_text_part_done(&payload, response_state)?,
        "response.output_item.added" => {
            register_output_item(&payload, response_state);
            Vec::new()
        }
        "response.function_call_arguments.delta" => {
            process_function_call_delta(&payload, response_state)?
        }
        "response.function_call_arguments.done" => {
            let key = semantic_output_key(&payload, &["output_index"])?;
            complete_function_call_arguments(&payload, &mut response_state.function_calls, &key)?;
            emit_function_call(&mut response_state.function_calls, &key)?
                .into_iter()
                .collect()
        }
        "response.output_item.done" => complete_output_item(&payload, response_state)?,
        "response.reasoning_text.delta" => {
            let delta = payload
                .get("delta")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if delta.is_empty() {
                Vec::new()
            } else {
                vec![partial_event(
                    REASONING_DELTA_EVENT_TYPE,
                    json!({}),
                    Some(delta.to_owned()),
                )]
            }
        }
        "response.reasoning_summary_text.delta" => {
            let delta = payload
                .get("delta")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if delta.is_empty() {
                Vec::new()
            } else {
                vec![partial_event(
                    REASONING_SUMMARY_DELTA_EVENT_TYPE,
                    json!({}),
                    Some(delta.to_owned()),
                )]
            }
        }
        "response.reasoning_summary_text.done" => {
            summarize_reasoning(&payload, response_state)?;
            Vec::new()
        }
        "response.reasoning_summary_part.added" | "response.reasoning_summary_part.done" => {
            summarize_reasoning_part(&payload, response_state)?;
            Vec::new()
        }
        "response.refusal.delta" => {
            accumulate_refusal_delta(&payload, response_state);
            Vec::new()
        }
        "response.refusal.done" => {
            let terminal = refusal_terminal(response_state, &payload)?;
            if terminal.is_some() {
                response_state.completed = true;
            }
            terminal.into_iter().collect()
        }
        "response.completed" => complete_response(response_state, &payload)?,
        "response.incomplete" => incomplete_response(response_state, &payload)?,
        "error" | "response.failed" => failed_response(response_state, &payload)?,
        _ => Vec::new(),
    };
    Ok(ProcessEventResult::Outputs(model_events))
}

fn partial_event(event_type: &str, payload: Value, message: Option<String>) -> ModelDriverEvent {
    ModelDriverEvent::ModelSpecificEvent {
        event_type: event_type.to_owned(),
        payload,
        message,
        persistence: ConversationEventPersistence::Unpersisted,
    }
}

fn content_delta_events(delta: &str) -> Vec<ModelDriverEvent> {
    if delta.is_empty() {
        return Vec::new();
    }
    vec![partial_event(
        CONTENT_DELTA_EVENT_TYPE,
        json!({}),
        Some(delta.to_owned()),
    )]
}

fn output_text_part_delta(
    payload: &Value,
    response_state: &mut ResponseState,
) -> Result<Vec<ModelDriverEvent>, OpenRouterResponsesError> {
    let Some(part) = output_text_part(payload) else {
        return Ok(Vec::new());
    };
    let delta = part.get("text").and_then(Value::as_str).unwrap_or_default();
    let key = semantic_output_key(payload, &["output_index", "content_index"])?;
    let output = accumulated_text(&mut response_state.assistant_outputs, key)?;
    output.streamed_text.push_str(delta);
    Ok(content_delta_events(delta))
}

fn output_text_part_done(
    payload: &Value,
    response_state: &mut ResponseState,
) -> Result<Vec<ModelDriverEvent>, OpenRouterResponsesError> {
    let Some(part) = output_text_part(payload) else {
        return Ok(Vec::new());
    };
    let key = semantic_output_key(payload, &["output_index", "content_index"])?;
    if let Some(output) = response_state
        .assistant_outputs
        .iter_mut()
        .find(|output| output.key == key)
        && !output.emitted
    {
        output.completed_text = Some(
            part.get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        );
    }
    Ok(
        emit_assistant_response(&mut response_state.assistant_outputs, &key)?
            .into_iter()
            .collect(),
    )
}

fn output_text_part(payload: &Value) -> Option<&Value> {
    payload
        .get("part")
        .filter(|part| part.get("type").and_then(Value::as_str) == Some("output_text"))
}

fn register_output_item(payload: &Value, response_state: &mut ResponseState) {
    let Some(item) = payload.get("item") else {
        return;
    };
    if item.get("type").and_then(Value::as_str) != Some("function_call") {
        return;
    }
    let Ok(key) = semantic_output_key(payload, &["output_index"]) else {
        return;
    };
    let call_id = item
        .get("call_id")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let name = item.get("name").and_then(Value::as_str).map(str::to_owned);
    upsert_function_call(&mut response_state.function_calls, key, call_id, name, None);
}

fn process_function_call_delta(
    payload: &Value,
    response_state: &mut ResponseState,
) -> Result<Vec<ModelDriverEvent>, OpenRouterResponsesError> {
    let delta = payload
        .get("delta")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if delta.is_empty() {
        return Ok(Vec::new());
    }
    let key = semantic_output_key(payload, &["output_index"])?;
    let correlation_payload = response_state
        .function_calls
        .iter()
        .find(|function_call| function_call.key == key)
        .and_then(|function_call| function_call.call_id.clone())
        .map(|call_id| json!({ "call_id": call_id }))
        .unwrap_or_else(|| json!({}));
    Ok(vec![partial_event(
        TOOL_CALL_ARGUMENTS_DELTA_EVENT_TYPE,
        correlation_payload,
        Some(delta.to_owned()),
    )])
}

fn summarize_reasoning(
    payload: &Value,
    response_state: &mut ResponseState,
) -> Result<(), OpenRouterResponsesError> {
    let summary_text = payload
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if summary_text.is_empty() {
        return Ok(());
    }
    let key = semantic_output_key(payload, &["output_index"])?;
    upsert_summary(response_state, &key, summary_text);
    Ok(())
}

fn summarize_reasoning_part(
    payload: &Value,
    response_state: &mut ResponseState,
) -> Result<(), OpenRouterResponsesError> {
    let Some(part) = payload.get("part") else {
        return Ok(());
    };
    let Some(text) = part.get("text").and_then(Value::as_str) else {
        return Ok(());
    };
    if text.is_empty() {
        return Ok(());
    }
    let key = semantic_output_key(payload, &["output_index"])?;
    upsert_summary(response_state, &key, text);
    Ok(())
}

fn upsert_summary(response_state: &mut ResponseState, key: &str, summary_text: &str) {
    let reasoning_item = accumulated_reasoning_item(&mut response_state.reasoning_items, key);
    if reasoning_item.provider_summary {
        return;
    }
    let entry = json!({ "type": "summary_text", "text": summary_text });
    if !reasoning_item.summary.contains(&entry) {
        reasoning_item.summary.push(entry);
    }
}

fn accumulate_refusal_delta(payload: &Value, response_state: &mut ResponseState) {
    if let Some(delta) = payload.get("delta").and_then(Value::as_str) {
        let refusal = response_state.refusal.take().unwrap_or_default();
        response_state.refusal = Some(refusal + delta);
    }
}

fn refusal_terminal(
    response_state: &mut ResponseState,
    payload: &Value,
) -> Result<Option<ModelDriverEvent>, OpenRouterResponsesError> {
    let completed_refusal = payload
        .get("refusal")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let refusal = if !completed_refusal.is_empty() {
        completed_refusal.to_owned()
    } else {
        response_state.refusal.take().unwrap_or_default()
    };
    if refusal.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(ModelDriverEvent::Terminal {
        outcome: failed_outcome(FailureCategory::Refusal, &refusal),
        continuation: ModelContinuation::Complete,
        usage: response_state.usage.take(),
    }))
}

fn complete_output_item(
    payload: &Value,
    response_state: &mut ResponseState,
) -> Result<Vec<ModelDriverEvent>, OpenRouterResponsesError> {
    let Some(item) = payload.get("item") else {
        return Ok(Vec::new());
    };
    let item_type = item.get("type").and_then(Value::as_str);
    let key = semantic_output_key(payload, &["output_index"])?;
    match item_type {
        Some("reasoning") => {
            capture_reasoning_item(item, &key, response_state);
            Ok(
                emit_reasoning_item(&mut response_state.reasoning_items, &key)?
                    .into_iter()
                    .collect(),
            )
        }
        Some("function_call") => {
            let call_id = item
                .get("call_id")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let name = item.get("name").and_then(Value::as_str).map(str::to_owned);
            let completed_arguments = completed_item_arguments(item)?;
            upsert_function_call(
                &mut response_state.function_calls,
                key.clone(),
                call_id,
                name,
                completed_arguments,
            );
            Ok(
                emit_function_call(&mut response_state.function_calls, &key)?
                    .into_iter()
                    .collect(),
            )
        }
        _ => Ok(Vec::new()),
    }
}

fn capture_reasoning_item(item: &Value, key: &str, response_state: &mut ResponseState) {
    let reasoning_item = accumulated_reasoning_item(&mut response_state.reasoning_items, key);
    if reasoning_item.item_id.is_none() {
        reasoning_item.item_id = item.get("id").and_then(Value::as_str).map(str::to_owned);
    }
    if reasoning_item.encrypted_content.is_none() {
        reasoning_item.encrypted_content = item
            .get("encrypted_content")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .filter(|content| !content.is_empty());
    }
    if let Some(summary) = item.get("summary").and_then(Value::as_array)
        && summary.iter().all(is_summary_text)
    {
        reasoning_item.summary = summary.clone();
        reasoning_item.provider_summary = true;
    }
}

fn emit_reasoning_item(
    reasoning_items: &mut [AccumulatedReasoningItem],
    key: &str,
) -> Result<Option<ModelDriverEvent>, OpenRouterResponsesError> {
    let reasoning_item = reasoning_items
        .iter_mut()
        .find(|reasoning_item| reasoning_item.key == key)
        .expect("the accumulated reasoning item should exist");
    if reasoning_item.emitted {
        return Ok(None);
    }
    if !has_reasoning_item_data(reasoning_item) {
        return Ok(None);
    }
    let mut payload = Map::new();
    if let Some(item_id) = &reasoning_item.item_id {
        payload.insert("item_id".to_owned(), Value::String(item_id.clone()));
    }
    if let Some(encrypted_content) = &reasoning_item.encrypted_content {
        payload.insert(
            "encrypted_content".to_owned(),
            Value::String(encrypted_content.clone()),
        );
    }
    payload.insert(
        "summary".to_owned(),
        Value::Array(reasoning_item.summary.clone()),
    );
    let message = reasoning_message(reasoning_item);
    reasoning_item.emitted = true;
    Ok(Some(ModelDriverEvent::ModelSpecificEvent {
        event_type: REASONING_ITEM_EVENT_TYPE.to_owned(),
        payload: Value::Object(payload),
        message,
        persistence: ConversationEventPersistence::Durable,
    }))
}

fn has_reasoning_item_data(reasoning_item: &AccumulatedReasoningItem) -> bool {
    reasoning_item
        .item_id
        .as_deref()
        .is_some_and(|item_id| !item_id.is_empty())
        || !reasoning_item.summary.is_empty()
        || reasoning_item
            .encrypted_content
            .as_deref()
            .is_some_and(|content| !content.is_empty())
}

fn reasoning_message(reasoning_item: &AccumulatedReasoningItem) -> Option<String> {
    if reasoning_item.summary.is_empty() {
        return None;
    }
    let message = reasoning_item
        .summary
        .iter()
        .filter_map(|summary| summary.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    (!message.is_empty()).then_some(message)
}

fn complete_response(
    response_state: &mut ResponseState,
    payload: &Value,
) -> Result<Vec<ModelDriverEvent>, OpenRouterResponsesError> {
    if response_state.completed {
        return Ok(Vec::new());
    }
    capture_usage(response_state, payload);

    let mut model_events = Vec::new();
    let completed_content = completed_response_content(payload)?;
    for completed_reasoning_item in completed_content.reasoning_items {
        let reasoning_item = accumulated_reasoning_item(
            &mut response_state.reasoning_items,
            &completed_reasoning_item.key,
        );
        if reasoning_item.item_id.is_none() {
            reasoning_item.item_id = Some(completed_reasoning_item.item_id.clone());
        }
        if reasoning_item.encrypted_content.is_none() {
            reasoning_item.encrypted_content = completed_reasoning_item.encrypted_content.clone();
        }
        if reasoning_item.summary.is_empty() {
            reasoning_item.summary = completed_reasoning_item.summary.clone();
            reasoning_item.provider_summary = true;
        }
        if let Some(event) = emit_reasoning_item(
            &mut response_state.reasoning_items,
            &completed_reasoning_item.key,
        )? {
            model_events.push(event);
        }
    }
    for completed_assistant_output in completed_content.assistant_outputs {
        if completed_output_already_emitted(
            &response_state.assistant_outputs,
            &completed_assistant_output.text,
        ) {
            continue;
        }
        if let Some(event) = emit_completed_text(
            &mut response_state.assistant_outputs,
            &completed_assistant_output.key,
            completed_assistant_output.text,
        )? {
            model_events.push(event);
        }
    }
    for completed_function_call in completed_content.function_calls {
        upsert_function_call(
            &mut response_state.function_calls,
            completed_function_call.key.clone(),
            completed_function_call.call_id,
            completed_function_call.name,
            Some(completed_function_call.arguments),
        );
        if let Some(event) = emit_function_call(
            &mut response_state.function_calls,
            &completed_function_call.key,
        )? {
            model_events.push(event);
        }
    }
    for key in un_emitted_keys(&response_state.assistant_outputs) {
        if let Some(event) = emit_assistant_response(&mut response_state.assistant_outputs, &key)? {
            model_events.push(event);
        }
    }
    for key in un_emitted_reasoning_keys(&response_state.reasoning_items) {
        if let Some(event) = emit_reasoning_item(&mut response_state.reasoning_items, &key)? {
            model_events.push(event);
        }
    }

    let has_model_output = response_state
        .assistant_outputs
        .iter()
        .any(|output| output.emitted)
        || response_state
            .function_calls
            .iter()
            .any(|function_call| function_call.emitted)
        || response_state
            .reasoning_items
            .iter()
            .any(|reasoning_item| reasoning_item.emitted);
    if !has_model_output {
        return Err(OpenRouterResponsesError::InvalidResponse(
            "the completed response contained no model message".to_owned(),
        ));
    }
    let has_requested_tools = response_state
        .function_calls
        .iter()
        .any(|function_call| function_call.emitted);
    let reasoning_only = response_state
        .reasoning_items
        .iter()
        .any(|reasoning_item| reasoning_item.emitted)
        && !response_state
            .assistant_outputs
            .iter()
            .any(|output| output.emitted);
    let continuation = if has_requested_tools || reasoning_only {
        ModelContinuation::Continue
    } else {
        ModelContinuation::Complete
    };
    model_events.push(ModelDriverEvent::Terminal {
        outcome: ModelOutcome::Succeeded,
        continuation,
        usage: response_state.usage.take(),
    });
    response_state.completed = true;
    Ok(model_events)
}

fn incomplete_response(
    response_state: &mut ResponseState,
    payload: &Value,
) -> Result<Vec<ModelDriverEvent>, OpenRouterResponsesError> {
    if response_state.completed {
        return Ok(Vec::new());
    }
    capture_usage(response_state, payload);
    let failed_reason = payload
        .get("response")
        .and_then(|response| response.get("incomplete_details"))
        .or_else(|| payload.get("incomplete_details"))
        .and_then(|details| {
            details
                .get("reason")
                .or_else(|| details.get("failed_reason"))
        })
        .and_then(Value::as_str)
        .unwrap_or_default();
    let (category, continuation) = match failed_reason {
        "content_filter" => (FailureCategory::Refusal, ModelContinuation::Complete),
        "max_output_tokens" | "duration_cutoff" => (
            FailureCategory::ProviderFailure,
            ModelContinuation::Continue,
        ),
        _ => (
            FailureCategory::ProviderFailure,
            ModelContinuation::Complete,
        ),
    };
    let model_events = vec![ModelDriverEvent::Terminal {
        outcome: failed_outcome(
            category,
            "The model response was cut off before completing.",
        ),
        continuation,
        usage: response_state.usage.take(),
    }];
    response_state.completed = true;
    Ok(model_events)
}

fn failed_response(
    response_state: &mut ResponseState,
    payload: &Value,
) -> Result<Vec<ModelDriverEvent>, OpenRouterResponsesError> {
    if response_state.completed {
        return Ok(Vec::new());
    }
    capture_usage(response_state, payload);
    let (category, message) = if is_context_limit_payload(payload) {
        (
            FailureCategory::ContextLimitExceeded,
            "The model context limit was exceeded.",
        )
    } else {
        (
            FailureCategory::ProviderFailure,
            "The model provider failed the invocation.",
        )
    };
    let model_events = vec![ModelDriverEvent::Terminal {
        outcome: failed_outcome(category, message),
        continuation: ModelContinuation::Complete,
        usage: response_state.usage.take(),
    }];
    response_state.completed = true;
    Ok(model_events)
}

fn capture_usage(response_state: &mut ResponseState, payload: &Value) {
    if let Some(usage) = parsed_usage(payload) {
        response_state.usage = Some(usage);
    }
}

fn parsed_usage(payload: &Value) -> Option<Usage> {
    let usage = payload
        .get("usage")
        .or_else(|| payload.pointer("/response/usage"))?;
    if !usage.is_object() {
        return None;
    }
    let token_count = |name: &str| usage.get(name).and_then(Value::as_u64);
    let cache_read_input_tokens = usage
        .pointer("/input_tokens_details/cached_tokens")
        .and_then(Value::as_u64);
    let cache_write_input_tokens = usage
        .pointer("/input_tokens_details/cache_write_tokens")
        .and_then(Value::as_u64);
    let reasoning_output_tokens = usage
        .pointer("/output_tokens_details/reasoning_tokens")
        .and_then(Value::as_u64);
    let cost_usd = usage.get("cost").and_then(Value::as_f64);
    Usage::try_new(
        token_count("input_tokens"),
        token_count("output_tokens"),
        token_count("total_tokens"),
        cache_read_input_tokens,
        cache_write_input_tokens,
        reasoning_output_tokens,
        cost_usd,
    )
    .ok()
}

#[derive(Default)]
struct CompletedResponseContent {
    assistant_outputs: Vec<CompletedText>,
    function_calls: Vec<CompletedFunctionCall>,
    reasoning_items: Vec<CompletedReasoningItem>,
}

struct CompletedText {
    key: String,
    text: String,
}

struct CompletedFunctionCall {
    key: String,
    call_id: Option<String>,
    name: Option<String>,
    arguments: String,
}

struct CompletedReasoningItem {
    key: String,
    item_id: String,
    encrypted_content: Option<String>,
    summary: Vec<Value>,
}

fn completed_response_content(
    payload: &Value,
) -> Result<CompletedResponseContent, OpenRouterResponsesError> {
    let Some(output_value) = payload
        .get("response")
        .and_then(|response| response.get("output"))
    else {
        return Ok(CompletedResponseContent::default());
    };
    let output = output_value.as_array().ok_or_else(|| {
        OpenRouterResponsesError::InvalidResponse(
            "the completed OpenRouter response contained non-array output".to_owned(),
        )
    })?;
    let mut assistant_outputs = Vec::new();
    let mut function_calls = Vec::new();
    let mut reasoning_items = Vec::new();
    for (output_index, output_item) in output.iter().enumerate() {
        let key = format!("output_index={output_index}");
        match output_item.get("type").and_then(Value::as_str) {
            Some("function_call") => {
                let arguments = output_item
                    .get("arguments")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        OpenRouterResponsesError::InvalidResponse(
                            "completed OpenRouter function call arguments were not a string"
                                .to_owned(),
                        )
                    })?;
                function_calls.push(CompletedFunctionCall {
                    key,
                    call_id: output_item
                        .get("call_id")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    name: output_item
                        .get("name")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    arguments: arguments.to_owned(),
                });
            }
            Some("reasoning") => {
                let item_id = output_item
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| key.clone());
                let summary = output_item
                    .get("summary")
                    .and_then(Value::as_array)
                    .filter(|summary| summary.iter().all(is_summary_text))
                    .cloned()
                    .unwrap_or_default();
                reasoning_items.push(CompletedReasoningItem {
                    key,
                    item_id,
                    encrypted_content: output_item
                        .get("encrypted_content")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .filter(|content| !content.is_empty()),
                    summary,
                });
            }
            Some("message") => {
                if let Some(text) = output_item_text(output_item, output_index) {
                    assistant_outputs.push(text);
                }
            }
            _ => {}
        }
    }
    Ok(CompletedResponseContent {
        assistant_outputs,
        function_calls,
        reasoning_items,
    })
}

fn output_item_text(output_item: &Value, output_index: usize) -> Option<CompletedText> {
    let content = output_item.get("content").and_then(Value::as_array)?;
    for (content_index, content_item) in content.iter().enumerate() {
        if content_item.get("type").and_then(Value::as_str) == Some("output_text") {
            let text = content_item.get("text").and_then(Value::as_str)?;
            if text.trim().is_empty() {
                continue;
            }
            return Some(CompletedText {
                key: format!("output_index={output_index};content_index={content_index}"),
                text: text.to_owned(),
            });
        }
    }
    None
}

fn completed_output_already_emitted(outputs: &[AccumulatedText], text: &str) -> bool {
    outputs.iter().any(|output| {
        output.emitted
            && output
                .completed_text
                .as_deref()
                .is_some_and(|completed| completed == text)
    })
}

fn emit_completed_text(
    outputs: &mut Vec<AccumulatedText>,
    key: &str,
    text: String,
) -> Result<Option<ModelDriverEvent>, OpenRouterResponsesError> {
    if let Some(output) = outputs.iter_mut().find(|output| output.key == key) {
        if output.emitted {
            return Ok(None);
        }
        output.completed_text = Some(text);
    } else {
        outputs.push(AccumulatedText {
            key: key.to_owned(),
            streamed_text: String::new(),
            completed_text: Some(text),
            emitted: false,
        });
    }
    emit_assistant_response(outputs, key)
}

fn emit_assistant_response(
    assistant_outputs: &mut [AccumulatedText],
    key: &str,
) -> Result<Option<ModelDriverEvent>, OpenRouterResponsesError> {
    let output = un_emitted_text(assistant_outputs, key)?;
    let assistant_text = preferred_text(&output.streamed_text, &output.completed_text);
    let Some(assistant_text) = assistant_text else {
        return Ok(None);
    };
    if assistant_text.trim().is_empty() {
        return Ok(None);
    }
    output.emitted = true;
    Ok(Some(ModelDriverEvent::AssistantResponse {
        content: assistant_text,
    }))
}

fn un_emitted_text<'a>(
    outputs: &'a mut [AccumulatedText],
    key: &str,
) -> Result<&'a mut AccumulatedText, OpenRouterResponsesError> {
    let output = outputs
        .iter_mut()
        .find(|output| output.key == key)
        .expect("the accumulated output should exist");
    if output.emitted {
        return Err(OpenRouterResponsesError::InvalidResponse(format!(
            "OpenRouter emitted semantic output {key} more than once"
        )));
    }
    Ok(output)
}

fn un_emitted_keys(outputs: &[AccumulatedText]) -> Vec<String> {
    outputs
        .iter()
        .filter(|output| !output.emitted)
        .map(|output| output.key.clone())
        .collect()
}

fn un_emitted_reasoning_keys(reasoning_items: &[AccumulatedReasoningItem]) -> Vec<String> {
    reasoning_items
        .iter()
        .filter(|reasoning_item| !reasoning_item.emitted)
        .map(|reasoning_item| reasoning_item.key.clone())
        .collect()
}

fn accumulated_text(
    outputs: &mut Vec<AccumulatedText>,
    key: String,
) -> Result<&mut AccumulatedText, OpenRouterResponsesError> {
    if let Some(position) = outputs.iter().position(|output| output.key == key) {
        return Ok(&mut outputs[position]);
    }
    if (key == "default" && !outputs.is_empty())
        || (key != "default" && outputs.iter().any(|output| output.key == "default"))
    {
        return Err(OpenRouterResponsesError::InvalidResponse(
            "OpenRouter semantic output identity changed while streaming".to_owned(),
        ));
    }
    outputs.push(AccumulatedText {
        key,
        streamed_text: String::new(),
        completed_text: None,
        emitted: false,
    });
    Ok(outputs
        .last_mut()
        .expect("the accumulated output was just added"))
}

fn accumulated_reasoning_item<'a>(
    reasoning_items: &'a mut Vec<AccumulatedReasoningItem>,
    key: &str,
) -> &'a mut AccumulatedReasoningItem {
    if let Some(position) = reasoning_items
        .iter()
        .position(|reasoning_item| reasoning_item.key == key)
    {
        return &mut reasoning_items[position];
    }
    reasoning_items.push(AccumulatedReasoningItem {
        key: key.to_owned(),
        item_id: None,
        encrypted_content: None,
        summary: Vec::new(),
        provider_summary: false,
        emitted: false,
    });
    reasoning_items
        .last_mut()
        .expect("the accumulated reasoning item was just added")
}

fn complete_text(
    payload: &Value,
    field: &str,
    output: &mut AccumulatedText,
) -> Result<(), OpenRouterResponsesError> {
    if output.emitted {
        return Ok(());
    }
    output.completed_text = text_field(payload, field)?;
    Ok(())
}

fn append_text_delta(
    payload: &Value,
    output: &mut AccumulatedText,
) -> Result<String, OpenRouterResponsesError> {
    if output.emitted {
        return Err(OpenRouterResponsesError::InvalidResponse(format!(
            "OpenRouter emitted a delta after completing semantic output {}",
            output.key
        )));
    }
    let delta = payload
        .get("delta")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            OpenRouterResponsesError::InvalidResponse(
                "an OpenRouter delta event did not contain a string delta".to_owned(),
            )
        })?;
    output.streamed_text.push_str(delta);
    Ok(delta.to_owned())
}

fn text_field(payload: &Value, field: &str) -> Result<Option<String>, OpenRouterResponsesError> {
    let Some(value) = payload.get(field) else {
        return Ok(None);
    };
    value
        .as_str()
        .map(|text| Some(text.to_owned()))
        .ok_or_else(|| {
            OpenRouterResponsesError::InvalidResponse(format!(
                "an OpenRouter completion event contained a non-string {field} field"
            ))
        })
}

fn preferred_text(streamed_text: &str, completed_text: &Option<String>) -> Option<String> {
    completed_text
        .clone()
        .or_else(|| (!streamed_text.is_empty()).then(|| streamed_text.to_owned()))
}

fn complete_function_call_arguments(
    payload: &Value,
    function_calls: &mut Vec<AccumulatedFunctionCall>,
    key: &str,
) -> Result<(), OpenRouterResponsesError> {
    let arguments = payload
        .get("arguments")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            OpenRouterResponsesError::InvalidResponse(
                "a completed function call did not contain string arguments".to_owned(),
            )
        })?
        .to_owned();
    if let Some(function_call) = function_calls
        .iter_mut()
        .find(|function_call| function_call.key == key)
    {
        function_call.completed_arguments = Some(arguments);
    } else {
        function_calls.push(AccumulatedFunctionCall {
            key: key.to_owned(),
            call_id: None,
            name: None,
            completed_arguments: Some(arguments),
            emitted: false,
        });
    }
    Ok(())
}

fn completed_item_arguments(item: &Value) -> Result<Option<String>, OpenRouterResponsesError> {
    match item.get("arguments") {
        Some(Value::String(arguments)) if !arguments.trim().is_empty() => {
            Ok(Some(arguments.clone()))
        }
        Some(Value::String(_)) | None => Ok(None),
        Some(_) => Err(OpenRouterResponsesError::InvalidResponse(
            "a function call contained non-string arguments".to_owned(),
        )),
    }
}

fn upsert_function_call(
    function_calls: &mut Vec<AccumulatedFunctionCall>,
    key: String,
    call_id: Option<String>,
    name: Option<String>,
    completed_arguments: Option<String>,
) {
    if let Some(function_call) = function_calls
        .iter_mut()
        .find(|function_call| function_call.key == key)
    {
        if function_call.call_id.is_none() {
            function_call.call_id = call_id;
        }
        if function_call.name.is_none() {
            function_call.name = name;
        }
        if completed_arguments.is_some() {
            function_call.completed_arguments = completed_arguments;
        }
        return;
    }
    function_calls.push(AccumulatedFunctionCall {
        key,
        call_id,
        name,
        completed_arguments,
        emitted: false,
    });
}

fn emit_function_call(
    function_calls: &mut [AccumulatedFunctionCall],
    key: &str,
) -> Result<Option<ModelDriverEvent>, OpenRouterResponsesError> {
    let function_call = function_calls
        .iter_mut()
        .find(|function_call| function_call.key == key)
        .ok_or_else(|| {
            OpenRouterResponsesError::InvalidResponse(format!(
                "a function call event referenced an unknown call {key}"
            ))
        })?;
    if function_call.emitted {
        return Ok(None);
    }
    let Some(completed_arguments) = function_call.completed_arguments.as_deref() else {
        return Ok(None);
    };
    let name = function_call.name.clone().ok_or_else(|| {
        OpenRouterResponsesError::InvalidResponse(
            "a completed function call did not contain a name".to_owned(),
        )
    })?;
    let arguments = parse_function_call_arguments(completed_arguments)?;
    function_call.emitted = true;
    Ok(Some(ModelDriverEvent::ToolRequest {
        tool_name: ToolName::try_new(name)
            .map_err(|error| OpenRouterResponsesError::InvalidResponse(error.to_string()))?,
        arguments,
        provider_call_id: function_call.call_id.clone(),
    }))
}

fn parse_function_call_arguments(arguments: &str) -> Result<Value, OpenRouterResponsesError> {
    if arguments.trim().is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    serde_json::from_str(arguments).map_err(|error| {
        OpenRouterResponsesError::InvalidResponse(format!(
            "a function call contained invalid JSON arguments: {error}"
        ))
    })
}

fn semantic_output_key(
    payload: &Value,
    indexes: &[&str],
) -> Result<String, OpenRouterResponsesError> {
    let mut key_parts = Vec::new();
    for index_name in indexes {
        if let Some(index) = payload.get(index_name) {
            let index = index.as_u64().ok_or_else(|| {
                OpenRouterResponsesError::InvalidResponse(format!(
                    "an OpenRouter semantic event contained an invalid {index_name}"
                ))
            })?;
            key_parts.push(format!("{index_name}={index}"));
        }
    }
    if !key_parts.is_empty() && key_parts.len() != indexes.len() {
        return Err(OpenRouterResponsesError::InvalidResponse(
            "an OpenRouter semantic event contained incomplete output indexes".to_owned(),
        ));
    }
    if key_parts.len() == indexes.len() {
        return Ok(key_parts.join(";"));
    }

    match payload.get("item_id") {
        Some(Value::String(item_id)) => Ok(format!("item_id={item_id}")),
        Some(_) => Err(OpenRouterResponsesError::InvalidResponse(
            "an OpenRouter semantic event contained a non-string item_id".to_owned(),
        )),
        None => Ok("default".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::str::FromStr;
    use std::thread;

    use futures_util::{StreamExt, stream};
    use reqwest::StatusCode;
    use serde_json::{Map, Value, json};

    use tog::conversation::{
        AssistantResponse, Conversation, ConversationEvent, ConversationEventId,
        ConversationEventPayload, ConversationEventPersistence, ConversationHistory,
        ConversationId, FailureCategory, ModelContinuation, ModelData, ModelId, ModelOutcome,
        ModelRequest, ModelSource, ModelSpecificEvent, ProviderId, ToolOutcome, ToolRequest,
        ToolResponse, TurnStart, User, UserContent,
    };
    use tog::conversation::{ToolDefinition, ToolName};
    use tog::conversation_event_store::ConversationEventRecord;
    use tog::model_driver::{ModelDriver, ModelDriverOutput, ModelOutputStream, TurnInput};

    use super::{
        ModelDriverEvent, OpenRouterResponsesError, OpenRouterResponsesModelDriver,
        PROVIDER_PAYLOAD_VERSION, REASONING_ITEM_EVENT_TYPE, ResponseByteStream,
        classify_response_failure, model_output_stream, provider_tool, responses_input,
    };

    fn conversation_event(
        conversation_id: ConversationId,
        position: u64,
        content: ConversationEventPayload,
    ) -> ConversationEventRecord {
        ConversationEventRecord::new(position, ConversationEvent::new(conversation_id, content))
    }

    fn user_event(
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

    fn turn_and_request(
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

    fn source() -> ModelSource {
        ModelSource::new(
            ProviderId::from_str("openrouter").expect("the provider should be valid"),
            ModelId::from_str("openai/gpt-5.6").expect("the model should be valid"),
        )
    }

    fn reasoning_event(
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

    #[test]
    fn responses_input_projects_canonical_events_and_ignores_auxiliary_records() {
        let conversation_id = ConversationId::new();
        let (turn_start, model_request) = turn_and_request(conversation_id);
        let request_id = model_request.event().id();
        let assistant = AssistantResponse::new(request_id, "Hello.".to_owned())
            .expect("the assistant response should be valid");
        let conversation = ConversationHistory::from_events(vec![
            user_event(conversation_id, 0, "Hello"),
            turn_start,
            model_request,
            conversation_event(
                conversation_id,
                3,
                ConversationEventPayload::AssistantResponse(assistant),
            ),
        ])
        .expect("the conversation should be valid");

        let input = responses_input(conversation.events());
        assert_eq!(
            input,
            json!([
                {
                    "type": "message",
                    "role": "user",
                    "content": [{ "type": "input_text", "text": "Hello" }]
                },
                {
                    "type": "message",
                    "role": "assistant",
                    "id": conversation.events()[3].event().id().to_string(),
                    "status": "completed",
                    "content": [{
                        "type": "output_text",
                        "text": "Hello.",
                        "annotations": []
                    }]
                }
            ])
        );
    }

    fn tool_definition(name: &str) -> ToolDefinition {
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

    #[test]
    fn provider_tools_translate_definitions_without_dropping_schema_metadata() {
        let definition = tool_definition("shell");

        let provider_tool = provider_tool(&definition);

        assert_eq!(provider_tool["type"], "function");
        assert_eq!(provider_tool["name"], "shell");
        assert_eq!(provider_tool["description"], "Run a command.");
        assert_eq!(
            provider_tool["parameters"],
            json!({
                "type": "object",
                "properties": { "command": { "type": "string" } }
            })
        );
        assert!(
            provider_tool["parameters"].get("$schema").is_none(),
            "the provider parameter schema must drop record schema metadata"
        );
        assert!(
            definition.parameters().as_value().get("$schema").is_some(),
            "the recorded definition must keep its schema metadata"
        );
    }

    #[test]
    fn responses_input_replays_reasoning_then_each_model_call_outputs_and_tool_results() {
        let conversation_id = ConversationId::new();
        let (turn_start, model_request) = turn_and_request(conversation_id);
        let request_id = model_request.event().id();
        let reasoning_record = conversation_event(
            conversation_id,
            3,
            reasoning_event(
                request_id,
                "rs_1",
                Some("gAAAA-secret"),
                json!([
                    { "type": "summary_text", "text": "Thinking." }
                ]),
            ),
        );
        let assistant_record = conversation_event(
            conversation_id,
            4,
            ConversationEventPayload::AssistantResponse(
                AssistantResponse::new(request_id, "Let me check.".to_owned())
                    .expect("the assistant response should be valid"),
            ),
        );
        let assistant_id = assistant_record.event().id();
        let tool_request_record = conversation_event(
            conversation_id,
            5,
            ConversationEventPayload::ToolRequest(
                ToolRequest::try_new(
                    request_id,
                    ToolName::try_new("shell".to_owned()).expect("the tool name should be valid"),
                    json!({ "command": "pwd" }),
                    Some(
                        ModelData::new(Map::from_iter([(
                            "call_id".to_owned(),
                            Value::String("call_1".to_owned()),
                        )]))
                        .expect("the model data should be valid"),
                    ),
                )
                .expect("the tool request should be valid"),
            ),
        );
        let tool_request_id = tool_request_record.event().id();
        let tool_response_record = conversation_event(
            conversation_id,
            6,
            ConversationEventPayload::ToolResponse(ToolResponse::new(
                tool_request_id,
                ToolOutcome::succeeded(json!({ "stdout": "/tmp\n" })),
            )),
        );
        let tool_response_id = tool_response_record.event().id();
        let conversation = ConversationHistory::from_events(vec![
            user_event(conversation_id, 0, "Run pwd"),
            turn_start,
            model_request,
            reasoning_record,
            assistant_record,
            tool_request_record,
            tool_response_record,
        ])
        .expect("the conversation should be valid");

        assert_eq!(
            responses_input(conversation.events()),
            json!([
                {
                    "type": "message",
                    "role": "user",
                    "content": [{ "type": "input_text", "text": "Run pwd" }]
                },
                {
                    "type": "reasoning",
                    "id": "rs_1",
                    "summary": [{ "type": "summary_text", "text": "Thinking." }],
                    "encrypted_content": "gAAAA-secret"
                },
                {
                    "type": "message",
                    "role": "assistant",
                    "id": assistant_id.to_string(),
                    "status": "completed",
                    "content": [{
                        "type": "output_text",
                        "text": "Let me check.",
                        "annotations": []
                    }]
                },
                {
                    "type": "function_call",
                    "id": tool_request_id.to_string(),
                    "call_id": "call_1",
                    "name": "shell",
                    "arguments": "{\"command\":\"pwd\"}"
                },
                {
                    "type": "function_call_output",
                    "id": tool_response_id.to_string(),
                    "call_id": "call_1",
                    "output": "{\"type\":\"succeeded\",\"value\":{\"stdout\":\"/tmp\\n\"}}"
                }
            ])
        );
    }

    #[test]
    fn responses_input_skips_incompatible_reasoning_items_and_keeps_portable_outputs() {
        let conversation_id = ConversationId::new();
        let (turn_start, model_request) = turn_and_request(conversation_id);
        let request_id = model_request.event().id();
        let mut incompatible_payload = json!({
            "item_id": "rs_old",
            "summary": [{ "type": "summary_text", "text": "Old." }]
        });
        incompatible_payload["version"] = json!(99);
        let incompatible_event = conversation_event(
            conversation_id,
            3,
            ConversationEventPayload::ModelSpecificEvent(
                ModelSpecificEvent::new(
                    request_id,
                    REASONING_ITEM_EVENT_TYPE.to_owned(),
                    PROVIDER_PAYLOAD_VERSION + 1,
                    incompatible_payload,
                    Some("Old.".to_owned()),
                )
                .expect("the reasoning event should be valid"),
            ),
        );
        let conversation = ConversationHistory::from_events(vec![
            user_event(conversation_id, 0, "Hello"),
            turn_start,
            model_request,
            incompatible_event,
        ])
        .expect("the conversation should be valid");

        assert_eq!(
            responses_input(conversation.events()),
            json!([
                {
                    "type": "message",
                    "role": "user",
                    "content": [{ "type": "input_text", "text": "Hello" }]
                }
            ]),
            "an incompatible reasoning payload must not be replayed"
        );
    }

    fn response_byte_stream(chunks: Vec<Vec<u8>>) -> ResponseByteStream {
        stream::iter(
            chunks
                .into_iter()
                .map(Ok::<Vec<u8>, OpenRouterResponsesError>),
        )
        .boxed()
    }

    async fn collect_driver_events(
        input: &str,
    ) -> Vec<Result<ModelDriverEvent, OpenRouterResponsesError>> {
        model_output_stream(response_byte_stream(vec![input.as_bytes().to_vec()]))
            .collect()
            .await
    }

    #[tokio::test]
    async fn content_deltas_stream_unpersisted_progress_then_an_assistant_response_and_usage() {
        let input = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"item_id\":\"msg_1\",\"output_index\":0,\"content_index\":0,\"delta\":\"Hello \"}\n\n",
            "data: {\"type\":\"response.content_part.delta\",\"response_id\":\"resp_1\",\"output_index\":0,\"content_index\":0,\"item_id\":\"msg_1\",\"part\":{\"type\":\"output_text\",\"text\":\"world\"}}\n\n",
            "data: {\"type\":\"response.output_text.done\",\"item_id\":\"msg_1\",\"output_index\":0,\"content_index\":0,\"text\":\"Hello world\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"status\":\"completed\",\"usage\":{\"input_tokens\":10,\"output_tokens\":5,\"output_tokens_details\":{\"reasoning_tokens\":2},\"input_tokens_details\":{\"cached_tokens\":3},\"total_tokens\":15,\"cost\":0.0002}}}\n\n",
            "data: [DONE]\n\n"
        );

        let events = collect_driver_events(input)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("the response stream should parse");

        assert_eq!(events.len(), 4, "two deltas, answer, then terminal");
        for (position, expected_text) in [(0, "Hello "), (1, "world")] {
            assert!(matches!(
                &events[position],
                ModelDriverEvent::ModelSpecificEvent {
                    event_type,
                    persistence,
                    message: Some(message),
                    ..
                } if event_type == "content_delta"
                    && *persistence == ConversationEventPersistence::Unpersisted
                    && message == expected_text
            ));
        }
        assert!(matches!(
            &events[2],
            ModelDriverEvent::AssistantResponse { content } if content == "Hello world"
        ));
        let ModelDriverEvent::Terminal {
            outcome,
            continuation,
            usage,
        } = &events[3]
        else {
            panic!("the final event should be a terminal");
        };
        assert!(matches!(outcome, ModelOutcome::Succeeded));
        assert_eq!(*continuation, ModelContinuation::Complete);
        let usage = usage.as_ref().expect("usage should be recorded");
        assert_eq!(usage.input_tokens(), Some(10));
        assert_eq!(usage.output_tokens(), Some(5));
        assert_eq!(usage.total_tokens(), Some(15));
        assert_eq!(usage.cache_read_input_tokens(), Some(3));
        assert_eq!(usage.reasoning_output_tokens(), Some(2));
        assert_eq!(usage.cost_usd(), Some(0.0002));
    }

    #[tokio::test]
    async fn parallel_tool_calls_correlate_argument_deltas_and_persist_provider_call_ids() {
        let input = concat!(
            "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"id\":\"fc_1\",\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"shell\",\"arguments\":\"\"}}\n\n",
            "data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_1\",\"output_index\":0,\"delta\":\"{\\\"command\\\":\\\"pwd\\\"}\"}\n\n",
            "data: {\"type\":\"response.output_item.added\",\"output_index\":1,\"item\":{\"id\":\"fc_2\",\"type\":\"function_call\",\"call_id\":\"call_2\",\"name\":\"shell\",\"arguments\":\"\"}}\n\n",
            "data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_2\",\"output_index\":1,\"delta\":\"{\\\"command\\\":\\\"ls\\\"}\"}\n\n",
            "data: {\"type\":\"response.function_call_arguments.done\",\"item_id\":\"fc_1\",\"output_index\":0,\"arguments\":\"{\\\"command\\\":\\\"pwd\\\"}\"}\n\n",
            "data: {\"type\":\"response.function_call_arguments.done\",\"item_id\":\"fc_2\",\"output_index\":1,\"arguments\":\"{\\\"command\\\":\\\"ls\\\"}\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"status\":\"completed\"}}\n\n",
            "data: [DONE]\n\n"
        );

        let events = collect_driver_events(input)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("the tool call stream should parse");

        assert_eq!(events.len(), 5, "two deltas, two requests, then terminal");
        assert!(matches!(
            &events[0],
            ModelDriverEvent::ModelSpecificEvent {
                event_type,
                payload,
                persistence,
                message: Some(message),
            } if event_type == "tool_call_arguments_delta"
                && payload["call_id"] == "call_1"
                && *persistence == ConversationEventPersistence::Unpersisted
                && message == "{\"command\":\"pwd\"}"
        ));
        assert!(matches!(
            &events[1],
            ModelDriverEvent::ModelSpecificEvent {
                event_type,
                payload,
                message: Some(message),
                ..
            } if event_type == "tool_call_arguments_delta"
                && payload["call_id"] == "call_2"
                && message == "{\"command\":\"ls\"}"
        ));
        let ModelDriverEvent::ToolRequest {
            tool_name,
            arguments,
            provider_call_id,
        } = &events[2]
        else {
            panic!("the first completed call should be a tool request");
        };
        assert_eq!(tool_name.as_str(), "shell");
        assert_eq!(arguments, &json!({ "command": "pwd" }));
        assert_eq!(provider_call_id.as_deref(), Some("call_1"));
        let ModelDriverEvent::ToolRequest {
            tool_name,
            arguments,
            provider_call_id,
        } = &events[3]
        else {
            panic!("the second completed call should be a tool request");
        };
        assert_eq!(tool_name.as_str(), "shell");
        assert_eq!(arguments, &json!({ "command": "ls" }));
        assert_eq!(provider_call_id.as_deref(), Some("call_2"));
        assert!(matches!(
            &events[4],
            ModelDriverEvent::Terminal {
                outcome: ModelOutcome::Succeeded,
                continuation: ModelContinuation::Continue,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn a_reasoning_only_completion_requests_continuation_and_persists_the_reasoning_item() {
        let input = concat!(
            "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"id\":\"rs_1\",\"type\":\"reasoning\",\"summary\":[{\"type\":\"summary_text\",\"text\":\"Thinking.\"}]}}\n\n",
            "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"id\":\"rs_1\",\"type\":\"reasoning\",\"summary\":[{\"type\":\"summary_text\",\"text\":\"Thinking.\"}],\"encrypted_content\":\"gAAAA-secret\"}}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"status\":\"completed\"}}\n\n",
            "data: [DONE]\n\n"
        );

        let events = collect_driver_events(input)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("the reasoning stream should parse");

        assert_eq!(events.len(), 2, "reasoning item then terminal");
        assert!(matches!(
            &events[0],
            ModelDriverEvent::ModelSpecificEvent {
                event_type,
                payload,
                persistence,
                message,
            } if event_type == REASONING_ITEM_EVENT_TYPE
                && *persistence == ConversationEventPersistence::Durable
                && payload["item_id"] == "rs_1"
                && payload["encrypted_content"] == "gAAAA-secret"
                && payload["summary"][0]["text"] == "Thinking."
                && message.as_deref() == Some("Thinking.")
        ));
        assert!(matches!(
            &events[1],
            ModelDriverEvent::Terminal {
                outcome: ModelOutcome::Succeeded,
                continuation: ModelContinuation::Continue,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn a_summary_only_reasoning_item_is_persisted_without_claiming_native_replay() {
        let input = concat!(
            "data: {\"type\":\"response.reasoning_summary_text.delta\",\"output_index\":0,\"summary_index\":0,\"delta\":\"Thinking\"}\n\n",
            "data: {\"type\":\"response.reasoning_summary_text.done\",\"output_index\":0,\"summary_index\":0,\"text\":\"Thinking carefully.\"}\n\n",
            "data: {\"type\":\"response.output_text.done\",\"item_id\":\"msg_1\",\"output_index\":0,\"content_index\":0,\"text\":\"Answer\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"status\":\"completed\"}}\n\n",
            "data: [DONE]\n\n"
        );

        let events = collect_driver_events(input)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("the summary stream should parse");

        assert!(events.iter().any(|event| matches!(
            event,
            ModelDriverEvent::ModelSpecificEvent {
                event_type,
                persistence,
                ..
            } if event_type == REASONING_ITEM_EVENT_TYPE
                && *persistence == ConversationEventPersistence::Durable
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            ModelDriverEvent::ModelSpecificEvent {
                event_type,
                persistence,
                ..
            } if event_type == "reasoning_summary_delta"
                && *persistence == ConversationEventPersistence::Unpersisted
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            ModelDriverEvent::AssistantResponse { content } if content == "Answer"
        )));
    }

    #[tokio::test]
    async fn malformed_json_is_a_stream_error() {
        let mut events =
            model_output_stream(response_byte_stream(vec![b"data: not-json\n\n".to_vec()]));

        assert!(matches!(
            events.next().await,
            Some(Err(OpenRouterResponsesError::InvalidResponse(_)))
        ));
    }

    #[tokio::test]
    async fn premature_body_end_before_a_terminal_is_a_stream_interruption() {
        let input = "data: {\"type\":\"response.output_text.delta\",\"item_id\":\"msg_1\",\"output_index\":0,\"content_index\":0,\"delta\":\"Hello\"}\n\n";
        let mut events = model_output_stream(response_byte_stream(vec![input.as_bytes().to_vec()]));

        assert!(matches!(
            events.next().await,
            Some(Ok(ModelDriverEvent::ModelSpecificEvent { message: Some(message), .. }))
                if message == "Hello"
        ));
        assert!(matches!(
            events.next().await,
            Some(Err(OpenRouterResponsesError::StreamInterrupted(_)))
        ));
    }

    #[tokio::test]
    async fn a_done_sentinel_before_a_terminal_is_an_invalid_response() {
        let input = concat!(
            "data: {\"type\":\"response.output_text.done\",\"item_id\":\"msg_1\",\"output_index\":0,\"content_index\":0,\"text\":\"Hello\"}\n\n",
            "data: [DONE]\n\n"
        );
        let mut events = model_output_stream(response_byte_stream(vec![input.as_bytes().to_vec()]));

        assert!(matches!(
            events.next().await,
            Some(Ok(ModelDriverEvent::AssistantResponse { .. }))
        ));
        assert!(matches!(
            events.next().await,
            Some(Err(OpenRouterResponsesError::InvalidResponse(_)))
        ));
    }

    #[tokio::test]
    async fn a_mid_stream_provider_error_yields_a_failed_terminal() {
        let input = concat!(
            "data: {\"type\":\"response.output_text.done\",\"item_id\":\"msg_1\",\"output_index\":0,\"content_index\":0,\"text\":\"Hello\"}\n\n",
            "data: {\"type\":\"error\",\"message\":\"late failure\"}\n\n",
            "data: [DONE]\n\n"
        );

        let events = collect_driver_events(input)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("the provider failure should produce a terminal");

        assert!(matches!(
            &events[0],
            ModelDriverEvent::AssistantResponse { content } if content == "Hello"
        ));
        assert!(matches!(
            &events[1],
            ModelDriverEvent::Terminal {
                outcome: ModelOutcome::Failed { failure },
                continuation: ModelContinuation::Complete,
                ..
            } if failure.category() == FailureCategory::ProviderFailure
        ));
    }

    #[tokio::test]
    async fn a_context_limit_stream_error_is_a_context_limit_terminal() {
        let input = concat!(
            "data: {\"type\":\"error\",\"code\":\"context_length_exceeded\",\"message\":\"too many tokens\"}\n\n",
            "data: [DONE]\n\n"
        );

        let events = collect_driver_events(input)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("the context limit error should produce a terminal");

        assert!(matches!(
            &events[0],
            ModelDriverEvent::Terminal {
                outcome: ModelOutcome::Failed { failure },
                continuation: ModelContinuation::Complete,
                ..
            } if failure.category() == FailureCategory::ContextLimitExceeded
        ));
    }

    #[tokio::test]
    async fn an_incomplete_reasoning_response_requests_continuation() {
        let input = concat!(
            "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"id\":\"rs_1\",\"type\":\"reasoning\",\"summary\":[{\"type\":\"summary_text\",\"text\":\"Thinking.\"}]}}\n\n",
            "data: {\"type\":\"response.incomplete\",\"response\":{\"id\":\"resp_1\",\"status\":\"incomplete\",\"incomplete_details\":{\"reason\":\"max_output_tokens\"}}}\n\n",
            "data: [DONE]\n\n"
        );

        let events = collect_driver_events(input)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("the incomplete response should parse");

        assert!(matches!(
            &events[0],
            ModelDriverEvent::ModelSpecificEvent {
                event_type,
                persistence,
                ..
            } if event_type == REASONING_ITEM_EVENT_TYPE
                && *persistence == ConversationEventPersistence::Durable
        ));
        assert!(matches!(
            &events[1],
            ModelDriverEvent::Terminal {
                outcome: ModelOutcome::Failed { failure },
                continuation: ModelContinuation::Continue,
                ..
            } if failure.category() == FailureCategory::ProviderFailure
        ));
    }

    #[test]
    fn response_statuses_map_to_typed_driver_errors() {
        assert!(matches!(
            classify_response_failure(StatusCode::UNAUTHORIZED, "unauthorized".to_owned()),
            Err(OpenRouterResponsesError::Authentication(_))
        ));
        assert!(matches!(
            classify_response_failure(StatusCode::TOO_MANY_REQUESTS, "slow down".to_owned()),
            Err(OpenRouterResponsesError::RateLimited(_))
        ));
        assert!(matches!(
            classify_response_failure(StatusCode::BAD_REQUEST, "bad request".to_owned()),
            Err(OpenRouterResponsesError::InvalidRequest(_))
        ));
        assert!(matches!(
            classify_response_failure(StatusCode::INTERNAL_SERVER_ERROR, "failed".to_owned()),
            Err(OpenRouterResponsesError::Provider(_))
        ));
    }

    #[test]
    fn a_context_limit_http_response_is_a_semantic_context_limit() {
        let result = classify_response_failure(
            StatusCode::BAD_REQUEST,
            json!({ "error": { "code": "context_length_exceeded" } }).to_string(),
        );
        assert!(result.is_ok());
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
    async fn invoke_streams_unpersisted_progress_and_records_the_final_response() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("the mock server should bind");
        let address = listener
            .local_addr()
            .expect("the mock server address should be available");
        let server = thread::spawn(move || {
            let (mut connection, _) = listener.accept().expect("the mock server should accept");
            let request = read_request(&connection);
            assert_eq!(request["model"], "openai/gpt-5.6");
            assert_eq!(request["stream"], true);
            assert_eq!(request["input"][0]["role"], "user");
            assert_eq!(request["input"][0]["content"][0]["text"], "Hello");
            let response_body = concat!(
                "data: {\"type\":\"response.output_text.delta\",\"item_id\":\"msg_1\",\"output_index\":0,\"content_index\":0,\"delta\":\"Answer\"}\n\n",
                "data: {\"type\":\"response.output_text.done\",\"item_id\":\"msg_1\",\"output_index\":0,\"content_index\":0,\"text\":\"Answer\"}\n\n",
                "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"status\":\"completed\",\"usage\":{\"input_tokens\":10,\"output_tokens\":5,\"cost\":0.0002}}}\n\n",
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
        assert!(
            matches!(&second, ModelDriverOutput::AssistantResponse(response)
            if response.content() == "Answer")
        );
        let terminal = expect_single(&mut model_events).await;
        let ModelDriverOutput::ModelResponse(response) = &terminal else {
            panic!("the final output should be a model response");
        };
        assert!(matches!(response.outcome(), ModelOutcome::Succeeded));
        assert_eq!(response.continuation(), ModelContinuation::Complete);
        assert_eq!(
            response.usage().and_then(|usage| usage.cost_usd()),
            Some(0.0002)
        );
        assert!(model_events.next().await.is_none());
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
                        if failure.category() == FailureCategory::Authentication
                )
        ));
        assert!(model_events.next().await.is_none());
        server.join().expect("the mock server should stop");
    }
}
