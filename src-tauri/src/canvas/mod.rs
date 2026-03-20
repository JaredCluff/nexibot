//! Agent-driven visual workspace (Canvas / A2UI).
//!
//! Allows agents to create, update, and manage visual panels in the UI.
//! The backend manages canvas state; the frontend renders via Tauri events.
#![allow(dead_code)]

pub mod protocol;
pub mod renderer;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{debug, info};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// The type of content rendered in a canvas panel.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PanelContentType {
    Markdown,
    Code { language: String },
    Table,
    Json,
    Image { base64: bool },
    Html,
}

/// Position of a panel on the canvas (pixels from top-left origin).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct PanelPosition {
    pub x: f64,
    pub y: f64,
}

impl Default for PanelPosition {
    fn default() -> Self {
        Self { x: 0.0, y: 0.0 }
    }
}

/// Size of a panel in logical pixels.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct PanelSize {
    pub width: f64,
    pub height: f64,
}

impl Default for PanelSize {
    fn default() -> Self {
        Self {
            width: 400.0,
            height: 300.0,
        }
    }
}

/// A single visual panel managed by the canvas system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanvasPanel {
    pub id: String,
    pub title: String,
    pub content_type: PanelContentType,
    pub content: String,
    pub position: PanelPosition,
    pub size: PanelSize,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub agent_id: String,
    pub visible: bool,
}

/// Layout strategy for arranging panels on the canvas.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
#[derive(Default)]
pub enum CanvasLayout {
    #[default]
    Freeform,
    Grid {
        columns: usize,
    },
    Stack,
}

/// Aggregate state of all panels and the current layout.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CanvasState {
    pub panels: HashMap<String, CanvasPanel>,
    pub layout: CanvasLayout,
}

/// Manages canvas panels and layout.
///
/// All mutations return enough information for the caller to emit
/// corresponding Tauri events via [`renderer`].
pub struct CanvasManager {
    pub state: CanvasState,
}

impl CanvasManager {
    /// Create a new, empty canvas manager.
    pub fn new() -> Self {
        info!("[CANVAS] Canvas manager initialized");
        Self {
            state: CanvasState::default(),
        }
    }

    /// Add a panel to the canvas. Returns the panel ID.
    pub fn create_panel(&mut self, panel: CanvasPanel) -> String {
        let id = panel.id.clone();
        debug!("[CANVAS] Creating panel '{}': {}", id, panel.title);
        self.state.panels.insert(id.clone(), panel);
        id
    }

    /// Update the content of an existing panel.
    pub fn update_panel(&mut self, id: &str, content: &str) -> Result<(), String> {
        let panel = self
            .state
            .panels
            .get_mut(id)
            .ok_or_else(|| format!("Panel '{}' not found", id))?;
        panel.content = content.to_string();
        panel.updated_at = Utc::now();
        debug!(
            "[CANVAS] Updated panel '{}' content ({} bytes)",
            id,
            content.len()
        );
        Ok(())
    }

    /// Update the title of an existing panel.
    pub fn update_panel_title(&mut self, id: &str, title: &str) -> Result<(), String> {
        let panel = self
            .state
            .panels
            .get_mut(id)
            .ok_or_else(|| format!("Panel '{}' not found", id))?;
        panel.title = title.to_string();
        panel.updated_at = Utc::now();
        debug!("[CANVAS] Updated panel '{}' title to '{}'", id, title);
        Ok(())
    }

    /// Remove a panel from the canvas.
    pub fn remove_panel(&mut self, id: &str) -> Result<(), String> {
        self.state
            .panels
            .remove(id)
            .ok_or_else(|| format!("Panel '{}' not found", id))?;
        debug!("[CANVAS] Removed panel '{}'", id);
        Ok(())
    }

    /// Get an immutable reference to a panel by ID.
    pub fn get_panel(&self, id: &str) -> Option<&CanvasPanel> {
        self.state.panels.get(id)
    }

