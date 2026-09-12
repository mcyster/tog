mod extension;
mod record;

pub(crate) use extension::{
    ConversationEventError, ConversationEventExtension, DriverEventEnvelope, DriverEventReadError,
    DriverEventReader, InvalidDriverEventEnvelope,
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
    Request(ConversationRequest),
    Driver(DriverConversationEvent),
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ConversationEventClass {
    Command,
    Fact,
}

impl ConversationEvent {
    #[allow(dead_code)]
    #[allow(dead_code)]
    pub(crate) fn class(&self) -> ConversationEventClass {
        match self {
            Self::Request(request) => request.class(),
            Self::Driver(event) => event.class(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ConversationRequest {
    UserMessageRequested(UserMessageRequest),
    TurnRequested {
        command_id: ConversationCommandId,
        turn_id: ConversationTurnId,
    },
}

impl ConversationRequest {
    pub(crate) fn command(&self) -> ConversationCommand {
        match self {
            Self::UserMessageRequested(request) => {
                ConversationCommand::UserMessageRequested(request.clone())
            }
            Self::TurnRequested {
                command_id,
                turn_id,
            } => ConversationCommand::TurnRequested {
                command_id: *command_id,
                turn_id: *turn_id,
            },
        }
    }

    pub(crate) fn class(&self) -> ConversationEventClass {
        ConversationEventClass::Command
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct UserMessageRequest {
    pub(crate) command_id: ConversationCommandId,
    pub(crate) content: Vec<UserContent>,
}

pub(crate) enum DriverConversationEvent {
    Command(Box<dyn ConversationEventExtension>),
    Fact(DriverConversationFact),
}

#[allow(dead_code)]
pub(crate) enum DriverConversationFact {
    Shared(ConversationFact),
    Extension(Box<dyn ConversationEventExtension>),
}

impl DriverConversationEvent {
    pub(crate) fn class(&self) -> ConversationEventClass {
        match self {
            Self::Command(_) => ConversationEventClass::Command,
            Self::Fact(_) => ConversationEventClass::Fact,
        }
    }
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
pub(crate) enum ConversationFact {
    User {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caused_by: Option<ConversationCommandId>,
        content: Vec<UserContent>,
    },
    Assistant {
        turn_id: ConversationTurnId,
        invocation_id: ModelInvocationId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<ModelData>,
        response: AssistantResponse,
    },
    Communication {
        turn_id: ConversationTurnId,
        invocation_id: ModelInvocationId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<ModelData>,
        communication: ModelCommunication,
    },
    Problem {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<ConversationTurnId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        invocation_id: Option<ModelInvocationId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<ModelData>,
        problem: ConversationProblem,
    },
    TurnCompleted {
        turn_id: ConversationTurnId,
        outcome: TurnOutcome,
    },
}

impl ConversationEventKind {
    pub(super) fn ensure_valid(&self) -> Result<(), InvalidConversationEventKind> {
        match self {
            Self::Command(_) => Ok(()),
            Self::Fact(ConversationFact::Assistant { data, response, .. }) => {
                if let Some(data) = data {
                    data.ensure_valid()
                        .map_err(InvalidConversationEventKind::ModelData)?;
                }
                response
                    .ensure_valid()
                    .map_err(InvalidConversationEventKind::Assistant)
            }
            Self::Fact(ConversationFact::Communication {
                data,
                communication,
                ..
            }) => {
                if let Some(data) = data {
                    data.ensure_valid()
                        .map_err(InvalidConversationEventKind::ModelData)?;
                }
                communication
                    .ensure_valid()
                    .map_err(InvalidConversationEventKind::ModelCommunication)
            }
            Self::Fact(ConversationFact::Problem { data, problem, .. }) => {
                if let Some(data) = data {
                    data.ensure_valid()
                        .map_err(InvalidConversationEventKind::ModelData)?;
                }
                problem
                    .ensure_valid()
                    .map_err(InvalidConversationEventKind::ConversationProblem)
            }
            Self::Fact(ConversationFact::User { .. })
            | Self::Fact(ConversationFact::TurnCompleted { .. }) => Ok(()),
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
    DriverEvent(InvalidDriverEventEnvelope),
}

impl Display for InvalidConversationEventKind {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Assistant(error) => Display::fmt(error, formatter),
            Self::ModelCommunication(error) => Display::fmt(error, formatter),
            Self::ConversationProblem(error) => Display::fmt(error, formatter),
            Self::ModelData(error) => Display::fmt(error, formatter),
            Self::DriverEvent(error) => Display::fmt(error, formatter),
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
        AssistantResponse, ConversationCommand, ConversationEventClass, ConversationEventKind,
        ConversationFact, DriverEventEnvelope, InvalidAssistantResponse, InvalidModelCommunication,
        ModelCommunication, ModelEventImportance, TurnOutcome,
    };
    use crate::conversation::{
        ConversationCommandId, ConversationProblem, ConversationTurnId, ModelInvocationId,
        ModelIssue,
    };

    #[test]
    fn assistant_is_a_top_level_event_with_model_provenance() {
        let event = ConversationEventKind::Fact(ConversationFact::Assistant {
            turn_id: ConversationTurnId::new(),
            invocation_id: ModelInvocationId::new(),
            data: None,
            response: AssistantResponse::new("The answer is 42.".to_owned())
                .expect("the assistant response should be valid"),
        });

        assert_eq!(
            serde_json::to_value(&event).expect("the event should serialize")["class"],
            "fact"
        );
    }

    #[test]
    fn problem_can_have_model_provenance_without_being_a_model_event() {
        let event = ConversationEventKind::Fact(ConversationFact::Problem {
            turn_id: Some(ConversationTurnId::new()),
            invocation_id: Some(ModelInvocationId::new()),
            data: None,
            problem: ConversationProblem::Issue(
                ModelIssue::try_refusal("I cannot comply.".to_owned())
                    .expect("the refusal should be valid"),
            ),
        });

        let serialized = serde_json::to_value(&event).expect("the problem should serialize");
        assert_eq!(serialized["class"], "fact");
        assert_eq!(serialized["event"]["type"], "problem");
    }

    #[test]
    fn commands_and_turn_completion_are_distinct_from_model_output() {
        let command = ConversationEventKind::Command(ConversationCommand::TurnRequested {
            command_id: ConversationCommandId::new(),
            turn_id: ConversationTurnId::new(),
        });
        let completed = ConversationEventKind::Fact(ConversationFact::TurnCompleted {
            turn_id: ConversationTurnId::new(),
            outcome: TurnOutcome::Succeeded,
        });

        assert_eq!(command.class(), super::ConversationEventClass::Command);
        assert_eq!(completed.class(), super::ConversationEventClass::Fact);
    }

    #[test]
    fn driver_event_envelope_preserves_its_classification_and_payload() {
        let envelope = DriverEventEnvelope::new(
            ConversationEventClass::Command,
            "test".to_owned(),
            "1".to_owned(),
            "invocation_requested".to_owned(),
            1,
            "An invocation was requested.".to_owned(),
            json!({ "invocation_id": "invocation_1" }),
        )
        .expect("the driver event envelope should be valid");
        let serialized = serde_json::to_value(&envelope).expect("the envelope should serialize");
        let restored: DriverEventEnvelope =
            serde_json::from_value(serialized).expect("the envelope should deserialize");

        assert_eq!(restored.class(), ConversationEventClass::Command);
        assert_eq!(restored.driver(), "test");
        assert_eq!(restored.event_type(), "invocation_requested");
        assert_eq!(restored.event_schema_version(), 1);
        assert_eq!(restored.description(), "An invocation was requested.");
        assert_eq!(restored.payload()["invocation_id"], "invocation_1");

        let fact = DriverEventEnvelope::new(
            ConversationEventClass::Fact,
            "test".to_owned(),
            "1".to_owned(),
            "invocation_finished".to_owned(),
            1,
            "An invocation finished.".to_owned(),
            json!({ "successful": true }),
        )
        .expect("the driver fact envelope should be valid");
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
