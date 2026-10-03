mod asset;
mod error;

use std::io::{self, Read};

pub use asset::{AssetId, AssetMetadata, MimeType};

pub trait AssetStore {
    fn add(
        &self,
        name: String,
        mime_type: MimeType,
        content: Box<dyn Read>,
    ) -> Result<AssetId, AssetStoreError>;

    fn metadata(&self, asset_id: AssetId) -> Result<AssetMetadata, AssetStoreLoadError>;

    #[allow(dead_code)]
    fn read(&self, asset_id: AssetId) -> Result<Box<dyn Read>, AssetStoreLoadError>;

    fn list(&self) -> Result<Vec<AssetMetadata>, AssetStoreError>;
}

#[derive(Debug)]
pub enum AssetStoreError {
    Io(io::Error),
    CorruptData,
}

#[derive(Debug)]
pub enum AssetStoreLoadError {
    NotFound(AssetId),
    Store(AssetStoreError),
}
