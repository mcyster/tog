mod error;
mod file_event_store;
mod log;
mod record;
#[cfg(test)]
mod tests;

pub(crate) use file_event_store::FileEventStore;

use std::io;

use serde::{Deserialize, Serialize};

use crate::conversation::{ConversationEvent, ConversationId};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct ConversationEventRecord {
    pub(crate) position: u64,
    pub(crate) schema_version: u32,
    #[serde(flatten)]
    pub(crate) event: ConversationEvent,
}

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
