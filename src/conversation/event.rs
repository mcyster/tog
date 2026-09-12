mod extension;
mod record;

pub(crate) use extension::{
    ConversationEventEnvelope, ConversationEventError, ConversationEventExtension,
    ConversationEventReadError, ConversationEventReader, InvalidConversationEventEnvelope,
};
pub(crate) use record::{ConversationEventRecord, StoredConversationEventKind};

use std::error::Error;
use std::fmt::{Display, Formatter};

use serde::{Deserialize, Serialize};

use super::{
    ConversationCommandId, ConversationProblem, ConversationTurnId, InvalidConversationProblem,
    InvalidModelData, ModelData, ModelInvocationId,
};

pub(crate) enum ConversationEvent {
    Command(ConversationCommand),
    Fact(ConversationFact),
    Extension(Box<dyn ConversationEventExtension>),
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ConversationEventClass {
    Command,
    Fact,
}

impl ConversationEvent {
    #[allow(dead_code)]
    pub(crate) fn class(&self) -> ConversationEventClass {
        match self {
            Self::Command(_) => ConversationEventClass::Command,
            Self::Fact(_) => ConversationEventClass::Fact,
            Self::Extension(event) => event.class(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct UserMessageRequest {
    pub(crate) command_id: ConversationCommandId,
    pub(crate) content: Vec<UserContent>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "class", content = "event", rename_all = "snake_case")]
pub(crate) enum ConversationEventKind {
    Command(ConversationCommand),
    Fact(ConversationFact),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ConversationCommand {
    UserMessageRequested(UserMessageRequest),
    TurnRequested {
        command_id: ConversationCommandId,
        turn_id: ConversationTurnId,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ConversationMessage {
    User {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caused_by: Option<ConversationCommandId>,
        content: Vec<UserContent>,
    },
    #[serde(rename = "assistant")]
    AssistantResponse {
        invocation_id: ModelInvocationId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<ModelData>,
        response: AssistantResponse,
    },
    Communication {
        invocation_id: ModelInvocationId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<ModelData>,
        communication: ModelCommunication,
    },
    Problem {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        invocation_id: Option<ModelInvocationId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<ModelData>,
        problem: ConversationProblem,
    },
}

impl ConversationMessage {
    fn ensure_valid(&self) -> Result<(), InvalidConversationEventKind> {
        match self {
            Self::User { .. } => Ok(()),
            Self::AssistantResponse { data, response, .. } => {
                if let Some(data) = data {
                    data.ensure_valid()
                        .map_err(InvalidConversationEventKind::ModelData)?;
                }
                response
                    .ensure_valid()
                    .map_err(InvalidConversationEventKind::Assistant)
            }
            Self::Communication {
                data,
                communication,
                ..
            } => {
                if let Some(data) = data {
                    data.ensure_valid()
                        .map_err(InvalidConversationEventKind::ModelData)?;
                }
                communication
                    .ensure_valid()
                    .map_err(InvalidConversationEventKind::ModelCommunication)
            }
            Self::Problem { data, problem, .. } => {
                if let Some(data) = data {
                    data.ensure_valid()
                        .map_err(InvalidConversationEventKind::ModelData)?;
                }
                problem
                    .ensure_valid()
                    .map_err(InvalidConversationEventKind::ConversationProblem)
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub(crate) enum ConversationFact {
    Message {
        #[serde(flatten)]
        message: ConversationMessage,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<ConversationTurnId>,
    },
    Lifecycle(ConversationLifecycle),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ConversationLifecycle {
    TurnCompleted {
        turn_id: ConversationTurnId,
        outcome: TurnOutcome,
    },
}

impl ConversationEventKind {
    pub(super) fn ensure_valid(&self) -> Result<(), InvalidConversationEventKind> {
        match self {
            Self::Command(_) => Ok(()),
            Self::Fact(ConversationFact::Message { message, .. }) => message.ensure_valid(),
            Self::Fact(ConversationFact::Lifecycle(_)) => Ok(()),
        }
    }

    pub(crate) fn class(&self) -> ConversationEventClass {
        match self {
            Self::Command(_) => ConversationEventClass::Command,
            Self::Fact(_) => ConversationEventClass::Fact,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TurnOutcome {
    Succeeded,
    Failed,
}

#[derive(Debug)]
pub(crate) enum InvalidConversationEventKind {
    Assistant(InvalidAssistantResponse),
    ModelCommunication(InvalidModelCommunication),
    ConversationProblem(InvalidConversationProblem),
    ModelData(InvalidModelData),
    Envelope(InvalidConversationEventEnvelope),
}

impl Display for InvalidConversationEventKind {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Assistant(error) => Display::fmt(error, formatter),
            Self::ModelCommunication(error) => Display::fmt(error, formatter),
            Self::ConversationProblem(error) => Display::fmt(error, formatter),
            Self::ModelData(error) => Display::fmt(error, formatter),
            Self::Envelope(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for InvalidConversationEventKind {}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ModelEvent {
    Assistant(AssistantResponse),
    Communication(ModelCommunication),
}

#[cfg(test)]
impl ModelEvent {
    pub(crate) fn message(&self) -> &str {
        match self {
            Self::Assistant(response) => response.message(),
            Self::Communication(communication) => communication.message(),
        }
    }

    pub(crate) fn importance(&self) -> ModelEventImportance {
        match self {
            Self::Assistant(_) => ModelEventImportance::Important,
            Self::Communication(communication) => communication.importance(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct AssistantResponse {
    message: String,
}

impl AssistantResponse {
    pub(crate) fn new(message: String) -> Result<Self, InvalidAssistantResponse> {
        let response = Self { message };
        response.ensure_valid()?;
        Ok(response)
    }

    pub(crate) fn message(&self) -> &str {
        &self.message
    }

    fn ensure_valid(&self) -> Result<(), InvalidAssistantResponse> {
        if self.message.trim().is_empty() {
            return Err(InvalidAssistantResponse::EmptyMessage);
        }
        Ok(())
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum InvalidAssistantResponse {
    EmptyMessage,
}

impl Display for InvalidAssistantResponse {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyMessage => write!(formatter, "assistant response message must not be empty"),
        }
    }
}

impl Error for InvalidAssistantResponse {}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ModelCommunication {
    message: String,
    importance: ModelEventImportance,
    subtype: String,
}

impl ModelCommunication {
    pub(crate) fn new(
        message: String,
        importance: ModelEventImportance,
        subtype: String,
    ) -> Result<Self, InvalidModelCommunication> {
        let communication = Self {
            message,
            importance,
            subtype,
        };
        communication.ensure_valid()?;
        Ok(communication)
    }

    pub(crate) fn message(&self) -> &str {
        &self.message
    }

    pub(crate) fn importance(&self) -> ModelEventImportance {
        self.importance
    }

    pub(crate) fn subtype(&self) -> &str {
        &self.subtype
    }

    fn ensure_valid(&self) -> Result<(), InvalidModelCommunication> {
        if self.message.trim().is_empty() {
            return Err(InvalidModelCommunication::EmptyMessage);
        }
        if self.subtype().trim().is_empty() {
            return Err(InvalidModelCommunication::EmptySubtype);
        }
        Ok(())
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum InvalidModelCommunication {
    EmptyMessage,
    EmptySubtype,
}

impl Display for InvalidModelCommunication {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyMessage => {
                write!(formatter, "model communication message must not be empty")
            }
            Self::EmptySubtype => {
                write!(formatter, "model communication subtype must not be empty")
            }
        }
    }
}

impl Error for InvalidModelCommunication {}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ModelEventImportance {
    Detailed,
    Interesting,
    Important,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub(crate) enum UserContent {
    Text(String),
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        AssistantResponse, ConversationCommand, ConversationEventClass, ConversationEventEnvelope,
        ConversationEventKind, ConversationFact, ConversationLifecycle, ConversationMessage,
        InvalidAssistantResponse, InvalidModelCommunication, ModelCommunication,
        ModelEventImportance, TurnOutcome,
    };
    use crate::conversation::{
        ConversationCommandId, ConversationProblem, ConversationTurnId, ModelInvocationId,
        ModelIssue,
    };

    #[test]
    fn assistant_is_a_top_level_event_with_model_provenance() {
        let event = ConversationEventKind::Fact(ConversationFact::Message {
            message: ConversationMessage::AssistantResponse {
                invocation_id: ModelInvocationId::new(),
                data: None,
                response: AssistantResponse::new("The answer is 42.".to_owned())
                    .expect("the assistant response should be valid"),
            },
            turn_id: Some(ConversationTurnId::new()),
        });

        let serialized = serde_json::to_value(&event).expect("the event should serialize");
        assert_eq!(serialized["class"], "fact");
        assert_eq!(serialized["event"]["type"], "assistant");
    }

    #[test]
    fn problem_can_have_model_provenance_without_being_a_model_event() {
        let event = ConversationEventKind::Fact(ConversationFact::Message {
            message: ConversationMessage::Problem {
                invocation_id: Some(ModelInvocationId::new()),
                data: None,
                problem: ConversationProblem::Issue(
                    ModelIssue::try_refusal("I cannot comply.".to_owned())
                        .expect("the refusal should be valid"),
                ),
            },
            turn_id: Some(ConversationTurnId::new()),
        });

        let serialized = serde_json::to_value(&event).expect("the problem should serialize");
        assert_eq!(serialized["class"], "fact");
        assert_eq!(serialized["event"]["type"], "problem");
    }

    #[test]
    fn conversation_fact_reuses_the_message_payload_with_turn_association() {
        let turn_id = ConversationTurnId::new();
        let event = ConversationEventKind::Fact(ConversationFact::Message {
            message: ConversationMessage::AssistantResponse {
                invocation_id: ModelInvocationId::new(),
                data: None,
                response: AssistantResponse::new("Hello.".to_owned())
                    .expect("the assistant response should be valid"),
            },
            turn_id: Some(turn_id),
        });

        let serialized = serde_json::to_value(&event).expect("the event should serialize");
        assert_eq!(serialized["event"]["type"], "assistant");
        assert_eq!(
            serialized["event"]["turn_id"],
            serde_json::to_value(turn_id).expect("the turn identifier should serialize")
        );

        let restored: ConversationEventKind =
            serde_json::from_value(serialized).expect("the event should deserialize");
        assert_eq!(restored, event);
    }

    #[test]
    fn commands_and_turn_completion_are_distinct_from_model_output() {
        let command = ConversationEventKind::Command(ConversationCommand::TurnRequested {
            command_id: ConversationCommandId::new(),
            turn_id: ConversationTurnId::new(),
        });
        let completed = ConversationEventKind::Fact(ConversationFact::Lifecycle(
            ConversationLifecycle::TurnCompleted {
                turn_id: ConversationTurnId::new(),
                outcome: TurnOutcome::Succeeded,
            },
        ));

        assert_eq!(command.class(), super::ConversationEventClass::Command);
        assert_eq!(completed.class(), super::ConversationEventClass::Fact);
    }

    #[test]
    fn conversation_event_envelope_preserves_its_classification_and_payload() {
        let envelope = ConversationEventEnvelope::new(
            ConversationEventClass::Command,
            "test".to_owned(),
            "1".to_owned(),
            "invocation_requested".to_owned(),
            1,
            "An invocation was requested.".to_owned(),
            json!({ "invocation_id": "invocation_1" }),
        )
        .expect("the conversation event envelope should be valid");
        let serialized = serde_json::to_value(&envelope).expect("the envelope should serialize");
        let restored: ConversationEventEnvelope =
            serde_json::from_value(serialized).expect("the envelope should deserialize");

        assert_eq!(restored.class(), ConversationEventClass::Command);
        assert_eq!(restored.namespace(), "test");
        assert_eq!(restored.namespace_version(), "1");
        assert_eq!(restored.event_type(), "invocation_requested");
        assert_eq!(restored.event_schema_version(), 1);
        assert_eq!(restored.description(), "An invocation was requested.");
        assert_eq!(restored.payload()["invocation_id"], "invocation_1");

        let fact = ConversationEventEnvelope::new(
            ConversationEventClass::Fact,
            "test".to_owned(),
            "1".to_owned(),
            "invocation_finished".to_owned(),
            1,
            "An invocation finished.".to_owned(),
            json!({ "successful": true }),
        )
        .expect("the conversation fact envelope should be valid");
        assert_eq!(fact.class(), ConversationEventClass::Fact);
    }

    #[test]
    fn assistant_response_rejects_an_empty_message() {
        assert_eq!(
            AssistantResponse::new(String::new()),
            Err(InvalidAssistantResponse::EmptyMessage)
        );
    }

    #[test]
    fn model_communication_validates_messages_and_subtypes() {
        assert_eq!(
            ModelCommunication::new(
                String::new(),
                ModelEventImportance::Detailed,
                "reasoning".to_owned(),
            ),
            Err(InvalidModelCommunication::EmptyMessage)
        );
        assert_eq!(
            ModelCommunication::new(
                "message".to_owned(),
                ModelEventImportance::Detailed,
                " ".to_owned(),
            ),
            Err(InvalidModelCommunication::EmptySubtype)
        );
    }
}
