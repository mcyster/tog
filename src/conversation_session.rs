use std::collections::HashSet;
use std::error::Error;
use std::fmt::{Display, Formatter};

use futures_util::StreamExt;

use crate::conversation::{
    Conversation, ConversationEvent, ConversationEventContent, ConversationEventId,
    ConversationHistory, ConversationId, FailureCategory, ModelOutcome, ModelRequest,
    ModelResponse, OperationFailure, ToolResponse, ToolsetDeclared, TurnEnd, TurnOutcome,
    TurnStart, User, UserContent, latest_toolset,
};
use crate::conversation_event_store::ConversationEventStore;
use crate::model_driver::{ModelDriver, ModelDriverError, ModelDriverOutput, TurnInput};
use crate::tools::ToolRegistry;
use crate::toolset::Toolset;

pub(crate) type ConversationSessionResult<T> = Result<T, Box<dyn Error>>;

pub(crate) const MAXIMUM_TOOL_CONTINUATION_ROUNDS: u32 = 8;

pub(crate) enum ConversationSessionProgress {
    InvocationStarted { model: String },
    EventCompleted { event: ConversationEvent },
}

pub(crate) struct ConversationSession<Store: ConversationEventStore> {
    conversation_id: ConversationId,
    event_store: Store,
    model_driver: Box<dyn ModelDriver>,
    tool_registry: ToolRegistry,
}

impl<Store: ConversationEventStore> ConversationSession<Store> {
    pub(crate) fn create(
        event_store: Store,
        model_driver: Box<dyn ModelDriver>,
        tool_registry: ToolRegistry,
    ) -> Self {
        Self {
            conversation_id: ConversationId::new(),
            event_store,
            model_driver,
            tool_registry,
        }
    }

    pub(crate) fn open(
        conversation_id: ConversationId,
        event_store: Store,
        model_driver: Box<dyn ModelDriver>,
        tool_registry: ToolRegistry,
    ) -> ConversationSessionResult<Self> {
        ConversationHistory::from_events(event_store.load(conversation_id)?)?;
        Ok(Self {
            conversation_id,
            event_store,
            model_driver,
            tool_registry,
        })
    }

    pub(crate) fn id(&self) -> ConversationId {
        self.conversation_id
    }

    pub(crate) fn add_user_request(
        &self,
        content: Vec<UserContent>,
    ) -> ConversationSessionResult<ConversationEventId> {
        let user = User::new(content).map_err(Box::new)?;
        let records = self.event_store.append(
            self.conversation_id,
            vec![self.complete_event(ConversationEventContent::User(user))],
        )?;
        Ok(records[0].event().id())
    }

