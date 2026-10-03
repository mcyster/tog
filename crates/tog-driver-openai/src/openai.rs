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
    AssistantResponse, ConversationEventId, ConversationEventPayload, FailureCategory,
    InvalidAssistantResponse, InvalidModelResponse, InvalidModelSpecificEvent, InvalidToolRequest,
    ModelData, ModelId, ModelOutcome, ModelResponse, ModelSource, ModelSpecificEvent,
    OperationFailure, ProviderId, ToolRequest, Usage, UserContent,
};
use tog::conversation::{ToolAvailability, ToolName};
use tog::conversation_event_store::ConversationEventRecord;
use tog::model_driver::{
    ModelDriver, ModelDriverError, ModelDriverOutput, ModelDriverOutputBatch, ModelOutputStream,
    TurnInput,
};

type ResponseByteStream = BoxStream<'static, Result<Vec<u8>, OpenAiError>>;
type ProviderOutputStream = BoxStream<'static, Result<ModelDriverEvent, OpenAiError>>;

const PROVIDER_PAYLOAD_VERSION: u32 = 1;

#[derive(Debug, Eq, PartialEq)]
pub enum OpenAiError {
    Authentication(String),
    RateLimited(String),
    Transport(String),
    InvalidRequest(String),
    InvalidResponse(String),
    StreamInterrupted(String),
    Provider(String),
}

