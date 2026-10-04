use std::error::Error;
use std::fmt::{Display, Formatter};
use std::str::FromStr;

use serde::de::Error as DeserializeError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

const PREFIX: &str = "cnv_";

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ConversationId(Uuid);

impl ConversationId {
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    pub fn storage_key(self) -> String {
        format!("{PREFIX}{}", self.0.simple())
    }
}

impl Default for ConversationId {
    fn default() -> Self {
        Self::new()
    }
}

impl Display for ConversationId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{PREFIX}{}", self.0.simple())
    }
}

impl FromStr for ConversationId {
    type Err = InvalidConversationId;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let uuid_text = text.strip_prefix(PREFIX).unwrap_or(text);
        Uuid::parse_str(uuid_text)
            .map(Self)
            .map_err(InvalidConversationId)
    }
}

impl Serialize for ConversationId {
    fn serialize<SerializerType>(
        &self,
        serializer: SerializerType,
    ) -> Result<SerializerType::Ok, SerializerType::Error>
    where
        SerializerType: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for ConversationId {
    fn deserialize<DeserializerType>(
        deserializer: DeserializerType,
    ) -> Result<Self, DeserializerType::Error>
    where
        DeserializerType: Deserializer<'de>,
    {
        let unvalidated_value = String::deserialize(deserializer)?;
        Self::from_str(&unvalidated_value).map_err(DeserializerType::Error::custom)
    }
}

#[derive(Debug)]
pub struct InvalidConversationId(uuid::Error);

impl Display for InvalidConversationId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "invalid conversation identifier: {}", self.0)
    }
}

impl Error for InvalidConversationId {}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::{ConversationId, InvalidConversationId};

    #[test]
    fn conversation_identifier_round_trips_through_its_display_and_storage_forms() {
        let conversation_id = ConversationId::new();

        assert_eq!(conversation_id.to_string(), conversation_id.storage_key());
        let reparsed = ConversationId::from_str(&conversation_id.storage_key())
            .expect("the displayed conversation identifier should parse back");
        assert_eq!(reparsed, conversation_id);
    }

    #[test]
    fn conversation_identifier_accepts_an_unprefixed_uuid() {
        let uuid_text = uuid::Uuid::now_v7().simple().to_string();

        let reparsed = ConversationId::from_str(&uuid_text)
            .expect("an unprefixed uuid should parse as a conversation identifier");

        assert_eq!(reparsed.to_string(), format!("cnv_{uuid_text}"));
    }

    #[test]
    fn conversation_identifier_rejects_unknown_text() {
        let error = ConversationId::from_str("not-a-conversation")
            .expect_err("the text should be rejected");

        assert!(matches!(error, InvalidConversationId { .. }));
    }

    #[test]
    fn conversation_identifier_serializes_to_its_display_representation_and_round_trips() {
        let conversation_id = ConversationId::new();

        let serialized = serde_json::to_value(conversation_id)
            .expect("the conversation identifier should serialize");
        assert_eq!(
            serialized,
            serde_json::Value::String(conversation_id.storage_key())
        );
        let deserialized: ConversationId =
            serde_json::from_value(serialized).expect("the conversation identifier should parse");
        assert_eq!(deserialized, conversation_id);
    }
}
