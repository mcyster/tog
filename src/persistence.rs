use std::error::Error;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;
use uuid::Uuid;

use crate::conversation::{
    Conversation, ConversationEvent, ConversationEventKind, ConversationEventRecord,
    ConversationId, StoredConversationEventKind,
};

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
        create_private_directory(&root_directory.join("conversations"))?;
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
        let conversations_directory = self.root_directory.join("conversations");
        let mut latest_event: Option<ConversationEventRecord> = None;
        for directory_entry in fs::read_dir(&conversations_directory)? {
            let directory_entry = directory_entry?;
            if !directory_entry.file_type()?.is_dir() {
                continue;
            }
            let Some(last_event) = read_last_event(&directory_entry.path().join("events"))? else {
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

    pub(crate) fn append_new_conversation_event(
        &self,
        conversation_id: ConversationId,
        event: ConversationEvent,
    ) -> io::Result<ConversationEventRecord> {
        let kind = match event {
            ConversationEvent::Command(command) => {
                StoredConversationEventKind::Shared(ConversationEventKind::Command(command))
            }
            ConversationEvent::Fact(fact) => {
                StoredConversationEventKind::Shared(ConversationEventKind::Fact(fact))
            }
            ConversationEvent::Extension(event) => StoredConversationEventKind::Extension(
                event.to_envelope().map_err(io::Error::other)?,
            ),
        };
        self.append_new_record(conversation_id, kind)
    }

    fn append_new_record(
        &self,
        conversation_id: ConversationId,
        kind: StoredConversationEventKind,
    ) -> io::Result<ConversationEventRecord> {
        let conversation_directory = self.conversation_directory(conversation_id);
        create_private_directory(&conversation_directory)?;
        let events_directory = conversation_directory.join("events");
        create_private_directory(&events_directory)?;
        let existing_events = self.load_conversation_events(conversation_id)?;
        let previous_position = if existing_events.is_empty() {
            None
        } else {
            let conversation =
                Conversation::from_events(existing_events).map_err(invalid_conversation_data)?;
            if conversation.id() != conversation_id {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("loaded {}, expected {conversation_id}", conversation.id()),
                ));
            }
            conversation.events().last().map(|event| event.position)
        };
        let conversation_event = match kind {
            StoredConversationEventKind::Shared(kind) => ConversationEventRecord::new(
                conversation_id,
                next_position(previous_position)?,
                kind,
            ),
            StoredConversationEventKind::Extension(event) => {
                ConversationEventRecord::new_extension(
                    conversation_id,
                    next_position(previous_position)?,
                    event,
                )
            }
        };
        write_json_atomically(
            &event_path(
                &events_directory,
                conversation_event.position,
                &conversation_event.id.storage_key(),
            ),
            &conversation_event,
        )?;
        Ok(conversation_event)
    }

    fn load_conversation_events(
        &self,
        conversation_id: ConversationId,
    ) -> io::Result<Vec<ConversationEventRecord>> {
        let mut events =
            read_json_directory(&self.conversation_directory(conversation_id).join("events"))?;
        events.sort_by_key(|event: &ConversationEventRecord| event.position);
        ensure_contiguous_positions(&events)?;
        Ok(events)
    }

    fn conversation_directory(&self, conversation_id: ConversationId) -> PathBuf {
        self.root_directory
            .join("conversations")
            .join(conversation_id.storage_key())
    }
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

fn event_path(directory: &Path, position: u64, identifier: &str) -> PathBuf {
    directory.join(format!("{position:020}-{identifier}.json"))
}

fn write_json_atomically<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let parent_directory = path
        .parent()
        .ok_or_else(|| io::Error::other("persisted file has no parent directory"))?;
    let temporary_path = parent_directory.join(format!(".tmp-{}", Uuid::now_v7().simple()));
    let mut temporary_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary_path)?;
    serde_json::to_writer(&mut temporary_file, value).map_err(io::Error::other)?;
    temporary_file.write_all(b"\n")?;
    temporary_file.sync_all()?;
    fs::rename(&temporary_path, path)?;
    File::open(parent_directory)?.sync_all()
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

