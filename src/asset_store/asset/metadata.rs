use std::error::Error;
use std::fmt::{Display, Formatter};
use std::str::FromStr;

use serde::de::Error as DeserializeError;
use serde::{Deserialize, Deserializer, Serialize};

use super::AssetId;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub(crate) struct MimeType(String);

impl MimeType {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for MimeType {
    type Err = InvalidMimeType;

    fn from_str(unvalidated_value: &str) -> Result<Self, Self::Err> {
        if unvalidated_value.trim().is_empty() {
            return Err(InvalidMimeType);
        }
        Ok(Self(unvalidated_value.to_owned()))
    }
}

impl<'de> Deserialize<'de> for MimeType {
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

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct InvalidMimeType;

impl Display for InvalidMimeType {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "mime type must not be empty")
    }
}

impl Error for InvalidMimeType {}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct AssetMetadata {
    id: AssetId,
    name: String,
    mime_type: MimeType,
    byte_size: u64,
}

impl AssetMetadata {
    pub(crate) fn new(id: AssetId, name: String, mime_type: MimeType, byte_size: u64) -> Self {
        Self {
            id,
            name,
            mime_type,
            byte_size,
        }
    }

    pub(crate) fn id(&self) -> AssetId {
        self.id
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn mime_type(&self) -> &MimeType {
        &self.mime_type
    }

    pub(crate) fn byte_size(&self) -> u64 {
        self.byte_size
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::{AssetId, AssetMetadata, MimeType};

    #[test]
    fn asset_metadata_serializes_with_snake_case_fields() {
        let asset_id = AssetId::new();
        let metadata = AssetMetadata::new(
            asset_id,
            "screenshot.png".to_owned(),
            MimeType::from_str("image/png").expect("the mime type should be valid"),
            1234,
        );

        let serialized = serde_json::to_value(&metadata).expect("the metadata should serialize");
        assert_eq!(serialized["byte_size"].as_u64(), Some(1234));
        assert_eq!(serialized["name"].as_str(), Some("screenshot.png"));
        assert_eq!(serialized["mime_type"].as_str(), Some("image/png"));
        assert!(serialized["id"].as_str().is_some());
    }

    #[test]
    fn asset_metadata_round_trips_and_preserves_observed_size() {
        let asset_id = AssetId::new();
        let metadata = AssetMetadata::new(
            asset_id,
            "screenshot.png".to_owned(),
            MimeType::from_str("image/png").expect("the mime type should be valid"),
            1234,
        );

        let serialized = serde_json::to_value(&metadata).expect("the metadata should serialize");
        let deserialized: AssetMetadata =
            serde_json::from_value(serialized.clone()).expect("the metadata should deserialize");

        assert_eq!(deserialized, metadata);
        assert_eq!(deserialized.id(), asset_id);
        assert_eq!(deserialized.name(), "screenshot.png");
        assert_eq!(deserialized.mime_type().as_str(), "image/png");
        assert_eq!(deserialized.byte_size(), 1234);
    }

    #[test]
    fn mime_type_rejects_empty_values() {
        assert_eq!(MimeType::from_str("\t"), Err(super::InvalidMimeType));
        assert!(serde_json::from_str::<MimeType>("\" \"").is_err());
    }

    #[test]
    fn asset_metadata_serializes_an_octet_stream_fallback() {
        let metadata = AssetMetadata::new(
            AssetId::new(),
            "notes.txt".to_owned(),
            MimeType::from_str("application/octet-stream").expect("the mime type should be valid"),
            0,
        );

        let serialized = serde_json::to_value(&metadata).expect("the metadata should serialize");
        assert_eq!(
            serialized["mime_type"].as_str(),
            Some("application/octet-stream")
        );
        assert_eq!(serialized["byte_size"].as_u64(), Some(0));
    }
}
