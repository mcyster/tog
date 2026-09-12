use std::collections::HashSet;
use std::error::Error;

use futures_util::StreamExt;

use crate::conversation::{
    ConversationCommandId, ConversationEvent, ConversationFact, ConversationId,
    ConversationLifecycle, ConversationMessage, ConversationProblem, ConversationRequest,
    ConversationTurnId, TurnOutcome, UserContent, UserMessageRequest, UserPrompt,
};
use crate::model_driver::{ModelDriver, ModelDriverError, ModelDriverOutput, TurnInput};
use crate::persistence::EventStore;

pub(crate) type ConversationSessionResult<T> = Result<T, Box<dyn Error>>;

pub(crate) enum ConversationSessionProgress {
    InvocationStarted { model: String },
    EventCompleted { event: ConversationFact },
    ProblemCompleted { problem: ConversationProblem },
}

pub(crate) struct ConversationSession {
    conversation_id: ConversationId,
    event_store: EventStore,
    model_driver: Box<dyn ModelDriver>,
}

impl ConversationSession {
    pub(crate) fn create(event_store: EventStore, model_driver: Box<dyn ModelDriver>) -> Self {
        Self {
            conversation_id: ConversationId::new(),
            event_store,
            model_driver,
        }
    }

    pub(crate) fn open(
        conversation_id: ConversationId,
        event_store: EventStore,
        model_driver: Box<dyn ModelDriver>,
    ) -> ConversationSessionResult<Self> {
        event_store.load_conversation(conversation_id)?;
        Ok(Self {
            conversation_id,
            event_store,
            model_driver,
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
        self.event_store.append_new_conversation_event(
            self.conversation_id,
            ConversationEvent::Request(ConversationRequest::UserMessageRequested(
                UserMessageRequest {
                    content: vec![UserContent::Text(user_prompt.text().to_owned())],
                    command_id,
                },
            )),
        )?;
        Ok(command_id)
    }

    pub(crate) async fn invoke(
        &self,
        mut report_progress: impl FnMut(ConversationSessionProgress) -> ConversationSessionResult<()>,
    ) -> ConversationSessionResult<TurnOutcome> {
        let turn_id = ConversationTurnId::new();
        self.event_store.append_new_conversation_event(
            self.conversation_id,
            ConversationEvent::Request(ConversationRequest::TurnRequested {
                command_id: ConversationCommandId::new(),
                turn_id,
            }),
        )?;
        let conversation = self.event_store.load_conversation(self.conversation_id)?;
        let pending_request_ids = conversation
            .pending_user_requests()
            .into_iter()
            .map(|request| request.command_id)
            .collect::<HashSet<_>>();
        let source = self.model_driver.source().clone();
        report_progress(ConversationSessionProgress::InvocationStarted {
            model: source.model().as_str().to_owned(),
        })?;

        let mut output_stream = self
            .model_driver
            .invoke(TurnInput::new(&conversation, turn_id))
            .await?;
        let mut accepted_request_ids = HashSet::new();
        let mut assistant_responded = false;
        let mut turn_failed = false;

        while let Some(output) = output_stream.next().await {
            let output = output?;
            match output {
                ModelDriverOutput::Message(ConversationMessage::User { caused_by, content }) => {
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
                    self.append_shared_fact(ConversationFact::Message {
                        message: ConversationMessage::User {
                            caused_by: Some(command_id),
                            content,
                        },
                        turn_id: None,
                    })?;
                }
                ModelDriverOutput::Message(ConversationMessage::AssistantResponse {
                    invocation_id,
                    data,
                    response,
                }) => {
                    assistant_responded = true;
                    self.report_shared_fact(
                        ConversationFact::Message {
                            message: ConversationMessage::AssistantResponse {
                                invocation_id,
                                data,
                                response,
                            },
                            turn_id: Some(turn_id),
                        },
                        &mut report_progress,
                    )?;
                }
                ModelDriverOutput::Message(ConversationMessage::Communication {
                    invocation_id,
                    data,
                    communication,
                }) => {
                    self.report_shared_fact(
                        ConversationFact::Message {
                            message: ConversationMessage::Communication {
                                invocation_id,
                                data,
                                communication,
                            },
                            turn_id: Some(turn_id),
                        },
                        &mut report_progress,
                    )?;
                }
                ModelDriverOutput::Message(ConversationMessage::Problem {
                    invocation_id,
                    data,
                    problem,
                }) => {
                    turn_failed = true;
                    self.report_shared_fact(
                        ConversationFact::Message {
                            message: ConversationMessage::Problem {
                                invocation_id,
                                data,
                                problem,
                            },
                            turn_id: Some(turn_id),
                        },
                        &mut report_progress,
                    )?;
                }
                ModelDriverOutput::Command(event) | ModelDriverOutput::Extension(event) => {
                    self.event_store.append_new_conversation_event(
                        self.conversation_id,
                        ConversationEvent::Extension(event),
                    )?;
                }
            }
        }

        let outcome = match (turn_failed, assistant_responded) {
            (true, _) => TurnOutcome::Failed,
            (false, true) => TurnOutcome::Succeeded,
            (false, false) => return Err(Box::new(ModelDriverError::IncompleteTurn)),
        };
        self.report_shared_fact(
            ConversationFact::Lifecycle(ConversationLifecycle::TurnCompleted { turn_id, outcome }),
            &mut report_progress,
        )?;
        Ok(outcome)
    }

    fn append_shared_fact(&self, fact: ConversationFact) -> ConversationSessionResult<()> {
        self.event_store
            .append_new_conversation_event(self.conversation_id, ConversationEvent::Fact(fact))?;
        Ok(())
    }

    fn report_shared_fact(
        &self,
        fact: ConversationFact,
        report_progress: &mut impl FnMut(ConversationSessionProgress) -> ConversationSessionResult<()>,
    ) -> ConversationSessionResult<()> {
        match &fact {
            ConversationFact::Message {
                message:
                    ConversationMessage::AssistantResponse { .. }
                    | ConversationMessage::Communication { .. },
                ..
            } => {
                let progress = ConversationSessionProgress::EventCompleted {
                    event: fact.clone(),
                };
                self.append_shared_fact(fact)?;
                report_progress(progress)?;
            }
            ConversationFact::Message {
                message: ConversationMessage::Problem { problem, .. },
                ..
            } => {
                let progress = ConversationSessionProgress::ProblemCompleted {
                    problem: problem.clone(),
                };
                self.append_shared_fact(fact)?;
                report_progress(progress)?;
            }
            ConversationFact::Message {
                message: ConversationMessage::User { .. },
                ..
            }
            | ConversationFact::Lifecycle(_) => {
                self.append_shared_fact(fact)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;
    use std::sync::{Arc, Mutex};

    use futures_util::future::BoxFuture;
    use futures_util::stream;
    use futures_util::{FutureExt, StreamExt};

    use super::{ConversationSession, ConversationSessionProgress};
    use crate::conversation::{
        AssistantResponse, ConversationEventExtension, ConversationEventKind, ConversationFact,
        ConversationLifecycle, ConversationMessage, ConversationProblem, DriverEventEnvelope,
        DriverEventReadError, DriverEventReader, InvocationError, ModelId, ModelInvocationId,
        ModelSource, ProviderId, StoredConversationEventKind, TurnOutcome, UserPrompt,
    };
    use crate::model_driver::{
        ModelDriver, ModelDriverError, ModelDriverOutput, ModelOutputStream, TurnInput,
    };
    use crate::persistence::EventStore;

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

    impl DriverEventReader for RecordingDriver {
        fn read_event(
            &self,
            _envelope: &DriverEventEnvelope,
        ) -> Result<Box<dyn ConversationEventExtension>, DriverEventReadError> {
            Err(DriverEventReadError::UnsupportedDriver)
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
            let mut output = pending_requests
                .into_iter()
                .map(|request| {
                    Ok(ModelDriverOutput::Message(ConversationMessage::User {
                        caused_by: Some(request.command_id),
                        content: request.content,
                    }))
                })
                .collect::<Vec<_>>();
            match self.response {
                RecordingResponse::Assistant => output.push(Ok(assistant_response())),
                RecordingResponse::Problem => output.push(Ok(problem_message())),
                RecordingResponse::AssistantThenProblem => {
                    output.push(Ok(assistant_response()));
                    output.push(Ok(problem_message()));
                }
                RecordingResponse::Nothing => {}
            }
            async move { Ok(stream::iter(output).boxed()) }.boxed()
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
}