    pub(crate) async fn invoke(
        &self,
        mut report_progress: impl FnMut(ConversationSessionProgress) -> ConversationSessionResult<()>,
    ) -> ConversationSessionResult<TurnOutcome> {
        let conversation = self.load()?;
        let trigger_user_id = pending_user_id(&conversation);
        let input_through = last_position(&conversation);
        let turn_records = self.event_store.append(
            self.conversation_id,
            vec![
                self.complete_event(ConversationEventContent::TurnStart(TurnStart::new(
                    trigger_user_id,
                    input_through,
                ))),
            ],
        )?;
        let turn_id = turn_records[0].event().id();

        let mut depends_on = Vec::new();
        let mut completed_tool_rounds = 0_u32;

        loop {
            let desired_toolset =
                Toolset::immediate(self.tool_registry.definitions()).map_err(Box::new)?;
            let conversation = self.load()?;
            let should_emit_toolset = match latest_toolset(conversation.events()) {
                None => !desired_toolset.entries().is_empty(),
                Some(effective) => effective != &desired_toolset,
            };
            if should_emit_toolset {
                self.event_store.append(
                    self.conversation_id,
                    vec![self.complete_event(ConversationEventContent::Toolset(
                        ToolsetDeclared::new(desired_toolset),
                    ))],
                )?;
            }
            let conversation = self.load()?;
            let input_through = last_position(&conversation);
            let source = self.model_driver.source().clone();
            let request_records = self.event_store.append(
                self.conversation_id,
                vec![
                    self.complete_event(ConversationEventContent::ModelRequest(
                        ModelRequest::new(
                            turn_id,
                            source.clone(),
                            input_through,
                            std::mem::take(&mut depends_on),
                            None,
                            None,
                        )
                        .map_err(Box::new)?,
                    )),
                ],
            )?;
            let model_request_id = request_records[0].event().id();
            let turn_input = TurnInput::new(&conversation, model_request_id, input_through);
            report_progress(ConversationSessionProgress::InvocationStarted {
                model: source.model().as_str().to_owned(),
            })?;

            let mut output_stream = self.model_driver.invoke(turn_input).await?;
            let mut outputs_for_request = Vec::new();
            let mut tool_requests = Vec::new();
            let mut terminal_response: Option<ModelResponse> = None;

            while let Some(output_batch) = output_stream.next().await {
                let output_batch = output_batch?;
                if terminal_response.is_some() {
                    return Err(Box::new(ModelDriverError::OutputAfterTerminalResponse {
                        model_request_id,
                    }));
                }
                let mut outputs = output_batch.into_outputs();
                if outputs.iter().any(|output| output.is_terminal_response()) {
                    if outputs.len() != 1 {
                        return Err(Box::new(ModelDriverError::TerminalResponseNotAlone {
                            model_request_id,
                        }));
                    }
                    let ModelDriverOutput::ModelResponse(driver_response) = outputs.remove(0)
                    else {
                        unreachable!("the terminal response output was just identified");
                    };
                    self.ensure_request_reference(&driver_response, model_request_id)?;
                    let response = ModelResponse::new(
                        model_request_id,
                        std::mem::take(&mut outputs_for_request),
                        driver_response.outcome().clone(),
                        driver_response.usage().cloned(),
                    )
                    .map_err(Box::new)?;
                    let records = self.event_store.append(
                        self.conversation_id,
                        vec![self.complete_event(ConversationEventContent::ModelResponse(
                            response.clone(),
                        ))],
                    )?;
                    report_progress(ConversationSessionProgress::EventCompleted {
                        event: records[0].event().clone(),
                    })?;
                    terminal_response = Some(response);
                    continue;
                }

                let mut events = Vec::new();
                for output in outputs {
                    match output {
                        ModelDriverOutput::AssistantResponse(response) => {
                            self.ensure_request_reference(&response, model_request_id)?;
                            events.push(ConversationEventContent::AssistantResponse(response));
                        }
                        ModelDriverOutput::ToolRequest(request) => {
                            self.ensure_request_reference(&request, model_request_id)?;
                            events.push(ConversationEventContent::ToolRequest(request));
                        }
                        ModelDriverOutput::ModelSpecificEvent(event) => {
                            self.ensure_request_reference(&event, model_request_id)?;
                            events.push(ConversationEventContent::ModelSpecificEvent(event));
                        }
                        ModelDriverOutput::ModelResponse(_) => {
                            unreachable!("terminal responses are handled as the sole batch output")
                        }
                    }
                }
                let records = self.event_store.append(
                    self.conversation_id,
                    events
                        .into_iter()
                        .map(|content| self.complete_event(content))
                        .collect(),
                )?;
                for record in &records {
                    match record.event().content() {
                        ConversationEventContent::AssistantResponse(_) => {
                            outputs_for_request.push(record.event().id());
                            report_progress(ConversationSessionProgress::EventCompleted {
                                event: record.event().clone(),
                            })?;
                        }
                        ConversationEventContent::ToolRequest(request) => {
                            outputs_for_request.push(record.event().id());
                            tool_requests.push((record.event().id(), request.clone()));
                            report_progress(ConversationSessionProgress::EventCompleted {
                                event: record.event().clone(),
                            })?;
                        }
                        ConversationEventContent::ModelSpecificEvent(_) => {
                            report_progress(ConversationSessionProgress::EventCompleted {
                                event: record.event().clone(),
                            })?;
                        }
                        _ => {}
                    }
                }
            }

            let terminal_response = match terminal_response {
                Some(response) => response,
                None => {
                    let failure = OperationFailure::try_new(
                        FailureCategory::StreamInterrupted,
                        "The model response stream ended without a terminal model response."
                            .to_owned(),
                        None,
                    )
                    .map_err(Box::new)?;
                    let response = ModelResponse::new(
                        model_request_id,
                        std::mem::take(&mut outputs_for_request),
                        ModelOutcome::Failed { failure },
                        None,
                    )
                    .map_err(Box::new)?;
                    let records = self.event_store.append(
                        self.conversation_id,
                        vec![self.complete_event(ConversationEventContent::ModelResponse(
                            response.clone(),
                        ))],
                    )?;
                    report_progress(ConversationSessionProgress::EventCompleted {
                        event: records[0].event().clone(),
                    })?;
                    response
                }
            };

            let succeeded = matches!(terminal_response.outcome(), ModelOutcome::Succeeded);
            if !succeeded || tool_requests.is_empty() {
                let outcome = if succeeded {
                    TurnOutcome::Succeeded
                } else {
                    TurnOutcome::Failed {
                        failure: match terminal_response.outcome() {
                            ModelOutcome::Failed { failure } => failure.clone(),
                            ModelOutcome::Succeeded => {
                                unreachable!("a failed outcome is matched above")
                            }
                        },
                    }
                };
                self.event_store.append(
                    self.conversation_id,
                    vec![self.complete_event(ConversationEventContent::TurnEnd(
                        TurnEnd::new(turn_id, outcome.clone()).map_err(Box::new)?,
                    ))],
                )?;
                return Ok(outcome);
            }

            for (tool_request_id, request) in &tool_requests {
                let outcome = self.tool_registry.execute(request).await;
                self.event_store.append(
                    self.conversation_id,
                    vec![self.complete_event(ConversationEventContent::ToolResponse(
                        ToolResponse::new(*tool_request_id, outcome),
                    ))],
                )?;
            }
            depends_on = tool_requests.iter().map(|(id, _)| *id).collect();
            completed_tool_rounds += 1;
            if completed_tool_rounds >= MAXIMUM_TOOL_CONTINUATION_ROUNDS {
                let failure = OperationFailure::try_new(
                    FailureCategory::ExecutionFailed,
                    format!(
                        "the tool continuation limit of {MAXIMUM_TOOL_CONTINUATION_ROUNDS} rounds was reached"
                    ),
                    None,
                )
                .map_err(Box::new)?;
                let outcome = TurnOutcome::Failed { failure };
                self.event_store.append(
                    self.conversation_id,
                    vec![self.complete_event(ConversationEventContent::TurnEnd(
                        TurnEnd::new(turn_id, outcome.clone()).map_err(Box::new)?,
                    ))],
                )?;
                return Err(Box::new(
                    ConversationSessionError::ToolContinuationLimitReached {
                        limit: MAXIMUM_TOOL_CONTINUATION_ROUNDS,
                    },
                ));
            }
        }
    }

