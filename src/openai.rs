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
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::conversation::Conversation;
use crate::conversation_event::{
    AssistantResponse, ConversationEventClass, ConversationEventEnvelope, ConversationEventError,
    ConversationEventExtension, ConversationEventKind, ConversationEventReadError,
    ConversationEventReader, ConversationFact, ConversationMessage, ConversationProblem,
    ConversationTurnId, InvalidAssistantResponse, InvalidConversationProblem,
    InvalidModelCommunication, InvocationError, ModelCommunication, ModelData, ModelEvent,
    ModelEventImportance, ModelId, ModelInvocationId, ModelIssue, ModelSource, ProviderId,
    StoredConversationEventKind, ToolCallId, ToolDefinition, ToolName, ToolRequest, ToolResponse,
    UserContent, UserMessageRequest,
};
use crate::model_driver::{
    ModelDriver, ModelDriverError, ModelDriverOutput, ModelDriverOutputBatch, ModelOutputStream,
    TurnInput,
};

type ResponseByteStream = BoxStream<'static, Result<Vec<u8>, OpenAiError>>;
type ProviderOutputStream = BoxStream<'static, Result<ModelDriverEvent, OpenAiError>>;

const OPEN_AI_NAMESPACE_VERSION: &str = "1";

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum OpenAiError {
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

enum ModelDriverEvent {
    Model {
        event: ModelEvent,
        data: Option<ModelData>,
    },
    Problem {
        problem: ModelIssue,
        data: Option<ModelData>,
    },
    ToolRequest {
        tool_name: ToolName,
        arguments: Value,
        provider_call_id: Option<String>,
    },
}

pub(crate) struct OpenAiModelDriver {
    http_client: Client,
    api_key: String,
    responses_url: String,
    source: ModelSource,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct OpenAiInvocationRequested {
    invocation_id: ModelInvocationId,
    turn_id: ConversationTurnId,
    model: ModelSource,
}

impl ConversationEventExtension for OpenAiInvocationRequested {
    fn class(&self) -> ConversationEventClass {
        ConversationEventClass::Command
    }

    fn namespace(&self) -> &str {
        "openai"
    }

    fn namespace_version(&self) -> &str {
        OPEN_AI_NAMESPACE_VERSION
    }

    fn event_type(&self) -> &str {
        "model_invocation_requested"
    }

    fn event_schema_version(&self) -> u32 {
        1
    }

    fn description(&self) -> &str {
        "OpenAI model invocation was requested."
    }

    fn serialize_payload(&self) -> Result<Value, ConversationEventError> {
        serde_json::to_value(self)
            .map_err(|error| ConversationEventError::Serialization(error.to_string()))
    }
}

impl OpenAiModelDriver {
    pub(crate) fn from_environment(model: ModelId) -> Result<Self, OpenAiError> {
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
        let conversation = input.conversation();
        let turn_id = input.turn_id();
        let pending_user_requests = input.pending_user_requests().to_vec();
        let pending_user_events = accepted_user_events(&pending_user_requests);
        let invocation_id = ModelInvocationId::new();
        let invocation_event = OpenAiInvocationRequested {
            invocation_id,
            turn_id,
            model: self.source.clone(),
        };
        let mut request_body = Map::new();
        request_body.insert(
            "model".to_owned(),
            Value::String(self.source.model().as_str().to_owned()),
        );
        request_body.insert(
            "input".to_owned(),
            semantic_input(conversation, &pending_user_requests),
        );
        let available_tools = conversation.available_tools();
        if !available_tools.is_empty() {
            request_body.insert(
                "tools".to_owned(),
                Value::Array(available_tools.iter().map(provider_tool).collect()),
            );
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
                    return Ok(invocation_error_stream(
                        pending_user_events,
                        invocation_event,
                        invocation_id,
                        error,
                        FailureStage::BeforeStream,
                    ));
                }
            };
            let response = match http_client.execute(request).await {
                Ok(response) => response,
                Err(error) => {
                    return Ok(invocation_error_stream(
                        pending_user_events,
                        invocation_event,
                        invocation_id,
                        OpenAiError::Transport(error.to_string()),
                        FailureStage::BeforeStream,
                    ));
                }
            };
            let response_status = response.status();
            if !response_status.is_success() {
                let response_body = match response.text().await {
                    Ok(response_body) => response_body,
                    Err(error) => {
                        return Ok(invocation_error_stream(
                            pending_user_events,
                            invocation_event,
                            invocation_id,
                            OpenAiError::Transport(error.to_string()),
                            FailureStage::BeforeStream,
                        ));
                    }
                };
                return match classify_response_failure(response_status, response_body) {
                    Ok(issue) => Ok(conversation_event_stream(
                        model_issue_stream(issue),
                        pending_user_events,
                        invocation_id,
                        Box::new(invocation_event),
                    )),
                    Err(error) => Ok(invocation_error_stream(
                        pending_user_events,
                        invocation_event,
                        invocation_id,
                        error,
                        FailureStage::BeforeStream,
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
                pending_user_events,
                invocation_id,
                Box::new(invocation_event),
            ))
        }
        .boxed()
    }
}

impl ConversationEventReader for OpenAiModelDriver {
    fn read_event(
        &self,
        envelope: &ConversationEventEnvelope,
    ) -> Result<Box<dyn ConversationEventExtension>, ConversationEventReadError> {
        if envelope.namespace() != "openai" {
            return Err(ConversationEventReadError::UnsupportedNamespace);
        }
        if envelope.event_type() != "model_invocation_requested"
            || envelope.event_schema_version() != 1
        {
            return Err(ConversationEventReadError::UnsupportedEvent);
        }
        serde_json::from_value::<OpenAiInvocationRequested>(envelope.payload().clone())
            .map(|event| Box::new(event) as Box<dyn ConversationEventExtension>)
            .map_err(|error| ConversationEventReadError::InvalidPayload(error.to_string()))
    }
}

fn semantic_input(
    conversation: &Conversation,
    pending_user_requests: &[UserMessageRequest],
) -> Value {
    let provider_call_ids = provider_tool_call_ids(conversation);
    let mut input =
        conversation
            .events()
            .iter()
            .filter_map(|conversation_event| match &conversation_event.kind {
                StoredConversationEventKind::Shared(ConversationEventKind::Fact(
                    ConversationFact::Message {
                        message: ConversationMessage::User { content, .. },
                        ..
                    },
                )) => {
                    let text = content
                        .iter()
                        .map(|content| match content {
                            UserContent::Text(text) => text.as_str(),
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    Some(json!({ "role": "user", "content": text }))
                }
                StoredConversationEventKind::Shared(ConversationEventKind::Fact(
                    ConversationFact::Message {
                        message: ConversationMessage::AssistantResponse { response, .. },
                        ..
                    },
                )) => Some(json!({ "role": "assistant", "content": response.message() })),
                StoredConversationEventKind::Shared(ConversationEventKind::Fact(
                    ConversationFact::ToolRequest { request, .. },
                )) => Some(provider_tool_request_input(request, &provider_call_ids)),
                StoredConversationEventKind::Shared(ConversationEventKind::Fact(
                    ConversationFact::ToolResponse { response, .. },
                )) => Some(provider_tool_response_input(response, &provider_call_ids)),
                StoredConversationEventKind::Shared(ConversationEventKind::Fact(
                    ConversationFact::Message {
                        message:
                            ConversationMessage::Communication { .. }
                            | ConversationMessage::Problem { .. },
                        ..
                    },
                ))
                | StoredConversationEventKind::Shared(ConversationEventKind::Fact(
                    ConversationFact::Lifecycle(_),
                ))
                | StoredConversationEventKind::Shared(ConversationEventKind::Fact(
                    ConversationFact::ToolsAvailable { .. },
                ))
                | StoredConversationEventKind::Shared(ConversationEventKind::Command(_))
                | StoredConversationEventKind::Extension(_) => None,
            })
            .collect::<Vec<_>>();
    input.extend(pending_user_requests.iter().map(|request| {
        let text = request
            .content
            .iter()
            .map(|content| match content {
                UserContent::Text(text) => text.as_str(),
            })
            .collect::<Vec<_>>()
            .join("\n");
        json!({ "role": "user", "content": text })
    }));
    Value::Array(input)
}

fn provider_tool(definition: &ToolDefinition) -> Value {
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

fn provider_tool_call_ids(conversation: &Conversation) -> HashMap<ToolCallId, String> {
    conversation
        .events()
        .iter()
        .filter_map(|conversation_event| match &conversation_event.kind {
            StoredConversationEventKind::Shared(ConversationEventKind::Fact(
                ConversationFact::ToolRequest { request, .. },
            )) => Some((request.call_id(), provider_tool_call_id(request))),
            _ => None,
        })
        .collect()
}

fn provider_tool_call_id(request: &ToolRequest) -> String {
    request
        .data()
        .and_then(|data| data.content().get("call_id"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| request.call_id().to_string())
}

fn provider_tool_request_input(
    request: &ToolRequest,
    provider_call_ids: &HashMap<ToolCallId, String>,
) -> Value {
    let call_id = provider_call_ids
        .get(&request.call_id())
        .cloned()
        .unwrap_or_else(|| request.call_id().to_string());
    json!({
        "type": "function_call",
        "call_id": call_id,
        "name": request.tool_name().as_str(),
        "arguments": serde_json::to_string(request.arguments())
            .expect("the tool request arguments should serialize"),
    })
}

fn provider_tool_response_input(
    response: &ToolResponse,
    provider_call_ids: &HashMap<ToolCallId, String>,
) -> Value {
    let call_id = provider_call_ids
        .get(&response.call_id())
        .cloned()
        .unwrap_or_else(|| response.call_id().to_string());
    json!({
        "type": "function_call_output",
        "call_id": call_id,
        "output": serde_json::to_string(response.outcome())
            .expect("the tool response outcome should serialize"),
    })
}

fn accepted_user_events(pending_user_requests: &[UserMessageRequest]) -> Vec<ModelDriverOutput> {
    pending_user_requests
        .iter()
        .map(|request| {
            ModelDriverOutput::Message(ConversationMessage::User {
                caused_by: Some(request.command_id),
                content: request.content.clone(),
            })
        })
        .collect()
}

fn classify_response_failure(status: StatusCode, body: String) -> Result<ModelIssue, OpenAiError> {
    if status == StatusCode::BAD_REQUEST && is_context_limit_error(&body) {
        return ModelIssue::try_context_limit_exceeded(
            "The model context limit was exceeded.".to_owned(),
        )
        .map_err(invalid_conversation_problem);
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

fn model_issue_stream(issue: ModelIssue) -> ProviderOutputStream {
    stream::once(async move {
        Ok(ModelDriverEvent::Problem {
            problem: issue,
            data: None,
        })
    })
    .boxed()
}

struct ConversationEventStreamState {
    provider_events: ProviderOutputStream,
    invocation_id: ModelInvocationId,
    pending_batches: VecDeque<ModelDriverOutputBatch>,
    terminated: bool,
}

fn invocation_error_stream(
    mut pending_user_events: Vec<ModelDriverOutput>,
    invocation_event: OpenAiInvocationRequested,
    invocation_id: ModelInvocationId,
    error: OpenAiError,
    failure_stage: FailureStage,
) -> ModelOutputStream {
    pending_user_events.push(ModelDriverOutput::Command(Box::new(invocation_event)));
    pending_user_events.push(ModelDriverOutput::Message(ConversationMessage::Problem {
        invocation_id: Some(invocation_id),
        data: None,
        problem: provider_problem(&error, failure_stage),
    }));
    let batch = ModelDriverOutputBatch::try_new(pending_user_events)
        .expect("an invocation failure batch contains an invocation event and a problem");
    stream::once(async move { Ok(batch) }).boxed()
}

fn conversation_event_stream(
    provider_events: ProviderOutputStream,
    initial_events: Vec<ModelDriverOutput>,
    invocation_id: ModelInvocationId,
    invocation_event: Box<dyn ConversationEventExtension>,
) -> ModelOutputStream {
    let mut initial_batch = initial_events;
    initial_batch.push(ModelDriverOutput::Command(invocation_event));
    let initial_batch = ModelDriverOutputBatch::try_new(initial_batch)
        .expect("a model driver batch includes its invocation event");
    stream::unfold(
        ConversationEventStreamState {
            provider_events,
            invocation_id,
            pending_batches: VecDeque::from([initial_batch]),
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
                    let output =
                        match translate_model_driver_event(driver_event, state.invocation_id) {
                            Ok(driver_output) => driver_output,
                            Err(error) => {
                                state.pending_batches = VecDeque::from([failure_batch(
                                    state.invocation_id,
                                    error,
                                    FailureStage::DuringStream,
                                )]);
                                state.terminated = true;
                                return state
                                    .pending_batches
                                    .pop_front()
                                    .map(|batch| (Ok(batch), state));
                            }
                        };
                    Some((Ok(ModelDriverOutputBatch::from(output)), state))
                }
                Some(Err(error)) => {
                    state.pending_batches = VecDeque::from([failure_batch(
                        state.invocation_id,
                        error,
                        FailureStage::DuringStream,
                    )]);
                    state.terminated = true;
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

#[derive(Clone, Copy)]
enum FailureStage {
    BeforeStream,
    DuringStream,
}

fn failure_batch(
    invocation_id: ModelInvocationId,
    error: OpenAiError,
    failure_stage: FailureStage,
) -> ModelDriverOutputBatch {
    ModelDriverOutputBatch::from(ModelDriverOutput::Message(ConversationMessage::Problem {
        invocation_id: Some(invocation_id),
        data: None,
        problem: provider_problem(&error, failure_stage),
    }))
}

fn provider_problem(error: &OpenAiError, failure_stage: FailureStage) -> ConversationProblem {
    let invocation_error = match error {
        OpenAiError::Authentication(_) => InvocationError::try_authentication(
            "The model provider could not authenticate the invocation.".to_owned(),
        ),
        OpenAiError::RateLimited(_) => InvocationError::try_rate_limited(
            "The model provider rate-limited the invocation.".to_owned(),
        ),
        OpenAiError::Transport(_) if matches!(failure_stage, FailureStage::DuringStream) => {
            InvocationError::try_stream_interrupted(
                "The model response stream was interrupted.".to_owned(),
            )
        }
        OpenAiError::Transport(_) => {
            InvocationError::try_transport("The model provider could not be reached.".to_owned())
        }
        OpenAiError::InvalidRequest(_) => InvocationError::try_invalid_request(
            "The model invocation request was invalid.".to_owned(),
        ),
        OpenAiError::InvalidResponse(_) => InvocationError::try_invalid_provider_response(
            "The model provider returned an invalid response.".to_owned(),
        ),
        OpenAiError::StreamInterrupted(_) => InvocationError::try_stream_interrupted(
            "The model response stream was interrupted.".to_owned(),
        ),
        OpenAiError::Provider(_) => InvocationError::try_provider_failure(
            "The model provider failed the invocation.".to_owned(),
        ),
    };
    ConversationProblem::Invocation(
        invocation_error.expect("the sanitized provider problem should be valid"),
    )
}

fn translate_model_driver_event(
    driver_event: ModelDriverEvent,
    invocation_id: ModelInvocationId,
) -> Result<ModelDriverOutput, OpenAiError> {
    let output = match driver_event {
        ModelDriverEvent::Model { event, data } => match event {
            ModelEvent::Assistant(response) => {
                ModelDriverOutput::Message(ConversationMessage::AssistantResponse {
                    invocation_id,
                    data,
                    response,
                })
            }
            ModelEvent::Communication(communication) => {
                ModelDriverOutput::Message(ConversationMessage::Communication {
                    invocation_id,
                    data,
                    communication,
                })
            }
        },
        ModelDriverEvent::Problem { problem, data } => {
            ModelDriverOutput::Message(ConversationMessage::Problem {
                invocation_id: Some(invocation_id),
                data,
                problem: ConversationProblem::Issue(problem),
            })
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
                .map_err(|error| OpenAiError::InvalidResponse(error.to_string()))?;
            let request =
                ToolRequest::try_new(ToolCallId::new(), tool_name, arguments, invocation_id, data)
                    .map_err(|error| OpenAiError::InvalidResponse(error.to_string()))?;
            ModelDriverOutput::ToolRequest(request)
        }
    };
    Ok(output)
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
            emit_refusal(&mut response_state.refusal_outputs, &key)?
                .into_iter()
                .collect()
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
    model_events.extend(emit_remaining_assistant_responses(
        &mut response_state.assistant_outputs,
    )?);
    model_events.extend(emit_remaining_refusals(
        &mut response_state.refusal_outputs,
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
    for completed_refusal in completed_content.refusals {
        if !completed_output_already_emitted(
            &response_state.refusal_outputs,
            &completed_refusal.key,
        ) {
            model_events.push(model_refusal(completed_refusal.text)?);
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
        .chain(&response_state.refusal_outputs)
        .any(|output| output.emitted)
        || response_state
            .function_calls
            .iter()
            .any(|function_call| function_call.emitted)
        || model_events.iter().any(|event| {
            matches!(
                event,
                ModelDriverEvent::Model {
                    event: ModelEvent::Assistant(_),
                    ..
                } | ModelDriverEvent::Problem { .. }
                    | ModelDriverEvent::ToolRequest { .. }
            )
        });
    if !has_completed_model_output {
        return Err(OpenAiError::InvalidResponse(
            "the completed response contained no model message".to_owned(),
        ));
    }
    response_state.completed = true;
    Ok(model_events)
}

fn emit_reasoning(
    reasoning_outputs: &mut [AccumulatedText],
    key: &str,
) -> Result<Option<ModelDriverEvent>, OpenAiError> {
    let output = un_emitted_text(reasoning_outputs, key)?;
    let Some(reasoning_text) = preferred_text(&output.streamed_text, &output.completed_text) else {
        return Ok(None);
    };
    let model_event =
        model_communication(reasoning_text, "reasoning", ModelEventImportance::Detailed)?;
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
    let model_event = model_communication(
        reasoning_summary,
        "reasoning_summary",
        ModelEventImportance::Interesting,
    )?;
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
    let model_event = assistant_response(assistant_text)?;
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
    let model_event = model_refusal(refusal)?;
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

fn assistant_response(message: String) -> Result<ModelDriverEvent, OpenAiError> {
    AssistantResponse::new(message)
        .map(ModelEvent::Assistant)
        .map(|event| ModelDriverEvent::Model { event, data: None })
        .map_err(invalid_assistant_response)
}

fn model_refusal(message: String) -> Result<ModelDriverEvent, OpenAiError> {
    ModelIssue::try_refusal(message)
        .map(|problem| ModelDriverEvent::Problem {
            problem,
            data: None,
        })
        .map_err(invalid_conversation_problem)
}

fn model_context_limit_exceeded() -> Result<ModelDriverEvent, OpenAiError> {
    ModelIssue::try_context_limit_exceeded("The model context limit was exceeded.".to_owned())
        .map(|problem| ModelDriverEvent::Problem {
            problem,
            data: None,
        })
        .map_err(invalid_conversation_problem)
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

fn model_communication(
    message: String,
    subtype: &str,
    importance: ModelEventImportance,
) -> Result<ModelDriverEvent, OpenAiError> {
    ModelCommunication::new(message, importance, subtype.to_owned())
        .map(ModelEvent::Communication)
        .map(|event| ModelDriverEvent::Model { event, data: None })
        .map_err(invalid_model_communication)
}

fn invalid_assistant_response(error: InvalidAssistantResponse) -> OpenAiError {
    OpenAiError::InvalidResponse(error.to_string())
}

fn invalid_model_communication(error: InvalidModelCommunication) -> OpenAiError {
    OpenAiError::InvalidResponse(error.to_string())
}

fn invalid_conversation_problem(error: InvalidConversationProblem) -> OpenAiError {
    OpenAiError::InvalidResponse(error.to_string())
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
    refusals: Vec<CompletedText>,
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
    let mut refusals = Vec::new();
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
            match content_item.get("type").and_then(Value::as_str) {
                Some("output_text") => {
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
                Some("refusal") => {
                    let refusal = content_item
                        .get("refusal")
                        .and_then(Value::as_str)
                        .ok_or_else(|| {
                            OpenAiError::InvalidResponse(
                                "completed OpenAI refusal was not a string".to_owned(),
                            )
                        })?;
                    refusals.push(CompletedText {
                        key,
                        text: refusal.to_owned(),
                    });
                }
                _ => {}
            }
        }
    }
    Ok(CompletedResponseContent {
        assistant_outputs,
        refusals,
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

    use futures_util::StreamExt;
    use futures_util::stream;
    use reqwest::StatusCode;
    use serde_json::{Map, Value, json};
    use time::OffsetDateTime;

    use crate::conversation::{Conversation, ConversationId};
    use crate::conversation_event::{
        AssistantResponse, ConversationEventId, ConversationEventKind, ConversationEventRecord,
        ConversationFact, ConversationMessage, ConversationProblem, ConversationTurnId,
        ModelCommunication, ModelData, ModelEvent, ModelEventImportance, ModelId,
        ModelInvocationId, ModelIssue, ModelSource, ProviderId, StoredConversationEventKind,
        ToolCallId, ToolDefinition, ToolExecutionProblem, ToolName, ToolOutcome, ToolRequest,
        ToolResponse, UserContent,
    };
    use crate::model_driver::{ModelDriver, ModelDriverOutput, ModelDriverOutputBatch, TurnInput};

    use super::{
        ModelDriverEvent, OpenAiError, OpenAiModelDriver, ResponseByteStream,
        classify_response_failure, model_communication, model_output_stream, provider_tool,
        semantic_input,
    };

    fn conversation_event(
        conversation_id: ConversationId,
        position: u64,
        kind: ConversationEventKind,
    ) -> ConversationEventRecord {
        ConversationEventRecord {
            conversation_id,
            position,
            id: ConversationEventId::new(),
            timestamp: OffsetDateTime::UNIX_EPOCH,
            schema_version: 7,
            kind: StoredConversationEventKind::Shared(kind),
        }
    }

    #[test]
    fn semantic_input_projects_canonical_events_and_ignores_model_data() {
        let conversation_id = ConversationId::new();
        let model_data = ModelData::new(Map::from_iter([(
            "native".to_owned(),
            Value::String("ignored".to_owned()),
        )]))
        .expect("the model data should be valid");
        let turn_id = ConversationTurnId::new();
        let invocation_id = ModelInvocationId::new();
        let conversation = Conversation::from_events(vec![
            conversation_event(
                conversation_id,
                0,
                ConversationEventKind::Fact(ConversationFact::Message {
                    message: ConversationMessage::User {
                        caused_by: None,
                        content: vec![UserContent::Text("Hello".to_owned())],
                    },
                    turn_id: None,
                }),
            ),
            conversation_event(
                conversation_id,
                1,
                ConversationEventKind::Fact(ConversationFact::Message {
                    message: ConversationMessage::Communication {
                        invocation_id,
                        data: Some(model_data.clone()),
                        communication: ModelCommunication::new(
                            "Reasoning".to_owned(),
                            ModelEventImportance::Detailed,
                            "reasoning".to_owned(),
                        )
                        .expect("the model communication should be valid"),
                    },
                    turn_id: Some(turn_id),
                }),
            ),
            conversation_event(
                conversation_id,
                2,
                ConversationEventKind::Fact(ConversationFact::Message {
                    message: ConversationMessage::AssistantResponse {
                        invocation_id,
                        data: Some(model_data),
                        response: AssistantResponse::new("Hello.".to_owned())
                            .expect("the assistant response should be valid"),
                    },
                    turn_id: Some(turn_id),
                }),
            ),
        ])
        .expect("the conversation should be valid");

        assert_eq!(
            semantic_input(&conversation, &[]),
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
        let invocation_id = ModelInvocationId::new();
        let call_id = ToolCallId::new();
        let model_data = ModelData::new(Map::from_iter([(
            "call_id".to_owned(),
            Value::String("call_native".to_owned()),
        )]))
        .expect("the model data should be valid");
        let request = ToolRequest::try_new(
            call_id,
            ToolName::try_new("shell".to_owned()).expect("the tool name should be valid"),
            json!({ "command": "pwd" }),
            invocation_id,
            Some(model_data),
        )
        .expect("the tool request should be valid");
        let response = ToolResponse::new(
            call_id,
            ToolOutcome::Result {
                value: json!({ "stdout": "/tmp\n" }),
            },
        );
        let conversation = Conversation::from_events(vec![
            conversation_event(
                conversation_id,
                0,
                ConversationEventKind::Fact(ConversationFact::Message {
                    message: ConversationMessage::User {
                        caused_by: None,
                        content: vec![UserContent::Text("Run pwd".to_owned())],
                    },
                    turn_id: None,
                }),
            ),
            conversation_event(
                conversation_id,
                1,
                ConversationEventKind::Fact(ConversationFact::ToolRequest {
                    request,
                    turn_id: Some(ConversationTurnId::new()),
                }),
            ),
            conversation_event(
                conversation_id,
                2,
                ConversationEventKind::Fact(ConversationFact::ToolResponse {
                    response,
                    turn_id: Some(ConversationTurnId::new()),
                }),
            ),
        ])
        .expect("the conversation should be valid");

        assert_eq!(
            semantic_input(&conversation, &[]),
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
                    "output": "{\"type\":\"result\",\"value\":{\"stdout\":\"/tmp\\n\"}}"
                }
            ])
        );
    }

    #[test]
    fn semantic_input_falls_back_to_the_portable_tool_call_id() {
        let conversation_id = ConversationId::new();
        let call_id = ToolCallId::new();
        let request = ToolRequest::try_new(
            call_id,
            ToolName::try_new("shell".to_owned()).expect("the tool name should be valid"),
            json!({ "command": "pwd" }),
            ModelInvocationId::new(),
            None,
        )
        .expect("the tool request should be valid");
        let conversation = Conversation::from_events(vec![
            conversation_event(
                conversation_id,
                0,
                ConversationEventKind::Fact(ConversationFact::ToolRequest {
                    request,
                    turn_id: None,
                }),
            ),
            conversation_event(
                conversation_id,
                1,
                ConversationEventKind::Fact(ConversationFact::ToolResponse {
                    response: ToolResponse::new(
                        call_id,
                        ToolOutcome::Problem {
                            problem: ToolExecutionProblem::unknown_tool(
                                ToolName::try_new("shell".to_owned())
                                    .expect("the tool name should be valid"),
                            ),
                        },
                    ),
                    turn_id: None,
                }),
            ),
        ])
        .expect("the conversation should be valid");

        let input = semantic_input(&conversation, &[]);
        assert_eq!(input[0]["call_id"], call_id.to_string());
        assert_eq!(input[1]["call_id"], call_id.to_string());
        let output: Value = serde_json::from_str(
            input[1]["output"]
                .as_str()
                .expect("the tool output should be a string"),
        )
        .expect("the tool output should be JSON");
        assert_eq!(output["type"], "problem");
        assert_eq!(output["problem"]["kind"], "unknown_tool");
        assert_eq!(output["problem"]["message"], "unknown tool: shell");
        assert!(output["problem"].get("details").is_none());
    }

    #[test]
    fn semantic_input_projects_tool_problem_details() {
        let conversation_id = ConversationId::new();
        let call_id = ToolCallId::new();
        let request = ToolRequest::try_new(
            call_id,
            ToolName::try_new("shell".to_owned()).expect("the tool name should be valid"),
            json!({ "command": "sleep 5" }),
            ModelInvocationId::new(),
            None,
        )
        .expect("the tool request should be valid");
        let response = ToolResponse::new(
            call_id,
            ToolOutcome::Problem {
                problem: ToolExecutionProblem::try_timed_out(
                    "the shell command timed out after 1 seconds".to_owned(),
                    Some(json!({
                        "timeout_seconds": 1,
                        "stdout": "partial",
                        "stderr": "",
                        "stdout_truncated": false,
                        "stderr_truncated": false
                    })),
                )
                .expect("the timeout problem should be valid"),
            },
        );
        let conversation = Conversation::from_events(vec![
            conversation_event(
                conversation_id,
                0,
                ConversationEventKind::Fact(ConversationFact::ToolRequest {
                    request,
                    turn_id: None,
                }),
            ),
            conversation_event(
                conversation_id,
                1,
                ConversationEventKind::Fact(ConversationFact::ToolResponse {
                    response,
                    turn_id: None,
                }),
            ),
        ])
        .expect("the conversation should be valid");

        let input = semantic_input(&conversation, &[]);
        let output: Value = serde_json::from_str(
            input[1]["output"]
                .as_str()
                .expect("the tool output should be a string"),
        )
        .expect("the tool output should be JSON");
        assert_eq!(output["type"], "problem");
        assert_eq!(output["problem"]["kind"], "timed_out");
        assert_eq!(
            output["problem"]["message"],
            "the shell command timed out after 1 seconds"
        );
        assert_eq!(output["problem"]["details"]["timeout_seconds"], 1);
        assert_eq!(output["problem"]["details"]["stdout"], "partial");
        assert_eq!(output["problem"]["details"]["stdout_truncated"], false);
    }

    fn response_byte_stream(chunks: Vec<Vec<u8>>) -> ResponseByteStream {
        stream::iter(chunks.into_iter().map(Ok::<Vec<u8>, OpenAiError>)).boxed()
    }

    fn one_byte_chunks(input: &str) -> Vec<Vec<u8>> {
        input.as_bytes().iter().map(|byte| vec![*byte]).collect()
    }

    async fn collect_events(input: &str) -> Vec<Result<ModelEvent, OpenAiError>> {
        model_output_stream(response_byte_stream(vec![input.as_bytes().to_vec()]))
            .map(|result| {
                result.and_then(|driver_event| match driver_event {
                    ModelDriverEvent::Model { event, .. } => Ok(event),
                    ModelDriverEvent::Problem { .. } => Err(OpenAiError::InvalidResponse(
                        "the test expected a model event, not a model problem".to_owned(),
                    )),
                    ModelDriverEvent::ToolRequest { .. } => Err(OpenAiError::InvalidResponse(
                        "the test expected a model event, not a tool request".to_owned(),
                    )),
                })
            })
            .collect()
            .await
    }

    async fn collect_outputs(input: &str) -> Vec<Result<ModelDriverEvent, OpenAiError>> {
        model_output_stream(response_byte_stream(vec![input.as_bytes().to_vec()]))
            .collect()
            .await
    }

    fn expect_model_event(driver_event: ModelDriverEvent) -> ModelEvent {
        match driver_event {
            ModelDriverEvent::Model { event, .. } => event,
            ModelDriverEvent::Problem { .. } | ModelDriverEvent::ToolRequest { .. } => {
                panic!("the output should be a model event")
            }
        }
    }

    fn expect_event(output: ModelDriverOutput) -> ConversationMessage {
        match output {
            ModelDriverOutput::Message(message) => message,
            ModelDriverOutput::ToolRequest(_)
            | ModelDriverOutput::Command(_)
            | ModelDriverOutput::Extension(_) => {
                panic!("the output should be a conversation message")
            }
        }
    }

    fn expect_single_event(batch: ModelDriverOutputBatch) -> ConversationMessage {
        let mut outputs = batch.into_outputs();
        assert_eq!(outputs.len(), 1, "the batch should hold one output");
        expect_event(outputs.remove(0))
    }

    fn test_conversation() -> Conversation {
        let conversation_id = ConversationId::new();
        Conversation::from_events(vec![conversation_event(
            conversation_id,
            0,
            ConversationEventKind::Fact(ConversationFact::Message {
                message: ConversationMessage::User {
                    caused_by: None,
                    content: vec![UserContent::Text("Hello".to_owned())],
                },
                turn_id: None,
            }),
        )])
        .expect("the conversation should be valid")
    }

    fn source() -> ModelSource {
        ModelSource::new(
            ProviderId::from_str("openai").expect("the provider identifier should be valid"),
            ModelId::from_str("gpt-5.6").expect("the model identifier should be valid"),
        )
    }

    fn driver_request(conversation: &Conversation) -> TurnInput<'_> {
        TurnInput::new(conversation, ConversationTurnId::new())
    }

    #[tokio::test]
    async fn invoke_returns_a_future_that_establishes_one_conversation_event_stream() {
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

        let conversation = test_conversation();
        let mut model_events = driver
            .invoke(driver_request(&conversation))
            .await
            .expect("the invocation should establish its stream");
        let invocation = model_events
            .next()
            .await
            .expect("the stream should yield an invocation batch")
            .expect("the invocation batch should be valid")
            .into_outputs();
        assert!(matches!(
            invocation.as_slice(),
            [ModelDriverOutput::Command(_)]
        ));
        let first_event = expect_single_event(
            model_events
                .next()
                .await
                .expect("the stream should yield reasoning")
                .expect("the reasoning should be valid"),
        );
        let second_event = expect_single_event(
            model_events
                .next()
                .await
                .expect("the stream should yield an answer")
                .expect("the answer should be valid"),
        );

        assert!(matches!(
            &first_event,
            ConversationMessage::Communication { .. }
        ));
        assert!(matches!(
            &second_event,
            ConversationMessage::AssistantResponse { .. }
        ));
        assert!(matches!(
            &first_event,
            ConversationMessage::Communication { data, .. } if data.is_none()
        ));
        assert!(matches!(
            &second_event,
            ConversationMessage::AssistantResponse { data, .. } if data.is_none()
        ));
        assert!(model_events.next().await.is_none());
        server.join().expect("the mock server should stop");
    }

    #[tokio::test]
    async fn an_early_http_failure_produces_a_problem() {
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

        let conversation = test_conversation();
        let result = driver.invoke(driver_request(&conversation)).await;

        let mut model_events = result.expect("the invocation should establish a stream");
        let outputs = model_events
            .next()
            .await
            .expect("the stream should yield a failure batch")
            .expect("the failure batch should be valid")
            .into_outputs();
        assert!(matches!(
            outputs.as_slice(),
            [
                ModelDriverOutput::Command(_),
                ModelDriverOutput::Message(ConversationMessage::Problem { .. })
            ]
        ));
        assert!(model_events.next().await.is_none());
        server.join().expect("the mock server should stop");
    }

    #[tokio::test]
    async fn a_context_limit_http_response_becomes_a_model_issue_stream() {
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

        let conversation = test_conversation();
        let mut model_events = driver
            .invoke(driver_request(&conversation))
            .await
            .expect("the context-limit outcome should establish a semantic stream");
        let invocation = model_events
            .next()
            .await
            .expect("the stream should yield an invocation batch")
            .expect("the invocation batch should be valid")
            .into_outputs();
        assert!(matches!(
            invocation.as_slice(),
            [ModelDriverOutput::Command(_)]
        ));
        let model_event = expect_single_event(
            model_events
                .next()
                .await
                .expect("the stream should yield a context-limit issue")
                .expect("the context-limit issue should be valid"),
        );

        assert!(matches!(
            &model_event,
            ConversationMessage::Problem {
                problem: ConversationProblem::Issue(ModelIssue::ContextLimitExceeded { .. }),
                ..
            }
        ));
        let ConversationMessage::Problem { problem, .. } = &model_event else {
            panic!("the output should be a model issue");
        };
        assert_eq!(problem.message(), "The model context limit was exceeded.");
        assert!(model_events.next().await.is_none());
        server.join().expect("the mock server should stop");
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

        assert_eq!(expect_model_event(model_event).message(), "Hello");
        assert!(model_events.next().await.is_none());
    }

    #[tokio::test]
    async fn a_completed_refusal_is_a_model_issue_not_an_assistant_response() {
        let input = concat!(
            "data: {\"type\":\"response.refusal.delta\",\"delta\":\"I cannot \"}\n\n",
            "data: {\"type\":\"response.refusal.done\",\"refusal\":\"I cannot comply.\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{}}\n\n"
        );

        let events = collect_outputs(input)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("the refusal stream should parse");

        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            ModelDriverEvent::Problem {
                problem: ModelIssue::Refusal { .. },
                ..
            }
        ));
        let ModelDriverEvent::Problem { problem: issue, .. } = &events[0] else {
            panic!("the output should be a refusal issue");
        };
        assert_eq!(issue.message(), "I cannot comply.");
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

        assert_eq!(events.len(), 3);
        assert_eq!(events[0].message(), "Detailed thought");
        assert_eq!(events[0].importance(), ModelEventImportance::Detailed);
        assert_eq!(events[1].message(), "Summary");
        assert_eq!(events[1].importance(), ModelEventImportance::Interesting);
        assert_eq!(events[2].message(), "Answer");
        assert_eq!(events[2].importance(), ModelEventImportance::Important);
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
        assert_eq!(expect_model_event(completed_event).message(), "Hello");
        assert!(matches!(
            model_events.next().await,
            Some(Err(OpenAiError::Provider(_)))
        ));
        assert!(model_events.next().await.is_none());
    }

    #[tokio::test]
    async fn context_limit_stream_failure_is_a_model_issue() {
        let input = concat!(
            "data: {\"type\":\"error\",\"code\":\"context_length_exceeded\",\"message\":\"raw details\"}\n\n",
            "data: [DONE]\n\n"
        );
        let events = collect_outputs(input)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("the context-limit failure should be semantic");

        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            ModelDriverEvent::Problem {
                problem: ModelIssue::ContextLimitExceeded { .. },
                ..
            }
        ));
    }

    #[tokio::test]
    async fn premature_body_end_is_a_stream_interruption() {
        let input = "data: {\"type\":\"response.output_text.done\",\"text\":\"Hello\"}\n\n";
        let mut model_events =
            model_output_stream(response_byte_stream(vec![input.as_bytes().to_vec()]));

        assert!(matches!(
            model_events.next().await,
            Some(Ok(ModelDriverEvent::Model {
                event: ModelEvent::Assistant(_),
                ..
            }))
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
            Some(Ok(ModelDriverEvent::Model {
                event: ModelEvent::Assistant(_),
                ..
            }))
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

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].message(), "Answer");
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

        let events = collect_outputs(input)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("the function call stream should parse");

        assert_eq!(events.len(), 1);
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

        let results = collect_outputs(input).await;

        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], Err(OpenAiError::InvalidResponse(_))));
    }

    #[tokio::test]
    async fn a_completed_response_supplies_function_call_arguments_as_a_fallback() {
        let input = concat!(
            "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"id\":\"fc_1\",\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"shell\",\"arguments\":\"\"}}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"output\":[{\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"shell\",\"arguments\":\"{\\\"command\\\":\\\"pwd\\\"}\"}]}}\n\n",
            "data: [DONE]\n\n"
        );

        let events = collect_outputs(input)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("the fallback function call should parse");

        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            ModelDriverEvent::ToolRequest { arguments, .. }
                if arguments == &json!({ "command": "pwd" })
        ));
    }

    #[tokio::test]
    async fn response_completed_adds_unstreamed_indexed_output_without_duplicates() {
        let input = concat!(
            "data: {\"type\":\"response.output_text.done\",\"output_index\":0,\"content_index\":0,\"text\":\"First\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"output\":[{\"content\":[{\"type\":\"output_text\",\"text\":\"First\"},{\"type\":\"output_text\",\"text\":\"Second\"}]}]}}\n\n"
        );

        let events = collect_events(input)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("the completed response fallback should parse");

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].message(), "First");
        assert_eq!(events[1].message(), "Second");
    }

    #[tokio::test]
    async fn distinct_indexed_output_completions_yield_distinct_semantic_events() {
        let input = concat!(
            "data: {\"type\":\"response.output_text.done\",\"output_index\":0,\"content_index\":0,\"text\":\"First\"}\n\n",
            "data: {\"type\":\"response.output_text.done\",\"output_index\":1,\"content_index\":0,\"text\":\"Second\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{}}\n\n"
        );

        let events = collect_events(input)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("the indexed output should parse");

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].message(), "First");
        assert_eq!(events[1].message(), "Second");
    }

    #[tokio::test]
    async fn duplicate_indexed_completion_is_a_stream_error_after_the_completed_event() {
        let input = concat!(
            "data: {\"type\":\"response.output_text.done\",\"output_index\":0,\"content_index\":0,\"text\":\"Answer\"}\n\n",
            "data: {\"type\":\"response.output_text.done\",\"output_index\":0,\"content_index\":0,\"text\":\"Answer\"}\n\n"
        );
        let mut events = model_output_stream(response_byte_stream(vec![input.as_bytes().to_vec()]));

        assert!(matches!(
            events.next().await,
            Some(Ok(ModelDriverEvent::Model {
                event: ModelEvent::Assistant(_),
                ..
            }))
        ));
        assert!(matches!(
            events.next().await,
            Some(Err(OpenAiError::InvalidResponse(_)))
        ));
    }

    #[tokio::test]
    async fn response_completed_supplies_final_object_fallback_at_end_of_stream() {
        let input = "data: {\"type\":\"response.completed\",\"response\":{\"output\":[{\"content\":[{\"type\":\"output_text\",\"text\":\"Fallback\"}]}]}}";

        let events = collect_events(input)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("the buffered final event should parse");

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].message(), "Fallback");
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
    fn invalid_model_communication_maps_to_invalid_response() {
        let empty_message = model_communication(
            "   ".to_owned(),
            "reasoning",
            ModelEventImportance::Detailed,
        );
        let empty_subtype = model_communication(
            "reasoning".to_owned(),
            "   ",
            ModelEventImportance::Detailed,
        );

        assert!(matches!(
            empty_message,
            Err(OpenAiError::InvalidResponse(_))
        ));
        assert!(matches!(
            empty_subtype,
            Err(OpenAiError::InvalidResponse(_))
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
}