impl Display for OpenAiError {
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

impl std::error::Error for OpenAiError {}

#[derive(Clone)]
enum ModelDriverEvent {
    AssistantResponse {
        content: String,
    },
    ModelSpecificEvent {
        event_type: String,
        message: Option<String>,
    },
    ToolRequest {
        tool_name: ToolName,
        arguments: Value,
        provider_call_id: Option<String>,
    },
    Terminal {
        outcome: TerminalModelOutcome,
        usage: Option<Usage>,
    },
}

#[derive(Clone)]
enum TerminalModelOutcome {
    Succeeded,
    Failed {
        category: FailureCategory,
        message: String,
    },
}

pub struct OpenAiModelDriver {
    http_client: Client,
    api_key: String,
    responses_url: String,
    source: ModelSource,
}

impl OpenAiModelDriver {
    pub fn from_environment(model: ModelId) -> Result<Self, OpenAiError> {
        let api_key = std::env::var("OPENAI_API_KEY")
            .map_err(|_| OpenAiError::Authentication("OPENAI_API_KEY must be set".to_owned()))?;
        let base_url = std::env::var("TOG_OPENAI_BASE_URL")
            .unwrap_or_else(|_| "https://api.openai.com/v1".to_owned());
        Ok(Self {
            http_client: Client::new(),
            api_key,
            responses_url: format!("{}/responses", base_url.trim_end_matches('/')),
            source: ModelSource::new(
                ProviderId::from_str("openai")
                    .expect("the OpenAI provider identifier should be valid"),
                model,
            ),
        })
    }
}

impl ModelDriver for OpenAiModelDriver {
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
        request_body.insert("input".to_owned(), semantic_input(events));
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
        request_body.insert("store".to_owned(), Value::Bool(true));

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
                    let error = OpenAiError::InvalidRequest(error.to_string());
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
                        OpenAiError::Transport(error.to_string()),
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
                            OpenAiError::Transport(error.to_string()),
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
                        .map_err(|error| OpenAiError::Transport(error.to_string()))
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

fn semantic_input(events: &[ConversationEventRecord]) -> Value {
    let provider_call_ids = provider_tool_call_ids(events);
    let input = events
        .iter()
        .filter_map(
            |conversation_event| match conversation_event.event().payload() {
                ConversationEventPayload::User(user) => {
                    let text = user
                        .content()
                        .iter()
                        .map(|content| match content {
                            UserContent::Text(text) => text.as_str(),
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    Some(json!({ "role": "user", "content": text }))
                }
                ConversationEventPayload::AssistantResponse(response) => {
                    Some(json!({ "role": "assistant", "content": response.content() }))
                }
                ConversationEventPayload::ToolRequest(request) => Some(
                    provider_tool_request_input(conversation_event, request, &provider_call_ids),
                ),
                ConversationEventPayload::ToolResponse(response) => {
                    Some(provider_tool_response_input(response, &provider_call_ids))
                }
                ConversationEventPayload::TurnStart(_)
                | ConversationEventPayload::TurnEnd(_)
                | ConversationEventPayload::ModelRequest(_)
                | ConversationEventPayload::ModelResponse(_)
                | ConversationEventPayload::ModelSpecificEvent(_)
                | ConversationEventPayload::Automation(_)
                | ConversationEventPayload::Context(_)
                | ConversationEventPayload::Tools(_) => None,
            },
        )
        .collect::<Vec<_>>();
    Value::Array(input)
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

fn provider_tool_request_input(
    conversation_event: &ConversationEventRecord,
    request: &ToolRequest,
    provider_call_ids: &HashMap<ConversationEventId, String>,
) -> Value {
    let call_id = provider_call_ids
        .get(&conversation_event.event().id())
        .cloned()
        .unwrap_or_else(|| conversation_event.event().id().to_string());
    json!({
        "type": "function_call",
        "call_id": call_id,
        "name": request.tool_name().as_str(),
        "arguments": serde_json::to_string(request.arguments())
            .expect("the tool request arguments should serialize"),
    })
}

fn provider_tool_response_input(
    response: &tog::conversation::ToolResponse,
    provider_call_ids: &HashMap<ConversationEventId, String>,
) -> Value {
    let call_id = provider_call_ids
        .get(&response.tool_request_id())
        .cloned()
        .unwrap_or_else(|| response.tool_request_id().to_string());
    json!({
        "type": "function_call_output",
        "call_id": call_id,
        "output": serde_json::to_string(response.outcome())
            .expect("the tool response outcome should serialize"),
    })
}

fn classify_response_failure(status: StatusCode, body: String) -> Result<(), OpenAiError> {
    if status == StatusCode::BAD_REQUEST && is_context_limit_error(&body) {
        return Ok(());
    }

    Err(match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => OpenAiError::Authentication(body),
        StatusCode::TOO_MANY_REQUESTS => OpenAiError::RateLimited(body),
        StatusCode::BAD_REQUEST | StatusCode::NOT_FOUND | StatusCode::UNPROCESSABLE_ENTITY => {
            OpenAiError::InvalidRequest(body)
        }
        _ => OpenAiError::Provider(format!("OpenAI Responses returned {status}: {body}")),
    })
}

fn is_context_limit_error(body: &str) -> bool {
    serde_json::from_str::<Value>(body)
        .ok()
        .is_some_and(|payload| is_context_limit_payload(&payload))
}

fn is_context_limit_payload(payload: &Value) -> bool {
    ["/code", "/error/code", "/response/error/code"]
        .into_iter()
        .filter_map(|pointer| payload.pointer(pointer).and_then(Value::as_str))
        .any(|code| matches!(code, "context_length_exceeded" | "context_window_exceeded"))
}

fn context_limit_stream(model_request_id: ConversationEventId) -> ModelOutputStream {
    terminal_model_output(
        stream::once(async move {
            Ok(ModelDriverEvent::Terminal {
                outcome: TerminalModelOutcome::Failed {
                    category: FailureCategory::ContextLimitExceeded,
                    message: "The model context limit was exceeded.".to_owned(),
                },
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
    error: OpenAiError,
) -> ModelOutputStream {
    let (category, message) = provider_failure(&error, failure_stage);
    terminal_model_output(
        stream::once(async move {
            Ok(ModelDriverEvent::Terminal {
                outcome: TerminalModelOutcome::Failed { category, message },
                usage: None,
            })
        })
        .boxed(),
        model_request_id,
    )
}

fn provider_failure(error: &OpenAiError, failure_stage: FailureStage) -> (FailureCategory, String) {
    match error {
        OpenAiError::Authentication(_) => (
            FailureCategory::Authentication,
            "The model provider could not authenticate the invocation.".to_owned(),
        ),
        OpenAiError::RateLimited(_) => (
            FailureCategory::RateLimited,
            "The model provider rate-limited the invocation.".to_owned(),
        ),
        OpenAiError::Transport(_) if matches!(failure_stage, FailureStage::DuringStream) => (
            FailureCategory::StreamInterrupted,
            "The model response stream was interrupted.".to_owned(),
        ),
        OpenAiError::Transport(_) => (
            FailureCategory::Transport,
            "The model provider could not be reached.".to_owned(),
        ),
        OpenAiError::InvalidRequest(_) => (
            FailureCategory::InvalidRequest,
            "The model invocation request was invalid.".to_owned(),
        ),
        OpenAiError::InvalidResponse(_) => (
            FailureCategory::InvalidProviderResponse,
            "The model provider returned an invalid response.".to_owned(),
        ),
        OpenAiError::StreamInterrupted(_) => (
            FailureCategory::StreamInterrupted,
            "The model response stream was interrupted.".to_owned(),
        ),
        OpenAiError::Provider(_) => (
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

fn fail_stream(state: &mut ConversationEventStreamState, error: OpenAiError) {
    let (category, message) = provider_failure(&error, FailureStage::DuringStream);
    let outcome = TerminalModelOutcome::Failed { category, message };
    let response = ModelResponse::new(
        state.model_request_id,
        Vec::new(),
        terminal_model_outcome(outcome),
        None,
    )
    .expect("the failed terminal model response should be valid");
    state.pending_batches = VecDeque::from([ModelDriverOutputBatch::from(
        ModelDriverOutput::ModelResponse(response),
    )]);
    state.terminated = true;
}

fn terminal_model_outcome(outcome: TerminalModelOutcome) -> ModelOutcome {
    match outcome {
        TerminalModelOutcome::Succeeded => ModelOutcome::Succeeded,
        TerminalModelOutcome::Failed { category, message } => ModelOutcome::Failed {
            failure: OperationFailure::try_new(category, message, None)
                .expect("the terminal failure should be valid"),
        },
    }
}

fn translate_model_driver_event(
    driver_event: ModelDriverEvent,
    model_request_id: ConversationEventId,
    state: &mut ConversationEventStreamState,
) -> Result<(), OpenAiError> {
    let output = match driver_event {
        ModelDriverEvent::AssistantResponse { content } => ModelDriverOutput::AssistantResponse(
            AssistantResponse::new(model_request_id, content)
                .map_err(invalid_assistant_response)?,
        ),
        ModelDriverEvent::ModelSpecificEvent {
            event_type,
            message,
        } => ModelDriverOutput::ModelSpecificEvent(
            ModelSpecificEvent::new(
                model_request_id,
                event_type,
                PROVIDER_PAYLOAD_VERSION,
                json!({}),
                message,
            )
            .map_err(invalid_model_specific_event)?,
        ),
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
                .map_err(|error| OpenAiError::InvalidResponse(error.to_string()))?;
            ModelDriverOutput::ToolRequest(
                ToolRequest::try_new(model_request_id, tool_name, arguments, data)
                    .map_err(invalid_tool_request)?,
            )
        }
        ModelDriverEvent::Terminal { outcome, usage } => {
            let response = ModelResponse::new(
                model_request_id,
                Vec::new(),
                terminal_model_outcome(outcome),
                usage,
            )
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

fn invalid_assistant_response(error: InvalidAssistantResponse) -> OpenAiError {
    OpenAiError::InvalidResponse(error.to_string())
}

fn invalid_model_specific_event(error: InvalidModelSpecificEvent) -> OpenAiError {
    OpenAiError::InvalidResponse(error.to_string())
}

fn invalid_tool_request(error: InvalidToolRequest) -> OpenAiError {
    OpenAiError::InvalidResponse(error.to_string())
}

fn invalid_model_response(error: InvalidModelResponse) -> OpenAiError {
    OpenAiError::InvalidResponse(error.to_string())
}

#[derive(Default)]
struct ResponseState {
    assistant_outputs: Vec<AccumulatedText>,
    refusal_outputs: Vec<AccumulatedText>,
    reasoning_outputs: Vec<AccumulatedText>,
    reasoning_summaries: Vec<AccumulatedText>,
    function_calls: Vec<AccumulatedFunctionCall>,
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
    fn push(&mut self, bytes: &[u8]) -> Result<(), OpenAiError> {
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

    fn finish(&mut self) -> Result<(), OpenAiError> {
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

    fn process_line(&mut self, line: Vec<u8>) -> Result<(), OpenAiError> {
        let line = String::from_utf8(line)
            .map_err(|error| OpenAiError::InvalidResponse(error.to_string()))?;
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

struct OpenAiStreamState {
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
    let state = OpenAiStreamState {
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
                    ResponseEnd::BodyEnded => OpenAiError::StreamInterrupted(
                        "the response body ended before response.completed".to_owned(),
                    ),
                    ResponseEnd::DoneSentinel => OpenAiError::InvalidResponse(
                        "response.completed was not received before [DONE]".to_owned(),
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
) -> Result<ProcessEventResult, OpenAiError> {
    if server_sent_event.data == "[DONE]" {
        return Ok(ProcessEventResult::Done);
    }

    let payload: Value = serde_json::from_str(&server_sent_event.data)
        .map_err(|error| OpenAiError::InvalidResponse(error.to_string()))?;
    let payload_event_type = match payload.get("type") {
        Some(Value::String(event_type)) => Some(event_type.clone()),
        Some(_) => {
            return Err(OpenAiError::InvalidResponse(
                "an OpenAI stream event contained a non-string type".to_owned(),
            ));
        }
        None => None,
    };
    if let (Some(payload_event_type), Some(server_sent_event_name)) =
        (&payload_event_type, &server_sent_event.name)
        && payload_event_type != server_sent_event_name
    {
        return Err(OpenAiError::InvalidResponse(format!(
            "OpenAI stream event type {server_sent_event_name} did not match payload type {payload_event_type}"
        )));
    }
    let event_type = payload_event_type
        .or(server_sent_event.name)
        .unwrap_or_else(|| "unknown".to_owned());

    if response_state.completed
        && !matches!(
            event_type.as_str(),
            "error" | "response.failed" | "response.completed"
        )
    {
        return Err(OpenAiError::InvalidResponse(format!(
            "OpenAI stream emitted {event_type} after response.completed"
        )));
    }

    let model_events = match event_type.as_str() {
        "response.output_text.delta" => {
            let key = semantic_output_key(&payload, &["output_index", "content_index"])?;
            append_delta(
                &payload,
                accumulated_text(&mut response_state.assistant_outputs, key)?,
            )?;
            Vec::new()
        }
        "response.refusal.delta" => {
            let key = semantic_output_key(&payload, &["output_index", "content_index"])?;
            append_delta(
                &payload,
                accumulated_text(&mut response_state.refusal_outputs, key)?,
            )?;
            Vec::new()
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
        "response.refusal.done" => {
            let key = semantic_output_key(&payload, &["output_index", "content_index"])?;
            complete_text(
                &payload,
                "refusal",
                accumulated_text(&mut response_state.refusal_outputs, key.clone())?,
            )?;
            let emitted = emit_refusal(&mut response_state.refusal_outputs, &key)?;
            if emitted.is_some() {
                response_state.completed = true;
            }
            emitted.into_iter().collect()
        }
        "response.reasoning_text.delta" => {
            let key = semantic_output_key(&payload, &["output_index", "content_index"])?;
            append_delta(
                &payload,
                accumulated_text(&mut response_state.reasoning_outputs, key)?,
            )?;
            Vec::new()
        }
        "response.reasoning_text.done" => {
            let key = semantic_output_key(&payload, &["output_index", "content_index"])?;
            complete_text(
                &payload,
                "text",
                accumulated_text(&mut response_state.reasoning_outputs, key.clone())?,
            )?;
            emit_reasoning(&mut response_state.reasoning_outputs, &key)?
                .into_iter()
                .collect()
        }
        "response.reasoning_summary_text.delta" => {
            let key = semantic_output_key(&payload, &["output_index", "summary_index"])?;
            append_delta(
                &payload,
                accumulated_text(&mut response_state.reasoning_summaries, key)?,
            )?;
            Vec::new()
        }
        "response.reasoning_summary_text.done" => {
            let key = semantic_output_key(&payload, &["output_index", "summary_index"])?;
            complete_text(
                &payload,
                "text",
                accumulated_text(&mut response_state.reasoning_summaries, key.clone())?,
            )?;
            emit_reasoning_summary(&mut response_state.reasoning_summaries, &key)?
                .into_iter()
                .collect()
        }
        "response.output_item.added" => {
            register_function_call(&payload, &mut response_state.function_calls)?;
            Vec::new()
        }
        "response.function_call_arguments.done" => {
            let key = semantic_output_key(&payload, &["output_index"])?;
            complete_function_call_arguments(&payload, &mut response_state.function_calls, &key)?;
            emit_function_call(&mut response_state.function_calls, &key)?
                .into_iter()
                .collect()
        }
        "response.output_item.done" => complete_function_call_item(&payload, response_state)?,
        "response.completed" => complete_response(response_state, &payload)?,
        "error" | "response.failed" if is_context_limit_payload(&payload) => {
            response_state.completed = true;
            vec![model_context_limit_exceeded()?]
        }
        "error" | "response.failed" => {
            return Err(OpenAiError::Provider(format!(
                "OpenAI stream emitted {event_type}: {payload}"
            )));
        }
        _ => Vec::new(),
    };
    Ok(ProcessEventResult::Outputs(model_events))
}

fn complete_response(
    response_state: &mut ResponseState,
    payload: &Value,
) -> Result<Vec<ModelDriverEvent>, OpenAiError> {
    if response_state.completed {
        return Err(OpenAiError::InvalidResponse(
            "response.completed was received more than once".to_owned(),
        ));
    }
    if !payload.get("response").is_some_and(Value::is_object) {
        return Err(OpenAiError::InvalidResponse(
            "response.completed did not contain an OpenAI response object".to_owned(),
        ));
    }

    let mut model_events = Vec::new();
    model_events.extend(emit_remaining_reasoning(
        &mut response_state.reasoning_outputs,
    )?);
    model_events.extend(emit_remaining_reasoning_summaries(
        &mut response_state.reasoning_summaries,
    )?);
    model_events.extend(emit_remaining_refusals(
        &mut response_state.refusal_outputs,
    )?);
    if let Some(refusal) = model_events.iter().find(|event| {
        matches!(
            event,
            ModelDriverEvent::Terminal {
                outcome: TerminalModelOutcome::Failed {
                    category: FailureCategory::Refusal,
                    ..
                },
                ..
            }
        )
    }) {
        return Ok(vec![refusal.clone()]);
    }
    model_events.extend(emit_remaining_assistant_responses(
        &mut response_state.assistant_outputs,
    )?);
    let completed_content = completed_response_content(payload)?;
    for completed_output in completed_content.assistant_outputs {
        if !completed_output_already_emitted(
            &response_state.assistant_outputs,
            &completed_output.key,
        ) {
            model_events.push(assistant_response(completed_output.text)?);
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
    if response_state
        .function_calls
        .iter()
        .any(|function_call| !function_call.emitted)
    {
        return Err(OpenAiError::InvalidResponse(
            "the completed response contained an incomplete function call".to_owned(),
        ));
    }
    let has_completed_model_output = response_state
        .assistant_outputs
        .iter()
        .any(|output| output.emitted)
        || response_state
            .function_calls
            .iter()
            .any(|function_call| function_call.emitted)
        || model_events.iter().any(|event| {
            matches!(
                event,
                ModelDriverEvent::AssistantResponse { .. } | ModelDriverEvent::ToolRequest { .. }
            )
        });
    if !has_completed_model_output {
        return Err(OpenAiError::InvalidResponse(
            "the completed response contained no model message".to_owned(),
        ));
    }
    let usage = completed_usage(payload);
    model_events.push(ModelDriverEvent::Terminal {
        outcome: TerminalModelOutcome::Succeeded,
        usage,
    });
    response_state.completed = true;
    Ok(model_events)
}

fn completed_usage(payload: &Value) -> Option<Usage> {
    let usage = payload.get("response")?.get("usage")?;
    let input_tokens = usage.get("input_tokens")?.as_u64()?;
    let output_tokens = usage.get("output_tokens")?.as_u64()?;
    Some(Usage::new(input_tokens, output_tokens))
}

fn emit_reasoning(
    reasoning_outputs: &mut [AccumulatedText],
    key: &str,
) -> Result<Option<ModelDriverEvent>, OpenAiError> {
    let output = un_emitted_text(reasoning_outputs, key)?;
    let Some(reasoning_text) = preferred_text(&output.streamed_text, &output.completed_text) else {
        return Ok(None);
    };
    let model_event = ModelDriverEvent::ModelSpecificEvent {
        event_type: "reasoning".to_owned(),
        message: Some(reasoning_text),
    };
    output.emitted = true;
    Ok(Some(model_event))
}

fn emit_reasoning_summary(
    reasoning_summaries: &mut [AccumulatedText],
    key: &str,
) -> Result<Option<ModelDriverEvent>, OpenAiError> {
    let output = un_emitted_text(reasoning_summaries, key)?;
    let Some(reasoning_summary) = preferred_text(&output.streamed_text, &output.completed_text)
    else {
        return Ok(None);
    };
    let model_event = ModelDriverEvent::ModelSpecificEvent {
        event_type: "reasoning_summary".to_owned(),
        message: Some(reasoning_summary),
    };
    output.emitted = true;
    Ok(Some(model_event))
}

fn emit_assistant_response(
    assistant_outputs: &mut [AccumulatedText],
    key: &str,
) -> Result<Option<ModelDriverEvent>, OpenAiError> {
    let output = un_emitted_text(assistant_outputs, key)?;
    let assistant_text = preferred_text(&output.streamed_text, &output.completed_text);
    let Some(assistant_text) = assistant_text else {
        return Ok(None);
    };
    let model_event = ModelDriverEvent::AssistantResponse {
        content: assistant_text,
    };
    output.emitted = true;
    Ok(Some(model_event))
}

fn emit_refusal(
    refusal_outputs: &mut [AccumulatedText],
    key: &str,
) -> Result<Option<ModelDriverEvent>, OpenAiError> {
    let output = un_emitted_text(refusal_outputs, key)?;
    let refusal = preferred_text(&output.streamed_text, &output.completed_text);
    let Some(refusal) = refusal else {
        return Ok(None);
    };
    let model_event = ModelDriverEvent::Terminal {
        outcome: TerminalModelOutcome::Failed {
            category: FailureCategory::Refusal,
            message: refusal,
        },
        usage: None,
    };
    output.emitted = true;
    Ok(Some(model_event))
}

fn emit_remaining_reasoning(
    outputs: &mut [AccumulatedText],
) -> Result<Vec<ModelDriverEvent>, OpenAiError> {
    let keys = un_emitted_keys(outputs);
    keys.into_iter()
        .filter_map(|key| emit_reasoning(outputs, &key).transpose())
        .collect()
}

fn emit_remaining_reasoning_summaries(
    outputs: &mut [AccumulatedText],
) -> Result<Vec<ModelDriverEvent>, OpenAiError> {
    let keys = un_emitted_keys(outputs);
    keys.into_iter()
        .filter_map(|key| emit_reasoning_summary(outputs, &key).transpose())
        .collect()
}

fn emit_remaining_assistant_responses(
    outputs: &mut [AccumulatedText],
) -> Result<Vec<ModelDriverEvent>, OpenAiError> {
    let keys = un_emitted_keys(outputs);
    keys.into_iter()
        .filter_map(|key| emit_assistant_response(outputs, &key).transpose())
        .collect()
}

fn emit_remaining_refusals(
    outputs: &mut [AccumulatedText],
) -> Result<Vec<ModelDriverEvent>, OpenAiError> {
    let keys = un_emitted_keys(outputs);
    keys.into_iter()
        .filter_map(|key| emit_refusal(outputs, &key).transpose())
        .collect()
}

fn register_function_call(
    payload: &Value,
    function_calls: &mut Vec<AccumulatedFunctionCall>,
) -> Result<(), OpenAiError> {
    let Some(item) = payload.get("item") else {
        return Ok(());
    };
    if item.get("type").and_then(Value::as_str) != Some("function_call") {
        return Ok(());
    }
    let key = semantic_output_key(payload, &["output_index"])?;
    let call_id = item
        .get("call_id")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let name = item.get("name").and_then(Value::as_str).map(str::to_owned);
    let completed_arguments = completed_function_call_arguments(item)?;
    upsert_function_call(function_calls, key, call_id, name, completed_arguments);
    Ok(())
}

fn complete_function_call_arguments(
    payload: &Value,
    function_calls: &mut Vec<AccumulatedFunctionCall>,
    key: &str,
) -> Result<(), OpenAiError> {
    let arguments = payload
        .get("arguments")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            OpenAiError::InvalidResponse(
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

fn complete_function_call_item(
    payload: &Value,
    response_state: &mut ResponseState,
) -> Result<Vec<ModelDriverEvent>, OpenAiError> {
    let Some(item) = payload.get("item") else {
        return Ok(Vec::new());
    };
    if item.get("type").and_then(Value::as_str) != Some("function_call") {
        return Ok(Vec::new());
    }
    let key = semantic_output_key(payload, &["output_index"])?;
    let call_id = item
        .get("call_id")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let name = item.get("name").and_then(Value::as_str).map(str::to_owned);
    let completed_arguments = item
        .get("arguments")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            OpenAiError::InvalidResponse(
                "a completed function call did not contain string arguments".to_owned(),
            )
        })?
        .to_owned();
    upsert_function_call(
        &mut response_state.function_calls,
        key.clone(),
        call_id,
        name,
        Some(completed_arguments),
    );
    Ok(
        emit_function_call(&mut response_state.function_calls, &key)?
            .into_iter()
            .collect(),
    )
}

fn completed_function_call_arguments(item: &Value) -> Result<Option<String>, OpenAiError> {
    match item.get("arguments") {
        Some(Value::String(arguments)) if !arguments.trim().is_empty() => {
            Ok(Some(arguments.clone()))
        }
        Some(Value::String(_)) | None => Ok(None),
        Some(_) => Err(OpenAiError::InvalidResponse(
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
) -> Result<Option<ModelDriverEvent>, OpenAiError> {
    let function_call = function_calls
        .iter_mut()
        .find(|function_call| function_call.key == key)
        .ok_or_else(|| {
            OpenAiError::InvalidResponse(format!(
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
        OpenAiError::InvalidResponse("a completed function call did not contain a name".to_owned())
    })?;
    let arguments = parse_function_call_arguments(completed_arguments)?;
    function_call.emitted = true;
    Ok(Some(ModelDriverEvent::ToolRequest {
        tool_name: ToolName::try_new(name)
            .map_err(|error| OpenAiError::InvalidResponse(error.to_string()))?,
        arguments,
        provider_call_id: function_call.call_id.clone(),
    }))
}

fn parse_function_call_arguments(arguments: &str) -> Result<Value, OpenAiError> {
    if arguments.trim().is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    serde_json::from_str(arguments).map_err(|error| {
        OpenAiError::InvalidResponse(format!(
            "a function call contained invalid JSON arguments: {error}"
        ))
    })
}

fn assistant_response(content: String) -> Result<ModelDriverEvent, OpenAiError> {
    Ok(ModelDriverEvent::AssistantResponse { content })
}

fn model_context_limit_exceeded() -> Result<ModelDriverEvent, OpenAiError> {
    Ok(ModelDriverEvent::Terminal {
        outcome: TerminalModelOutcome::Failed {
            category: FailureCategory::ContextLimitExceeded,
            message: "The model context limit was exceeded.".to_owned(),
        },
        usage: None,
    })
}

fn semantic_output_key(payload: &Value, indexes: &[&str]) -> Result<String, OpenAiError> {
    let mut key_parts = Vec::new();
    for index_name in indexes {
        if let Some(index) = payload.get(index_name) {
            let index = index.as_u64().ok_or_else(|| {
                OpenAiError::InvalidResponse(format!(
                    "an OpenAI semantic event contained an invalid {index_name}"
                ))
            })?;
            key_parts.push(format!("{index_name}={index}"));
        }
    }
    if !key_parts.is_empty() && key_parts.len() != indexes.len() {
        return Err(OpenAiError::InvalidResponse(
            "an OpenAI semantic event contained incomplete output indexes".to_owned(),
        ));
    }
    if key_parts.len() == indexes.len() {
        return Ok(key_parts.join(";"));
    }

    match payload.get("item_id") {
        Some(Value::String(item_id)) => Ok(format!("item_id={item_id}")),
        Some(_) => Err(OpenAiError::InvalidResponse(
            "an OpenAI semantic event contained a non-string item_id".to_owned(),
        )),
        None => Ok("default".to_owned()),
    }
}

fn accumulated_text(
    outputs: &mut Vec<AccumulatedText>,
    key: String,
) -> Result<&mut AccumulatedText, OpenAiError> {
    if let Some(position) = outputs.iter().position(|output| output.key == key) {
        return Ok(&mut outputs[position]);
    }
    if (key == "default" && !outputs.is_empty())
        || (key != "default" && outputs.iter().any(|output| output.key == "default"))
    {
        return Err(OpenAiError::InvalidResponse(
            "OpenAI semantic output identity changed while streaming".to_owned(),
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

fn un_emitted_text<'a>(
    outputs: &'a mut [AccumulatedText],
    key: &str,
) -> Result<&'a mut AccumulatedText, OpenAiError> {
    let output = outputs
        .iter_mut()
        .find(|output| output.key == key)
        .expect("the accumulated output should exist");
    if output.emitted {
        return Err(OpenAiError::InvalidResponse(format!(
            "OpenAI emitted semantic output {key} more than once"
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

fn complete_text(
    payload: &Value,
    field: &str,
    output: &mut AccumulatedText,
) -> Result<(), OpenAiError> {
    if output.emitted {
        return Err(OpenAiError::InvalidResponse(format!(
            "OpenAI emitted semantic output {} more than once",
            output.key
        )));
    }
    output.completed_text = text_field(payload, field)?;
    Ok(())
}

fn append_delta(payload: &Value, output: &mut AccumulatedText) -> Result<(), OpenAiError> {
    if output.emitted {
        return Err(OpenAiError::InvalidResponse(format!(
            "OpenAI emitted a delta after completing semantic output {}",
            output.key
        )));
    }
    let delta = payload
        .get("delta")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            OpenAiError::InvalidResponse(
                "an OpenAI delta event did not contain a string delta".to_owned(),
            )
        })?;
    output.streamed_text.push_str(delta);
    Ok(())
}

fn text_field(payload: &Value, field: &str) -> Result<Option<String>, OpenAiError> {
    let Some(value) = payload.get(field) else {
        return Ok(None);
    };
    value
        .as_str()
        .map(|text| Some(text.to_owned()))
        .ok_or_else(|| {
            OpenAiError::InvalidResponse(format!(
                "an OpenAI completion event contained a non-string {field} field"
            ))
        })
}

fn preferred_text(streamed_text: &str, completed_text: &Option<String>) -> Option<String> {
    completed_text
        .clone()
        .or_else(|| (!streamed_text.is_empty()).then(|| streamed_text.to_owned()))
}

#[derive(Default)]
struct CompletedResponseContent {
    assistant_outputs: Vec<CompletedText>,
    function_calls: Vec<CompletedFunctionCall>,
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

fn completed_response_content(payload: &Value) -> Result<CompletedResponseContent, OpenAiError> {
    let Some(output_value) = payload
        .get("response")
        .and_then(|response| response.get("output"))
    else {
        return Ok(CompletedResponseContent::default());
    };
    let output = output_value.as_array().ok_or_else(|| {
        OpenAiError::InvalidResponse(
            "the completed OpenAI response contained non-array output".to_owned(),
        )
    })?;
    let mut assistant_outputs = Vec::new();
    let mut function_calls = Vec::new();
    for (output_index, output_item) in output.iter().enumerate() {
        if output_item.get("type").and_then(Value::as_str) == Some("function_call") {
            let arguments = output_item
                .get("arguments")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    OpenAiError::InvalidResponse(
                        "completed OpenAI function call arguments were not a string".to_owned(),
                    )
                })?;
            function_calls.push(CompletedFunctionCall {
                key: format!("output_index={output_index}"),
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
            continue;
        }
        let Some(content_value) = output_item.get("content") else {
            continue;
        };
        let content = content_value.as_array().ok_or_else(|| {
            OpenAiError::InvalidResponse(
                "the completed OpenAI response contained non-array content".to_owned(),
            )
        })?;
        for (content_index, content_item) in content.iter().enumerate() {
            let key = format!("output_index={output_index};content_index={content_index}");
            if content_item.get("type").and_then(Value::as_str) == Some("output_text") {
                let content_text = content_item
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        OpenAiError::InvalidResponse(
                            "completed OpenAI output text was not a string".to_owned(),
                        )
                    })?;
                assistant_outputs.push(CompletedText {
                    key,
                    text: content_text.to_owned(),
                });
            }
        }
    }
    Ok(CompletedResponseContent {
        assistant_outputs,
        function_calls,
    })
}

fn completed_output_already_emitted(outputs: &[AccumulatedText], key: &str) -> bool {
    outputs
        .iter()
        .any(|output| output.emitted && (output.key == "default" || output.key == key))
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
        ConversationEventPayload, ConversationHistory, ConversationId, FailureCategory, ModelData,
        ModelId, ModelOutcome, ModelRequest, ModelSource, ModelSpecificEvent, ProviderId,
        ToolOutcome, ToolRequest, ToolResponse, TurnStart, User, UserContent,
    };
    use tog::conversation::{ToolDefinition, ToolName};
    use tog::conversation_event_store::ConversationEventRecord;
    use tog::model_driver::{ModelDriver, ModelDriverOutput, ModelOutputStream, TurnInput};

    use super::{
        ModelDriverEvent, OpenAiError, OpenAiModelDriver, ResponseByteStream, TerminalModelOutcome,
        classify_response_failure, model_output_stream, provider_tool, semantic_input,
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

    #[test]
    fn semantic_input_projects_canonical_events_and_ignores_auxiliary_records() {
        let conversation_id = ConversationId::new();
        let (turn_start, model_request) = turn_and_request(conversation_id);
        let request_id = model_request.event().id();
        let reasoning = ModelSpecificEvent::new(
            request_id,
            "reasoning".to_owned(),
            1,
            json!({}),
            Some("Thinking.".to_owned()),
        )
        .expect("the reasoning event should be valid");
        let assistant = AssistantResponse::new(request_id, "Hello.".to_owned())
            .expect("the assistant response should be valid");
        let conversation = ConversationHistory::from_events(vec![
            user_event(conversation_id, 0, "Hello"),
            turn_start,
            model_request,
            conversation_event(
                conversation_id,
                3,
                ConversationEventPayload::ModelSpecificEvent(reasoning),
            ),
            conversation_event(
                conversation_id,
                4,
                ConversationEventPayload::AssistantResponse(assistant),
            ),
        ])
        .expect("the conversation should be valid");

        assert_eq!(
            semantic_input(conversation.events()),
            json!([
                { "role": "user", "content": "Hello" },
                { "role": "assistant", "content": "Hello." }
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
    fn provider_tools_translate_definitions_without_mutating_them() {
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
            definition.parameters().as_value().get("$schema").is_some(),
            "the recorded definition must keep its schema metadata"
        );
    }

    #[test]
    fn semantic_input_reconstructs_tool_requests_and_responses() {
        let conversation_id = ConversationId::new();
        let (turn_start, model_request) = turn_and_request(conversation_id);
        let request_id = model_request.event().id();
        let model_data = ModelData::new(Map::from_iter([(
            "call_id".to_owned(),
            Value::String("call_native".to_owned()),
        )]))
        .expect("the model data should be valid");
        let request = ToolRequest::try_new(
            request_id,
            ToolName::try_new("shell".to_owned()).expect("the tool name should be valid"),
            json!({ "command": "pwd" }),
            Some(model_data),
        )
        .expect("the tool request should be valid");
        let tool_request_record = conversation_event(
            conversation_id,
            3,
            ConversationEventPayload::ToolRequest(request),
        );
        let tool_request_id = tool_request_record.event().id();
        let response_record = conversation_event(
            conversation_id,
            4,
            ConversationEventPayload::ToolResponse(ToolResponse::new(
                tool_request_id,
                ToolOutcome::succeeded(json!({ "stdout": "/tmp\n" })),
            )),
        );
        let conversation = ConversationHistory::from_events(vec![
            user_event(conversation_id, 0, "Run pwd"),
            turn_start,
            model_request,
            tool_request_record,
            response_record,
        ])
        .expect("the conversation should be valid");

        assert_eq!(
            semantic_input(conversation.events()),
            json!([
                { "role": "user", "content": "Run pwd" },
                {
                    "type": "function_call",
                    "call_id": "call_native",
                    "name": "shell",
                    "arguments": "{\"command\":\"pwd\"}"
                },
                {
                    "type": "function_call_output",
                    "call_id": "call_native",
                    "output": "{\"type\":\"succeeded\",\"value\":{\"stdout\":\"/tmp\\n\"}}"
                }
            ])
        );
    }

    fn response_byte_stream(chunks: Vec<Vec<u8>>) -> ResponseByteStream {
        stream::iter(chunks.into_iter().map(Ok::<Vec<u8>, OpenAiError>)).boxed()
    }

    fn one_byte_chunks(input: &str) -> Vec<Vec<u8>> {
        input.as_bytes().iter().map(|byte| vec![*byte]).collect()
    }

    async fn collect_driver_events(input: &str) -> Vec<Result<ModelDriverEvent, OpenAiError>> {
        model_output_stream(response_byte_stream(vec![input.as_bytes().to_vec()]))
            .collect()
            .await
    }

    async fn collect_events(input: &str) -> Vec<Result<ModelDriverEvent, OpenAiError>> {
        collect_driver_events(input).await
    }

    #[tokio::test]
    async fn several_sse_events_yield_reasoning_before_the_answer_and_a_terminal() {
        let input = concat!(
            "data: {\"type\":\"response.reasoning_text.delta\",\"delta\":\"Detailed \"}\n\n",
            "data: {\"type\":\"response.reasoning_text.done\",\"text\":\"Detailed thought\"}\n\n",
            "data: {\"type\":\"response.reasoning_summary_text.done\",\"text\":\"Summary\"}\n\n",
            "data: {\"type\":\"response.output_text.done\",\"text\":\"Answer\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{}}\n\n"
        );

        let events = collect_events(input)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("the response stream should parse");

        assert_eq!(events.len(), 4);
        let ModelDriverEvent::ModelSpecificEvent {
            event_type,
            message,
        } = &events[0]
        else {
            panic!("the first event should be reasoning");
        };
        assert_eq!(event_type, "reasoning");
        assert_eq!(message.as_deref(), Some("Detailed thought"));
        let ModelDriverEvent::ModelSpecificEvent {
            event_type,
            message,
        } = &events[1]
        else {
            panic!("the second event should be a reasoning summary");
        };
        assert_eq!(event_type, "reasoning_summary");
        assert_eq!(message.as_deref(), Some("Summary"));
        assert!(matches!(
            &events[2],
            ModelDriverEvent::AssistantResponse { content } if content == "Answer"
        ));
        assert!(matches!(
            &events[3],
            ModelDriverEvent::Terminal {
                outcome: TerminalModelOutcome::Succeeded,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn a_completed_refusal_is_a_failed_terminal() {
        let input = concat!(
            "data: {\"type\":\"response.refusal.delta\",\"delta\":\"I cannot \"}\n\n",
            "data: {\"type\":\"response.refusal.done\",\"refusal\":\"I cannot comply.\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{}}\n\n"
        );

        let events = collect_events(input)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("the refusal stream should parse");

        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            ModelDriverEvent::Terminal {
                outcome: TerminalModelOutcome::Failed {
                    category: FailureCategory::Refusal,
                    message,
                },
                ..
            } if message == "I cannot comply."
        ));
    }

    #[tokio::test]
    async fn several_sse_events_in_one_chunk_yield_reasoning_before_the_answer() {
        let input = concat!(
            "data: {\"type\":\"response.reasoning_text.delta\",\"delta\":\"Detailed \"}\n\n",
            "data: {\"type\":\"response.reasoning_text.done\",\"text\":\"Detailed thought\"}\n\n",
            "data: {\"type\":\"response.reasoning_summary_text.done\",\"text\":\"Summary\"}\n\n",
            "data: {\"type\":\"response.output_text.done\",\"text\":\"Answer\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{}}\n\n"
        );

        let events = collect_events(input)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("the response stream should parse");

        assert_eq!(events.len(), 4);
        assert!(matches!(
            &events[2],
            ModelDriverEvent::AssistantResponse { content } if content == "Answer"
        ));
    }

    #[tokio::test]
    async fn arbitrary_byte_boundaries_crlf_multiline_data_and_event_fields_parse() {
        let input = concat!(
            "event: response.output_text.delta\r\n",
            "data: {\"delta\":\r\n",
            "data: \"Hello\"}\r\n\r\n",
            "event: response.output_text.done\r\n",
            "data: {}\r\n\r\n",
            "event: response.completed\r\n",
            "data: {\"response\":{}}\r\n\r\n",
            "data: [DONE]\r\n\r\n"
        );
        let mut model_events = model_output_stream(response_byte_stream(one_byte_chunks(input)));

        let model_event = model_events
            .next()
            .await
            .expect("the stream should yield an event")
            .expect("the event should be valid");

        assert!(matches!(
            model_event,
            ModelDriverEvent::AssistantResponse { ref content } if content == "Hello"
        ));
        assert!(matches!(
            model_events.next().await,
            Some(Ok(ModelDriverEvent::Terminal { .. }))
        ));
        assert!(model_events.next().await.is_none());
    }

    #[tokio::test]
    async fn a_completed_function_call_becomes_a_portable_tool_request() {
        let input = concat!(
            "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"id\":\"fc_1\",\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"shell\",\"arguments\":\"\"}}\n\n",
            "data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_1\",\"output_index\":0,\"delta\":\"{\\\"command\\\":\"}\n\n",
            "data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_1\",\"output_index\":0,\"delta\":\"\\\"pwd\\\"}\"}\n\n",
            "data: {\"type\":\"response.function_call_arguments.done\",\"item_id\":\"fc_1\",\"output_index\":0,\"arguments\":\"{\\\"command\\\":\\\"pwd\\\"}\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{}}\n\n",
            "data: [DONE]\n\n"
        );

        let events = collect_events(input)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("the function call stream should parse");

        assert_eq!(events.len(), 2);
        let ModelDriverEvent::ToolRequest {
            tool_name,
            arguments,
            provider_call_id,
        } = &events[0]
        else {
            panic!("the output should be a tool request");
        };
        assert_eq!(tool_name.as_str(), "shell");
        assert_eq!(arguments, &json!({ "command": "pwd" }));
        assert_eq!(provider_call_id.as_deref(), Some("call_1"));
    }

    #[tokio::test]
    async fn partially_streamed_function_arguments_are_never_executed() {
        let input = concat!(
            "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"id\":\"fc_1\",\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"shell\",\"arguments\":\"\"}}\n\n",
            "data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_1\",\"output_index\":0,\"delta\":\"{\\\"command\\\":\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{}}\n\n"
        );

        let results = collect_events(input).await;

        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], Err(OpenAiError::InvalidResponse(_))));
    }

    #[tokio::test]
    async fn a_late_stream_failure_follows_the_completed_model_event() {
        let input = concat!(
            "data: {\"type\":\"response.output_text.done\",\"text\":\"Hello\"}\n\n",
            "data: {\"type\":\"error\",\"message\":\"late failure\"}\n\n"
        );
        let mut model_events =
            model_output_stream(response_byte_stream(vec![input.as_bytes().to_vec()]));

        let completed_event = model_events
            .next()
            .await
            .expect("the stream should yield an event")
            .expect("the completed event should be valid");
        assert!(matches!(
            completed_event,
            ModelDriverEvent::AssistantResponse { content } if content == "Hello"
        ));
        assert!(matches!(
            model_events.next().await,
            Some(Err(OpenAiError::Provider(_)))
        ));
        assert!(model_events.next().await.is_none());
    }

    #[tokio::test]
    async fn premature_body_end_is_a_stream_interruption() {
        let input = "data: {\"type\":\"response.output_text.done\",\"text\":\"Hello\"}\n\n";
        let mut model_events =
            model_output_stream(response_byte_stream(vec![input.as_bytes().to_vec()]));

        assert!(matches!(
            model_events.next().await,
            Some(Ok(ModelDriverEvent::AssistantResponse { .. }))
        ));
        assert!(matches!(
            model_events.next().await,
            Some(Err(OpenAiError::StreamInterrupted(_)))
        ));
    }

    #[tokio::test]
    async fn missing_response_completed_is_a_stream_error_even_after_done() {
        let input = concat!(
            "data: {\"type\":\"response.output_text.done\",\"text\":\"Hello\"}\n\n",
            "data: [DONE]\n\n"
        );
        let mut model_events =
            model_output_stream(response_byte_stream(vec![input.as_bytes().to_vec()]));

        assert!(matches!(
            model_events.next().await,
            Some(Ok(ModelDriverEvent::AssistantResponse { .. }))
        ));
        assert!(matches!(
            model_events.next().await,
            Some(Err(OpenAiError::InvalidResponse(_)))
        ));
    }

    #[tokio::test]
    async fn response_completed_fallback_does_not_duplicate_a_done_event() {
        let input = concat!(
            "data: {\"type\":\"response.output_text.done\",\"text\":\"Answer\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"output\":[{\"content\":[{\"type\":\"output_text\",\"text\":\"Answer\"}]}]}}\n\n",
            "data: [DONE]\n\n"
        );

        let events = collect_events(input)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("the response stream should parse");

        let assistant_count = events
            .iter()
            .filter(|event| matches!(event, ModelDriverEvent::AssistantResponse { .. }))
            .count();
        assert_eq!(assistant_count, 1);
    }

    #[tokio::test]
    async fn a_completed_response_supplies_function_call_arguments_as_a_fallback() {
        let input = concat!(
            "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"id\":\"fc_1\",\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"shell\",\"arguments\":\"\"}}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"output\":[{\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"shell\",\"arguments\":\"{\\\"command\\\":\\\"pwd\\\"}\"}]}}\n\n",
            "data: [DONE]\n\n"
        );

        let events = collect_events(input)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("the fallback function call should parse");

        assert_eq!(events.len(), 2);
        assert!(matches!(
            &events[0],
            ModelDriverEvent::ToolRequest { arguments, .. }
                if arguments == &json!({ "command": "pwd" })
        ));
    }

    #[tokio::test]
    async fn malformed_json_is_a_stream_error() {
        let mut events =
            model_output_stream(response_byte_stream(vec![b"data: not-json\n\n".to_vec()]));

        assert!(matches!(
            events.next().await,
            Some(Err(OpenAiError::InvalidResponse(_)))
        ));
    }

    #[test]
    fn response_statuses_map_to_typed_driver_errors() {
        assert!(matches!(
            classify_response_failure(StatusCode::UNAUTHORIZED, "unauthorized".to_owned()),
            Err(OpenAiError::Authentication(_))
        ));
        assert!(matches!(
            classify_response_failure(StatusCode::TOO_MANY_REQUESTS, "slow down".to_owned()),
            Err(OpenAiError::RateLimited(_))
        ));
        assert!(matches!(
            classify_response_failure(StatusCode::BAD_REQUEST, "bad request".to_owned()),
            Err(OpenAiError::InvalidRequest(_))
        ));
        assert!(matches!(
            classify_response_failure(StatusCode::INTERNAL_SERVER_ERROR, "failed".to_owned()),
            Err(OpenAiError::Provider(_))
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

    fn source() -> ModelSource {
        ModelSource::new(
            ProviderId::from_str("openai").expect("the provider identifier should be valid"),
            ModelId::from_str("gpt-5.6").expect("the model identifier should be valid"),
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

    fn read_request(connection: &TcpStream) {
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
    }

    #[tokio::test]
    async fn invoke_emits_outputs_and_a_terminal_model_response() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("the mock server should bind");
        let address = listener
            .local_addr()
            .expect("the mock server address should be available");
        let server = thread::spawn(move || {
            let (mut connection, _) = listener.accept().expect("the mock server should accept");
            read_request(&connection);
            let response_body = concat!(
                "data: {\"type\":\"response.reasoning_text.done\",\"text\":\"Reasoning\"}\n\n",
                "data: {\"type\":\"response.output_text.done\",\"text\":\"Answer\"}\n\n",
                "data: {\"type\":\"response.completed\",\"response\":{}}\n\n",
                "data: [DONE]\n\n"
            );
            write!(
                connection,
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
                response_body.len()
            )
            .expect("the mock response should write");
        });
        let driver = OpenAiModelDriver {
            http_client: reqwest::Client::new(),
            api_key: "test-key".to_owned(),
            responses_url: format!("http://{address}/responses"),
            source: source(),
        };

        let conversation = test_conversation("Hello");
        let mut model_events = driver
            .invoke(driver_request(&conversation))
            .await
            .expect("the invocation should establish its stream");
        let first = expect_single(&mut model_events).await;
        assert!(matches!(first, ModelDriverOutput::ModelSpecificEvent(_)));
        let second = expect_single(&mut model_events).await;
        assert!(matches!(second, ModelDriverOutput::AssistantResponse(_)));
        let terminal = expect_single(&mut model_events).await;
        assert!(matches!(
            terminal,
            ModelDriverOutput::ModelResponse(response)
                if matches!(response.outcome(), ModelOutcome::Succeeded)
        ));
        assert!(model_events.next().await.is_none());
        server.join().expect("the mock server should stop");
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
        let driver = OpenAiModelDriver {
            http_client: reqwest::Client::new(),
            api_key: "test-key".to_owned(),
            responses_url: format!("http://{address}/responses"),
            source: source(),
        };

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

    #[tokio::test]
    async fn a_context_limit_http_response_becomes_a_failed_terminal() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("the mock server should bind");
        let address = listener
            .local_addr()
            .expect("the mock server address should be available");
        let server = thread::spawn(move || {
            let (mut connection, _) = listener.accept().expect("the mock server should accept");
            read_request(&connection);
            let body = json!({
                "error": {
                    "code": "context_length_exceeded",
                    "message": "raw provider details"
                }
            })
            .to_string();
            write!(
                connection,
                "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .expect("the mock response should write");
        });
        let driver = OpenAiModelDriver {
            http_client: reqwest::Client::new(),
            api_key: "test-key".to_owned(),
            responses_url: format!("http://{address}/responses"),
            source: source(),
        };

        let conversation = test_conversation("Hello");
        let mut model_events = driver
            .invoke(driver_request(&conversation))
            .await
            .expect("the context-limit outcome should establish a semantic stream");
        let batch = model_events
            .next()
            .await
            .expect("the stream should yield a terminal batch")
            .expect("the terminal batch should be valid")
            .into_outputs();
        assert_eq!(batch.len(), 1);
        assert!(matches!(
            &batch[0],
            ModelDriverOutput::ModelResponse(response)
                if matches!(
                    response.outcome(),
                    ModelOutcome::Failed { failure }
                        if failure.category() == FailureCategory::ContextLimitExceeded
                            && failure.message() == "The model context limit was exceeded."
                )
        ));
        assert!(model_events.next().await.is_none());
        server.join().expect("the mock server should stop");
    }
}