    /// List all panels (unordered).
    pub fn list_panels(&self) -> Vec<&CanvasPanel> {
        self.state.panels.values().collect()
    }

    /// Remove all panels from the canvas.
    pub fn clear_all(&mut self) {
        let count = self.state.panels.len();
        self.state.panels.clear();
        info!("[CANVAS] Cleared all {} panels", count);
    }

    /// Set the canvas layout strategy.
    pub fn set_layout(&mut self, layout: CanvasLayout) {
        info!("[CANVAS] Layout changed to {:?}", layout);
        self.state.layout = layout;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_panel(id: &str, title: &str) -> CanvasPanel {
        CanvasPanel {
            id: id.to_string(),
            title: title.to_string(),
            content_type: PanelContentType::Markdown,
            content: "Hello world".to_string(),
            position: PanelPosition::default(),
            size: PanelSize::default(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            agent_id: "test-agent".to_string(),
            visible: true,
        }
    }

    #[test]
    fn test_create_and_get_panel() {
        let mut mgr = CanvasManager::new();
        let id = mgr.create_panel(make_panel("p1", "Test Panel"));
        assert_eq!(id, "p1");
        assert!(mgr.get_panel("p1").is_some());
        assert_eq!(mgr.get_panel("p1").unwrap().title, "Test Panel");
    }

    #[test]
    fn test_update_panel_content() {
        let mut mgr = CanvasManager::new();
        mgr.create_panel(make_panel("p1", "Test"));
        assert!(mgr.update_panel("p1", "Updated content").is_ok());
        assert_eq!(mgr.get_panel("p1").unwrap().content, "Updated content");
    }

    #[test]
    fn test_update_panel_not_found() {
        let mut mgr = CanvasManager::new();
        assert!(mgr.update_panel("missing", "content").is_err());
    }

    #[test]
    fn test_update_panel_title() {
        let mut mgr = CanvasManager::new();
        mgr.create_panel(make_panel("p1", "Old Title"));
        assert!(mgr.update_panel_title("p1", "New Title").is_ok());
        assert_eq!(mgr.get_panel("p1").unwrap().title, "New Title");
    }

    #[test]
    fn test_remove_panel() {
        let mut mgr = CanvasManager::new();
        mgr.create_panel(make_panel("p1", "Test"));
        assert!(mgr.remove_panel("p1").is_ok());
        assert!(mgr.get_panel("p1").is_none());
    }

    #[test]
    fn test_remove_panel_not_found() {
        let mut mgr = CanvasManager::new();
        assert!(mgr.remove_panel("missing").is_err());
    }

    #[test]
    fn test_list_panels() {
        let mut mgr = CanvasManager::new();
        mgr.create_panel(make_panel("p1", "Panel 1"));
        mgr.create_panel(make_panel("p2", "Panel 2"));
        assert_eq!(mgr.list_panels().len(), 2);
    }

    #[test]
    fn test_clear_all() {
        let mut mgr = CanvasManager::new();
        mgr.create_panel(make_panel("p1", "Panel 1"));
        mgr.create_panel(make_panel("p2", "Panel 2"));
        mgr.clear_all();
        assert_eq!(mgr.list_panels().len(), 0);
    }

    #[test]
    fn test_set_layout() {
        let mut mgr = CanvasManager::new();
        mgr.set_layout(CanvasLayout::Grid { columns: 3 });
        assert_eq!(mgr.state.layout, CanvasLayout::Grid { columns: 3 });
    }

    #[test]
    fn test_panel_serialization() {
        let panel = make_panel("p1", "Serialization Test");
        let json = serde_json::to_string(&panel).unwrap();
        let deserialized: CanvasPanel = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, "p1");
        assert_eq!(deserialized.title, "Serialization Test");
    }

    #[test]
    fn test_layout_serialization() {
        let layout = CanvasLayout::Grid { columns: 4 };
        let json = serde_json::to_string(&layout).unwrap();
        let deserialized: CanvasLayout = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, CanvasLayout::Grid { columns: 4 });
    }
}
