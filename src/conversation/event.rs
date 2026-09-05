use std::error::Error;
use std::fmt::{Display, Formatter};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;

use super::{
    ConversationCommandId, ConversationEventId, ConversationId, ConversationProblem,
    ConversationTurnId, InvalidConversationProblem, InvalidModelData, ModelDetails,
    ModelInvocationId, ModelSource,
};

pub(crate) const SCHEMA_VERSION: u32 = 11;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ConversationEvent {
    pub(crate) conversation_id: ConversationId,
    pub(crate) position: u64,
    pub(crate) id: ConversationEventId,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) timestamp: OffsetDateTime,
    pub(crate) schema_version: u32,
    #[serde(flatten)]
    pub(crate) kind: ConversationRecordKind,
}

impl ConversationEvent {
    pub(crate) fn new(
        conversation_id: ConversationId,
        position: u64,
        kind: ConversationEventKind,
    ) -> Self {
        Self {
            conversation_id,
            position,
            id: ConversationEventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            schema_version: SCHEMA_VERSION,
            kind: ConversationRecordKind::Event(kind),
        }
    }

    pub(crate) fn new_driver(
        conversation_id: ConversationId,
        position: u64,
        event: DriverEventEnvelope,
    ) -> Self {
        Self {
            conversation_id,
            position,
            id: ConversationEventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            schema_version: SCHEMA_VERSION,
            kind: ConversationRecordKind::Driver(event),
        }
    }

