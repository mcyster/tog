use std::collections::HashSet;
use std::error::Error;

use futures_util::StreamExt;

use crate::conversation::{
    ConversationCommandId, ConversationEvent, ConversationFact, ConversationId,
    ConversationProblem, ConversationRequest, ConversationTurnId, DriverConversationEvent,
    DriverConversationFact, TurnOutcome, UserContent, UserMessageRequest, UserPrompt,
};
use crate::model_driver::{ModelDriver, ModelDriverError, TurnInput};
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
        let source = self.model_driver.source().clone();
        report_progress(ConversationSessionProgress::InvocationStarted {
            model: source.model().as_str().to_owned(),
        })?;

        let mut output_stream = self
            .model_driver
            .invoke(TurnInput::new(&conversation, turn_id))
            .await?;
        let mut completed_outcome = None;
        let mut accepted_request_ids = HashSet::new();

        while let Some(output) = output_stream.next().await {
            let output = output?;
            if completed_outcome.is_some() {
                return Err(Box::new(ModelDriverError::OutputAfterCompletion {
                    event_type: driver_event_type(&output),
                }));
            }
            match output {
                DriverConversationEvent::Command(event) => {
                    self.event_store.append_new_conversation_event(
                        self.conversation_id,
                        ConversationEvent::Driver(DriverConversationEvent::Command(event)),
                    )?;
                }
                DriverConversationEvent::Fact(DriverConversationFact::Extension(event)) => {
                    self.event_store.append_new_conversation_event(
                        self.conversation_id,
                        ConversationEvent::Driver(DriverConversationEvent::Fact(
                            DriverConversationFact::Extension(event),
                        )),
                    )?;
                }
                DriverConversationEvent::Fact(DriverConversationFact::Shared(fact)) => {
                    match &fact {
                        ConversationFact::User { caused_by, .. } => {
                            let Some(command_id) = caused_by else {
                                return Err(Box::new(ModelDriverError::MissingTurnIdentity));
                            };
                            if !pending_request_ids(&conversation).contains(command_id)
                                || !accepted_request_ids.insert(*command_id)
                            {
                                return Err(Box::new(ModelDriverError::DisallowedEventKind {
                                    event_type: "user".to_owned(),
                                }));
                            }
                            self.append_shared_fact(fact)?;
                        }
                        ConversationFact::Assistant {
                            turn_id: fact_turn_id,
                            ..
                        }
                        | ConversationFact::Communication {
                            turn_id: fact_turn_id,
                            ..
                        } => {
                            ensure_turn_id(*fact_turn_id, &turn_id)?;
                            self.report_shared_fact(fact, &mut report_progress)?;
                        }
                        ConversationFact::TurnCompleted {
                            turn_id: fact_turn_id,
                            outcome,
                        } => {
                            ensure_turn_id(*fact_turn_id, &turn_id)?;
                            completed_outcome = Some(*outcome);
                            self.report_shared_fact(fact, &mut report_progress)?;
                        }
                        ConversationFact::Problem {
                            turn_id: problem_turn_id,
                            ..
                        } => {
                            ensure_optional_turn_id(*problem_turn_id, &turn_id)?;
                            self.report_shared_fact(fact, &mut report_progress)?;
                        }
                    }
                }
            }
        }

        completed_outcome.ok_or_else(|| ModelDriverError::IncompleteTurn.into())
    }

    fn append_shared_fact(&self, fact: ConversationFact) -> ConversationSessionResult<()> {
        self.event_store.append_new_conversation_event(
            self.conversation_id,
            ConversationEvent::Driver(DriverConversationEvent::Fact(
                DriverConversationFact::Shared(fact),
            )),
        )?;
        Ok(())
    }

    fn report_shared_fact(
        &self,
        fact: ConversationFact,
        report_progress: &mut impl FnMut(ConversationSessionProgress) -> ConversationSessionResult<()>,
    ) -> ConversationSessionResult<()> {
        match &fact {
            ConversationFact::Assistant { .. } | ConversationFact::Communication { .. } => {
                let progress = ConversationSessionProgress::EventCompleted {
                    event: fact.clone(),
                };
                self.append_shared_fact(fact)?;
                report_progress(progress)?;
            }
            ConversationFact::Problem { problem, .. } => {
                let progress = ConversationSessionProgress::ProblemCompleted {
                    problem: problem.clone(),
                };
                self.append_shared_fact(fact)?;
                report_progress(progress)?;
            }
            ConversationFact::User { .. } | ConversationFact::TurnCompleted { .. } => {
                self.append_shared_fact(fact)?;
            }
        }
        Ok(())
    }
}

