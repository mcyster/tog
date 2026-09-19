use std::collections::HashSet;
use std::error::Error;
use std::fmt::{Display, Formatter};

use futures_util::StreamExt;

use crate::conversation::{
    ConversationCommand, ConversationCommandId, ConversationEvent, ConversationFact,
    ConversationId, ConversationLifecycle, ConversationMessage, ConversationProblem,
    ConversationTurnId, ToolResponse, TurnOutcome, UserContent, UserMessageRequest, UserPrompt,
};
use crate::model_driver::{ModelDriver, ModelDriverError, ModelDriverOutput, TurnInput};
use crate::persistence::EventStore;
use crate::tools::ToolRegistry;

pub(crate) type ConversationSessionResult<T> = Result<T, Box<dyn Error>>;

pub(crate) const MAXIMUM_TOOL_CONTINUATION_ROUNDS: u32 = 8;

pub(crate) enum ConversationSessionProgress {
    InvocationStarted { model: String },
    EventCompleted { event: ConversationFact },
    ProblemCompleted { problem: ConversationProblem },
}

pub(crate) struct ConversationSession {
    conversation_id: ConversationId,
    event_store: EventStore,
    model_driver: Box<dyn ModelDriver>,
    tool_registry: ToolRegistry,
}

impl ConversationSession {
    pub(crate) fn create(
        event_store: EventStore,
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
        event_store: EventStore,
        model_driver: Box<dyn ModelDriver>,
        tool_registry: ToolRegistry,
    ) -> ConversationSessionResult<Self> {
        event_store.load_conversation(conversation_id)?;
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
        user_prompt: UserPrompt,
    ) -> ConversationSessionResult<ConversationCommandId> {
        let command_id = ConversationCommandId::new();
        self.event_store.append_new_conversation_events(
            self.conversation_id,
            vec![ConversationEvent::Command(
                ConversationCommand::UserMessageRequested(UserMessageRequest {
                    content: vec![UserContent::Text(user_prompt.text().to_owned())],
                    command_id,
                }),
            )],
        )?;
        Ok(command_id)
    }