    fn load(&self) -> ConversationSessionResult<ConversationHistory> {
        Ok(ConversationHistory::from_events(
            self.event_store.load(self.conversation_id)?,
        )?)
    }

    fn complete_event(&self, content: ConversationEventContent) -> ConversationEvent {
        ConversationEvent::new(self.conversation_id, content)
    }

    fn ensure_request_reference<T: crate::conversation::ModelEvent>(
        &self,
        model_event: &T,
        model_request_id: ConversationEventId,
    ) -> Result<(), Box<dyn Error>> {
        let found = model_event.model_request_id();
        if found != model_request_id {
            return Err(Box::new(
                ModelDriverError::UnexpectedModelRequestReference {
                    expected: model_request_id,
                    found,
                },
            ));
        }
        Ok(())
    }
}

fn pending_user_id(conversation: &ConversationHistory) -> Option<ConversationEventId> {
    let mut referenced_user_ids = HashSet::new();
    for event in conversation.events() {
        if let ConversationEventContent::TurnStart(turn_start) = event.event.content()
            && let Some(user_id) = turn_start.user_id()
        {
            referenced_user_ids.insert(user_id);
        }
    }
    conversation
        .events()
        .iter()
        .find_map(|event| match event.event.content() {
            ConversationEventContent::User(_)
                if !referenced_user_ids.contains(&event.event.id()) =>
            {
                Some(event.event.id())
            }
            _ => None,
        })
}

fn last_position(conversation: &ConversationHistory) -> u64 {
    conversation
        .events()
        .last()
        .map(|event| event.position)
        .expect("a loaded conversation contains at least one event")
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum ConversationSessionError {
    ToolContinuationLimitReached { limit: u32 },
}

impl Display for ConversationSessionError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ToolContinuationLimitReached { limit } => write!(
                formatter,
                "the tool continuation limit of {limit} rounds was reached"
            ),
        }
    }
}

