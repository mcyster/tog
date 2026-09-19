use std::error::Error;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::conversation::{
    Conversation, ConversationEvent, ConversationEventKind, ConversationEventRecord,
    ConversationId, StoredConversationEventKind,
};

const CONVERSATIONS_DIRECTORY_NAME: &str = "conversations";
const EVENT_LOG_FILE_NAME: &str = "events.log";
const LEGACY_EVENTS_DIRECTORY_NAME: &str = "events";
const CRC32_IEEE_POLYNOMIAL: u32 = 0xedb8_8320;

pub(crate) struct EventStore {
    root_directory: PathBuf,
}

impl EventStore {
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

    pub(crate) fn new(root_directory: PathBuf) -> io::Result<Self> {
        create_private_directory(&root_directory)?;
        create_private_directory(&root_directory.join(CONVERSATIONS_DIRECTORY_NAME))?;
        Ok(Self { root_directory })
    }

    pub(crate) fn load_conversation(
        &self,
        conversation_id: ConversationId,
    ) -> io::Result<Conversation> {
        let events = self.load_conversation_log(conversation_id)?;
        let conversation = Conversation::from_events(events).map_err(invalid_conversation_data)?;
        if conversation.id() != conversation_id {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("loaded {}, expected {conversation_id}", conversation.id()),
            ));
        }
        Ok(conversation)
    }

    pub(crate) fn load_conversation_log(
        &self,
        conversation_id: ConversationId,
    ) -> io::Result<Vec<ConversationEventRecord>> {
        self.load_conversation_events(conversation_id)
    }

    pub(crate) fn latest_conversation_id(&self) -> io::Result<ConversationId> {
        let conversations_directory = self.root_directory.join(CONVERSATIONS_DIRECTORY_NAME);
        let mut latest_event: Option<ConversationEventRecord> = None;
        for directory_entry in fs::read_dir(&conversations_directory)? {
            let directory_entry = directory_entry?;
            if !directory_entry.file_type()?.is_dir() {
                continue;
            }
            let Some(last_event) = read_last_conversation_event(&directory_entry.path())? else {
                continue;
            };
            if latest_event
                .as_ref()
                .is_none_or(|current| last_event.timestamp > current.timestamp)
            {
                latest_event = Some(last_event);
            }
        }
        latest_event
            .map(|event| event.conversation_id)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no conversations found"))
    }

    pub(crate) fn append_new_conversation_events(
        &self,
        conversation_id: ConversationId,
        events: Vec<ConversationEvent>,
    ) -> io::Result<Vec<ConversationEventRecord>> {
        if events.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "an appended event batch must not be empty",
            ));
        }
        let kinds = events
            .into_iter()
            .map(stored_kind)
            .collect::<io::Result<Vec<_>>>()?;
        let conversation_directory = self.conversation_directory(conversation_id);
        create_private_directory(&conversation_directory)?;
        let log_path = conversation_directory.join(EVENT_LOG_FILE_NAME);
        let log = read_conversation_log(&log_path)?;
        let log_exists = log.is_some();
        let legacy_directory = conversation_directory.join(LEGACY_EVENTS_DIRECTORY_NAME);
        let (existing_events, committed_length, migration_prefix) = match log {
            Some(log) => (log.events, log.committed_length, None),
            None => match read_legacy_events(&legacy_directory)? {
                Some(legacy_events) => {
                    let prefix = if legacy_events.is_empty() {
                        None
                    } else {
                        Some(encode_event_batch(&legacy_events)?)
                    };
                    (legacy_events, 0, prefix)
                }
                None => (Vec::new(), 0, None),
            },
        };
        let previous_position = validate_existing_events(conversation_id, &existing_events)?;
        let first_position = next_position(previous_position)?;
        let batch = kinds
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
            .collect::<io::Result<Vec<_>>>()?;
        let batch_bytes = encode_event_batch(&batch)?;
        if let Some(prefix) = migration_prefix {
            let mut contents = prefix;
            contents.extend_from_slice(&batch_bytes);
            write_file_atomically(&log_path, &contents)?;
            remove_superseded_legacy_events(&legacy_directory);
        } else if log_exists {
            append_batch_to_log(&log_path, committed_length, &batch_bytes)?;
        } else {
            create_log(&log_path, &batch_bytes)?;
        }
        Ok(batch)
    }

    fn load_conversation_events(
        &self,
        conversation_id: ConversationId,
    ) -> io::Result<Vec<ConversationEventRecord>> {
        let conversation_directory = self.conversation_directory(conversation_id);
        if let Some(log) = read_conversation_log(&conversation_directory.join(EVENT_LOG_FILE_NAME))?
        {
            return Ok(log.events);
        }
        if let Some(events) =
            read_legacy_events(&conversation_directory.join(LEGACY_EVENTS_DIRECTORY_NAME))?
        {
            return Ok(events);
        }
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("no events found for {conversation_id}"),
        ))
    }

    fn conversation_directory(&self, conversation_id: ConversationId) -> PathBuf {
        self.root_directory
            .join(CONVERSATIONS_DIRECTORY_NAME)
            .join(conversation_id.storage_key())
    }
}