    pub(crate) async fn invoke(
        &self,
        mut report_progress: impl FnMut(ConversationSessionProgress) -> ConversationSessionResult<()>,
    ) -> ConversationSessionResult<TurnOutcome> {
        let turn_id = ConversationTurnId::new();
        self.event_store.append_new_conversation_events(
            self.conversation_id,
            vec![ConversationEvent::Command(
                ConversationCommand::TurnRequested {
                    command_id: ConversationCommandId::new(),
                    turn_id,
                },
            )],
        )?;
        let source = self.model_driver.source().clone();
        let mut assistant_responded = false;
        let mut turn_failed = false;
        let mut completed_tool_rounds = 0_u32;

        loop {
            self.append_shared_fact(ConversationFact::ToolsAvailable {
                tools: self.tool_registry.definitions(),
            })?;
            let conversation = self.event_store.load_conversation(self.conversation_id)?;
            let pending_request_ids = conversation
                .pending_user_requests()
                .into_iter()
                .map(|request| request.command_id)
                .collect::<HashSet<_>>();
            report_progress(ConversationSessionProgress::InvocationStarted {
                model: source.model().as_str().to_owned(),
            })?;

            let mut output_stream = self
                .model_driver
                .invoke(TurnInput::new(&conversation, turn_id))
                .await?;
            let mut accepted_request_ids = HashSet::new();
            let mut tool_requests = Vec::new();

            while let Some(output_batch) = output_stream.next().await {
                let output_batch = output_batch?;
                let mut events = Vec::new();
                let mut progress_reports = Vec::new();
                for output in output_batch.into_outputs() {
                    match output {
                        ModelDriverOutput::Message(ConversationMessage::User {
                            caused_by,
                            content,
                        }) => {
                            let Some(command_id) = caused_by else {
                                return Err(Box::new(ModelDriverError::UnassociatedUserMessage));
                            };
                            if !pending_request_ids.contains(&command_id)
                                || !accepted_request_ids.insert(command_id)
                            {
                                return Err(Box::new(ModelDriverError::UnexpectedUserRequest {
                                    command_id,
                                }));
                            }
                            events.push(ConversationEvent::Fact(ConversationFact::Message {
                                message: ConversationMessage::User {
                                    caused_by: Some(command_id),
                                    content,
                                },
                                turn_id: None,
                            }));
                        }
                        ModelDriverOutput::Message(ConversationMessage::AssistantResponse {
                            invocation_id,
                            data,
                            response,
                        }) => {
                            assistant_responded = true;
                            let fact = ConversationFact::Message {
                                message: ConversationMessage::AssistantResponse {
                                    invocation_id,
                                    data,
                                    response,
                                },
                                turn_id: Some(turn_id),
                            };
                            progress_reports.push(ConversationSessionProgress::EventCompleted {
                                event: fact.clone(),
                            });
                            events.push(ConversationEvent::Fact(fact));
                        }
                        ModelDriverOutput::Message(ConversationMessage::Communication {
                            invocation_id,
                            data,
                            communication,
                        }) => {
                            let fact = ConversationFact::Message {
                                message: ConversationMessage::Communication {
                                    invocation_id,
                                    data,
                                    communication,
                                },
                                turn_id: Some(turn_id),
                            };
                            progress_reports.push(ConversationSessionProgress::EventCompleted {
                                event: fact.clone(),
                            });
                            events.push(ConversationEvent::Fact(fact));
                        }
                        ModelDriverOutput::Message(ConversationMessage::Problem {
                            invocation_id,
                            data,
                            problem,
                        }) => {
                            turn_failed = true;
                            progress_reports.push(ConversationSessionProgress::ProblemCompleted {
                                problem: problem.clone(),
                            });
                            events.push(ConversationEvent::Fact(ConversationFact::Message {
                                message: ConversationMessage::Problem {
                                    invocation_id,
                                    data,
                                    problem,
                                },
                                turn_id: Some(turn_id),
                            }));
                        }
                        ModelDriverOutput::ToolRequest(request) => {
                            events.push(ConversationEvent::Fact(ConversationFact::ToolRequest {
                                request: request.clone(),
                                turn_id: Some(turn_id),
                            }));
                            tool_requests.push(request);
                        }
                        ModelDriverOutput::Command(event) | ModelDriverOutput::Extension(event) => {
                            events.push(ConversationEvent::Extension(event));
                        }
                    }
                }
                self.event_store
                    .append_new_conversation_events(self.conversation_id, events)?;
                for progress in progress_reports {
                    report_progress(progress)?;
                }
            }

            if turn_failed || tool_requests.is_empty() {
                break;
            }

            for request in &tool_requests {
                let outcome = self.tool_registry.execute(request).await;
                self.append_shared_fact(ConversationFact::ToolResponse {
                    response: ToolResponse::new(request.call_id(), outcome),
                    turn_id: Some(turn_id),
                })?;
            }
            assistant_responded = false;
            completed_tool_rounds += 1;
            if completed_tool_rounds >= MAXIMUM_TOOL_CONTINUATION_ROUNDS {
                self.append_shared_fact(ConversationFact::Lifecycle(
                    ConversationLifecycle::TurnCompleted {
                        turn_id,
                        outcome: TurnOutcome::Failed,
                    },
                ))?;
                return Err(Box::new(
                    ConversationSessionError::ToolContinuationLimitReached {
                        limit: MAXIMUM_TOOL_CONTINUATION_ROUNDS,
                    },
                ));
            }
        }

        let outcome = match (turn_failed, assistant_responded) {
            (true, _) => TurnOutcome::Failed,
            (false, true) => TurnOutcome::Succeeded,
            (false, false) => return Err(Box::new(ModelDriverError::IncompleteTurn)),
        };
        self.append_shared_fact(ConversationFact::Lifecycle(
            ConversationLifecycle::TurnCompleted { turn_id, outcome },
        ))?;
        Ok(outcome)
    }

    fn append_shared_fact(&self, fact: ConversationFact) -> ConversationSessionResult<()> {
        self.event_store.append_new_conversation_events(
            self.conversation_id,
            vec![ConversationEvent::Fact(fact)],
        )?;
        Ok(())
    }
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
    use std::path::Path;
    use std::str::FromStr;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use futures_util::future::BoxFuture;
    use futures_util::stream;
    use futures_util::{FutureExt, StreamExt};
    use schemars::json_schema;
    use serde_json::{Value, json};

