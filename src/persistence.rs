mod file_event_store;
mod legacy;
mod log;
#[cfg(test)]
mod tests;

pub(crate) use file_event_store::FileEventStore;

use std::error::Error;
use std::fmt::{Display, Formatter};
use std::io;

use crate::conversation::{
    Conversation, ConversationEventBatch, ConversationEventRecord, ConversationId,
    InvalidConversation,
};

pub(crate) trait ConversationEventStore {
    fn load(&self, id: ConversationId) -> Result<Conversation, ConversationEventStoreError>;

    fn load_events(
        &self,
        id: ConversationId,
    ) -> Result<Vec<ConversationEventRecord>, ConversationEventStoreError>;

    fn latest_id(&self) -> Result<ConversationId, ConversationEventStoreError>;

    fn append(
        &self,
        id: ConversationId,
        events: ConversationEventBatch,
    ) -> Result<Vec<ConversationEventRecord>, ConversationEventStoreError>;
}

#[derive(Debug)]
pub(crate) enum ConversationEventStoreError {
    NoConversations,
    ConversationNotFound(ConversationId),
    ConversationMismatch {
        expected: ConversationId,
        found: ConversationId,
    },
    InvalidConversation(InvalidConversation),
    Storage(io::Error),
}

impl Display for ConversationEventStoreError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoConversations => write!(formatter, "no conversations found"),
            Self::ConversationNotFound(id) => write!(formatter, "no events found for {id}"),
            Self::ConversationMismatch { expected, found } => {
                write!(formatter, "loaded {found}, expected {expected}")
            }
            Self::InvalidConversation(error) => Display::fmt(error, formatter),
            Self::Storage(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for ConversationEventStoreError {}

impl From<io::Error> for ConversationEventStoreError {
    fn from(error: io::Error) -> Self {
        Self::Storage(error)
    }
}
