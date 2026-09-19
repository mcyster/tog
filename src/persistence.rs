mod file_event_store;
mod legacy;
mod log;
#[cfg(test)]
mod tests;

pub(crate) use file_event_store::FileEventStore;

use std::io;

use crate::conversation::{
    Conversation, ConversationEventBatch, ConversationEventRecord, ConversationId,
};

pub(crate) trait ConversationEventStore {
    fn load(&self, id: ConversationId) -> io::Result<Conversation>;

    fn load_events(&self, id: ConversationId) -> io::Result<Vec<ConversationEventRecord>>;

    fn latest_id(&self) -> io::Result<ConversationId>;

    fn append(
        &self,
        id: ConversationId,
        events: ConversationEventBatch,
    ) -> io::Result<Vec<ConversationEventRecord>>;
}
