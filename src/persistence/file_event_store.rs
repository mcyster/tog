use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use super::log;
use super::{
    ConversationEventStore, ConversationStoreAppendError, ConversationStoreError,
    ConversationStoreLoadError,
};
use crate::conversation::{
    ConversationEvent, ConversationEventKind, ConversationEventRecord, ConversationId,
    StoredConversationEventKind,
};

const CONVERSATIONS_DIRECTORY_NAME: &str = "conversations";

pub(crate) struct FileEventStore {
    root_directory: PathBuf,
}

impl FileEventStore {
    pub(crate) fn new(root_directory: PathBuf) -> io::Result<Self> {
        create_private_directory(&root_directory)?;
        create_private_directory(&root_directory.join(CONVERSATIONS_DIRECTORY_NAME))?;
        Ok(Self { root_directory })
    }

    pub(crate) fn from_environment() -> io::Result<Self> {
        let root_directory = if let Some(configured_directory) = std::env::var_os("TOG_DATA_DIR") {
            PathBuf::from(configured_directory)
        } else if let Some(data_home) = std::env::var_os("XDG_DATA_HOME") {
            PathBuf::from(data_home).join("tog")
        } else if let Some(home_directory) = std::env::var_os("HOME") {
            PathBuf::from(home_directory).join(".local/share/tog")
        } else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "TOG_DATA_DIR, XDG_DATA_HOME, or HOME must be set",
            ));
        };

        Self::new(root_directory)
    }

    pub(super) fn conversation_directory(&self, conversation_id: ConversationId) -> PathBuf {
        self.root_directory
            .join(CONVERSATIONS_DIRECTORY_NAME)
            .join(conversation_id.storage_key())
    }
}

impl ConversationEventStore for FileEventStore {
    fn load(
        &self,
        conversation_id: ConversationId,
    ) -> Result<Vec<ConversationEventRecord>, ConversationStoreLoadError> {
        let conversation_directory = self.conversation_directory(conversation_id);
        let Some(log) = log::read(&conversation_directory)? else {
            return Err(ConversationStoreLoadError::NotFound(conversation_id));
        };
        ensure_records_belong_to(conversation_id, &log.events)?;
        Ok(log.events)
    }

    fn latest_id(&self) -> Result<Option<ConversationId>, ConversationStoreError> {
        let conversations_directory = self.root_directory.join(CONVERSATIONS_DIRECTORY_NAME);
        let mut latest_event: Option<ConversationEventRecord> = None;
        for directory_entry in fs::read_dir(&conversations_directory)? {
            let directory_entry = directory_entry?;
            if !directory_entry.file_type()?.is_dir() {
                continue;
            }
            let Some(last_event) = read_last_event(&directory_entry.path())? else {
                continue;
            };
            if latest_event
                .as_ref()
                .is_none_or(|current| last_event.timestamp > current.timestamp)
            {
                latest_event = Some(last_event);
            }
        }
        Ok(latest_event.map(|event| event.conversation_id))
    }

    fn append(
        &self,
        conversation_id: ConversationId,
        events: Vec<ConversationEvent>,
    ) -> Result<Vec<ConversationEventRecord>, ConversationStoreAppendError> {
        let kinds = stored_event_kinds(events)?;
        let conversation_directory = self.conversation_directory(conversation_id);
        create_private_directory(&conversation_directory)?;
        let existing_log = log::read(&conversation_directory)?;
        let existing_events = existing_log
            .as_ref()
            .map(|log| log.events.as_slice())
            .unwrap_or_default();
        let previous_position = validate_existing_events(conversation_id, existing_events)?;
        let batch = build_event_batch(conversation_id, previous_position, kinds)?;
        commit_batch(&conversation_directory, existing_log, &batch)?;
        Ok(batch)
    }
}

fn commit_batch(
    conversation_directory: &Path,
    existing_log: Option<log::ConversationLog>,
    batch: &[ConversationEventRecord],
) -> io::Result<()> {
    let batch_bytes = log::encode_batch(batch)?;
    match existing_log {
        Some(log) => log::append(conversation_directory, log.committed_length, &batch_bytes),
        None => log::create(conversation_directory, &batch_bytes),
    }
}

fn read_last_event(conversation_directory: &Path) -> io::Result<Option<ConversationEventRecord>> {
    Ok(log::read(conversation_directory)?.and_then(|log| log.events.into_iter().last()))
}

fn validate_existing_events(
    conversation_id: ConversationId,
    existing_events: &[ConversationEventRecord],
) -> Result<Option<u64>, ConversationStoreError> {
    if existing_events.is_empty() {
        return Ok(None);
    }
    ensure_records_belong_to(conversation_id, existing_events)?;
    log::ensure_contiguous_positions(existing_events)?;
    Ok(existing_events.last().map(|event| event.position))
}

fn ensure_records_belong_to(
    conversation_id: ConversationId,
    events: &[ConversationEventRecord],
) -> Result<(), ConversationStoreError> {
    for event in events {
        if event.conversation_id != conversation_id {
            return Err(ConversationStoreError::CorruptData);
        }
    }
    Ok(())
}

fn build_event_batch(
    conversation_id: ConversationId,
    previous_position: Option<u64>,
    kinds: Vec<StoredConversationEventKind>,
) -> io::Result<Vec<ConversationEventRecord>> {
    let first_position = next_position(previous_position)?;
    kinds
        .into_iter()
        .enumerate()
        .map(|(offset, kind)| {
            let position = first_position
                .checked_add(u64::try_from(offset).map_err(io::Error::other)?)
                .ok_or_else(|| io::Error::other("event position overflow"))?;
            Ok(match kind {
                StoredConversationEventKind::Shared(kind) => {
                    ConversationEventRecord::new(conversation_id, position, kind)
                }
                StoredConversationEventKind::Extension(event) => {
                    ConversationEventRecord::new_extension(conversation_id, position, event)
                }
            })
        })
        .collect()
}

fn stored_event_kinds(
    events: Vec<ConversationEvent>,
) -> Result<Vec<StoredConversationEventKind>, ConversationStoreAppendError> {
    if events.is_empty() {
        return Err(ConversationStoreAppendError::EmptyBatch);
    }
    events
        .into_iter()
        .map(stored_kind)
        .collect::<io::Result<Vec<_>>>()
        .map_err(ConversationStoreAppendError::from)
}

fn stored_kind(event: ConversationEvent) -> io::Result<StoredConversationEventKind> {
    match event {
        ConversationEvent::Command(command) => Ok(StoredConversationEventKind::Shared(
            ConversationEventKind::Command(command),
        )),
        ConversationEvent::Fact(fact) => Ok(StoredConversationEventKind::Shared(
            ConversationEventKind::Fact(fact),
        )),
        ConversationEvent::Extension(event) => event
            .to_envelope()
            .map(StoredConversationEventKind::Extension)
            .map_err(io::Error::other),
    }
}

fn next_position(previous_position: Option<u64>) -> io::Result<u64> {
    match previous_position {
        Some(position) => position
            .checked_add(1)
            .ok_or_else(|| io::Error::other("event position overflow")),
        None => Ok(0),
    }
}

fn create_private_directory(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}
