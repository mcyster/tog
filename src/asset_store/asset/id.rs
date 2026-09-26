use std::error::Error;
use std::fmt::{Display, Formatter};
use std::str::FromStr;

use serde::de::Error as DeserializeError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct AssetId(Uuid);

impl AssetId {
    pub(crate) fn new() -> Self {
        Self(Uuid::now_v7())
    }

    pub(crate) fn storage_key(self) -> String {
        self.0.simple().to_string()
    }
}

impl Display for AssetId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "asset_{}", self.0.simple())
    }
}

impl FromStr for AssetId {
    type Err = InvalidAssetId;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let uuid_text = text.strip_prefix("asset_").unwrap_or(text);
        Uuid::parse_str(uuid_text).map(Self).map_err(InvalidAssetId)
    }
}

impl Serialize for AssetId {
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

impl<'de> Deserialize<'de> for AssetId {
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
pub(crate) struct InvalidAssetId(uuid::Error);

impl Display for InvalidAssetId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "invalid asset identifier: {}", self.0)
    }
}

impl Error for InvalidAssetId {}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::{AssetId, InvalidAssetId};

    #[test]
    fn asset_identifier_round_trips_through_its_display_and_storage_forms() {
        let asset_id = AssetId::new();

        assert_eq!(
            asset_id.to_string(),
            format!("asset_{}", asset_id.storage_key())
        );
        let reparsed = AssetId::from_str(&format!("asset_{}", asset_id.storage_key()))
            .expect("the displayed asset identifier should parse back");
        assert_eq!(reparsed, asset_id);
    }

    #[test]
    fn asset_identifier_accepts_an_unprefixed_uuid() {
        let asset_id = AssetId::new();

        let reparsed = AssetId::from_str(&asset_id.storage_key())
            .expect("the storage key should parse as an asset identifier");

        assert_eq!(reparsed, asset_id);
    }

    #[test]
    fn asset_identifier_rejects_unknown_text() {
        let error = AssetId::from_str("not-an-asset").expect_err("the text should be rejected");

        assert!(matches!(error, InvalidAssetId { .. }));
    }

    #[test]
    fn asset_identifier_serializes_to_its_display_representation_and_round_trips() {
        let asset_id = AssetId::new();

        let serialized =
            serde_json::to_value(asset_id).expect("the asset identifier should serialize");
        assert_eq!(
            serialized,
            serde_json::Value::String(format!("asset_{}", asset_id.storage_key()))
        );
        let deserialized: AssetId =
            serde_json::from_value(serialized).expect("the asset identifier should parse");
        assert_eq!(deserialized, asset_id);
    }
}
