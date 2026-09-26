use std::error::Error;
use std::fmt::{Display, Formatter};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub(crate) enum UserContent {
    Text(String),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct User {
    content: Vec<UserContent>,
}

impl User {
    pub(crate) fn new(content: Vec<UserContent>) -> Result<Self, InvalidUser> {
        let user = Self { content };
        user.ensure_valid()?;
        Ok(user)
    }

    pub(crate) fn content(&self) -> &[UserContent] {
        &self.content
    }

    pub(crate) fn ensure_valid(&self) -> Result<(), InvalidUser> {
        if self.content.is_empty() {
            return Err(InvalidUser::EmptyContent);
        }
        Ok(())
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum InvalidUser {
    EmptyContent,
}

impl Display for InvalidUser {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyContent => write!(formatter, "user content must not be empty"),
        }
    }
}

impl Error for InvalidUser {}
