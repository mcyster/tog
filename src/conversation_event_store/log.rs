use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::conversation_event::ConversationEventRecord;

const EVENT_LOG_FILE_NAME: &str = "events.log";
const CRC32_IEEE_POLYNOMIAL: u32 = 0xedb8_8320;

pub(super) struct ConversationLog {
    pub(super) events: Vec<ConversationEventRecord>,
    pub(super) committed_length: u64,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "transaction", rename_all = "snake_case")]
enum ConversationLogMarker {
    Begin,
    Commit(ConversationEventBatchCommit),
}

#[derive(Deserialize, Serialize)]
struct ConversationEventBatchCommit {
    event_count: u64,
    first_position: u64,
    last_position: u64,
    crc: String,
}

pub(super) fn log_path(conversation_directory: &Path) -> PathBuf {
    conversation_directory.join(EVENT_LOG_FILE_NAME)
}

pub(super) fn read(conversation_directory: &Path) -> io::Result<Option<ConversationLog>> {
    let mut file = match File::open(log_path(conversation_directory)) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let log = decode(&bytes)?;
    ensure_contiguous_positions(&log.events)?;
    Ok(Some(log))
}

pub(super) fn encode_batch(events: &[ConversationEventRecord]) -> io::Result<Vec<u8>> {
    let (Some(first_event), Some(last_event)) = (events.first(), events.last()) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "an encoded event batch must not be empty",
        ));
    };
    let mut event_lines = Vec::new();
    for event in events {
        serde_json::to_writer(&mut event_lines, event).map_err(io::Error::other)?;
        event_lines.push(b'\n');
    }
    let commit = ConversationEventBatchCommit {
        event_count: u64::try_from(events.len()).map_err(io::Error::other)?,
        first_position: first_event.position,
        last_position: last_event.position,
        crc: format!("{:08x}", crc32(&event_lines)),
    };
    let mut bytes = Vec::new();
    serde_json::to_writer(&mut bytes, &ConversationLogMarker::Begin).map_err(io::Error::other)?;
    bytes.push(b'\n');
    bytes.extend_from_slice(&event_lines);
    serde_json::to_writer(&mut bytes, &ConversationLogMarker::Commit(commit))
        .map_err(io::Error::other)?;
    bytes.push(b'\n');
    Ok(bytes)
}

pub(super) fn append(
    conversation_directory: &Path,
    committed_length: u64,
    batch_bytes: &[u8],
) -> io::Result<()> {
    let mut log_file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(log_path(conversation_directory))?;
    log_file.set_len(committed_length)?;
    log_file.seek(SeekFrom::Start(committed_length))?;
    log_file.write_all(batch_bytes)?;
    log_file.sync_all()
}

pub(super) fn create(conversation_directory: &Path, batch_bytes: &[u8]) -> io::Result<()> {
    let path = log_path(conversation_directory);
    let parent_directory = path
        .parent()
        .ok_or_else(|| io::Error::other("persisted file has no parent directory"))?;
    let mut log_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?;
    log_file.write_all(batch_bytes)?;
    log_file.sync_all()?;
    File::open(parent_directory)?.sync_all()
}

pub(super) fn crc32(bytes: &[u8]) -> u32 {
    let mut remainder = 0xffff_ffff_u32;
    for byte in bytes {
        remainder ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = (remainder & 1).wrapping_neg();
            remainder = (remainder >> 1) ^ (CRC32_IEEE_POLYNOMIAL & mask);
        }
    }
    !remainder
}

pub(super) fn ensure_contiguous_positions(events: &[ConversationEventRecord]) -> io::Result<()> {
    for (expected_position, event) in (0_u64..).zip(events) {
        if event.position != expected_position {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "expected conversation event position {expected_position}, found {}",
                    event.position
                ),
            ));
        }
    }
    Ok(())
}

fn decode(bytes: &[u8]) -> io::Result<ConversationLog> {
    let mut events = Vec::new();
    let mut pending_events: Vec<ConversationEventRecord> = Vec::new();
    let mut pending_bytes: Vec<u8> = Vec::new();
    let mut transaction_open = false;
    let mut committed_length = 0_u64;
    let mut offset = 0_usize;
    while let Some(line_length) = bytes[offset..].iter().position(|byte| *byte == b'\n') {
        let line_end = offset + line_length;
        let line = &bytes[offset..line_end];
        let line_bytes = &bytes[offset..=line_end];
        let value: Value = serde_json::from_slice(line).map_err(|error| {
            corruption(format!(
                "conversation log line at byte {offset} is not valid JSON: {error}"
            ))
        })?;
        if value.get("transaction").is_some() {
            let marker: ConversationLogMarker = serde_json::from_value(value).map_err(|error| {
                corruption(format!(
                    "conversation log marker at byte {offset} is invalid: {error}"
                ))
            })?;
            match marker {
                ConversationLogMarker::Begin => {
                    if transaction_open {
                        return Err(corruption(format!(
                            "conversation log line at byte {offset} opens a transaction while one is open"
                        )));
                    }
                    transaction_open = true;
                }
                ConversationLogMarker::Commit(commit) => {
                    if !transaction_open {
                        return Err(corruption(format!(
                            "conversation log line at byte {offset} commits without an open transaction"
                        )));
                    }
                    validate_batch_commit(&commit, &pending_events, &pending_bytes)?;
                    events.append(&mut pending_events);
                    pending_bytes.clear();
                    transaction_open = false;
                    committed_length = u64::try_from(line_end + 1).map_err(io::Error::other)?;
                }
            }
        } else {
            if !transaction_open {
                return Err(corruption(format!(
                    "conversation log event at byte {offset} is outside a transaction"
                )));
            }
            let event: ConversationEventRecord =
                serde_json::from_value(value).map_err(|error| {
                    corruption(format!(
                        "conversation log event at byte {offset} is invalid: {error}"
                    ))
                })?;
            pending_events.push(event);
            pending_bytes.extend_from_slice(line_bytes);
        }
        offset = line_end + 1;
    }
    Ok(ConversationLog {
        events,
        committed_length,
    })
}

fn validate_batch_commit(
    commit: &ConversationEventBatchCommit,
    pending_events: &[ConversationEventRecord],
    pending_bytes: &[u8],
) -> io::Result<()> {
    let event_count = u64::try_from(pending_events.len()).map_err(io::Error::other)?;
    let first_position = pending_events.first().map(|event| event.position);
    let last_position = pending_events.last().map(|event| event.position);
    let expected_crc = format!("{:08x}", crc32(pending_bytes));
    if commit.event_count == 0
        || commit.event_count != event_count
        || Some(commit.first_position) != first_position
        || Some(commit.last_position) != last_position
        || commit.crc != expected_crc
    {
        return Err(corruption(
            "a conversation log commit marker does not match its transaction",
        ));
    }
    Ok(())
}

fn corruption(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}
