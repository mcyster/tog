use std::fmt::{Display, Formatter};

use uuid::Uuid;

use super::{ConversationEvent, ConversationEventId, InvalidConversationEvent};

impl ConversationEventId {
    pub(crate) fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Display for ConversationEventId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "conversation_event_{}", self.0.simple())
    }
}

impl ConversationEvent {
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
            Self::Data(event) => event.ensure_valid().map_err(InvalidConversationEvent::Data),
        }
    }
}