    use super::{
        ConversationSession, ConversationSessionProgress, MAXIMUM_TOOL_CONTINUATION_ROUNDS,
    };
    use crate::conversation::{
        AssistantResponse, ConversationEventEnvelope, ConversationEventExtension,
        ConversationEventKind, ConversationEventReadError, ConversationEventReader,
        ConversationFact, ConversationId, ConversationLifecycle, ConversationMessage,
        ConversationProblem, InvocationError, ModelId, ModelInvocationId, ModelSource, ProviderId,
        StoredConversationEventKind, ToolCallId, ToolDefinition, ToolExecutionProblem,
        ToolExecutionProblemKind, ToolName, ToolOutcome, ToolRequest, TurnOutcome, UserPrompt,
    };
    use crate::model_driver::{
        ModelDriver, ModelDriverError, ModelDriverOutput, ModelDriverOutputBatch,
        ModelOutputStream, TurnInput,
    };
    use crate::persistence::EventStore;
    use crate::tools::{ExecutableTool, ShellTool, ToolRegistry};

    enum RecordingResponse {
        Assistant,
        Problem,
        AssistantThenProblem,
        Nothing,
    }

    struct RecordingDriver {
        source: ModelSource,
        pending_counts: Arc<Mutex<Vec<usize>>>,
        response: RecordingResponse,
    }

    impl ConversationEventReader for RecordingDriver {
        fn read_event(
            &self,
            _envelope: &ConversationEventEnvelope,
        ) -> Result<Box<dyn ConversationEventExtension>, ConversationEventReadError> {
            Err(ConversationEventReadError::UnsupportedNamespace)
        }
    }

    impl ModelDriver for RecordingDriver {
        fn source(&self) -> &ModelSource {
            &self.source
        }