fn pending_request_ids(
    conversation: &crate::conversation::Conversation,
) -> HashSet<ConversationCommandId> {
    conversation
        .pending_user_requests()
        .iter()
        .map(|request| request.command_id)
        .collect()
}

fn ensure_turn_id(
    actual_turn_id: ConversationTurnId,
    expected_turn_id: &ConversationTurnId,
) -> Result<(), ModelDriverError> {
    if actual_turn_id != *expected_turn_id {
        return Err(ModelDriverError::WrongTurnIdentity {
            expected: *expected_turn_id,
            actual: actual_turn_id,
        });
    }
    Ok(())
}

fn ensure_optional_turn_id(
    actual_turn_id: Option<ConversationTurnId>,
    expected_turn_id: &ConversationTurnId,
) -> Result<(), ModelDriverError> {
    actual_turn_id
        .map(|actual_turn_id| ensure_turn_id(actual_turn_id, expected_turn_id))
        .unwrap_or(Ok(()))
}

fn driver_event_type(event: &DriverConversationEvent) -> String {
    match event {
        DriverConversationEvent::Command(_) => "driver_command".to_owned(),
        DriverConversationEvent::Fact(DriverConversationFact::Shared(fact)) => match fact {
            ConversationFact::User { .. } => "user".to_owned(),
            ConversationFact::Assistant { .. } => "assistant".to_owned(),
            ConversationFact::Communication { .. } => "communication".to_owned(),
            ConversationFact::Problem { .. } => "problem".to_owned(),
            ConversationFact::TurnCompleted { .. } => "turn_completed".to_owned(),
        },
        DriverConversationEvent::Fact(DriverConversationFact::Extension(_)) => {
            "driver_fact".to_owned()
        }
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
        ConversationEventExtension, ConversationFact, DriverConversationEvent,
        DriverConversationFact, DriverEventEnvelope, DriverEventReadError, DriverEventReader,
        ModelId, ModelSource, ProviderId, TurnOutcome, UserPrompt,
    };
    use crate::model_driver::{ModelDriver, ModelDriverError, ModelOutputStream, TurnInput};
    use crate::persistence::EventStore;

    struct RecordingDriver {
        source: ModelSource,
        pending_counts: Arc<Mutex<Vec<usize>>>,
        outcome: TurnOutcome,
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
            let turn_id = input.turn_id();
            let mut output = pending_requests
                .into_iter()
                .map(|request| {
                    Ok(DriverConversationEvent::Fact(
                        DriverConversationFact::Shared(ConversationFact::User {
                            caused_by: Some(request.command_id),
                            content: request.content,
                        }),
                    ))
                })
                .collect::<Vec<_>>();
            output.push(Ok(DriverConversationEvent::Fact(
                DriverConversationFact::Shared(ConversationFact::TurnCompleted {
                    turn_id,
                    outcome: self.outcome,
                }),
            )));
            async move { Ok(stream::iter(output).boxed()) }.boxed()
        }
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
                outcome: TurnOutcome::Succeeded,
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
                outcome: TurnOutcome::Succeeded,
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
                outcome: TurnOutcome::Failed,
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
}
