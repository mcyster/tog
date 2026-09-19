use std::fs::{self, File};
use std::io::{self, BufReader};
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;

use super::log::ensure_contiguous_positions;
use crate::conversation::ConversationEventRecord;

const LEGACY_EVENTS_DIRECTORY_NAME: &str = "events";

pub(super) fn events_directory(conversation_directory: &Path) -> PathBuf {
    conversation_directory.join(LEGACY_EVENTS_DIRECTORY_NAME)
}

pub(super) fn read_events(
    conversation_directory: &Path,
) -> io::Result<Option<Vec<ConversationEventRecord>>> {
    let mut events = match read_json_directory::<ConversationEventRecord>(&events_directory(
        conversation_directory,
    )) {
        Ok(events) => events,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    events.sort_by_key(|event: &ConversationEventRecord| event.position);
    ensure_contiguous_positions(&events)?;
    Ok(Some(events))
}

pub(super) fn read_last_event(
    conversation_directory: &Path,
) -> io::Result<Option<ConversationEventRecord>> {
    let directory_entries = match fs::read_dir(events_directory(conversation_directory)) {
        Ok(directory_entries) => directory_entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut last_path: Option<PathBuf> = None;
    for directory_entry in directory_entries {
        let path = directory_entry?.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "json")
            && last_path.as_ref().is_none_or(|current| path > *current)
        {
            last_path = Some(path);
        }
    }
    let Some(last_path) = last_path else {
        return Ok(None);
    };
    let file = File::open(last_path)?;
    match serde_json::from_reader::<_, ConversationEventRecord>(BufReader::new(file)) {
        Ok(event) => Ok(Some(event)),
        Err(_) => Ok(None),
    }
}

pub(super) fn remove(conversation_directory: &Path) {
    let _ = fs::remove_dir_all(events_directory(conversation_directory));
}

fn read_json<T: DeserializeOwned>(path: &Path) -> io::Result<T> {
    let file = File::open(path)?;
    serde_json::from_reader(BufReader::new(file))
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn read_json_directory<T: DeserializeOwned>(directory: &Path) -> io::Result<Vec<T>> {
    let mut values = Vec::new();
    for directory_entry in fs::read_dir(directory)? {
        let directory_entry = directory_entry?;
        let path = directory_entry.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            values.push(read_json(&path)?);
        }
    }
    Ok(values)
}
