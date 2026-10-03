use std::error::Error;
use std::fmt::{Display, Formatter};
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ConversationId(Uuid);

impl ConversationId {
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    pub fn storage_key(self) -> String {
        self.0.simple().to_string()
    }
}

impl Default for ConversationId {
    fn default() -> Self {
        Self::new()
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
pub struct InvalidConversationId(uuid::Error);

impl Display for InvalidConversationId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "invalid conversation identifier: {}", self.0)
    }
}

impl Error for InvalidConversationId {}
