mod error;
mod file_asset_store;
#[cfg(test)]
mod tests;

pub(crate) use file_asset_store::FileAssetStore;

use std::io::{self, Read};

use crate::asset::{AssetId, AssetMetadata, AssetName, MimeType};

pub(crate) trait AssetStore {
    fn add(
        &self,
        name: AssetName,
        mime_type: MimeType,
        content: Box<dyn Read>,
    ) -> Result<AssetId, AssetStoreError>;

    fn metadata(&self, asset_id: AssetId) -> Result<AssetMetadata, AssetStoreLoadError>;

    #[allow(dead_code)]
    fn open_content(&self, asset_id: AssetId) -> Result<Box<dyn Read>, AssetStoreLoadError>;

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