fn read_last_event(events_directory: &Path) -> io::Result<Option<ConversationEventRecord>> {
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
    use schemars::json_schema;
    use serde_json::{Map, Value, json};

    use super::{EventStore, event_path, write_json_atomically};
    use crate::conversation::{
        AssistantResponse, ConversationCommandId, ConversationEvent, ConversationEventRecord,
        ConversationFact, ConversationId, ConversationMessage, ConversationTurnId, ModelData,
        ModelInvocationId, ToolCallId, ToolDefinition, ToolName, ToolOutcome, ToolRequest,
        ToolResponse, UserContent,
    };

    fn temporary_store() -> EventStore {
        let directory = std::env::temp_dir().join(format!("tog-test-{}", uuid::Uuid::now_v7()));
        EventStore::new(directory).expect("the event store should be created")
    }

    fn user_fact(content: &str) -> ConversationEvent {
        ConversationEvent::Fact(ConversationFact::Message {
            message: ConversationMessage::User {
                caused_by: Some(ConversationCommandId::new()),
                content: vec![UserContent::Text(content.to_owned())],
            },
            turn_id: None,
        })
    }

    #[test]
    fn event_store_assigns_canonical_envelope_metadata() {
        let store = temporary_store();
        let conversation_id = ConversationId::new();

        let first_event = store
            .append_new_conversation_event(conversation_id, user_fact("first"))
            .expect("the first event should be persisted");
        let second_event = store
            .append_new_conversation_event(conversation_id, user_fact("second"))
            .expect("the second event should be persisted");

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
        let model_event = store
            .append_new_conversation_event(conversation_id, assistant)
            .expect("the model event should be persisted");

        let conversation = store
            .load_conversation(conversation_id)
            .expect("the conversation should load");
        assert_eq!(conversation.events()[0], model_event);
    }

    #[test]
    fn event_store_rejects_a_log_with_a_missing_middle_record() {
        let store = temporary_store();
        let conversation_id = ConversationId::new();
        for content in ["first", "second", "third"] {
            store
                .append_new_conversation_event(conversation_id, user_fact(content))
                .expect("the event should be persisted");
        }
        let events_directory = store.conversation_directory(conversation_id).join("events");
        let mut event_paths = std::fs::read_dir(&events_directory)
            .expect("the persisted events should be readable")
            .map(|entry| entry.expect("the event entry should be readable").path())
            .collect::<Vec<_>>();
        event_paths.sort();
        std::fs::remove_file(&event_paths[1]).expect("the middle event should be removed");

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
                .append_new_conversation_event(conversation_id, user_fact("fourth"))
                .is_err()
        );
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
            .append_new_conversation_event(
                conversation_id,
                ConversationEvent::Fact(ConversationFact::ToolsAvailable {
                    tools: vec![tool_definition.clone()],
                }),
            )
            .expect("the tool definitions should persist");
        store
            .append_new_conversation_event(
                conversation_id,
                ConversationEvent::Fact(ConversationFact::ToolRequest {
                    request: request.clone(),
                    turn_id: Some(turn_id),
                }),
            )
            .expect("the tool request should persist");
        store
            .append_new_conversation_event(
                conversation_id,
                ConversationEvent::Fact(ConversationFact::ToolResponse {
                    response: response.clone(),
                    turn_id: Some(turn_id),
                }),
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

    fn timestamp(day: u64) -> time::OffsetDateTime {
        time::OffsetDateTime::UNIX_EPOCH + time::Duration::days(day as i64)
    }

    fn set_event_timestamp(
        store: &EventStore,
        event: &ConversationEventRecord,
        timestamp: time::OffsetDateTime,
    ) {
        let mut rewritten_event = event.clone();
        rewritten_event.timestamp = timestamp;
        let events_directory = store
            .conversation_directory(event.conversation_id)
            .join("events");
        write_json_atomically(
            &event_path(
                &events_directory,
                rewritten_event.position,
                &rewritten_event.id.storage_key(),
            ),
            &rewritten_event,
        )
        .expect("the event timestamp should be written");
    }

    #[test]
    fn latest_conversation_is_the_most_recently_active_one() {
        let store = temporary_store();
        let first_conversation_id = ConversationId::new();
        let second_conversation_id = ConversationId::new();
        let first_event = store
            .append_new_conversation_event(first_conversation_id, user_fact("first"))
            .expect("the first event should be persisted");
        let second_event = store
            .append_new_conversation_event(second_conversation_id, user_fact("second"))
            .expect("the second event should be persisted");
        set_event_timestamp(&store, &first_event, timestamp(1));
        set_event_timestamp(&store, &second_event, timestamp(2));

        assert_eq!(
            store
                .latest_conversation_id()
                .expect("the latest conversation should be found"),
            second_conversation_id
        );

        let third_event = store
            .append_new_conversation_event(first_conversation_id, user_fact("third"))
            .expect("the third event should be persisted");
        set_event_timestamp(&store, &third_event, timestamp(3));

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
            .append_new_conversation_event(conversation_id, user_fact("first"))
            .expect("the event should be persisted");
        let empty_conversation_id = ConversationId::new();
        std::fs::create_dir_all(
            store
                .conversation_directory(empty_conversation_id)
                .join("events"),
        )
        .expect("the empty conversation directory should be created");

        assert_eq!(
            store
                .latest_conversation_id()
                .expect("the latest conversation should be found"),
            conversation_id
        );
    }

    fn replace_last_event_contents(
        store: &EventStore,
        conversation_id: ConversationId,
        contents: &str,
    ) {
        let events_directory = store.conversation_directory(conversation_id).join("events");
        let mut event_paths = std::fs::read_dir(&events_directory)
            .expect("the events should be readable")
            .map(|entry| entry.expect("the event entry should be readable").path())
            .collect::<Vec<_>>();
        event_paths.sort();
        std::fs::write(
            event_paths
                .last()
                .expect("the conversation should have an event"),
            contents,
        )
        .expect("the event contents should be written");
    }

    #[test]
    fn latest_conversation_ignores_an_earlier_schema() {
        let store = temporary_store();
        let legacy_conversation_id = ConversationId::new();
        store
            .append_new_conversation_event(legacy_conversation_id, user_fact("legacy"))
            .expect("the event should be persisted");
        replace_last_event_contents(
            &store,
            legacy_conversation_id,
            concat!(
                r#"{"position":0,"id":"01a00692-c0dc-7402-a70f-67ae862a5eb5","#,
                r#""timestamp_milliseconds":1786816676060,"schema_version":1,"#,
                r#""event":{"type":"user","text":"test"}}"#
            ),
        );

        let error = store
            .latest_conversation_id()
            .expect_err("the earlier schema should not count as a conversation");
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);

        let current_conversation_id = ConversationId::new();
        store
            .append_new_conversation_event(current_conversation_id, user_fact("current"))
            .expect("the event should be persisted");

        assert_eq!(
            store
                .latest_conversation_id()
                .expect("the latest conversation should be found"),
            current_conversation_id
        );
    }
}