    pub(super) fn ensure_valid(&self) -> Result<(), InvalidConversationEventKind> {
        self.kind.ensure_valid()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub(crate) enum ConversationRecordKind {
    Event(ConversationEventKind),
    Driver(DriverEventEnvelope),
}

impl ConversationRecordKind {
    pub(super) fn ensure_valid(&self) -> Result<(), InvalidConversationEventKind> {
        match self {
            Self::Event(event) => event.ensure_valid(),
            Self::Driver(event) => event
                .ensure_valid()
                .map_err(InvalidConversationEventKind::DriverEvent),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct DriverEventEnvelope {
    driver: String,
    driver_version: String,
    event_type: String,
    event_schema_version: u32,
    description: String,
    payload: Value,
}

#[allow(dead_code)]
impl DriverEventEnvelope {
    pub(crate) fn new(
        driver: String,
        driver_version: String,
        event_type: String,
        event_schema_version: u32,
        description: String,
        payload: Value,
    ) -> Result<Self, InvalidDriverEventEnvelope> {
        let event = Self {
            driver,
            driver_version,
            event_type,
            event_schema_version,
            description,
            payload,
        };
        event.ensure_valid()?;
        Ok(event)
    }

    pub(crate) fn driver(&self) -> &str {
        &self.driver
    }

    pub(crate) fn driver_version(&self) -> &str {
        &self.driver_version
    }

    pub(crate) fn event_type(&self) -> &str {
        &self.event_type
    }

    pub(crate) fn event_schema_version(&self) -> u32 {
        self.event_schema_version
    }

    pub(crate) fn payload(&self) -> &Value {
        &self.payload
    }

    fn ensure_valid(&self) -> Result<(), InvalidDriverEventEnvelope> {
        if self.driver.trim().is_empty() {
            return Err(InvalidDriverEventEnvelope::DriverName);
        }
        if self.driver_version.trim().is_empty() {
            return Err(InvalidDriverEventEnvelope::DriverVersion);
        }
        if self.event_type.trim().is_empty() {
            return Err(InvalidDriverEventEnvelope::EventType);
        }
        if self.description.trim().is_empty() {
            return Err(InvalidDriverEventEnvelope::Description);
        }
        Ok(())
    }
}

#[derive(Debug)]
pub(crate) enum InvalidDriverEventEnvelope {
    DriverName,
    DriverVersion,
    EventType,
    Description,
}

impl Display for InvalidDriverEventEnvelope {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::DriverName => "driver name must not be empty",
            Self::DriverVersion => "driver version must not be empty",
            Self::EventType => "driver event type must not be empty",
            Self::Description => "driver event description must not be empty",
        };
        write!(formatter, "{message}")
    }
}

impl Error for InvalidDriverEventEnvelope {}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ConversationEventKind {
    UserMessageRequested {
        command_id: ConversationCommandId,
        content: Vec<UserContent>,
    },
    User {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caused_by: Option<ConversationCommandId>,
        content: Vec<UserContent>,
    },
    TurnRequested {
        command_id: ConversationCommandId,
        turn_id: ConversationTurnId,
        model: ModelSource,
    },
    Assistant {
        turn_id: ConversationTurnId,
        invocation_id: ModelInvocationId,
        model: ModelDetails,
        response: AssistantResponse,
    },
    Communication {
        turn_id: ConversationTurnId,
        invocation_id: ModelInvocationId,
        model: ModelDetails,
        communication: ModelCommunication,
    },
    Problem {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<ConversationTurnId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        invocation_id: Option<ModelInvocationId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<ModelDetails>,
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
            Self::UserMessageRequested { .. } | Self::User { .. } => Ok(()),
            Self::TurnRequested { .. } => Ok(()),
            Self::Assistant {
                model, response, ..
            } => {
                model
                    .ensure_valid()
                    .map_err(InvalidConversationEventKind::ModelData)?;
                response
                    .ensure_valid()
                    .map_err(InvalidConversationEventKind::Assistant)
            }
            Self::Communication {
                model,
                communication,
                ..
            } => {
                model
                    .ensure_valid()
                    .map_err(InvalidConversationEventKind::ModelData)?;
                communication
                    .ensure_valid()
                    .map_err(InvalidConversationEventKind::ModelCommunication)
            }
            Self::Problem { model, problem, .. } => {
                if let Some(model) = model {
                    model
                        .ensure_valid()
                        .map_err(InvalidConversationEventKind::ModelData)?;
                }
                problem
                    .ensure_valid()
                    .map_err(InvalidConversationEventKind::ConversationProblem)
            }
            Self::TurnCompleted { .. } => Ok(()),
        }
    }

    pub(crate) fn is_command(&self) -> bool {
        matches!(
            self,
            Self::UserMessageRequested { .. } | Self::TurnRequested { .. }
        )
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TurnOutcome {
    Succeeded,
    Failed,
}

#[derive(Debug)]
pub(super) enum InvalidConversationEventKind {
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
    use std::str::FromStr;

    use serde_json::json;

    use super::{
        AssistantResponse, ConversationEventKind, InvalidAssistantResponse,
        InvalidModelCommunication, ModelCommunication, ModelDetails, ModelEventImportance,
        TurnOutcome,
    };
    use crate::conversation::{
        ConversationCommandId, ConversationProblem, ConversationTurnId, ModelId, ModelInvocationId,
        ModelIssue, ModelSource, ProviderId,
    };

    fn source() -> ModelSource {
        ModelSource::new(
            ProviderId::from_str("openai").expect("the provider identifier should be valid"),
            ModelId::from_str("gpt-5.6").expect("the model identifier should be valid"),
        )
    }

    #[test]
    fn assistant_is_a_top_level_event_with_model_provenance() {
        let event = ConversationEventKind::Assistant {
            turn_id: ConversationTurnId::new(),
            invocation_id: ModelInvocationId::new(),
            model: ModelDetails::new(source(), None).expect("the model details should be valid"),
            response: AssistantResponse::new("The answer is 42.".to_owned())
                .expect("the assistant response should be valid"),
        };

        assert_eq!(
            serde_json::to_value(&event).expect("the event should serialize"),
            json!({
                "type": "assistant",
                "turn_id": event_turn_id(&event),
                "invocation_id": event_invocation_id(&event),
                "model": { "source": { "provider": "openai", "model": "gpt-5.6" } },
                "response": { "message": "The answer is 42." }
            })
        );
    }

    #[test]
    fn problem_can_have_model_provenance_without_being_a_model_event() {
        let event = ConversationEventKind::Problem {
            turn_id: Some(ConversationTurnId::new()),
            invocation_id: Some(ModelInvocationId::new()),
            model: Some(
                ModelDetails::new(source(), None).expect("the model details should be valid"),
            ),
            problem: ConversationProblem::Issue(
                ModelIssue::try_refusal("I cannot comply.".to_owned())
                    .expect("the refusal should be valid"),
            ),
        };

        let serialized = serde_json::to_value(&event).expect("the problem should serialize");
        assert_eq!(serialized["type"], "problem");
        assert!(serialized.get("event").is_none());
        assert!(serialized.get("message").is_none());
    }

    #[test]
    fn commands_and_turn_completion_are_distinct_from_model_output() {
        let command = ConversationEventKind::TurnRequested {
            command_id: ConversationCommandId::new(),
            turn_id: ConversationTurnId::new(),
            model: source(),
        };
        let completed = ConversationEventKind::TurnCompleted {
            turn_id: ConversationTurnId::new(),
            outcome: TurnOutcome::Succeeded,
        };

        assert!(command.is_command());
        assert!(!completed.is_command());
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

    fn event_turn_id(event: &ConversationEventKind) -> ConversationTurnId {
        let ConversationEventKind::Assistant { turn_id, .. } = event else {
            panic!("the event should be an assistant event");
        };
        *turn_id
    }

    fn event_invocation_id(event: &ConversationEventKind) -> ModelInvocationId {
        let ConversationEventKind::Assistant { invocation_id, .. } = event else {
            panic!("the event should be an assistant event");
        };
        *invocation_id
    }
}
