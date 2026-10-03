use std::fs;
use std::io;
use std::path::{Path, PathBuf};

mod log;

use tog::conversation::{ConversationEvent, ConversationId};
use tog::conversation_event_store::{
    ConversationEventRecord, ConversationEventStore, ConversationStoreAppendError,
    ConversationStoreError, ConversationStoreLoadError,
};
use tog_context::environment::data_directory;

use crate::private_directory;

const CONVERSATIONS_DIRECTORY_NAME: &str = "conversations";

pub struct FileEventStore {
    root_directory: PathBuf,
}

impl FileEventStore {
    pub fn new(root_directory: PathBuf) -> io::Result<Self> {
        private_directory::create(&root_directory)?;
        private_directory::create(&root_directory.join(CONVERSATIONS_DIRECTORY_NAME))?;
        Ok(Self { root_directory })
    }

    pub fn from_environment() -> io::Result<Self> {
        Self::new(data_directory()?)
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
                .is_none_or(|current| last_event.event().timestamp() > current.event().timestamp())
            {
                latest_event = Some(last_event);
            }
        }
        Ok(latest_event.map(|event| event.event().conversation_id()))
    }

    fn append(
        &self,
        conversation_id: ConversationId,
        events: Vec<ConversationEvent>,
    ) -> Result<Vec<ConversationEventRecord>, ConversationStoreAppendError> {
        if events.is_empty() {
            return Err(ConversationStoreAppendError::EmptyBatch);
        }
        let conversation_directory = self.conversation_directory(conversation_id);
        private_directory::create(&conversation_directory)?;
        let existing_log = log::read(&conversation_directory)?;
        let existing_events = existing_log
            .as_ref()
            .map(|log| log.events.as_slice())
            .unwrap_or_default();
        let previous_position = validate_existing_events(conversation_id, existing_events)?;
        let batch = build_event_batch(previous_position, events)?;
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
    Ok(existing_events.last().map(|event| event.position()))
}

fn ensure_records_belong_to(
    conversation_id: ConversationId,
    events: &[ConversationEventRecord],
) -> Result<(), ConversationStoreError> {
    for event in events {
        if event.event().conversation_id() != conversation_id {
            return Err(ConversationStoreError::CorruptData);
        }
    }
    Ok(())
}

fn build_event_batch(
    previous_position: Option<u64>,
    events: Vec<ConversationEvent>,
) -> io::Result<Vec<ConversationEventRecord>> {
    let first_position = next_position(previous_position)?;
    events
        .into_iter()
        .enumerate()
        .map(|(offset, event)| {
            let position = first_position
                .checked_add(u64::try_from(offset).map_err(io::Error::other)?)
                .ok_or_else(|| io::Error::other("event position overflow"))?;
            Ok(ConversationEventRecord::new(position, event))
        })
        .collect()
}

fn next_position(previous_position: Option<u64>) -> io::Result<u64> {
    match previous_position {
        Some(position) => position
            .checked_add(1)
            .ok_or_else(|| io::Error::other("event position overflow")),
        None => Ok(0),
    }
}
