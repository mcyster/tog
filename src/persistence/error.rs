use std::error::Error;
use std::fmt::{Display, Formatter};
use std::io;

use super::ConversationEventStoreError;

impl Display for ConversationEventStoreError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoConversations => write!(formatter, "no conversations found"),
            Self::ConversationNotFound(id) => write!(formatter, "no events found for {id}"),
            Self::ConversationMismatch { expected, found } => {
                write!(formatter, "loaded {found}, expected {expected}")
            }
            Self::EmptyBatch => write!(formatter, "an appended event batch must not be empty"),
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
