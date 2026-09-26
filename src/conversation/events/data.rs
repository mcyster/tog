use std::error::Error;
use std::fmt::{Display, Formatter};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct Data {
    content: Map<String, Value>,
}

impl Data {
    #[cfg(test)]
    pub(crate) fn new(content: Map<String, Value>) -> Result<Self, InvalidData> {
        let data = Self { content };
        data.ensure_valid()?;
        Ok(data)
    }

    #[cfg(test)]
    pub(crate) fn content(&self) -> &Map<String, Value> {
        &self.content
    }

    pub(crate) fn ensure_valid(&self) -> Result<(), InvalidData> {
        if self.content.is_empty() {
            return Err(InvalidData::EmptyContent);
        }
        Ok(())
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum InvalidData {
    EmptyContent,
}

impl Display for InvalidData {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyContent => write!(formatter, "data content must not be empty"),
        }
    }
}

impl Error for InvalidData {}
