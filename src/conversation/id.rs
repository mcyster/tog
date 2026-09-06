use std::error::Error;
use std::fmt::{Display, Formatter};
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub(crate) struct ConversationId(Uuid);

impl ConversationId {
    pub(crate) fn new() -> Self {
        Self(Uuid::now_v7())
    }

    pub(crate) fn storage_key(self) -> String {
        self.0.simple().to_string()
    }
}

impl Display for ConversationId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "conversation_{}", self.0.simple())
    }
}

impl FromStr for ConversationId {
    type Err = InvalidConversationId;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let uuid_text = text.strip_prefix("conversation_").unwrap_or(text);
        Uuid::parse_str(uuid_text)
            .map(Self)
            .map_err(InvalidConversationId)
    }
}

#[derive(Debug)]
pub(crate) struct InvalidConversationId(uuid::Error);

impl Display for InvalidConversationId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "invalid conversation identifier: {}", self.0)
    }
}

impl Error for InvalidConversationId {}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub(crate) struct ConversationEventId(Uuid);

impl ConversationEventId {
    pub(crate) fn new() -> Self {
        Self(Uuid::now_v7())
    }

    pub(crate) fn storage_key(self) -> String {
        self.0.simple().to_string()
    }
}

impl Display for ConversationEventId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "conversation_event_{}", self.0.simple())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub(crate) struct ConversationCommandId(Uuid);

impl ConversationCommandId {
    pub(crate) fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Display for ConversationCommandId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "conversation_command_{}", self.0.simple())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub(crate) struct ConversationTurnId(Uuid);

impl ConversationTurnId {
    pub(crate) fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Display for ConversationTurnId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "conversation_turn_{}", self.0.simple())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub(crate) struct ModelInvocationId(Uuid);

impl ModelInvocationId {
    pub(crate) fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Display for ModelInvocationId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "model_invocation_{}", self.0.simple())
    }
}
