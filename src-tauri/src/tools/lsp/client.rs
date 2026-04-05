//! JSON-RPC 2.0 client over stdio for LSP servers.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin};
use tokio::sync::{Mutex, RwLock};

static REQUEST_ID: AtomicU64 = AtomicU64::new(1);

fn next_id() -> u64 {
    REQUEST_ID.fetch_add(1, Ordering::SeqCst)
}

#[derive(Debug, Serialize)]
struct JsonRpcRequest {
    jsonrpc: &'static str,
    id: u64,
    method: String,
    params: Value,
}

#[derive(Debug, Deserialize)]
struct JsonRpcResponse {
    id: Option<u64>,
    result: Option<Value>,
    error: Option<Value>,
}

/// State of the LSP client connection.
#[derive(Debug, Clone, PartialEq)]
pub enum ClientState {
    Stopped,
    Starting,
    Running,
    Error(String),
}

pub struct LspClient {
    state: Arc<RwLock<ClientState>>,
    stdin: Arc<Mutex<ChildStdin>>,
    responses: Arc<RwLock<HashMap<u64, Value>>>,
    _child: Arc<Mutex<Child>>,
}

impl LspClient {
    pub async fn start(
        command: &str,
        args: &[String],
        working_dir: &std::path::Path,
    ) -> anyhow::Result<Self> {
        use tokio::process::Command;
        let mut child = Command::new(command)
            .args(args)
            .current_dir(working_dir)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()?;

        let stdin = child.stdin.take()
            .ok_or_else(|| anyhow::anyhow!("Failed to get stdin"))?;
        let stdout = child.stdout.take()
            .ok_or_else(|| anyhow::anyhow!("Failed to get stdout"))?;

        let state = Arc::new(RwLock::new(ClientState::Starting));
        let responses: Arc<RwLock<HashMap<u64, Value>>> = Arc::new(RwLock::new(HashMap::new()));
        let stdin = Arc::new(Mutex::new(stdin));

        let responses_c = responses.clone();
        let state_c = state.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            loop {
                match read_lsp_message(&mut reader).await {
                    Ok(Some(msg)) => {
                        if let Ok(resp) = serde_json::from_str::<JsonRpcResponse>(&msg) {
                            if let Some(id) = resp.id {
                                let result = resp.result.unwrap_or(Value::Null);
                                responses_c.write().await.insert(id, result);
                            }
                        }
                    }
                    Ok(None) => break,
                    Err(_) => {
                        *state_c.write().await = ClientState::Error("read error".to_string());
                        break;
                    }
                }
            }
        });

        let client = LspClient { state, stdin, responses, _child: Arc::new(Mutex::new(child)) };
        client.initialize(working_dir).await?;
        *client.state.write().await = ClientState::Running;
        Ok(client)
    }

    async fn initialize(&self, root: &std::path::Path) -> anyhow::Result<()> {
        let root_uri = format!("file://{}", root.to_string_lossy());
        let params = serde_json::json!({
            "processId": std::process::id(),
            "rootUri": root_uri,
            "capabilities": {
                "textDocument": {
                    "definition": { "linkSupport": false },
                    "references": {},
                    "hover": { "contentFormat": ["plaintext", "markdown"] },
                    "documentSymbol": { "hierarchicalDocumentSymbolSupport": true },
                    "implementation": {}
                },
                "workspace": { "symbol": {} }
            }
        });
        let id = next_id();
        self.send_request("initialize", params, id).await?;
        self.wait_for_response(id, std::time::Duration::from_secs(30)).await?;
        self.send_notification("initialized", serde_json::json!({})).await?;
        Ok(())
    }

    pub async fn send_request(&self, method: &str, params: Value, id: u64) -> anyhow::Result<()> {
        let req = JsonRpcRequest { jsonrpc: "2.0", id, method: method.to_string(), params };
        let body = serde_json::to_string(&req)?;
        let msg = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(msg.as_bytes()).await?;
        stdin.flush().await?;
        Ok(())
    }

    pub async fn send_notification(&self, method: &str, params: Value) -> anyhow::Result<()> {
        let notif = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        });
        let body = serde_json::to_string(&notif)?;
        let msg = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(msg.as_bytes()).await?;
        stdin.flush().await?;
        Ok(())
    }

    pub async fn wait_for_response(&self, id: u64, timeout: std::time::Duration) -> anyhow::Result<Value> {
        let start = std::time::Instant::now();
        loop {
            if start.elapsed() > timeout {
                return Err(anyhow::anyhow!("LSP request {} timed out", id));
            }
            {
                let mut responses = self.responses.write().await;
                if let Some(val) = responses.remove(&id) {
                    return Ok(val);
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    pub async fn request(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        let id = next_id();
        self.send_request(method, params, id).await?;
        self.wait_for_response(id, std::time::Duration::from_secs(10)).await
    }

    pub async fn open_file(&self, uri: &str, language_id: &str, text: &str) -> anyhow::Result<()> {
        self.send_notification("textDocument/didOpen", serde_json::json!({
            "textDocument": { "uri": uri, "languageId": language_id, "version": 1, "text": text }
        })).await
    }

    pub fn is_healthy(&self) -> bool {
        matches!(self.state.try_read().map(|s| s.clone()), Ok(ClientState::Running))
    }
}

async fn read_lsp_message<R: AsyncBufReadExt + Unpin>(reader: &mut R) -> anyhow::Result<Option<String>> {
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await? == 0 {
            return Ok(None);
        }
        let line = line.trim();
        if line.is_empty() { break; }
        if line.to_lowercase().starts_with("content-length:") {
            content_length = line.split(':')
                .nth(1)
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(0);
        }
    }
    if content_length == 0 { return Ok(None); }
    let mut buf = vec![0u8; content_length];
    use tokio::io::AsyncReadExt;
    reader.read_exact(&mut buf).await?;
    Ok(Some(String::from_utf8(buf)?))
}

/// Map file extension to LSP languageId.
pub fn language_id_for_extension(ext: &str) -> &'static str {
    match ext.to_lowercase().as_str() {
        "rs" => "rust",
        "ts" | "tsx" => "typescript",
        "js" | "jsx" => "javascript",
        "py" => "python",
        "go" => "go",
        "java" => "java",
        "cpp" | "cc" | "cxx" => "cpp",
        "c" => "c",
        "cs" => "csharp",
        "rb" => "ruby",
        "swift" => "swift",
        "kt" | "kts" => "kotlin",
        _ => "plaintext",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_language_id_for_extension() {
        assert_eq!(language_id_for_extension("rs"), "rust");
        assert_eq!(language_id_for_extension("ts"), "typescript");
        assert_eq!(language_id_for_extension("py"), "python");
        assert_eq!(language_id_for_extension("unknown"), "plaintext");
    }

    #[test]
    fn test_language_id_case_insensitive() {
        assert_eq!(language_id_for_extension("RS"), "rust");
        assert_eq!(language_id_for_extension("TS"), "typescript");
    }

    #[test]
    fn test_next_id_increments() {
        let id1 = next_id();
        let id2 = next_id();
        assert!(id2 > id1);
    }
}
