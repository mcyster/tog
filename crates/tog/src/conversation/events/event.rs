use std::fmt::{Display, Formatter};

use time::OffsetDateTime;
use uuid::Uuid;

use crate::conversation::{ConversationEvent, ConversationEventPayload, ConversationId};

use super::{ConversationEventId, InvalidConversationEvent};

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
        write!(formatter, "conversation_event_{}", self.0.simple())
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

    pub fn ensure_valid(&self) -> Result<(), InvalidConversationEvent> {
        self.payload.ensure_valid()
    }
}

impl ConversationEventPayload {
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
