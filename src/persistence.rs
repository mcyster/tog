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

/// Durable storage for conversation event batches.
pub(crate) trait EventStore {
    /// Reconstructs the model-facing conversation from its committed records.
    ///
    /// The projection contains the accepted semantic history only. Command,
    /// turn lifecycle, and driver extension records stay in the log and are
    /// excluded from it.
    ///
    /// Fails when the conversation has no committed records, mixes conversation
    /// identifiers, or contains an invalid record.
    fn load_conversation(&self, conversation_id: ConversationId) -> io::Result<Conversation>;

    /// Returns every committed record for the conversation in position order.
    ///
    /// Unlike [`EventStore::load_conversation`], the result includes command,
    /// lifecycle, and driver extension records. Consumers use it to inspect or
    /// present the complete durable log rather than the model-facing history.
    ///
    /// Fails when the conversation has no committed records.
    fn load_conversation_log(
        &self,
        conversation_id: ConversationId,
    ) -> io::Result<Vec<ConversationEventRecord>>;

    /// Returns the identifier of the conversation whose latest committed
    /// record carries the newest timestamp.
    ///
    /// Fails when no conversation has a committed record.
    fn latest_conversation_id(&self) -> io::Result<ConversationId>;

    /// Appends `events` as one atomic batch and returns their committed records
    /// in the order supplied.
    ///
    /// Positions are assigned at the append boundary. An empty batch is
    /// rejected. Readers observe either every record in the batch or none of
    /// it, and a successful append is durable before it returns.
    fn append_new_conversation_events(
        &self,
        conversation_id: ConversationId,
        events: Vec<ConversationEvent>,
    ) -> io::Result<Vec<ConversationEventRecord>>;
}