        fn invoke<'invoke>(
            &'invoke self,
            input: TurnInput<'invoke>,
        ) -> BoxFuture<'invoke, Result<ModelOutputStream, ModelDriverError>> {
            let pending_requests = input.pending_user_requests().to_vec();
            self.pending_counts
                .lock()
                .expect("the pending request list should lock")
                .push(pending_requests.len());
            let mut batches = Vec::new();
            let pending_user_events = pending_requests
                .into_iter()
                .map(|request| {
                    ModelDriverOutput::Message(ConversationMessage::User {
                        caused_by: Some(request.command_id),
                        content: request.content,
                    })
                })
                .collect::<Vec<_>>();
            if !pending_user_events.is_empty() {
                batches.push(
                    ModelDriverOutputBatch::try_new(pending_user_events)
                        .expect("the pending user events should form a batch"),
                );
            }
            match self.response {
                RecordingResponse::Assistant => {
                    batches.push(ModelDriverOutputBatch::from(assistant_response()));
                }
                RecordingResponse::Problem => {
                    batches.push(ModelDriverOutputBatch::from(problem_message()));
                }
                RecordingResponse::AssistantThenProblem => batches.push(
                    ModelDriverOutputBatch::try_new(vec![assistant_response(), problem_message()])
                        .expect("the response and problem should form a batch"),
                ),
                RecordingResponse::Nothing => {}
            }
            async move { Ok(stream::iter(batches.into_iter().map(Ok)).boxed()) }.boxed()
        }
    }

    fn assistant_response() -> ModelDriverOutput {
        ModelDriverOutput::Message(ConversationMessage::AssistantResponse {
            invocation_id: ModelInvocationId::new(),
            data: None,
            response: AssistantResponse::new("Hello.".to_owned())
                .expect("the assistant response should be valid"),
        })
    }

    fn problem_message() -> ModelDriverOutput {
        ModelDriverOutput::Message(ConversationMessage::Problem {
            invocation_id: Some(ModelInvocationId::new()),
            data: None,
            problem: ConversationProblem::Invocation(
                InvocationError::try_provider_failure("the provider failed".to_owned())
                    .expect("the provider failure should be valid"),
            ),
        })
    }

    fn source() -> ModelSource {
        ModelSource::new(
            ProviderId::from_str("test").expect("the provider should be valid"),
            ModelId::from_str("test-model").expect("the model should be valid"),
        )
    }

    fn temporary_directory() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("tog-session-test-{}", uuid::Uuid::now_v7()))
    }

    struct ScriptedInvocation {
        available_tools: Vec<String>,
        tool_responses: Vec<ToolOutcome>,
    }

    struct ScriptedDriver {
        source: ModelSource,
        invocations: Arc<Mutex<Vec<ScriptedInvocation>>>,
        script: Mutex<std::collections::VecDeque<Vec<ModelDriverOutput>>>,
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

    impl ConversationEventReader for ScriptedDriver {
        fn read_event(
            &self,
            _envelope: &ConversationEventEnvelope,
        ) -> Result<Box<dyn ConversationEventExtension>, ConversationEventReadError> {
            Err(ConversationEventReadError::UnsupportedNamespace)
        }
    }

    impl ModelDriver for ScriptedDriver {
        fn source(&self) -> &ModelSource {
            &self.source
        }

        fn invoke<'invoke>(
            &'invoke self,
            input: TurnInput<'invoke>,
        ) -> BoxFuture<'invoke, Result<ModelOutputStream, ModelDriverError>> {
            let conversation = input.conversation();
            let available_tools = conversation
                .available_tools()
                .iter()
                .map(|tool| tool.name().as_str().to_owned())
                .collect();
            let tool_responses = conversation
                .events()
                .iter()
                .filter_map(|event| match &event.kind {
                    StoredConversationEventKind::Shared(ConversationEventKind::Fact(
                        ConversationFact::ToolResponse { response, .. },
                    )) => Some(response.outcome().clone()),
                    _ => None,
                })
                .collect();
            self.invocations
                .lock()
                .expect("the invocation list should lock")
                .push(ScriptedInvocation {
                    available_tools,
                    tool_responses,
                });
            let pending_requests = input.pending_user_requests().to_vec();
            let mut batches = Vec::new();
            let pending_user_events = pending_requests
                .into_iter()
                .map(|request| {
                    ModelDriverOutput::Message(ConversationMessage::User {
                        caused_by: Some(request.command_id),
                        content: request.content,
                    })
                })
                .collect::<Vec<_>>();
            if !pending_user_events.is_empty() {
                batches.push(
                    ModelDriverOutputBatch::try_new(pending_user_events)
                        .expect("the pending user events should form a batch"),
                );
            }
            let scripted_outputs = self
                .script
                .lock()
                .expect("the script should lock")
                .pop_front()
                .unwrap_or_default();
            if !scripted_outputs.is_empty() {
                batches.push(
                    ModelDriverOutputBatch::try_new(scripted_outputs)
                        .expect("the scripted outputs should form a batch"),
                );
            }
            async move { Ok(stream::iter(batches.into_iter().map(Ok)).boxed()) }.boxed()
        }
    }

    struct SharedScriptedDriver(Arc<ScriptedDriver>);

    impl ConversationEventReader for SharedScriptedDriver {
        fn read_event(
            &self,
            envelope: &ConversationEventEnvelope,
        ) -> Result<Box<dyn ConversationEventExtension>, ConversationEventReadError> {
            self.0.read_event(envelope)
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
        ) -> BoxFuture<'execute, Result<Value, ToolExecutionProblem>> {
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

    fn shell_registry() -> ToolRegistry {
        let mut registry = ToolRegistry::default();
        registry.register(ShellTool::new());
        registry
    }

    fn tool_request(tool_name: &str, arguments: Value) -> ModelDriverOutput {
        ModelDriverOutput::ToolRequest(
            ToolRequest::try_new(
                ToolCallId::new(),
                ToolName::try_new(tool_name.to_owned()).expect("the tool name should be valid"),
                arguments,
                ModelInvocationId::new(),
                None,
            )
            .expect("the tool request should be valid"),
        )
    }

    fn loaded_facts(directory: &Path, conversation_id: ConversationId) -> Vec<ConversationFact> {
        EventStore::new(directory.to_path_buf())
            .expect("the store should reopen")
            .load_conversation(conversation_id)
            .expect("the conversation should load")
            .events()
            .iter()
            .filter_map(|event| match &event.kind {
                StoredConversationEventKind::Shared(ConversationEventKind::Fact(fact)) => {
                    Some(fact.clone())
                }
                StoredConversationEventKind::Shared(ConversationEventKind::Command(_))
                | StoredConversationEventKind::Extension(_) => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn opening_a_session_preserves_pending_requests_and_invocation_without_new_input_works() {
        let directory = temporary_directory();
        let pending_counts = Arc::new(Mutex::new(Vec::new()));
        let session = ConversationSession::create(
            EventStore::new(directory.clone()).expect("the store should be created"),
            Box::new(RecordingDriver {
                source: source(),
                pending_counts: Arc::clone(&pending_counts),
                response: RecordingResponse::Assistant,
            }),
            ToolRegistry::default(),
        );
        let conversation_id = session.id();
        session
            .add_user_request(UserPrompt::from_str("hello").expect("the prompt should be valid"))
            .expect("the request should be recorded");
        assert_eq!(
            session
                .invoke(|_| Ok(()))
                .await
                .expect("the first invocation should complete"),
            TurnOutcome::Succeeded
        );

        let reopened = ConversationSession::open(
            conversation_id,
            EventStore::new(directory).expect("the store should reopen"),
            Box::new(RecordingDriver {
                source: source(),
                pending_counts: Arc::clone(&pending_counts),
                response: RecordingResponse::Assistant,
            }),
            ToolRegistry::default(),
        )
        .expect("the session should open");
        assert_eq!(reopened.id(), conversation_id);
        assert_eq!(
            reopened
                .invoke(|progress| {
                    if let ConversationSessionProgress::ProblemCompleted { problem } = progress {
                        let _ = problem;
                    }
                    Ok(())
                })
                .await
                .expect("an invocation without new input should complete"),
            TurnOutcome::Succeeded
        );

        assert_eq!(
            *pending_counts
                .lock()
                .expect("the pending request list should lock"),
            [1, 0]
        );
    }

    #[tokio::test]
    async fn a_failed_turn_outcome_is_returned_to_the_caller() {
        let session = ConversationSession::create(
            EventStore::new(temporary_directory()).expect("the store should be created"),
            Box::new(RecordingDriver {
                source: source(),
                pending_counts: Arc::new(Mutex::new(Vec::new())),
                response: RecordingResponse::Problem,
            }),
            ToolRegistry::default(),
        );
        session
            .add_user_request(UserPrompt::from_str("hello").expect("the prompt should be valid"))
            .expect("the request should be recorded");

        assert_eq!(
            session
                .invoke(|_| Ok(()))
                .await
                .expect("the failed invocation should still complete"),
            TurnOutcome::Failed
        );
    }

    #[tokio::test]
    async fn a_late_problem_fails_the_turn_but_preserves_earlier_output() {
        let directory = temporary_directory();
        let session = ConversationSession::create(
            EventStore::new(directory.clone()).expect("the store should be created"),
            Box::new(RecordingDriver {
                source: source(),
                pending_counts: Arc::new(Mutex::new(Vec::new())),
                response: RecordingResponse::AssistantThenProblem,
            }),
            ToolRegistry::default(),
        );
        let conversation_id = session.id();
        session
            .add_user_request(UserPrompt::from_str("hello").expect("the prompt should be valid"))
            .expect("the request should be recorded");

        assert_eq!(
            session
                .invoke(|_| Ok(()))
                .await
                .expect("the invocation should complete"),
            TurnOutcome::Failed
        );

        let conversation = EventStore::new(directory)
            .expect("the store should reopen")
            .load_conversation(conversation_id)
            .expect("the conversation should load");
        let facts = conversation
            .events()
            .iter()
            .filter_map(|event| match &event.kind {
                StoredConversationEventKind::Shared(ConversationEventKind::Fact(fact)) => {
                    Some(fact.clone())
                }
                StoredConversationEventKind::Shared(ConversationEventKind::Command(_))
                | StoredConversationEventKind::Extension(_) => None,
            })
            .collect::<Vec<_>>();

        assert!(matches!(
            facts.as_slice(),
            [
                ConversationFact::ToolsAvailable { .. },
                ConversationFact::Message {
                    message: ConversationMessage::User { .. },
                    ..
                },
                ConversationFact::Message {
                    message: ConversationMessage::AssistantResponse { .. },
                    ..
                },
                ConversationFact::Message {
                    message: ConversationMessage::Problem { .. },
                    ..
                },
                ConversationFact::Lifecycle(ConversationLifecycle::TurnCompleted {
                    outcome: TurnOutcome::Failed,
                    ..
                })
            ]
        ));
    }

    #[tokio::test]
    async fn a_driver_that_ends_without_output_is_an_incomplete_turn() {
        let session = ConversationSession::create(
            EventStore::new(temporary_directory()).expect("the store should be created"),
            Box::new(RecordingDriver {
                source: source(),
                pending_counts: Arc::new(Mutex::new(Vec::new())),
                response: RecordingResponse::Nothing,
            }),
            ToolRegistry::default(),
        );
        session
            .add_user_request(UserPrompt::from_str("hello").expect("the prompt should be valid"))
            .expect("the request should be recorded");

        let error = session
            .invoke(|_| Ok(()))
            .await
            .expect_err("an invocation without output should be rejected");
        assert_eq!(
            error.to_string(),
            "the model driver ended without an assistant response or problem"
        );
    }

    #[tokio::test]
    async fn tools_available_is_recorded_and_reaches_every_invocation() {
        let directory = temporary_directory();
        let mut registry = ToolRegistry::default();
        registry.register(ObservingTool::new());
        let driver = Arc::new(ScriptedDriver::new(vec![vec![assistant_response()]]));
        let session = ConversationSession::create(
            EventStore::new(directory.clone()).expect("the store should be created"),
            Box::new(SharedScriptedDriver(Arc::clone(&driver))),
            registry,
        );
        let conversation_id = session.id();
        session
            .add_user_request(UserPrompt::from_str("hello").expect("the prompt should be valid"))
            .expect("the request should be recorded");

        assert_eq!(
            session
                .invoke(|_| Ok(()))
                .await
                .expect("the invocation should complete"),
            TurnOutcome::Succeeded
        );

        let invocations = driver
            .invocations
            .lock()
            .expect("the invocation list should lock");
        assert_eq!(invocations.len(), 1);
        assert_eq!(invocations[0].available_tools, ["observe"]);
        let facts = loaded_facts(&directory, conversation_id);
        assert!(matches!(
            facts.as_slice(),
            [
                ConversationFact::ToolsAvailable { tools },
                ConversationFact::Message {
                    message: ConversationMessage::User { .. },
                    ..
                },
                ConversationFact::Message {
                    message: ConversationMessage::AssistantResponse { .. },
                    ..
                },
                ConversationFact::Lifecycle(ConversationLifecycle::TurnCompleted {
                    outcome: TurnOutcome::Succeeded,
                    ..
                })
            ] if tools.len() == 1 && tools[0].name().as_str() == "observe"
        ));
    }

    #[tokio::test]
    async fn a_shell_request_executes_and_its_response_reaches_the_next_invocation() {
        let directory = temporary_directory();
        let driver = Arc::new(ScriptedDriver::new(vec![
            vec![tool_request("shell", json!({ "command": "printf hello" }))],
            vec![assistant_response()],
        ]));
        let session = ConversationSession::create(
            EventStore::new(directory.clone()).expect("the store should be created"),
            Box::new(SharedScriptedDriver(Arc::clone(&driver))),
            shell_registry(),
        );
        let conversation_id = session.id();
        session
            .add_user_request(UserPrompt::from_str("run it").expect("the prompt should be valid"))
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
            ToolOutcome::Result { value } if value["stdout"] == "hello" && value["exit_status"]["code"] == 0
        )));
        let facts = loaded_facts(&directory, conversation_id);
        assert!(facts.iter().any(|fact| matches!(
            fact,
            ConversationFact::ToolResponse {
                response,
                ..
            } if matches!(
                response.outcome(),
                ToolOutcome::Result { value } if value["stdout"] == "hello"
            )
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
            ],
            vec![assistant_response()],
        ]));
        let session = ConversationSession::create(
            EventStore::new(directory.clone()).expect("the store should be created"),
            Box::new(SharedScriptedDriver(Arc::clone(&driver))),
            registry,
        );
        session
            .add_user_request(UserPrompt::from_str("run both").expect("the prompt should be valid"))
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
        let driver = Arc::new(ScriptedDriver::new(vec![
            vec![
                assistant_response(),
                tool_request("observe", json!({ "order": "first" })),
            ],
            vec![assistant_response()],
        ]));
        let mut registry = ToolRegistry::default();
        registry.register(ObservingTool::new());
        let session = ConversationSession::create(
            EventStore::new(directory.clone()).expect("the store should be created"),
            Box::new(SharedScriptedDriver(Arc::clone(&driver))),
            registry,
        );
        let conversation_id = session.id();
        session
            .add_user_request(
                UserPrompt::from_str("keep going").expect("the prompt should be valid"),
            )
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
        let facts = loaded_facts(&directory, conversation_id);
        assert_eq!(
            facts
                .iter()
                .filter(|fact| matches!(
                    fact,
                    ConversationFact::Lifecycle(ConversationLifecycle::TurnCompleted { .. })
                ))
                .count(),
            1,
            "the turn must complete exactly once"
        );
        let assistant_positions = facts
            .iter()
            .enumerate()
            .filter_map(|(position, fact)| {
                matches!(
                    fact,
                    ConversationFact::Message {
                        message: ConversationMessage::AssistantResponse { .. },
                        ..
                    }
                )
                .then_some(position)
            })
            .collect::<Vec<_>>();
        assert_eq!(assistant_positions.len(), 2);
    }

    #[tokio::test]
    async fn a_turn_is_incomplete_when_the_final_invocation_has_no_response() {
        let driver = Arc::new(ScriptedDriver::new(vec![
            vec![
                assistant_response(),
                tool_request("observe", json!({ "order": "first" })),
            ],
            Vec::new(),
        ]));
        let mut registry = ToolRegistry::default();
        registry.register(ObservingTool::new());
        let session = ConversationSession::create(
            EventStore::new(temporary_directory()).expect("the store should be created"),
            Box::new(SharedScriptedDriver(Arc::clone(&driver))),
            registry,
        );
        session
            .add_user_request(
                UserPrompt::from_str("keep going").expect("the prompt should be valid"),
            )
            .expect("the request should be recorded");

        let error = session
            .invoke(|_| Ok(()))
            .await
            .expect_err("the turn should need a response after the last tool round");
        assert_eq!(
            error.to_string(),
            "the model driver ended without an assistant response or problem"
        );
    }

    #[tokio::test]
    async fn a_tool_execution_problem_is_returned_to_the_model_without_failing_the_turn() {
        let directory = temporary_directory();
        let driver = Arc::new(ScriptedDriver::new(vec![
            vec![tool_request("missing", json!({ "command": "pwd" }))],
            vec![assistant_response()],
        ]));
        let session = ConversationSession::create(
            EventStore::new(directory.clone()).expect("the store should be created"),
            Box::new(SharedScriptedDriver(Arc::clone(&driver))),
            ToolRegistry::default(),
        );
        let conversation_id = session.id();
        session
            .add_user_request(UserPrompt::from_str("try it").expect("the prompt should be valid"))
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
            ToolOutcome::Problem { problem }
                if problem.kind() == ToolExecutionProblemKind::UnknownTool
                    && problem.message() == "unknown tool: missing"
                    && problem.details().is_none()
        )));
        drop(invocations);

        let facts = loaded_facts(&directory, conversation_id);
        let request_call_id = facts
            .iter()
            .find_map(|fact| match fact {
                ConversationFact::ToolRequest { request, .. } => Some(request.call_id()),
                _ => None,
            })
            .expect("the tool request should be persisted");
        let response_call_id = facts
            .iter()
            .find_map(|fact| match fact {
                ConversationFact::ToolResponse { response, .. } => Some(response.call_id()),
                _ => None,
            })
            .expect("the tool response should be persisted");
        assert_eq!(
            response_call_id, request_call_id,
            "the problem response must stay correlated to its request"
        );
    }

    #[tokio::test]
    async fn reaching_the_tool_continuation_limit_is_reported_explicitly() {
        let directory = temporary_directory();
        let script = (0..MAXIMUM_TOOL_CONTINUATION_ROUNDS)
            .map(|round| vec![tool_request("missing", json!({ "round": round }))])
            .collect();
        let driver = Arc::new(ScriptedDriver::new(script));
        let session = ConversationSession::create(
            EventStore::new(directory.clone()).expect("the store should be created"),
            Box::new(SharedScriptedDriver(Arc::clone(&driver))),
            ToolRegistry::default(),
        );
        let conversation_id = session.id();
        session
            .add_user_request(UserPrompt::from_str("loop").expect("the prompt should be valid"))
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
        let facts = loaded_facts(&directory, conversation_id);
        assert!(matches!(
            facts.last(),
            Some(ConversationFact::Lifecycle(
                ConversationLifecycle::TurnCompleted {
                    outcome: TurnOutcome::Failed,
                    ..
                }
            ))
        ));
    }
}
