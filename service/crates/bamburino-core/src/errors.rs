// SPDX-License-Identifier: AGPL-3.0-or-later
//! Structured tool errors shared by the service layers.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ToolErrorCode {
    StudioNotRunning,
    DriverDisabled,
    DriverUnauthorized,
    DriverTokenMissing,
    VersionMismatch,
    Busy,
    RevisionConflict,
    UnknownKey,
    InvalidValue,
    ScopeNotAllowed,
    NotWritable,
    NotWritableByDriver,
    ObjectNotFound,
    PlateNotFound,
    JobNotFound,
    PartiallyApplied,
    InvalidArgument,
    DriverError,
}

impl ToolErrorCode {
    pub fn http_status(self) -> u16 {
        use ToolErrorCode::*;
        match self {
            RevisionConflict => 409,
            Busy | StudioNotRunning | DriverDisabled => 503,
            DriverUnauthorized | DriverTokenMissing => 401,
            UnknownKey | InvalidValue | ScopeNotAllowed | NotWritable | NotWritableByDriver
            | InvalidArgument => 422,
            ObjectNotFound | PlateNotFound | JobNotFound => 404,
            VersionMismatch => 412,
            PartiallyApplied => 207,
            DriverError => 502,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, thiserror::Error)]
#[error("{code:?}: {message}")]
pub struct ToolError {
    pub code: ToolErrorCode,
    pub message: String,
    pub http_status: u16,
    pub details: Value,
}

impl ToolError {
    /// Builds an error with the canonical HTTP status of `code` and empty details.
    pub fn new(code: ToolErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            http_status: code.http_status(),
            details: json!({}),
        }
    }

    pub fn with_details(mut self, details: Value) -> Self {
        self.details = details;
        self
    }

    /// `{"error":{"code","message","http_status","details"}}`
    pub fn to_json(&self) -> Value {
        json!({ "error": {
            "code": self.code,
            "message": self.message,
            "http_status": self.http_status,
            "details": self.details,
        }})
    }
}