struct ConversationLog {
    events: Vec<ConversationEventRecord>,
    committed_length: u64,
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

fn validate_existing_events(
    conversation_id: ConversationId,
    existing_events: &[ConversationEventRecord],
) -> io::Result<Option<u64>> {
    if existing_events.is_empty() {
        return Ok(None);
    }
    let conversation =
        Conversation::from_events(existing_events.to_vec()).map_err(invalid_conversation_data)?;
    if conversation.id() != conversation_id {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("loaded {}, expected {conversation_id}", conversation.id()),
        ));
    }
    ensure_contiguous_positions(existing_events)?;
    Ok(existing_events.last().map(|event| event.position))
}

fn read_conversation_log(path: &Path) -> io::Result<Option<ConversationLog>> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let log = decode_log(&bytes)?;
    ensure_contiguous_positions(&log.events)?;
    Ok(Some(log))
}

fn decode_log(bytes: &[u8]) -> io::Result<ConversationLog> {
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

fn encode_event_batch(events: &[ConversationEventRecord]) -> io::Result<Vec<u8>> {
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

fn corruption(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn crc32(bytes: &[u8]) -> u32 {
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

fn append_batch_to_log(
    log_path: &Path,
    committed_length: u64,
    batch_bytes: &[u8],
) -> io::Result<()> {
    let mut log_file = OpenOptions::new().read(true).write(true).open(log_path)?;
    log_file.set_len(committed_length)?;
    log_file.seek(SeekFrom::Start(committed_length))?;
    log_file.write_all(batch_bytes)?;
    log_file.sync_all()
}

fn create_log(log_path: &Path, batch_bytes: &[u8]) -> io::Result<()> {
    let parent_directory = log_path
        .parent()
        .ok_or_else(|| io::Error::other("persisted file has no parent directory"))?;
    let mut log_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(log_path)?;
    log_file.write_all(batch_bytes)?;
    log_file.sync_all()?;
    File::open(parent_directory)?.sync_all()
}

fn read_legacy_events(directory: &Path) -> io::Result<Option<Vec<ConversationEventRecord>>> {
    let mut events = match read_json_directory::<ConversationEventRecord>(directory) {
        Ok(events) => events,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    events.sort_by_key(|event: &ConversationEventRecord| event.position);
    ensure_contiguous_positions(&events)?;
    Ok(Some(events))
}

fn read_last_conversation_event(
    conversation_directory: &Path,
) -> io::Result<Option<ConversationEventRecord>> {
    if let Some(log) = read_conversation_log(&conversation_directory.join(EVENT_LOG_FILE_NAME))? {
        return Ok(log.events.into_iter().last());
    }
    read_last_legacy_event(&conversation_directory.join(LEGACY_EVENTS_DIRECTORY_NAME))
}

fn remove_superseded_legacy_events(legacy_directory: &Path) {
    let _ = fs::remove_dir_all(legacy_directory);
}

fn create_private_directory(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

fn next_position(previous_position: Option<u64>) -> io::Result<u64> {
    match previous_position {
        Some(position) => position
            .checked_add(1)
            .ok_or_else(|| io::Error::other("event position overflow")),
        None => Ok(0),
    }
}

fn ensure_contiguous_positions(events: &[ConversationEventRecord]) -> io::Result<()> {
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

#[cfg(test)]
fn event_path(directory: &Path, position: u64, identifier: &str) -> PathBuf {
    directory.join(format!("{position:020}-{identifier}.json"))
}

fn write_file_atomically(path: &Path, contents: &[u8]) -> io::Result<()> {
    let parent_directory = path
        .parent()
        .ok_or_else(|| io::Error::other("persisted file has no parent directory"))?;
    let temporary_path = parent_directory.join(format!(".tmp-{}", Uuid::now_v7().simple()));
    let mut temporary_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary_path)?;
    temporary_file.write_all(contents)?;
    temporary_file.sync_all()?;
    fs::rename(&temporary_path, path)?;
    File::open(parent_directory)?.sync_all()
}

#[cfg(test)]
fn write_json_atomically<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let mut contents = serde_json::to_vec(value).map_err(io::Error::other)?;
    contents.push(b'\n');
    write_file_atomically(path, &contents)
}

fn read_json<T: DeserializeOwned>(path: &Path) -> io::Result<T> {
    let file = File::open(path)?;
    serde_json::from_reader(BufReader::new(file)).map_err(io::Error::other)
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

fn read_last_legacy_event(events_directory: &Path) -> io::Result<Option<ConversationEventRecord>> {
    let directory_entries = match fs::read_dir(events_directory) {
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

fn invalid_conversation_data(error: impl Error + Send + Sync + 'static) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::path::PathBuf;

    use schemars::json_schema;
    use serde_json::{Map, Value, json};

    use super::{
        EVENT_LOG_FILE_NAME, EventStore, LEGACY_EVENTS_DIRECTORY_NAME, encode_event_batch,
        event_path, write_file_atomically, write_json_atomically,
    };
    use crate::conversation::{
        AssistantResponse, ConversationCommandId, ConversationEvent, ConversationEventKind,
        ConversationEventRecord, ConversationFact, ConversationId, ConversationMessage,
        ConversationTurnId, ModelData, ModelInvocationId, ToolCallId, ToolDefinition, ToolName,
        ToolOutcome, ToolRequest, ToolResponse, UserContent,
    };

    fn temporary_store() -> EventStore {
        let directory = std::env::temp_dir().join(format!("tog-test-{}", uuid::Uuid::now_v7()));
        EventStore::new(directory).expect("the event store should be created")
    }

    fn conversation_event_log_path(store: &EventStore, conversation_id: ConversationId) -> PathBuf {
        store
            .conversation_directory(conversation_id)
            .join(EVENT_LOG_FILE_NAME)
    }

    fn user_message_fact(content: &str) -> ConversationFact {
        ConversationFact::Message {
            message: ConversationMessage::User {
                caused_by: Some(ConversationCommandId::new()),
                content: vec![UserContent::Text(content.to_owned())],
            },
            turn_id: None,
        }
    }

    fn user_fact(content: &str) -> ConversationEvent {
        ConversationEvent::Fact(user_message_fact(content))
    }

    fn user_kind(content: &str) -> ConversationEventKind {
        ConversationEventKind::Fact(user_message_fact(content))
    }

    #[test]
    fn crc32_matches_the_standard_check_value() {
        assert_eq!(super::crc32(b"123456789"), 0xcbf4_3926);
    }

    #[test]
    fn event_store_assigns_canonical_envelope_metadata() {
        let store = temporary_store();
        let conversation_id = ConversationId::new();

        let first_batch = store
            .append_new_conversation_events(conversation_id, vec![user_fact("first")])
            .expect("the first event should be persisted");
        let second_batch = store
            .append_new_conversation_events(conversation_id, vec![user_fact("second")])
            .expect("the second event should be persisted");
        let first_event = &first_batch[0];
        let second_event = &second_batch[0];

        assert_eq!(first_event.conversation_id, conversation_id);
        assert_eq!(first_event.position, 0);
        assert_eq!(first_event.schema_version, 13);
        assert_ne!(first_event.timestamp, time::OffsetDateTime::UNIX_EPOCH);
        assert_eq!(second_event.conversation_id, conversation_id);
        assert_eq!(second_event.position, 1);
        assert_eq!(second_event.schema_version, 13);
        assert_ne!(second_event.timestamp, time::OffsetDateTime::UNIX_EPOCH);
        assert_ne!(second_event.id, first_event.id);

        let conversation = store
            .load_conversation(conversation_id)
            .expect("the conversation should load");
        assert_eq!(conversation.events()[0].position, 0);
        assert_eq!(conversation.events()[1].position, 1);
        assert!(
            conversation
                .events()
                .iter()
                .all(|event| event.conversation_id == conversation_id)
        );
        assert!(
            !store
                .conversation_directory(conversation_id)
                .join("conversation.json")
                .exists()
        );
        assert!(conversation_event_log_path(&store, conversation_id).exists());
    }

    #[test]
    fn appending_an_event_batch_commits_its_events_in_order() {
        let store = temporary_store();
        let conversation_id = ConversationId::new();

        let appended = store
            .append_new_conversation_events(
                conversation_id,
                vec![user_fact("first"), user_fact("second"), user_fact("third")],
            )
            .expect("the batch should be persisted");

        assert_eq!(
            appended
                .iter()
                .map(|event| event.position)
                .collect::<Vec<_>>(),
            [0, 1, 2]
        );
        let loaded = store
            .load_conversation_log(conversation_id)
            .expect("the log should load");
        assert_eq!(loaded, appended);
    }

    #[test]
    fn appending_an_empty_event_batch_is_rejected() {
        let store = temporary_store();
        let conversation_id = ConversationId::new();

        let error = store
            .append_new_conversation_events(conversation_id, Vec::new())
            .expect_err("an empty batch should be rejected");

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn event_store_rejects_committed_positions_with_a_gap() {
        let store = temporary_store();
        let conversation_id = ConversationId::new();
        let mut events = vec![
            ConversationEventRecord::new(conversation_id, 0, user_kind("first")),
            ConversationEventRecord::new(conversation_id, 1, user_kind("second")),
        ];
        events[1].position = 2;
        std::fs::create_dir_all(store.conversation_directory(conversation_id))
            .expect("the conversation directory should be created");
        write_file_atomically(
            &conversation_event_log_path(&store, conversation_id),
            &encode_event_batch(&events).expect("the batch should encode"),
        )
        .expect("the log should be written");

        let error = store
            .load_conversation(conversation_id)
            .expect_err("the incomplete log should be rejected");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(
            error.to_string(),
            "expected conversation event position 1, found 2"
        );
        assert!(
            store
                .append_new_conversation_events(conversation_id, vec![user_fact("third")])
                .is_err()
        );
    }

    #[test]
    fn recovery_ignores_an_incomplete_trailing_batch() {
        let store = temporary_store();
        let conversation_id = ConversationId::new();
        store
            .append_new_conversation_events(conversation_id, vec![user_fact("committed")])
            .expect("the committed event should be persisted");
        let log_path = conversation_event_log_path(&store, conversation_id);
        let torn_batch = encode_event_batch(&[ConversationEventRecord::new(
            conversation_id,
            1,
            user_kind("torn"),
        )])
        .expect("the torn batch should encode");
        let mut log_file = std::fs::OpenOptions::new()
            .append(true)
            .open(&log_path)
            .expect("the log should open");
        log_file
            .write_all(&torn_batch[..torn_batch.len() - 8])
            .expect("the torn tail should be written");

        let loaded = store
            .load_conversation_log(conversation_id)
            .expect("the committed log should load");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].position, 0);

        store
            .append_new_conversation_events(conversation_id, vec![user_fact("after")])
            .expect("the torn tail should be discarded");
        let loaded = store
            .load_conversation_log(conversation_id)
            .expect("the log should load");
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[1].position, 1);
    }

    #[test]
    fn recovery_ignores_a_complete_transaction_without_a_commit_marker() {
        let store = temporary_store();
        let conversation_id = ConversationId::new();
        store
            .append_new_conversation_events(conversation_id, vec![user_fact("committed")])
            .expect("the committed event should be persisted");
        let log_path = conversation_event_log_path(&store, conversation_id);
        let mut uncommitted = Vec::new();
        uncommitted.extend_from_slice(b"{\"transaction\":\"begin\"}\n");
        let event = ConversationEventRecord::new(conversation_id, 1, user_kind("uncommitted"));
        serde_json::to_writer(&mut uncommitted, &event).expect("the event should encode");
        uncommitted.push(b'\n');
        let mut log_file = std::fs::OpenOptions::new()
            .append(true)
            .open(&log_path)
            .expect("the log should open");
        log_file
            .write_all(&uncommitted)
            .expect("the uncommitted transaction should be written");

        let loaded = store
            .load_conversation_log(conversation_id)
            .expect("the committed log should load");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].position, 0);

        store
            .append_new_conversation_events(conversation_id, vec![user_fact("after")])
            .expect("the uncommitted transaction should be discarded");
        let loaded = store
            .load_conversation_log(conversation_id)
            .expect("the log should load");
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[1].position, 1);
    }

    #[test]
    fn corruption_inside_committed_history_is_rejected() {
        let store = temporary_store();
        let conversation_id = ConversationId::new();
        store
            .append_new_conversation_events(conversation_id, vec![user_fact("first")])
            .expect("the event should be persisted");
        let log_path = conversation_event_log_path(&store, conversation_id);
        let mut bytes = std::fs::read(&log_path).expect("the log should be readable");
        let content_offset = bytes
            .windows(b"first".len())
            .position(|window| window == b"first")
            .expect("the event content should be present");
        bytes[content_offset] = b'F';
        std::fs::write(&log_path, &bytes).expect("the corrupted log should be written");

        let error = store
            .load_conversation(conversation_id)
            .expect_err("corruption inside committed history should be rejected");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn event_store_persists_model_data_on_the_event_envelope() {
        let store = temporary_store();
        let conversation_id = ConversationId::new();
        let model_data = ModelData::new(Map::from_iter([(
            "response_id".to_owned(),
            Value::String("resp_1".to_owned()),
        )]))
        .expect("the model data should be valid");

        let assistant = ConversationEvent::Fact(ConversationFact::Message {
            message: ConversationMessage::AssistantResponse {
                invocation_id: ModelInvocationId::new(),
                data: Some(model_data),
                response: AssistantResponse::new("The answer is 42.".to_owned())
                    .expect("the assistant response should be valid"),
            },
            turn_id: Some(ConversationTurnId::new()),
        });
        let batch = store
            .append_new_conversation_events(conversation_id, vec![assistant])
            .expect("the model event should be persisted");

        let conversation = store
            .load_conversation(conversation_id)
            .expect("the conversation should load");
        assert_eq!(conversation.events()[0], batch[0]);
    }

    #[test]
    fn event_store_round_trips_tool_definitions_requests_and_responses() {
        let store = temporary_store();
        let conversation_id = ConversationId::new();
        let call_id = ToolCallId::new();
        let turn_id = ConversationTurnId::new();
        let tool_definition = ToolDefinition::try_new(
            ToolName::try_new("shell".to_owned()).expect("the tool name should be valid"),
            "Run a command.".to_owned(),
            json_schema!({ "type": "object" }),
            json_schema!({ "type": "object" }),
        )
        .expect("the tool definition should be valid");
        let request = ToolRequest::try_new(
            call_id,
            ToolName::try_new("shell".to_owned()).expect("the tool name should be valid"),
            json!({ "command": "pwd" }),
            ModelInvocationId::new(),
            None,
        )
        .expect("the tool request should be valid");
        let response = ToolResponse::new(
            call_id,
            ToolOutcome::Result {
                value: json!({ "stdout": "/tmp\n" }),
            },
        );

        store
            .append_new_conversation_events(
                conversation_id,
                vec![ConversationEvent::Fact(ConversationFact::ToolsAvailable {
                    tools: vec![tool_definition.clone()],
                })],
            )
            .expect("the tool definitions should persist");
        store
            .append_new_conversation_events(
                conversation_id,
                vec![ConversationEvent::Fact(ConversationFact::ToolRequest {
                    request: request.clone(),
                    turn_id: Some(turn_id),
                })],
            )
            .expect("the tool request should persist");
        store
            .append_new_conversation_events(
                conversation_id,
                vec![ConversationEvent::Fact(ConversationFact::ToolResponse {
                    response: response.clone(),
                    turn_id: Some(turn_id),
                })],
            )
            .expect("the tool response should persist");

        let conversation = store
            .load_conversation(conversation_id)
            .expect("the conversation should load");

        assert_eq!(conversation.available_tools(), [tool_definition]);
        assert!(matches!(
            &conversation.events()[1].kind,
            crate::conversation::StoredConversationEventKind::Shared(
                crate::conversation::ConversationEventKind::Fact(
                    ConversationFact::ToolRequest { request: restored, turn_id: Some(restored_turn) }
                )
            ) if restored == &request && restored_turn == &turn_id
        ));
        assert!(matches!(
            &conversation.events()[2].kind,
            crate::conversation::StoredConversationEventKind::Shared(
                crate::conversation::ConversationEventKind::Fact(
                    ConversationFact::ToolResponse { response: restored, .. }
                )
            ) if restored == &response
        ));
    }

    #[test]
    fn event_store_migrates_legacy_per_event_files_on_append() {
        let store = temporary_store();
        let conversation_id = ConversationId::new();
        let conversation_directory = store.conversation_directory(conversation_id);
        let legacy_directory = conversation_directory.join(LEGACY_EVENTS_DIRECTORY_NAME);
        std::fs::create_dir_all(&legacy_directory).expect("the legacy directory should be created");
        let first = ConversationEventRecord::new(conversation_id, 0, user_kind("first"));
        let second = ConversationEventRecord::new(conversation_id, 1, user_kind("second"));
        write_json_atomically(
            &event_path(&legacy_directory, first.position, "first"),
            &first,
        )
        .expect("the first legacy event should be written");
        write_json_atomically(
            &event_path(&legacy_directory, second.position, "second"),
            &second,
        )
        .expect("the second legacy event should be written");

        let loaded = store
            .load_conversation_log(conversation_id)
            .expect("the legacy events should load");
        assert_eq!(loaded, vec![first.clone(), second.clone()]);

        let appended = store
            .append_new_conversation_events(conversation_id, vec![user_fact("third")])
            .expect("the new event should migrate the legacy log");
        assert_eq!(appended[0].position, 2);

        let loaded = store
            .load_conversation_log(conversation_id)
            .expect("the migrated log should load");
        assert_eq!(loaded.len(), 3);
        assert_eq!(loaded[0], first);
        assert_eq!(loaded[1], second);
        assert!(!legacy_directory.exists());
        assert!(conversation_event_log_path(&store, conversation_id).exists());
    }

    fn timestamp(day: u64) -> time::OffsetDateTime {
        time::OffsetDateTime::UNIX_EPOCH + time::Duration::days(day as i64)
    }

    fn set_event_timestamp(
        store: &EventStore,
        event: &ConversationEventRecord,
        timestamp: time::OffsetDateTime,
    ) {
        let mut events = store
            .load_conversation_log(event.conversation_id)
            .expect("the log should load");
        for loaded_event in &mut events {
            if loaded_event.id == event.id {
                loaded_event.timestamp = timestamp;
            }
        }
        write_file_atomically(
            &conversation_event_log_path(store, event.conversation_id),
            &encode_event_batch(&events).expect("the batch should encode"),
        )
        .expect("the event timestamp should be written");
    }

    #[test]
    fn latest_conversation_is_the_most_recently_active_one() {
        let store = temporary_store();
        let first_conversation_id = ConversationId::new();
        let second_conversation_id = ConversationId::new();
        let first_batch = store
            .append_new_conversation_events(first_conversation_id, vec![user_fact("first")])
            .expect("the first event should be persisted");
        let second_batch = store
            .append_new_conversation_events(second_conversation_id, vec![user_fact("second")])
            .expect("the second event should be persisted");
        set_event_timestamp(&store, &first_batch[0], timestamp(1));
        set_event_timestamp(&store, &second_batch[0], timestamp(2));

        assert_eq!(
            store
                .latest_conversation_id()
                .expect("the latest conversation should be found"),
            second_conversation_id
        );

        let third_batch = store
            .append_new_conversation_events(first_conversation_id, vec![user_fact("third")])
            .expect("the third event should be persisted");
        set_event_timestamp(&store, &third_batch[0], timestamp(3));

        assert_eq!(
            store
                .latest_conversation_id()
                .expect("the latest conversation should be found"),
            first_conversation_id
        );
    }

    #[test]
    fn latest_conversation_is_absent_without_conversations() {
        let store = temporary_store();

        let error = store
            .latest_conversation_id()
            .expect_err("the latest conversation should be missing");

        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
        assert_eq!(error.to_string(), "no conversations found");
    }

    #[test]
    fn latest_conversation_ignores_conversations_without_events() {
        let store = temporary_store();
        let conversation_id = ConversationId::new();
        store
            .append_new_conversation_events(conversation_id, vec![user_fact("first")])
            .expect("the event should be persisted");
        let empty_conversation_id = ConversationId::new();
        std::fs::create_dir_all(store.conversation_directory(empty_conversation_id))
            .expect("the empty conversation directory should be created");

        assert_eq!(
            store
                .latest_conversation_id()
                .expect("the latest conversation should be found"),
            conversation_id
        );
    }

    #[test]
    fn latest_conversation_ignores_an_earlier_schema() {
        let store = temporary_store();
        let legacy_conversation_id = ConversationId::new();
        let legacy_directory = store
            .conversation_directory(legacy_conversation_id)
            .join(LEGACY_EVENTS_DIRECTORY_NAME);
        std::fs::create_dir_all(&legacy_directory).expect("the legacy directory should be created");
        std::fs::write(
            legacy_directory.join("00000000000000000000-01a00692c0dc7402a70f67ae862a5eb5.json"),
            concat!(
                r#"{"position":0,"id":"01a00692-c0dc-7402-a70f-67ae862a5eb5","#,
                r#""timestamp_milliseconds":1786816676060,"schema_version":1,"#,
                r#""event":{"type":"user","text":"test"}}"#
            ),
        )
        .expect("the earlier schema event should be written");

        let error = store
            .latest_conversation_id()
            .expect_err("the earlier schema should not count as a conversation");
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);

        let current_conversation_id = ConversationId::new();
        store
            .append_new_conversation_events(current_conversation_id, vec![user_fact("current")])
            .expect("the event should be persisted");

        assert_eq!(
            store
                .latest_conversation_id()
                .expect("the latest conversation should be found"),
            current_conversation_id
        );
    }
}
