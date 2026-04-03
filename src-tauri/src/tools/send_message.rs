use crate::tool_registry::{PermissionDecision, Tool, ToolContext, ToolResult};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

// ─── Agent Name Registry ─────────────────────────────────────────────────────

#[derive(Default, Clone)]
pub struct AgentNameRegistry {
    names: HashMap<String, String>, // name -> agent_id
}

impl AgentNameRegistry {
    pub fn register(&mut self, name: String, agent_id: String) {
        self.names.insert(name, agent_id);
    }
    pub fn resolve(&self, name_or_id: &str) -> Option<&str> {
        self.names.get(name_or_id).map(|s| s.as_str())
    }
    pub fn all_ids(&self) -> Vec<String> {
        self.names.values().cloned().collect()
    }
    pub fn unregister(&mut self, agent_id: &str) {
        self.names.retain(|_, v| v != agent_id);
    }
}

// ─── Agent Message ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMessage {
    pub from: String,
    pub to: String,
    pub text: String,
    pub timestamp: DateTime<Utc>,
}

// ─── Inbox (filesystem-based for background agents) ──────────────────────────

pub struct AgentInbox {
    pub base_dir: PathBuf,
}

impl AgentInbox {
    pub fn new(base_dir: PathBuf) -> Self { AgentInbox { base_dir } }

    pub fn inbox_dir(&self, agent_id: &str) -> PathBuf {
        self.base_dir.join(agent_id).join("inbox")
    }

    pub async fn write_message(&self, msg: &AgentMessage) -> anyhow::Result<()> {
        let dir = self.inbox_dir(&msg.to);
        tokio::fs::create_dir_all(&dir).await?;
        let filename = format!("{}.json", msg.timestamp.timestamp_nanos_opt().unwrap_or(0));
        let path = dir.join(filename);
        let json = serde_json::to_string(msg)?;
        tokio::fs::write(path, json).await?;
        Ok(())
    }

    pub async fn drain_messages(&self, agent_id: &str) -> Vec<AgentMessage> {
        let dir = self.inbox_dir(agent_id);
        let mut msgs = Vec::new();
        if let Ok(mut rd) = tokio::fs::read_dir(&dir).await {
            while let Ok(Some(entry)) = rd.next_entry().await {
                if let Ok(data) = tokio::fs::read_to_string(entry.path()).await {
                    if let Ok(msg) = serde_json::from_str::<AgentMessage>(&data) {
                        let _ = tokio::fs::remove_file(entry.path()).await;
                        msgs.push(msg);
                    }
                }
            }
        }
        msgs.sort_by_key(|m| m.timestamp);
        msgs
    }
}

// ─── Tool ────────────────────────────────────────────────────────────────────

pub struct SendMessageTool {
    pub registry: Arc<RwLock<AgentNameRegistry>>,
    pub inbox: Arc<AgentInbox>,
    pub in_process_queues: Arc<RwLock<HashMap<String, tokio::sync::mpsc::UnboundedSender<AgentMessage>>>>,
}

#[async_trait]
impl Tool for SendMessageTool {
    fn name(&self) -> &str { "nexibot_send_message" }
    fn description(&self) -> &str {
        "Send a message to another agent by name or ID. Use \"*\" to broadcast to all active agents."
    }
    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "to": { "type": "string", "description": "Agent name, ID, or \"*\" for broadcast" },
                "message": { "type": "string", "description": "Message text to send" }
            },
            "required": ["to", "message"]
        })
    }
    async fn check_permissions(&self, _: &Value, _: &ToolContext) -> PermissionDecision {
        PermissionDecision::Allow
    }

    async fn call(&self, input: Value, ctx: ToolContext) -> ToolResult {
        let to = match input["to"].as_str() {
            Some(t) => t.to_string(),
            None => return ToolResult::err("to is required"),
        };
        let text = match input["message"].as_str() {
            Some(m) => m.to_string(),
            None => return ToolResult::err("message is required"),
        };

        let msg = AgentMessage {
            from: ctx.agent_id.clone(),
            to: to.clone(),
            text,
            timestamp: Utc::now(),
        };

        if to == "*" {
            let reg = self.registry.read().await;
            let ids = reg.all_ids();
            drop(reg);
            let mut count = 0;
            for id in ids {
                if id == ctx.agent_id { continue; }
                let mut m = msg.clone();
                m.to = id.clone();
                let _ = self.deliver(&id, m).await;
                count += 1;
            }
            return ToolResult::ok(format!("Broadcast sent to {} agents.", count));
        }

        let resolved_id = {
            let reg = self.registry.read().await;
            reg.resolve(&to).map(|s| s.to_string()).unwrap_or(to.clone())
        };

        match self.deliver(&resolved_id, msg).await {
            Ok(()) => ToolResult::ok(format!("Message sent to agent {}.", resolved_id)),
            Err(e) => ToolResult::err(format!("Failed to deliver message: {}", e)),
        }
    }
}

impl SendMessageTool {
    async fn deliver(&self, agent_id: &str, msg: AgentMessage) -> anyhow::Result<()> {
        let queues = self.in_process_queues.read().await;
        if let Some(tx) = queues.get(agent_id) {
            let _ = tx.send(msg);
            return Ok(());
        }
        drop(queues);
        self.inbox.write_message(&msg).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_register_and_resolve() {
        let mut reg = AgentNameRegistry::default();
        reg.register("worker".to_string(), "agent_abc".to_string());
        assert_eq!(reg.resolve("worker"), Some("agent_abc"));
        assert_eq!(reg.resolve("agent_abc"), None);
    }

    #[test]
    fn test_registry_unregister() {
        let mut reg = AgentNameRegistry::default();
        reg.register("worker".to_string(), "agent_abc".to_string());
        reg.unregister("agent_abc");
        assert!(reg.resolve("worker").is_none());
    }

    #[tokio::test]
    async fn test_inbox_write_and_drain() {
        let dir = tempfile::tempdir().unwrap().into_path();
        let inbox = AgentInbox::new(dir);
        let msg = AgentMessage {
            from: "agent_1".to_string(),
            to: "agent_2".to_string(),
            text: "hello!".to_string(),
            timestamp: Utc::now(),
        };
        inbox.write_message(&msg).await.unwrap();
        let msgs = inbox.drain_messages("agent_2").await;
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].text, "hello!");
        let msgs2 = inbox.drain_messages("agent_2").await;
        assert!(msgs2.is_empty());
    }

    #[tokio::test]
    async fn test_send_message_via_in_process_queue() {
        let registry = Arc::new(RwLock::new(AgentNameRegistry::default()));
        {
            let mut reg = registry.write().await;
            reg.register("target".to_string(), "agent_target".to_string());
        }

        let dir = tempfile::tempdir().unwrap().into_path();
        let inbox = Arc::new(AgentInbox::new(dir));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let queues = Arc::new(RwLock::new({
            let mut m = HashMap::new();
            m.insert("agent_target".to_string(), tx);
            m
        }));

        let tool = SendMessageTool { registry, inbox, in_process_queues: queues };
        let ctx = ToolContext {
            session_key: "s".into(),
            agent_id: "agent_sender".into(),
            working_dir: std::path::PathBuf::from("/tmp"),
        };

        let result = tool.call(serde_json::json!({"to": "target", "message": "do the thing"}), ctx).await;
        assert!(result.success);

        let received = rx.recv().await.unwrap();
        assert_eq!(received.text, "do the thing");
        assert_eq!(received.from, "agent_sender");
    }
}
