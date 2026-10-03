use std::error::Error;
use std::fmt::{Display, Formatter};
use std::io;

use super::{AssetStoreError, AssetStoreLoadError};

impl Display for AssetStoreLoadError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(asset_id) => write!(formatter, "no asset found for {asset_id}"),
            Self::Store(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for AssetStoreLoadError {}

impl From<AssetStoreError> for AssetStoreLoadError {
    fn from(error: AssetStoreError) -> Self {
        Self::Store(error)
    }
}

impl From<io::Error> for AssetStoreLoadError {
    fn from(error: io::Error) -> Self {
        Self::Store(error.into())
    }
}

impl Display for AssetStoreError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => Display::fmt(error, formatter),
            Self::CorruptData => write!(formatter, "corrupt asset data"),
        }
    }
}

impl Error for AssetStoreError {}

impl From<io::Error> for AssetStoreError {
    fn from(error: io::Error) -> Self {
        if error.kind() == io::ErrorKind::InvalidData {
            Self::CorruptData
        } else {
            Self::Io(error)
        }
    }
}
