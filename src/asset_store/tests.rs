use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use super::{AssetStore, AssetStoreLoadError, FileAssetStore};
use crate::asset::{AssetId, AssetName, MimeType};

fn temporary_store() -> FileAssetStore {
    let directory = std::env::temp_dir().join(format!("tog-asset-test-{}", uuid::Uuid::now_v7()));
    FileAssetStore::new(directory).expect("the asset store should be created")
}

fn write_source(directory: &Path, file_name: &str, bytes: &[u8]) -> PathBuf {
    std::fs::create_dir_all(directory).expect("the source directory should be created");
    let path = directory.join(file_name);
    std::fs::write(&path, bytes).expect("the source file should be written");
    path
}

fn content_reader(source_path: &Path) -> Box<dyn Read> {
    let file = File::open(source_path).expect("the source file should open");
    Box::new(file) as Box<dyn Read>
}

fn asset_name(name: &str) -> AssetName {
    AssetName::from_str(name).expect("the asset name should be valid")
}

fn mime_type(mime_type: &str) -> MimeType {
    MimeType::from_str(mime_type).expect("the mime type should be valid")
}

fn read_all(mut content: Box<dyn Read>) -> Vec<u8> {
    let mut bytes = Vec::new();
    content
        .read_to_end(&mut bytes)
        .expect("the asset content should be readable");
    bytes
}

fn source_directory() -> PathBuf {
    std::env::temp_dir().join(format!("tog-asset-source-{}", uuid::Uuid::now_v7()))
}

#[test]
fn adding_an_asset_round_trips_its_metadata_and_content() {
    let store = temporary_store();
    let source_path = write_source(&source_directory(), "hello.txt", b"hello asset");

    let asset_id = store
        .add(
            asset_name("hello.txt"),
            mime_type("text/plain"),
            content_reader(&source_path),
        )
        .expect("the asset should be added");

    let metadata = store
        .metadata(asset_id)
        .expect("the asset metadata should load");
    assert_eq!(metadata.id(), asset_id);
    assert_eq!(metadata.name().as_str(), "hello.txt");
    assert_eq!(metadata.mime_type().as_str(), "text/plain");
    assert_eq!(metadata.byte_size(), b"hello asset".len() as u64);
    assert_eq!(
        read_all(
            store
                .open_content(asset_id)
                .expect("the asset content should open")
        ),
        b"hello asset"
    );
}

#[test]
fn asset_store_preserves_assets_across_reopen() {
    let directory = std::env::temp_dir().join(format!("tog-asset-test-{}", uuid::Uuid::now_v7()));
    let store = FileAssetStore::new(directory.clone()).expect("the asset store should be created");
    let source_path = write_source(&source_directory(), "notes.txt", b"persisted bytes");
    let asset_id = store
        .add(
            asset_name("notes.txt"),
            mime_type("text/plain"),
            content_reader(&source_path),
        )
        .expect("the asset should be added");

    let reopened = FileAssetStore::new(directory).expect("the asset store should reopen");
    let metadata = reopened
        .metadata(asset_id)
        .expect("the asset metadata should load after reopen");
    assert_eq!(metadata.name().as_str(), "notes.txt");
    assert_eq!(metadata.mime_type().as_str(), "text/plain");
    assert_eq!(metadata.byte_size(), b"persisted bytes".len() as u64);
    assert_eq!(
        read_all(
            reopened
                .open_content(asset_id)
                .expect("the asset content should open")
        ),
        b"persisted bytes"
    );
}

#[test]
fn stored_asset_content_survives_source_changes() {
    let store = temporary_store();
    let source_directory = source_directory();
    let source_path = write_source(&source_directory, "mutable.txt", b"original");
    let asset_id = store
        .add(
            asset_name("mutable.txt"),
            mime_type("text/plain"),
            content_reader(&source_path),
        )
        .expect("the asset should be added");
    std::fs::write(&source_path, b"changed").expect("the source file should change");

    assert_eq!(
        read_all(
            store
                .open_content(asset_id)
                .expect("the asset content should open")
        ),
        b"original"
    );
}

#[test]
fn asset_store_allows_duplicate_names() {
    let store = temporary_store();
    let source_directory = source_directory();
    let first_path = write_source(&source_directory, "same.txt", b"first");
    let second_path = write_source(&source_directory, "same-again.txt", b"first");
    let first_id = store
        .add(
            asset_name("same.txt"),
            mime_type("text/plain"),
            content_reader(&first_path),
        )
        .expect("the first asset should be added");
    let second_id = store
        .add(
            asset_name("same.txt"),
            mime_type("text/plain"),
            content_reader(&second_path),
        )
        .expect("the second asset should be added");

    assert_ne!(first_id, second_id);
    let metadatas = store.list().expect("the assets should list");
    assert_eq!(
        metadatas
            .iter()
            .filter(|metadata| metadata.name().as_str() == "same.txt")
            .count(),
        2
    );
}

#[test]
fn asset_store_reports_missing_assets() {
    let store = temporary_store();

    let missing_id = AssetId::new();
    assert!(matches!(
        store.metadata(missing_id),
        Err(AssetStoreLoadError::NotFound(_))
    ));
    assert!(matches!(
        store.open_content(missing_id),
        Err(AssetStoreLoadError::NotFound(_))
    ));
}

#[test]
fn asset_store_rejects_an_incomplete_staging_write_in_listings() {
    let store = temporary_store();
    let source_path = write_source(&source_directory(), "committed.txt", b"committed");
    let asset_id = store
        .add(
            asset_name("committed.txt"),
            mime_type("text/plain"),
            content_reader(&source_path),
        )
        .expect("the committed asset should be added");
    let assets_directory = store.asset_directory(asset_id);
    let assets_parent = assets_directory
        .parent()
        .expect("the assets directory should exist");
    let staging_name = format!(".tmp-{}", uuid::Uuid::now_v7().simple());
    let staging_directory = assets_parent.join(&staging_name);
    std::fs::create_dir_all(&staging_directory).expect("the staging directory should be created");
    std::fs::write(staging_directory.join("content"), b"partial")
        .expect("the partial content should be written");

    let metadatas = store.list().expect("the assets should list");
    assert_eq!(metadatas.len(), 1);
    assert_eq!(metadatas[0].id(), asset_id);
}

#[test]
fn asset_store_discards_corrupt_metadata_as_corruption() {
    let store = temporary_store();
    let asset_id = AssetId::new();
    let asset_directory = store.asset_directory(asset_id);
    std::fs::create_dir_all(&asset_directory).expect("the asset directory should be created");
    std::fs::write(asset_directory.join("metadata.json"), b"not json")
        .expect("the corrupt metadata should be written");

    assert!(matches!(
        store.metadata(asset_id),
        Err(AssetStoreLoadError::Store(
            super::AssetStoreError::CorruptData
        ))
    ));
    let error = store
        .list()
        .expect_err("corrupt metadata should fail the listing");
    assert!(matches!(error, super::AssetStoreError::CorruptData));
}
