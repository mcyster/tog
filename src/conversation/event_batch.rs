use std::error::Error;
use std::fmt::{Display, Formatter};

use super::ConversationEvent;

pub(crate) struct ConversationEventBatch {
    events: Vec<ConversationEvent>,
}

impl ConversationEventBatch {
    pub(crate) fn into_events(self) -> Vec<ConversationEvent> {
        self.events
    }
}

impl TryFrom<Vec<ConversationEvent>> for ConversationEventBatch {
    type Error = EmptyConversationEventBatch;

    fn try_from(events: Vec<ConversationEvent>) -> Result<Self, Self::Error> {
        if events.is_empty() {
            return Err(EmptyConversationEventBatch);
        }
        Ok(Self { events })
    }
}

impl From<ConversationEvent> for ConversationEventBatch {
    fn from(event: ConversationEvent) -> Self {
        Self {
            events: vec![event],
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct EmptyConversationEventBatch;

impl Display for EmptyConversationEventBatch {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "a conversation event batch must not be empty")
    }
}

impl Error for EmptyConversationEventBatch {}

#[cfg(test)]
mod tests {
    use super::{ConversationEventBatch, EmptyConversationEventBatch};

    #[test]
    fn an_empty_batch_cannot_be_constructed() {
        assert_eq!(
            ConversationEventBatch::try_from(Vec::new()).err(),
            Some(EmptyConversationEventBatch)
        );
    }
}
