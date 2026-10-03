mod error;
mod record;

use std::io;

use serde::{Deserialize, Serialize};

use crate::conversation::{ConversationEvent, ConversationId};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ConversationEventRecord {
    position: u64,
    schema_version: u32,
    #[serde(flatten)]
    event: ConversationEvent,
}

pub trait ConversationEventStore {
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
pub enum ConversationStoreLoadError {
    NotFound(ConversationId),
    Store(ConversationStoreError),
}

#[derive(Debug)]
pub enum ConversationStoreAppendError {
    EmptyBatch,
    Store(ConversationStoreError),
}

#[derive(Debug)]
pub enum ConversationStoreError {
    Io(io::Error),
    CorruptData,
}
