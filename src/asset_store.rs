mod asset;
mod error;
mod file_asset_store;
#[cfg(test)]
mod tests;

use std::io::{self, Read};

pub(crate) use asset::{AssetId, AssetMetadata, MimeType};
pub(crate) use file_asset_store::FileAssetStore;

pub(crate) trait AssetStore {
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
pub(crate) enum AssetStoreError {
    Io(io::Error),
    CorruptData,
}

#[derive(Debug)]
pub(crate) enum AssetStoreLoadError {
    NotFound(AssetId),
    Store(AssetStoreError),
}
