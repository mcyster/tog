mod file_event_store;
mod legacy;
mod log;
#[cfg(test)]
mod tests;

pub(crate) use file_event_store::FileEventStore;

use std::io;

use crate::conversation::{
    Conversation, ConversationEvent, ConversationEventRecord, ConversationId,
};

pub(crate) trait ConversationEventStore {
    /// Reconstructs the model-facing projection, excluding command, lifecycle,
    /// and extension records.
    ///
    /// Fails when the conversation has no committed records or its records are
    /// invalid.
    fn load(&self, id: ConversationId) -> io::Result<Conversation>;

    /// Returns every committed record, including the command, lifecycle, and
    /// extension records that `load` excludes.
    ///
    /// Fails when the conversation has no committed records.
    fn load_events(&self, id: ConversationId) -> io::Result<Vec<ConversationEventRecord>>;

    /// Returns the conversation with the newest committed record.
    ///
    /// Fails when no conversation has a committed record.
    fn latest_id(&self) -> io::Result<ConversationId>;

    /// Commits `events` as one atomic, durable batch and returns their records
    /// in order.
    ///
    /// Positions are assigned at the append boundary, and an empty batch is
    /// rejected.
    fn append(
        &self,
        id: ConversationId,
        events: Vec<ConversationEvent>,
    ) -> io::Result<Vec<ConversationEventRecord>>;
}
