use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{fmt, path::PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Invocation {
    pub operation: String,
    pub state_directory: Option<PathBuf>,
    pub arguments: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolError {
    pub code: String,
    pub message: String,
    pub effect: String,
    pub details: Value,
    #[serde(skip)]
    pub exit_code: u8,
}

impl ToolError {
    pub fn new(code: &str, message: impl Into<String>, exit_code: u8) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            effect: if code == "outcome_unknown" {
                "unknown"
            } else {
                "none"
            }
            .into(),
            details: json!({}),
            exit_code,
        }
    }
    pub fn detail(mut self, details: Value) -> Self {
        if self.effect == "none"
            && details.get("fileCommitted").and_then(Value::as_bool) == Some(true)
        {
            self.effect = "partial".into();
        }
        self.details = details;
        self
    }
    pub fn effect(mut self, effect: impl Into<String>) -> Self {
        self.effect = effect.into();
        self
    }
    pub fn envelope(&self) -> Value {
        let data = self.details.get("data").cloned().unwrap_or(Value::Null);
        json!({"schemaVersion":1,"status":if self.effect == "partial" { "partial" } else { "error" },"data":data,"error":self})
    }
}
impl fmt::Display for ToolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for ToolError {}

pub fn success_envelope(data: Value) -> Value {
    json!({"schemaVersion":1,"status":"ok","data":data,"error":null})
}
