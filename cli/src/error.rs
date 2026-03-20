//! Error types for NexiBot CLI

use std::fmt;

#[derive(Debug)]
pub enum CliError {
    /// Network/HTTP error
    Network(String),
    /// API error response
    ApiError { status: u16, message: String },
    /// Configuration error
    Config(String),
    /// Invalid argument
    InvalidArgument(String),
    /// IO error
    Io(std::io::Error),
    /// JSON serialization/deserialization error
    Json(serde_json::Error),
    /// Server not responding
    ServerUnreachable,
    /// Authentication failed
    Unauthorized,
    /// Resource not found
    NotFound(String),
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CliError::Network(msg) => write!(f, "Network error: {}", msg),
            CliError::ApiError { status, message } => {
                write!(f, "API error {}: {}", status, message)
            }
            CliError::Config(msg) => write!(f, "Configuration error: {}", msg),
            CliError::InvalidArgument(msg) => write!(f, "Invalid argument: {}", msg),
            CliError::Io(err) => write!(f, "IO error: {}", err),
            CliError::Json(err) => write!(f, "JSON error: {}", err),
            CliError::ServerUnreachable => write!(f, "NexiBot server is not reachable"),
            CliError::Unauthorized => write!(f, "Unauthorized - check your API token"),
            CliError::NotFound(msg) => write!(f, "Not found: {}", msg),
        }
    }
}

impl std::error::Error for CliError {}

impl From<std::io::Error> for CliError {
    fn from(err: std::io::Error) -> Self {
        CliError::Io(err)
    }
}

impl From<serde_json::Error> for CliError {
    fn from(err: serde_json::Error) -> Self {
        CliError::Json(err)
    }
}

impl From<reqwest::Error> for CliError {
    fn from(err: reqwest::Error) -> Self {
        if err.is_timeout() {
            CliError::ServerUnreachable
        } else {
            CliError::Network(err.to_string())
        }
    }
}
