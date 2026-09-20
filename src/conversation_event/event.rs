use std::error::Error;
use std::fmt::{Display, Formatter};

use super::{
    ConversationEvent, ConversationEventClass, ConversationEventKind, ConversationFact,
    InvalidConversationEventKind,
};

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

impl ConversationEventKind {
    pub(super) fn ensure_valid(&self) -> Result<(), InvalidConversationEventKind> {
        match self {
            Self::Command(_) => Ok(()),
            Self::Fact(ConversationFact::Message { message, .. }) => message.ensure_valid(),
            Self::Fact(ConversationFact::Lifecycle(_)) => Ok(()),
            Self::Fact(ConversationFact::ToolsAvailable { tools }) => {
                for tool in tools {
                    tool.ensure_valid()
                        .map_err(InvalidConversationEventKind::Tool)?;
                }
                Ok(())
            }
            Self::Fact(ConversationFact::ToolRequest { request, .. }) => request
                .ensure_valid()
                .map_err(InvalidConversationEventKind::Tool),
            Self::Fact(ConversationFact::ToolResponse { response, .. }) => response
                .ensure_valid()
                .map_err(InvalidConversationEventKind::Tool),
        }
    }

    pub(crate) fn class(&self) -> ConversationEventClass {
        match self {
            Self::Command(_) => ConversationEventClass::Command,
            Self::Fact(_) => ConversationEventClass::Fact,
        }
    }
}

impl Display for InvalidConversationEventKind {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Assistant(error) => Display::fmt(error, formatter),
            Self::ModelCommunication(error) => Display::fmt(error, formatter),
            Self::ConversationProblem(error) => Display::fmt(error, formatter),
            Self::ModelData(error) => Display::fmt(error, formatter),
            Self::Tool(error) => Display::fmt(error, formatter),
            Self::Envelope(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for InvalidConversationEventKind {}
