use std::error::Error;
use std::fmt::{Display, Formatter};
use std::io;

use super::{ConversationStoreAppendError, ConversationStoreError, ConversationStoreLoadError};

impl Display for ConversationStoreLoadError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(id) => write!(formatter, "no events found for {id}"),
            Self::Store(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for ConversationStoreLoadError {}

impl From<ConversationStoreError> for ConversationStoreLoadError {
    fn from(error: ConversationStoreError) -> Self {
        Self::Store(error)
    }
}

impl From<io::Error> for ConversationStoreLoadError {
    fn from(error: io::Error) -> Self {
        Self::Store(error.into())
    }
}

impl Display for ConversationStoreAppendError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyBatch => write!(formatter, "an appended event batch must not be empty"),
            Self::Store(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for ConversationStoreAppendError {}

impl From<ConversationStoreError> for ConversationStoreAppendError {
    fn from(error: ConversationStoreError) -> Self {
        Self::Store(error)
    }
}

impl From<io::Error> for ConversationStoreAppendError {
    fn from(error: io::Error) -> Self {
        Self::Store(error.into())
    }
}

impl Display for ConversationStoreError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => Display::fmt(error, formatter),
            Self::CorruptData => write!(formatter, "corrupt conversation data"),
        }
    }
}

impl Error for ConversationStoreError {}

impl From<io::Error> for ConversationStoreError {
    fn from(error: io::Error) -> Self {
        if error.kind() == io::ErrorKind::InvalidData {
            Self::CorruptData
        } else {
            Self::Io(error)
        }
    }
}
