//! JSON message protocol for the WebSocket gateway.
//!
//! Defines the wire format for messages exchanged between WebSocket clients
//! and the gateway server. All messages are JSON-encoded with a `type` tag
//! for polymorphic deserialization.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Client -> Server
// ---------------------------------------------------------------------------

/// Messages sent from a client to the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Send a chat message.
    SendMessage {
        /// The user's text.
        text: String,
        /// Which session this message belongs to.
        session_id: String,
    },
    /// Return the result of a tool execution requested by the server.
    ToolResult {
        /// The tool_use_id the server assigned to the request.
        tool_use_id: String,
        /// The result payload.
        result: String,
    },
    /// Keep-alive ping.
    Ping,
}

// ---------------------------------------------------------------------------
// Server -> Client
// ---------------------------------------------------------------------------

/// Messages sent from the server to a client.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// A chunk of assistant text (may be streamed).
    TextChunk { text: String, session_id: String },
    /// The assistant wants the client to execute a tool.
    ToolCall {
        tool_name: String,
        tool_input: Value,
        tool_use_id: String,
    },
    /// An error occurred.
    Error {
        message: String,
        #[serde(default)]
        code: Option<String>,
    },
    /// Response to a Ping.
    Pong,
    /// A new session was created for this connection.
    SessionCreated { session_id: String },
    /// Sent immediately after a successful WebSocket upgrade.
    Connected { version: String },
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse a JSON string into a [`ClientMessage`].
pub fn parse_client_message(json: &str) -> Result<ClientMessage> {
    serde_json::from_str(json).map_err(|e| anyhow::anyhow!("Invalid client message: {}", e))
}

/// Serialize a [`ServerMessage`] to a JSON string.
pub fn serialize_server_message(msg: &ServerMessage) -> Result<String> {
    serde_json::to_string(msg)
        .map_err(|e| anyhow::anyhow!("Failed to serialize server message: {}", e))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_send_message() {
        let json = r#"{"type":"send_message","text":"hello","session_id":"s1"}"#;
        let msg = parse_client_message(json).unwrap();
        match msg {
            ClientMessage::SendMessage { text, session_id } => {
                assert_eq!(text, "hello");
                assert_eq!(session_id, "s1");
            }
            _ => panic!("Expected SendMessage"),
        }
    }

    #[test]
    fn test_parse_tool_result() {
        let json = r#"{"type":"tool_result","tool_use_id":"tu-1","result":"42"}"#;
        let msg = parse_client_message(json).unwrap();
        match msg {
            ClientMessage::ToolResult {
                tool_use_id,
                result,
            } => {
                assert_eq!(tool_use_id, "tu-1");
                assert_eq!(result, "42");
            }
            _ => panic!("Expected ToolResult"),
        }
    }

    #[test]
    fn test_parse_ping() {
        let json = r#"{"type":"ping"}"#;
        let msg = parse_client_message(json).unwrap();
        assert!(matches!(msg, ClientMessage::Ping));
    }

    #[test]
    fn test_parse_invalid_json() {
        let result = parse_client_message("not json");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_unknown_type() {
        let json = r#"{"type":"unknown_thing"}"#;
        let result = parse_client_message(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_serialize_text_chunk() {
        let msg = ServerMessage::TextChunk {
            text: "hi".to_string(),
            session_id: "s1".to_string(),
        };
        let json = serialize_server_message(&msg).unwrap();
        assert!(json.contains("\"type\":\"text_chunk\""));
        assert!(json.contains("\"text\":\"hi\""));
    }

    #[test]
    fn test_serialize_pong() {
        let msg = ServerMessage::Pong;
        let json = serialize_server_message(&msg).unwrap();
        assert!(json.contains("\"type\":\"pong\""));
    }

    #[test]
    fn test_serialize_error() {
        let msg = ServerMessage::Error {
            message: "boom".to_string(),
            code: Some("E001".to_string()),
        };
        let json = serialize_server_message(&msg).unwrap();
        assert!(json.contains("\"message\":\"boom\""));
        assert!(json.contains("\"code\":\"E001\""));
    }

    #[test]
    fn test_serialize_tool_call() {
        let msg = ServerMessage::ToolCall {
            tool_name: "search".to_string(),
            tool_input: serde_json::json!({"query": "rust"}),
            tool_use_id: "tu-99".to_string(),
        };
        let json = serialize_server_message(&msg).unwrap();
        assert!(json.contains("\"tool_name\":\"search\""));
        assert!(json.contains("\"tool_use_id\":\"tu-99\""));
    }

    #[test]
    fn test_serialize_connected() {
        let msg = ServerMessage::Connected {
            version: "0.1.0".to_string(),
        };
        let json = serialize_server_message(&msg).unwrap();
        assert!(json.contains("\"version\":\"0.1.0\""));
    }

    #[test]
    fn test_serialize_session_created() {
        let msg = ServerMessage::SessionCreated {
            session_id: "abc-123".to_string(),
        };
        let json = serialize_server_message(&msg).unwrap();
        assert!(json.contains("\"session_id\":\"abc-123\""));
    }

    #[test]
    fn test_roundtrip_client_message() {
        let original = ClientMessage::SendMessage {
            text: "round trip".to_string(),
            session_id: "sess-42".to_string(),
        };
        let json = serde_json::to_string(&original).unwrap();
        let parsed = parse_client_message(&json).unwrap();
        match parsed {
            ClientMessage::SendMessage { text, session_id } => {
                assert_eq!(text, "round trip");
                assert_eq!(session_id, "sess-42");
            }
            _ => panic!("Expected SendMessage"),
        }
    }
}