impl Error for ConversationSessionError {}
#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::str::FromStr;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use futures_util::future::BoxFuture;
    use futures_util::stream;
    use futures_util::{FutureExt, StreamExt};
    use schemars::json_schema;
    use serde_json::{Value, json};

    use super::{ConversationSession, MAXIMUM_TOOL_CONTINUATION_ROUNDS};
    use crate::conversation::{
        AssistantResponse, ConversationEventContent, ConversationEventId, FailureCategory, ModelId,
        ModelOutcome, ModelResponse, ModelSource, ModelSpecificEvent, OperationFailure, ProviderId,
        ToolOutcome, ToolRequest, TurnOutcome, UserContent,
    };
    use crate::conversation_event_store::{
        ConversationEventRecord, ConversationEventStore, FileEventStore,
    };
    use crate::model_driver::{
        ModelDriver, ModelDriverError, ModelDriverOutput, ModelDriverOutputBatch,
        ModelOutputStream, TurnInput,
    };
    use crate::tools::{ExecutableTool, ShellTool, ToolRegistry};
    use crate::toolset::{ToolAvailability, ToolDefinition, ToolName};

    fn source() -> ModelSource {
        ModelSource::new(
            ProviderId::from_str("test").expect("the provider should be valid"),
            ModelId::from_str("test-model").expect("the model should be valid"),
        )
    }

    fn temporary_directory() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("tog-session-test-{}", uuid::Uuid::now_v7()))
    }

    fn user_content(text: &str) -> Vec<UserContent> {
        vec![UserContent::Text(text.to_owned())]
    }

    fn assistant_response() -> ModelDriverOutput {
        ModelDriverOutput::AssistantResponse(
            AssistantResponse::new(ConversationEventId::new(), "Hello.".to_owned())
                .expect("the assistant response should be valid"),
        )
    }

    fn terminated_response() -> ModelDriverOutput {
        ModelDriverOutput::ModelResponse(
            ModelResponse::new(
                ConversationEventId::new(),
                Vec::new(),
                ModelOutcome::Succeeded,
                None,
            )
            .expect("the terminal response should be valid"),
        )
    }

    fn failed_response(failure: FailureCategory) -> ModelDriverOutput {
        ModelDriverOutput::ModelResponse(
            ModelResponse::new(
                ConversationEventId::new(),
                Vec::new(),
                ModelOutcome::Failed {
                    failure: OperationFailure::try_new(
                        failure,
                        "the provider failed".to_owned(),
                        None,
                    )
                    .expect("the failure should be valid"),
                },
                None,
            )
            .expect("the failed terminal response should be valid"),
        )
    }

    fn reasoning_event() -> ModelDriverOutput {
        ModelDriverOutput::ModelSpecificEvent(
            ModelSpecificEvent::new(
                ConversationEventId::new(),
                "reasoning".to_owned(),
                1,
                json!({}),
                Some("Thinking.".to_owned()),
            )
            .expect("the reasoning event should be valid"),
        )
    }

    fn tool_request(tool_name: &str, arguments: Value) -> ModelDriverOutput {
        ModelDriverOutput::ToolRequest(
            ToolRequest::try_new(
                ConversationEventId::new(),
                ToolName::try_new(tool_name.to_owned()).expect("the tool name should be valid"),
                arguments,
                None,
            )
            .expect("the tool request should be valid"),
        )
    }

    fn bind_outputs(
        outputs: Vec<ModelDriverOutput>,
        model_request_id: ConversationEventId,
    ) -> Vec<ModelDriverOutputBatch> {
        outputs
            .into_iter()
            .map(|output| match output {
                ModelDriverOutput::AssistantResponse(response) => {
                    ModelDriverOutput::AssistantResponse(
                        AssistantResponse::new(model_request_id, response.content().to_owned())
                            .expect("the assistant response should be valid"),
                    )
                }
                ModelDriverOutput::ToolRequest(request) => ModelDriverOutput::ToolRequest(
                    ToolRequest::try_new(
                        model_request_id,
                        request.tool_name().clone(),
                        request.arguments().clone(),
                        request.data().cloned(),
                    )
                    .expect("the tool request should be valid"),
                ),
                ModelDriverOutput::ModelSpecificEvent(event) => {
                    ModelDriverOutput::ModelSpecificEvent(
                        ModelSpecificEvent::new(
                            model_request_id,
                            event.provider_event_type().to_owned(),
                            event.provider_payload_version(),
                            event.payload().clone(),
                            event.message().map(str::to_owned),
                        )
                        .expect("the specific event should be valid"),
                    )
                }
                ModelDriverOutput::ModelResponse(response) => ModelDriverOutput::ModelResponse(
                    ModelResponse::new(
                        model_request_id,
                        Vec::new(),
                        response.outcome().clone(),
                        response.usage().cloned(),
                    )
                    .expect("the terminal response should be valid"),
                ),
            })
            .map(ModelDriverOutputBatch::from)
            .collect()
    }

    #[derive(Default)]
    struct ScriptedInvocation {
        available_tools: Vec<String>,
        tool_responses: Vec<ToolOutcome>,
        events: Vec<ConversationEventRecord>,
    }

    struct ScriptedDriver {
        source: ModelSource,
        invocations: Arc<Mutex<Vec<ScriptedInvocation>>>,
        script: Mutex<VecDeque<Vec<ModelDriverOutput>>>,
    }

    impl ScriptedDriver {
        fn new(script: Vec<Vec<ModelDriverOutput>>) -> Self {
            Self {
                source: source(),
                invocations: Arc::new(Mutex::new(Vec::new())),
                script: Mutex::new(script.into()),
            }
        }
    }

    struct SharedScriptedDriver(Arc<ScriptedDriver>);

    impl ModelDriver for ScriptedDriver {
        fn source(&self) -> &ModelSource {
            &self.source
        }

        fn invoke<'invoke>(
            &'invoke self,
            input: TurnInput<'invoke>,
        ) -> BoxFuture<'invoke, Result<ModelOutputStream, ModelDriverError>> {
            let available_tools = input
                .toolset()
                .map(|toolset| {
                    toolset
                        .entries()
                        .iter()
                        .map(|entry| entry.definition().name().as_str().to_owned())
                        .collect()
                })
                .unwrap_or_default();
            let tool_responses = input
                .events()
                .iter()
                .filter_map(|event| match event.event.content() {
                    ConversationEventContent::ToolResponse(response) => {
                        Some(response.outcome().clone())
                    }
                    _ => None,
                })
                .collect();
            let events = input.events().to_vec();
            self.invocations
                .lock()
                .expect("the invocation list should lock")
                .push(ScriptedInvocation {
                    available_tools,
                    tool_responses,
                    events,
                });
            let model_request_id = input.model_request_id();
            let scripted_outputs = self
                .script
                .lock()
                .expect("the script should lock")
                .pop_front()
                .unwrap_or_default();
            let batches = bind_outputs(scripted_outputs, model_request_id);
            async move { Ok(stream::iter(batches.into_iter().map(Ok)).boxed()) }.boxed()
        }
    }

    impl ModelDriver for SharedScriptedDriver {
        fn source(&self) -> &ModelSource {
            self.0.source()
        }

        fn invoke<'invoke>(
            &'invoke self,
            input: TurnInput<'invoke>,
        ) -> BoxFuture<'invoke, Result<ModelOutputStream, ModelDriverError>> {
            self.0.invoke(input)
        }
    }

    fn shell_registry() -> ToolRegistry {
        let mut registry = ToolRegistry::default();
        registry.register(ShellTool::new());
        registry
    }

    struct ObservingTool {
        definition: ToolDefinition,
        active: Arc<AtomicUsize>,
        maximum_active: Arc<AtomicUsize>,
        executions: Arc<Mutex<Vec<Value>>>,
    }

    impl ObservingTool {
        fn new() -> Self {
            Self {
                definition: ToolDefinition::try_new(
                    ToolName::try_new("observe".to_owned()).expect("the tool name should be valid"),
                    "Observe execution order and overlap.".to_owned(),
                    json_schema!({ "type": "object" }),
                    json_schema!({ "type": "object" }),
                )
                .expect("the tool definition should be valid"),
                active: Arc::new(AtomicUsize::new(0)),
                maximum_active: Arc::new(AtomicUsize::new(0)),
                executions: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    impl ExecutableTool for ObservingTool {
        fn definition(&self) -> &ToolDefinition {
            &self.definition
        }

        fn execute<'execute>(
            &'execute self,
            arguments: Value,
        ) -> BoxFuture<'execute, Result<Value, OperationFailure>> {
            async move {
                let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
                self.maximum_active.fetch_max(active, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(10)).await;
                self.executions
                    .lock()
                    .expect("the execution list should lock")
                    .push(arguments.clone());
                self.active.fetch_sub(1, Ordering::SeqCst);
                Ok(json!({ "observed": arguments }))
            }
            .boxed()
        }
    }

    fn loaded_events(
        directory: &std::path::Path,
        conversation_id: crate::conversation::ConversationId,
    ) -> Vec<ConversationEventRecord> {
        FileEventStore::new(directory.to_path_buf())
            .expect("the store should reopen")
            .load(conversation_id)
            .expect("the conversation should load")
    }

    fn event_type(event: &ConversationEventContent) -> &'static str {
        match event {
            ConversationEventContent::User(_) => "user",
            ConversationEventContent::TurnStart(_) => "turn_start",
            ConversationEventContent::TurnEnd(_) => "turn_end",
            ConversationEventContent::AssistantResponse(_) => "assistant_response",
            ConversationEventContent::ToolRequest(_) => "tool_request",
            ConversationEventContent::ToolResponse(_) => "tool_response",
            ConversationEventContent::ModelRequest(_) => "model_request",
            ConversationEventContent::ModelResponse(_) => "model_response",
            ConversationEventContent::ModelSpecificEvent(_) => "model_specific_event",
            ConversationEventContent::Automation(_) => "automation",
            ConversationEventContent::Context(_) => "context",
            ConversationEventContent::Toolset(_) => "toolset",
        }
    }

    #[tokio::test]
    async fn a_successful_turn_records_turn_lifecycle() {
        let directory = temporary_directory();
        let driver = Arc::new(ScriptedDriver::new(vec![vec![
            reasoning_event(),
            assistant_response(),
            terminated_response(),
        ]]));
        let session = ConversationSession::create(
            FileEventStore::new(directory.clone()).expect("the store should be created"),
            Box::new(SharedScriptedDriver(Arc::clone(&driver))),
            ToolRegistry::default(),
        );
        let conversation_id = session.id();
        session
            .add_user_request(user_content("hello"))
            .expect("the request should be recorded");

        assert_eq!(
            session
                .invoke(|_| Ok(()))
                .await
                .expect("the invocation should complete"),
            TurnOutcome::Succeeded
        );

        assert_eq!(
            driver
                .invocations
                .lock()
                .expect("the invocation list should lock")
                .len(),
            1
        );
        let event_types = loaded_events(&directory, conversation_id)
            .iter()
            .map(|event| event_type(event.event.content()))
            .collect::<Vec<_>>();
        assert!(matches!(
            event_types.as_slice(),
            [
                "user",
                "turn_start",
                "model_request",
                "model_specific_event",
                "assistant_response",
                "model_response",
                "turn_end"
            ]
        ));
    }

    #[tokio::test]
    async fn the_session_exposes_registered_tools_as_immediate() {
        let directory = temporary_directory();
        let driver = Arc::new(ScriptedDriver::new(vec![vec![
            assistant_response(),
            terminated_response(),
        ]]));
        let session = ConversationSession::create(
            FileEventStore::new(directory.clone()).expect("the store should be created"),
            Box::new(SharedScriptedDriver(Arc::clone(&driver))),
            shell_registry(),
        );
        let conversation_id = session.id();
        session
            .add_user_request(user_content("hello"))
            .expect("the request should be recorded");
        session
            .invoke(|_| Ok(()))
            .await
            .expect("the invocation should complete");

        let events = loaded_events(&directory, conversation_id);
        let toolset = events
            .iter()
            .find_map(|event| match event.event.content() {
                ConversationEventContent::Toolset(declared) => Some(declared.toolset()),
                _ => None,
            })
            .expect("the toolset should be persisted");
        assert_eq!(toolset.entries().len(), 1);
        assert!(
            toolset
                .entries()
                .iter()
                .all(|entry| entry.availability() == ToolAvailability::Immediate)
        );
        assert_eq!(toolset.entries()[0].definition().name().as_str(), "shell");
    }

    #[tokio::test]
    async fn a_failed_terminal_outcome_fails_the_turn() {
        let session = ConversationSession::create(
            FileEventStore::new(temporary_directory()).expect("the store should be created"),
            Box::new(ScriptedDriver::new(vec![vec![failed_response(
                FailureCategory::ProviderFailure,
            )]])),
            ToolRegistry::default(),
        );
        session
            .add_user_request(user_content("hello"))
            .expect("the request should be recorded");

        assert!(matches!(
            session
                .invoke(|_| Ok(()))
                .await
                .expect("the failed invocation should still complete"),
            TurnOutcome::Failed { .. }
        ));
    }

    #[tokio::test]
    async fn a_stream_that_ends_without_a_terminal_response_fails_the_turn_explicitly() {
        let directory = temporary_directory();
        let session = ConversationSession::create(
            FileEventStore::new(directory.clone()).expect("the store should be created"),
            Box::new(ScriptedDriver::new(vec![vec![assistant_response()]])),
            ToolRegistry::default(),
        );
        let conversation_id = session.id();
        session
            .add_user_request(user_content("hello"))
            .expect("the request should be recorded");

        assert!(matches!(
            session
                .invoke(|_| Ok(()))
                .await
                .expect("the incomplete stream should fail the turn"),
            TurnOutcome::Failed { .. }
        ));

        let events = loaded_events(&directory, conversation_id);
        let model_responses = events
            .iter()
            .filter(|event| {
                matches!(
                    event.event.content(),
                    ConversationEventContent::ModelResponse(_)
                )
            })
            .count();
        assert_eq!(
            model_responses, 1,
            "the engine records one terminal response"
        );
        assert!(matches!(
            events.last().map(|event| event.event.content()),
            Some(ConversationEventContent::TurnEnd(turn_end))
                if matches!(turn_end.outcome(), TurnOutcome::Failed { .. })
        ));
    }

    #[tokio::test]
    async fn a_terminal_response_followed_by_more_output_is_a_contract_error() {
        let driver = Arc::new(ScriptedDriver::new(vec![vec![
            terminated_response(),
            assistant_response(),
        ]]));
        let session = ConversationSession::create(
            FileEventStore::new(temporary_directory()).expect("the store should be created"),
            Box::new(SharedScriptedDriver(Arc::clone(&driver))),
            ToolRegistry::default(),
        );
        session
            .add_user_request(user_content("hello"))
            .expect("the request should be recorded");

        let error = session
            .invoke(|_| Ok(()))
            .await
            .expect_err("output after the terminal response should be rejected");
        assert!(error.to_string().contains("after the terminal"));
    }

    #[tokio::test]
    async fn a_shell_request_executes_and_its_response_reaches_the_next_invocation() {
        let directory = temporary_directory();
        let driver = Arc::new(ScriptedDriver::new(vec![
            vec![
                tool_request("shell", json!({ "command": "printf hello" })),
                terminated_response(),
            ],
            vec![assistant_response(), terminated_response()],
        ]));
        let session = ConversationSession::create(
            FileEventStore::new(directory.clone()).expect("the store should be created"),
            Box::new(SharedScriptedDriver(Arc::clone(&driver))),
            shell_registry(),
        );
        let conversation_id = session.id();
        session
            .add_user_request(user_content("run it"))
            .expect("the request should be recorded");

        assert_eq!(
            session
                .invoke(|_| Ok(()))
                .await
                .expect("the tool turn should complete"),
            TurnOutcome::Succeeded
        );

        let invocations = driver
            .invocations
            .lock()
            .expect("the invocation list should lock");
        assert_eq!(invocations.len(), 2);
        assert_eq!(invocations[1].available_tools, ["shell"]);
        assert!(invocations[1].tool_responses.iter().any(|outcome| matches!(
            outcome,
            ToolOutcome::Succeeded { value } if value["stdout"] == "hello" && value["exit_status"]["code"] == 0
        )));
        drop(invocations);
        let events = loaded_events(&directory, conversation_id);
        assert!(events.iter().any(|event| matches!(
            event.event.content(),
            ConversationEventContent::ToolResponse(response)
                if matches!(response.outcome(), ToolOutcome::Succeeded { value } if value["stdout"] == "hello")
        )));
    }

    #[tokio::test]
    async fn multiple_requests_execute_sequentially_in_request_order() {
        let directory = temporary_directory();
        let observing_tool = ObservingTool::new();
        let maximum_active = Arc::clone(&observing_tool.maximum_active);
        let executions = Arc::clone(&observing_tool.executions);
        let mut registry = ToolRegistry::default();
        registry.register(observing_tool);
        let driver = Arc::new(ScriptedDriver::new(vec![
            vec![
                tool_request("observe", json!({ "order": "first" })),
                tool_request("observe", json!({ "order": "second" })),
                terminated_response(),
            ],
            vec![assistant_response(), terminated_response()],
        ]));
        let session = ConversationSession::create(
            FileEventStore::new(directory.clone()).expect("the store should be created"),
            Box::new(SharedScriptedDriver(Arc::clone(&driver))),
            registry,
        );
        session
            .add_user_request(user_content("run both"))
            .expect("the request should be recorded");

        assert_eq!(
            session
                .invoke(|_| Ok(()))
                .await
                .expect("the tool turn should complete"),
            TurnOutcome::Succeeded
        );

        assert_eq!(
            maximum_active.load(Ordering::SeqCst),
            1,
            "tool executions must not overlap"
        );
        assert_eq!(
            *executions.lock().expect("the execution list should lock"),
            [json!({ "order": "first" }), json!({ "order": "second" })]
        );
        let invocations = driver
            .invocations
            .lock()
            .expect("the invocation list should lock");
        assert_eq!(invocations.len(), 2);
        assert_eq!(invocations[1].tool_responses.len(), 2);
    }

    #[tokio::test]
    async fn assistant_text_with_tool_requests_does_not_complete_the_turn() {
        let directory = temporary_directory();
        let mut registry = ToolRegistry::default();
        registry.register(ObservingTool::new());
        let driver = Arc::new(ScriptedDriver::new(vec![
            vec![
                assistant_response(),
                tool_request("observe", json!({ "order": "first" })),
                terminated_response(),
            ],
            vec![assistant_response(), terminated_response()],
        ]));
        let session = ConversationSession::create(
            FileEventStore::new(directory.clone()).expect("the store should be created"),
            Box::new(SharedScriptedDriver(Arc::clone(&driver))),
            registry,
        );
        let conversation_id = session.id();
        session
            .add_user_request(user_content("keep going"))
            .expect("the request should be recorded");

        assert_eq!(
            session
                .invoke(|_| Ok(()))
                .await
                .expect("the tool turn should complete"),
            TurnOutcome::Succeeded
        );

        assert_eq!(
            driver
                .invocations
                .lock()
                .expect("the invocation list should lock")
                .len(),
            2,
            "the driver must be invoked again after the tool response"
        );
        let events = loaded_events(&directory, conversation_id);
        assert_eq!(
            events
                .iter()
                .filter(|event| {
                    matches!(event.event.content(), ConversationEventContent::TurnEnd(_))
                })
                .count(),
            1,
            "the turn must complete exactly once"
        );
    }

    #[tokio::test]
    async fn a_tool_execution_problem_is_returned_to_the_model_without_failing_the_turn() {
        let directory = temporary_directory();
        let driver = Arc::new(ScriptedDriver::new(vec![
            vec![
                tool_request("missing", json!({ "command": "pwd" })),
                terminated_response(),
            ],
            vec![assistant_response(), terminated_response()],
        ]));
        let session = ConversationSession::create(
            FileEventStore::new(directory.clone()).expect("the store should be created"),
            Box::new(SharedScriptedDriver(Arc::clone(&driver))),
            ToolRegistry::default(),
        );
        let conversation_id = session.id();
        session
            .add_user_request(user_content("try it"))
            .expect("the request should be recorded");

        assert_eq!(
            session
                .invoke(|_| Ok(()))
                .await
                .expect("the tool problem should not fail the turn"),
            TurnOutcome::Succeeded
        );

        let invocations = driver
            .invocations
            .lock()
            .expect("the invocation list should lock");
        assert!(invocations[1].tool_responses.iter().any(|outcome| matches!(
            outcome,
            ToolOutcome::Failed { failure }
                if failure.category() == FailureCategory::UnknownTool
                    && failure.message() == "unknown tool: missing"
                    && failure.details().is_none()
        )));
        drop(invocations);
        let events = loaded_events(&directory, conversation_id);
        let tool_request = events
            .iter()
            .find_map(|event| match event.event.content() {
                ConversationEventContent::ToolRequest(request) => {
                    Some((event.event.id(), request.clone()))
                }
                _ => None,
            })
            .expect("the tool request should be persisted");
        let tool_response = events
            .iter()
            .find_map(|event| match event.event.content() {
                ConversationEventContent::ToolResponse(response) => Some(response.clone()),
                _ => None,
            })
            .expect("the tool response should be persisted");
        assert_eq!(
            tool_response.tool_request_id(),
            tool_request.0,
            "the problem response must stay correlated to its request"
        );
    }

    #[tokio::test]
    async fn reaching_the_tool_continuation_limit_is_reported_explicitly() {
        let directory = temporary_directory();
        let script = (0..MAXIMUM_TOOL_CONTINUATION_ROUNDS)
            .map(|round| {
                vec![
                    tool_request("missing", json!({ "round": round })),
                    terminated_response(),
                ]
            })
            .collect();
        let session = ConversationSession::create(
            FileEventStore::new(directory.clone()).expect("the store should be created"),
            Box::new(ScriptedDriver::new(script)),
            ToolRegistry::default(),
        );
        let conversation_id = session.id();
        session
            .add_user_request(user_content("loop"))
            .expect("the request should be recorded");

        let error = session
            .invoke(|_| Ok(()))
            .await
            .expect_err("the continuation limit should be reported");
        assert_eq!(
            error.to_string(),
            format!(
                "the tool continuation limit of {MAXIMUM_TOOL_CONTINUATION_ROUNDS} rounds was reached"
            )
        );
        let events = loaded_events(&directory, conversation_id);
        assert!(matches!(
            events.last().map(|event| event.event.content()),
            Some(ConversationEventContent::TurnEnd(turn_end))
                if matches!(turn_end.outcome(), TurnOutcome::Failed { .. })
        ));
    }

    #[tokio::test]
    async fn reopening_a_conversation_continues_without_new_input() {
        let directory = temporary_directory();
        let first_driver = Arc::new(ScriptedDriver::new(vec![vec![
            assistant_response(),
            terminated_response(),
        ]]));
        let session = ConversationSession::create(
            FileEventStore::new(directory.clone()).expect("the store should be created"),
            Box::new(SharedScriptedDriver(Arc::clone(&first_driver))),
            ToolRegistry::default(),
        );
        let conversation_id = session.id();
        session
            .add_user_request(user_content("hello"))
            .expect("the request should be recorded");
        assert_eq!(
            session
                .invoke(|_| Ok(()))
                .await
                .expect("the first invocation should complete"),
            TurnOutcome::Succeeded
        );

        let second_driver = Arc::new(ScriptedDriver::new(vec![vec![
            assistant_response(),
            terminated_response(),
        ]]));
        let reopened = ConversationSession::open(
            conversation_id,
            FileEventStore::new(directory.clone()).expect("the store should reopen"),
            Box::new(SharedScriptedDriver(Arc::clone(&second_driver))),
            ToolRegistry::default(),
        )
        .expect("the session should open");
        assert_eq!(reopened.id(), conversation_id);
        assert_eq!(
            reopened
                .invoke(|_| Ok(()))
                .await
                .expect("an invocation without new input should complete"),
            TurnOutcome::Succeeded
        );
        assert_eq!(
            second_driver
                .invocations
                .lock()
                .expect("the invocation list should lock")[0]
                .events
                .iter()
                .filter(|event| {
                    matches!(event.event.content(), ConversationEventContent::User(_))
                })
                .count(),
            1,
            "the reopened session replays the committed user event"
        );
        let recorded = loaded_events(&directory.clone(), conversation_id);
        let first_invocation = first_driver
            .invocations
            .lock()
            .expect("the invocation list should lock");
        let first_user = first_invocation[0]
            .events
            .iter()
            .find(|event| matches!(event.event.content(), ConversationEventContent::User(_)))
            .expect("the first session invoked with the user event");
        let second_invocation = second_driver
            .invocations
            .lock()
            .expect("the invocation list should lock");
        let replayed_user = second_invocation[0]
            .events
            .iter()
            .find(|event| matches!(event.event.content(), ConversationEventContent::User(_)))
            .expect("the reopened session replayed the user event");
        assert_eq!(
            replayed_user.event.id(),
            first_user.event.id(),
            "the replayed event keeps its original identity"
        );
        assert!(
            recorded
                .iter()
                .any(|event| event.event.id() == first_user.event.id())
        );
    }

    #[tokio::test]
    async fn the_second_invocation_depends_on_the_tool_requests_it_wait_for() {
        let directory = temporary_directory();
        let mut registry = ToolRegistry::default();
        registry.register(ObservingTool::new());
        let driver = Arc::new(ScriptedDriver::new(vec![
            vec![
                tool_request("observe", json!({ "order": "first" })),
                terminated_response(),
            ],
            vec![assistant_response(), terminated_response()],
        ]));
        let session = ConversationSession::create(
            FileEventStore::new(directory.clone()).expect("the store should be created"),
            Box::new(SharedScriptedDriver(Arc::clone(&driver))),
            registry,
        );
        let conversation_id = session.id();
        session
            .add_user_request(user_content("run it"))
            .expect("the request should be recorded");
        session
            .invoke(|_| Ok(()))
            .await
            .expect("the tool turn should complete");

        let events = loaded_events(&directory, conversation_id);
        let tool_request_id = events
            .iter()
            .find_map(|event| match event.event.content() {
                ConversationEventContent::ToolRequest(_) => Some(event.event.id()),
                _ => None,
            })
            .expect("the tool request should be recorded");
        let second_model_request = events
            .iter()
            .filter_map(|event| match event.event.content() {
                ConversationEventContent::ModelRequest(request) => Some(request.clone()),
                _ => None,
            })
            .next_back()
            .expect("the second model request should be recorded");
        assert_eq!(second_model_request.depends_on(), &[tool_request_id]);
    }
}
