use std::error::Error;
use std::fmt::{Display, Formatter};

use super::InvalidConversationEvent;

impl Display for InvalidConversationEvent {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::User(error) => Display::fmt(error, formatter),
            Self::TurnEnd(error) => Display::fmt(error, formatter),
            Self::AssistantResponse(error) => Display::fmt(error, formatter),
            Self::ToolRequest(error) => Display::fmt(error, formatter),
            Self::ToolResponse(error) => Display::fmt(error, formatter),
            Self::ModelRequest(error) => Display::fmt(error, formatter),
            Self::ModelResponse(error) => Display::fmt(error, formatter),
            Self::ModelSpecificEvent(error) => Display::fmt(error, formatter),
            Self::Automation(error) => Display::fmt(error, formatter),
            Self::Context(error) => Display::fmt(error, formatter),
            Self::Tools(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for InvalidConversationEvent {}
