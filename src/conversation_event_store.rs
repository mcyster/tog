mod error;
mod file_event_store;
mod log;
#[cfg(test)]
mod tests;

pub(crate) use file_event_store::FileEventStore;

use std::io;

use crate::conversation::ConversationId;
use crate::conversation_event::{ConversationEvent, ConversationEventRecord};

pub(crate) trait ConversationEventStore {
    fn load(
        &self,
        id: ConversationId,
    ) -> Result<Vec<ConversationEventRecord>, ConversationStoreLoadError>;

    fn latest_id(&self) -> Result<Option<ConversationId>, ConversationStoreError>;

    fn append(
        &self,
        id: ConversationId,
        events: Vec<ConversationEvent>,
    ) -> Result<Vec<ConversationEventRecord>, ConversationStoreAppendError>;
}

#[derive(Debug)]
pub(crate) enum ConversationStoreLoadError {
    NotFound(ConversationId),
    Store(ConversationStoreError),
}

#[derive(Debug)]
pub(crate) enum ConversationStoreAppendError {
    EmptyBatch,
    Store(ConversationStoreError),
}

#[derive(Debug)]
pub(crate) enum ConversationStoreError {
    Io(io::Error),
    CorruptData,
}
