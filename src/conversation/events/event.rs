use std::fmt::{Display, Formatter};

use time::OffsetDateTime;
use uuid::Uuid;

use crate::conversation::{ConversationEvent, ConversationEventContent, ConversationId};

use super::{ConversationEventId, InvalidConversationEvent};

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
    pub(crate) fn new(conversation_id: ConversationId, content: ConversationEventContent) -> Self {
        Self {
            id: ConversationEventId::new(),
            conversation_id,
            timestamp: OffsetDateTime::now_utc(),
            content,
        }
    }

    #[cfg(test)]
    pub(crate) fn at(
        conversation_id: ConversationId,
        id: ConversationEventId,
        timestamp: OffsetDateTime,
        content: ConversationEventContent,
    ) -> Self {
        Self {
            id,
            conversation_id,
            timestamp,
            content,
        }
    }

    pub(crate) fn id(&self) -> ConversationEventId {
        self.id
    }

    pub(crate) fn conversation_id(&self) -> ConversationId {
        self.conversation_id
    }

    pub(crate) fn timestamp(&self) -> OffsetDateTime {
        self.timestamp
    }

    pub(crate) fn content(&self) -> &ConversationEventContent {
        &self.content
    }

    pub(crate) fn ensure_valid(&self) -> Result<(), InvalidConversationEvent> {
        self.content.ensure_valid()
    }
}

impl ConversationEventContent {
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
            Self::Toolset(toolset) => toolset
                .ensure_valid()
                .map_err(InvalidConversationEvent::Toolset),
        }
    }
}
