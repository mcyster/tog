mod error;
mod file_event_store;
mod legacy;
mod log;
#[cfg(test)]
mod tests;

pub(crate) use file_event_store::FileEventStore;

use std::io;

use crate::conversation::{ConversationEvent, ConversationEventRecord, ConversationId};

pub(crate) trait ConversationEventStore {
    fn load(
        &self,
        id: ConversationId,
    ) -> Result<Vec<ConversationEventRecord>, ConversationEventStoreError>;

    fn latest_id(&self) -> Result<ConversationId, ConversationEventStoreError>;

    fn append(
        &self,
        id: ConversationId,
        events: Vec<ConversationEvent>,
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
    EmptyBatch,
    Storage(io::Error),
}
