use std::fs;
use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use tog::asset_store::{
    AssetId, AssetMetadata, AssetStore, AssetStoreError, AssetStoreLoadError, MimeType,
};
use tog_context::environment::data_directory;

use crate::private_directory;

const ASSETS_DIRECTORY_NAME: &str = "assets";
const METADATA_FILE_NAME: &str = "metadata.json";
const CONTENT_FILE_NAME: &str = "content";

pub struct FileAssetStore {
    root_directory: PathBuf,
}

impl FileAssetStore {
    pub fn new(root_directory: PathBuf) -> io::Result<Self> {
        private_directory::create(&root_directory)?;
        private_directory::create(&root_directory.join(ASSETS_DIRECTORY_NAME))?;
        Ok(Self { root_directory })
    }

    pub fn from_environment() -> io::Result<Self> {
        Self::new(data_directory()?)
    }

    pub(super) fn asset_directory(&self, asset_id: AssetId) -> PathBuf {
        self.root_directory
            .join(ASSETS_DIRECTORY_NAME)
            .join(asset_id.storage_key())
    }
}

impl AssetStore for FileAssetStore {
    fn add(
        &self,
        name: String,
        mime_type: MimeType,
        content: Box<dyn Read>,
    ) -> Result<AssetId, AssetStoreError> {
        let asset_id = AssetId::new();
        let assets_directory = self.root_directory.join(ASSETS_DIRECTORY_NAME);
        let staging_directory =
            assets_directory.join(format!(".tmp-{}", uuid::Uuid::now_v7().simple()));
        let asset_directory = self.asset_directory(asset_id);

        let write_outcome =
            write_staged_asset(&staging_directory, asset_id, name, mime_type, content);
        if let Err(error) = write_outcome {
            fs::remove_dir_all(&staging_directory).ok();
            return Err(error);
        }

        fs::rename(&staging_directory, &asset_directory).map_err(AssetStoreError::from)?;
        File::open(&assets_directory)?.sync_all()?;
        Ok(asset_id)
    }

    fn metadata(&self, asset_id: AssetId) -> Result<AssetMetadata, AssetStoreLoadError> {
        let Some(metadata) = read_metadata(&self.asset_directory(asset_id), asset_id)? else {
            return Err(AssetStoreLoadError::NotFound(asset_id));
        };
        Ok(metadata)
    }

    fn read(&self, asset_id: AssetId) -> Result<Box<dyn Read>, AssetStoreLoadError> {
        self.metadata(asset_id)?;
        let content_path = self.asset_directory(asset_id).join(CONTENT_FILE_NAME);
        match File::open(&content_path) {
            Ok(content) => Ok(Box::new(content) as Box<dyn Read>),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                Err(AssetStoreLoadError::Store(AssetStoreError::CorruptData))
            }
            Err(error) => Err(AssetStoreLoadError::from(error)),
        }
    }

    fn list(&self) -> Result<Vec<AssetMetadata>, AssetStoreError> {
        let assets_directory = self.root_directory.join(ASSETS_DIRECTORY_NAME);
        let mut metadatas = Vec::new();
        for directory_entry in fs::read_dir(&assets_directory)? {
            let directory_entry = directory_entry?;
            if !directory_entry.file_type()?.is_dir() {
                continue;
            }
            let Some(asset_id) = directory_asset_id(&directory_entry.path()) else {
                continue;
            };
            let Some(metadata) = read_metadata(&self.asset_directory(asset_id), asset_id)? else {
                continue;
            };
            metadatas.push(metadata);
        }
        metadatas.sort_unstable_by_key(|metadata| metadata.id().storage_key());
        Ok(metadatas)
    }
}

fn write_staged_asset(
    staging_directory: &Path,
    asset_id: AssetId,
    name: String,
    mime_type: MimeType,
    content: Box<dyn Read>,
) -> Result<(), AssetStoreError> {
    let mut write_back = content;
    private_directory::create(staging_directory)?;
    let mut content_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(staging_directory.join(CONTENT_FILE_NAME))?;
    let byte_size = io::copy(&mut write_back, &mut content_file)?;
    content_file.sync_all()?;
    let metadata = AssetMetadata::new(asset_id, name, mime_type, byte_size);
    write_metadata_file(&staging_directory.join(METADATA_FILE_NAME), &metadata)?;
    File::open(staging_directory)?.sync_all()?;
    Ok(())
}

fn write_metadata_file(metadata_path: &Path, metadata: &AssetMetadata) -> io::Result<()> {
    let mut metadata_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(metadata_path)?;
    serde_json::to_writer(&mut metadata_file, metadata).map_err(io::Error::other)?;
    metadata_file.sync_all()
}

fn read_metadata(
    asset_directory: &Path,
    expected_asset_id: AssetId,
) -> Result<Option<AssetMetadata>, AssetStoreError> {
    let metadata_path = asset_directory.join(METADATA_FILE_NAME);
    let bytes = match fs::read(&metadata_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(AssetStoreError::from(error)),
    };
    let metadata = serde_json::from_slice::<AssetMetadata>(&bytes)
        .map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid asset metadata: {error}"),
            )
        })
        .map_err(AssetStoreError::from)?;
    if metadata.id() != expected_asset_id {
        return Err(AssetStoreError::CorruptData);
    }
    Ok(Some(metadata))
}

fn directory_asset_id(directory_path: &Path) -> Option<AssetId> {
    directory_path
        .file_name()
        .and_then(|file_name| file_name.to_str())
        .and_then(|file_name_text| AssetId::from_str(file_name_text).ok())
}
