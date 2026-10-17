use std::fmt::{Display, Formatter};
use std::str::FromStr;

use serde::de::Error as DeserializeError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::conversation::{
    ConversationEvent, ConversationEventPayload, ConversationEventPersistence, ConversationId,
};

use super::{ConversationEventId, InvalidConversationEvent, InvalidConversationEventId};

const PREFIX: &str = "evt_";

impl ConversationEventId {
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for ConversationEventId {
    fn default() -> Self {
        Self::new()
    }
}

impl Display for ConversationEventId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{PREFIX}{}", self.0.simple())
    }
}

impl FromStr for ConversationEventId {
    type Err = InvalidConversationEventId;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let uuid_text = text.strip_prefix(PREFIX).unwrap_or(text);
        Uuid::parse_str(uuid_text)
            .map(Self)
            .map_err(InvalidConversationEventId)
    }
}

impl Serialize for ConversationEventId {
    fn serialize<SerializerType>(
        &self,
        serializer: SerializerType,
    ) -> Result<SerializerType::Ok, SerializerType::Error>
    where
        SerializerType: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for ConversationEventId {
    fn deserialize<DeserializerType>(
        deserializer: DeserializerType,
    ) -> Result<Self, DeserializerType::Error>
    where
        DeserializerType: Deserializer<'de>,
    {
        let unvalidated_value = String::deserialize(deserializer)?;
        Self::from_str(&unvalidated_value).map_err(DeserializerType::Error::custom)
    }
}

impl ConversationEvent {
    pub fn new(conversation_id: ConversationId, payload: ConversationEventPayload) -> Self {
        Self {
            id: ConversationEventId::new(),
            conversation_id,
            timestamp: OffsetDateTime::now_utc(),
            payload,
        }
    }

    pub fn id(&self) -> ConversationEventId {
        self.id
    }

    pub fn conversation_id(&self) -> ConversationId {
        self.conversation_id
    }

    pub fn timestamp(&self) -> OffsetDateTime {
        self.timestamp
    }

    pub fn payload(&self) -> &ConversationEventPayload {
        &self.payload
    }

    pub fn persistence(&self) -> ConversationEventPersistence {
        match &self.payload {
            ConversationEventPayload::ModelSpecificEvent(event) => event.persistence(),
            _ => ConversationEventPersistence::Durable,
        }
    }

    pub fn ensure_valid(&self) -> Result<(), InvalidConversationEvent> {
        self.payload.ensure_valid()
    }
}

impl ConversationEventPayload {
    pub fn describe(&self) -> String {
        match self {
            Self::User(event) => format!("user {}", event.describe()),
            Self::TurnStart(event) => format!("turn_start {}", event.describe()),
            Self::TurnEnd(event) => format!("turn_end {}", event.describe()),
            Self::AssistantResponse(event) => format!("assistant_response {}", event.describe()),
            Self::ToolRequest(event) => format!("tool_request {}", event.describe()),
            Self::ToolResponse(event) => format!("tool_response {}", event.describe()),
            Self::ModelRequest(event) => format!("model_request {}", event.describe()),
            Self::ModelResponse(event) => format!("model_response {}", event.describe()),
            Self::ModelSpecificEvent(event) => {
                format!("model_specific_event {}", event.describe())
            }
            Self::Automation(event) => format!("automation {}", event.describe()),
            Self::Context(event) => format!("context {}", event.describe()),
            Self::Tools(tools) => format!("tools {}", tools.describe()),
        }
    }

    pub(crate) fn ensure_valid(&self) -> Result<(), InvalidConversationEvent> {
        match self {
            Self::User(event) => event.ensure_valid().map_err(InvalidConversationEvent::User),
            Self::TurnStart(event) => event.ensure_valid(),
            Self::TurnEnd(event) => event
                .ensure_valid()
                .map_err(InvalidConversationEvent::TurnEnd),
            Self::AssistantResponse(event) => event
                .ensure_valid()
                .map_err(InvalidConversationEvent::AssistantResponse),
            Self::ToolRequest(event) => event
                .ensure_valid()
                .map_err(InvalidConversationEvent::ToolRequest),
            Self::ToolResponse(event) => event
                .ensure_valid()
                .map_err(InvalidConversationEvent::ToolResponse),
            Self::ModelRequest(event) => event
                .ensure_valid()
                .map_err(InvalidConversationEvent::ModelRequest),
            Self::ModelResponse(event) => event
                .ensure_valid()
                .map_err(InvalidConversationEvent::ModelResponse),
            Self::ModelSpecificEvent(event) => event
                .ensure_valid()
                .map_err(InvalidConversationEvent::ModelSpecificEvent),
            Self::Automation(event) => event
                .ensure_valid()
                .map_err(InvalidConversationEvent::Automation),
            Self::Context(event) => event
                .ensure_valid()
                .map_err(InvalidConversationEvent::Context),
            Self::Tools(tools) => tools
                .ensure_valid()
                .map_err(InvalidConversationEvent::Tools),
        }
    }
}
