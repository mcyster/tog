use std::error::Error;
use std::fmt::{Display, Formatter};

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[allow(dead_code)]
const DEFAULT_CONTEXT_TYPE: &str = "string";

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct Context {
    name: String,
    context_type: String,
    value: Value,
}

impl Context {
    #[allow(dead_code)]
    pub(crate) fn try_new(
        name: String,
        context_type: String,
        value: Value,
    ) -> Result<Self, InvalidContext> {
        let context_type = if context_type.trim().is_empty() {
            DEFAULT_CONTEXT_TYPE.to_owned()
        } else {
            context_type
        };
        let name = if name.trim().is_empty() {
            context_type.clone()
        } else {
            name
        };
        let context = Self {
            name,
            context_type,
            value,
        };
        context.ensure_valid()?;
        Ok(context)
    }

    #[allow(dead_code)]
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    #[allow(dead_code)]
    pub(crate) fn context_type(&self) -> &str {
        &self.context_type
    }

    #[allow(dead_code)]
    pub(crate) fn value(&self) -> &Value {
        &self.value
    }

    pub(crate) fn ensure_valid(&self) -> Result<(), InvalidContext> {
        if self.name.trim().is_empty() {
            return Err(InvalidContext::EmptyName);
        }
        if self.context_type.trim().is_empty() {
            return Err(InvalidContext::EmptyType);
        }
        Ok(())
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum InvalidContext {
    EmptyName,
    EmptyType,
}

impl Display for InvalidContext {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyName => write!(formatter, "context name must not be empty"),
            Self::EmptyType => write!(formatter, "context type must not be empty"),
        }
    }
}

impl Error for InvalidContext {}
