use std::error::Error;
use std::fmt::{Display, Formatter};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct Automation {
    content: String,
}

impl Automation {
    #[cfg(test)]
    pub(crate) fn new(content: String) -> Result<Self, InvalidAutomation> {
        let automation = Self { content };
        automation.ensure_valid()?;
        Ok(automation)
    }

    #[cfg(test)]
    pub(crate) fn content(&self) -> &str {
        &self.content
    }

    pub(crate) fn ensure_valid(&self) -> Result<(), InvalidAutomation> {
        if self.content.trim().is_empty() {
            return Err(InvalidAutomation::EmptyContent);
        }
        Ok(())
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum InvalidAutomation {
    EmptyContent,
}

impl Display for InvalidAutomation {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyContent => write!(formatter, "automation content must not be empty"),
        }
    }
}

impl Error for InvalidAutomation {}
