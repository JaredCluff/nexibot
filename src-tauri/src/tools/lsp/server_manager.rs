use super::client::{language_id_for_extension, LspClient};
use crate::config::LspConfig;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub struct ServerEntry {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub extensions: Vec<String>,
    pub client: Option<LspClient>,
    pub restart_count: u32,
}

pub struct LspServerManager {
    servers: HashMap<String, ServerEntry>,
    extension_map: HashMap<String, String>,
    opened_files: HashMap<String, String>,
    working_dir: PathBuf,
}

impl LspServerManager {
    pub fn from_config(config: &LspConfig, working_dir: PathBuf) -> Self {
        let mut servers = HashMap::new();
        let mut extension_map = HashMap::new();

        for (name, cfg) in &config.servers {
            for ext in &cfg.extensions {
                let key = ext.trim_start_matches('.').to_lowercase();
                extension_map.entry(key).or_insert_with(|| name.clone());
            }
            servers.insert(name.clone(), ServerEntry {
                name: name.clone(),
                command: cfg.command.clone(),
                args: cfg.args.clone(),
                extensions: cfg.extensions.clone(),
                client: None,
                restart_count: 0,
            });
        }

        LspServerManager { servers, extension_map, opened_files: HashMap::new(), working_dir }
    }

    pub fn has_servers(&self) -> bool { !self.servers.is_empty() }

    pub fn server_for_file(&self, path: &Path) -> Option<&str> {
        let ext = path.extension()?.to_str()?.to_lowercase();
        self.extension_map.get(&ext).map(|s| s.as_str())
    }

    pub async fn ensure_started(&mut self, server_name: &str) -> anyhow::Result<()> {
        let entry = self.servers.get_mut(server_name)
            .ok_or_else(|| anyhow::anyhow!("Unknown server: {}", server_name))?;

        if entry.client.as_ref().map(|c| c.is_healthy()).unwrap_or(false) {
            return Ok(());
        }

        if entry.restart_count >= 3 {
            return Err(anyhow::anyhow!("Server {} failed too many times", server_name));
        }

        let client = LspClient::start(&entry.command, &entry.args, &self.working_dir).await?;
        entry.client = Some(client);
        entry.restart_count += 1;
        Ok(())
    }

    pub async fn request(
        &mut self,
        path: &Path,
        method: &str,
        params: Value,
    ) -> anyhow::Result<Value> {
        let server_name = self.server_for_file(path)
            .ok_or_else(|| anyhow::anyhow!(
                "No LSP server configured for {}",
                path.extension().and_then(|e| e.to_str()).unwrap_or("unknown")
            ))?.to_string();

        self.ensure_started(&server_name).await?;

        let uri = format!("file://{}", path.to_string_lossy());
        if !self.opened_files.contains_key(&uri) {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            let lang_id = language_id_for_extension(ext);
            let content = tokio::fs::read_to_string(path).await.unwrap_or_default();
            let entry = self.servers.get(&server_name).unwrap();
            if let Some(client) = &entry.client {
                client.open_file(&uri, lang_id, &content).await?;
                self.opened_files.insert(uri.clone(), server_name.clone());
            }
        }

        let entry = self.servers.get(&server_name).unwrap();
        match &entry.client {
            Some(client) => client.request(method, params).await,
            None => Err(anyhow::anyhow!("Server not running")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{LspConfig, LspServerConfig};

    fn make_config_with_rust_server() -> LspConfig {
        let mut servers = std::collections::HashMap::new();
        servers.insert("rust".to_string(), LspServerConfig {
            command: "rust-analyzer".to_string(),
            args: vec![],
            extensions: vec![".rs".to_string()],
        });
        LspConfig { servers }
    }

    #[test]
    fn test_from_config_builds_extension_map() {
        let cfg = make_config_with_rust_server();
        let mgr = LspServerManager::from_config(&cfg, PathBuf::from("/tmp"));
        assert!(mgr.has_servers());
        assert_eq!(mgr.server_for_file(Path::new("foo.rs")), Some("rust"));
        assert_eq!(mgr.server_for_file(Path::new("foo.py")), None);
    }

    #[test]
    fn test_no_server_for_unknown_extension() {
        let cfg = make_config_with_rust_server();
        let mgr = LspServerManager::from_config(&cfg, PathBuf::from("/tmp"));
        assert_eq!(mgr.server_for_file(Path::new("foo.java")), None);
    }

    #[test]
    fn test_empty_config_no_servers() {
        let cfg = LspConfig::default();
        let mgr = LspServerManager::from_config(&cfg, PathBuf::from("/tmp"));
        assert!(!mgr.has_servers());
    }
}
