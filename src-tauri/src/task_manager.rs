//! Background task management for the agentic voice system.
//!
//! Tracks background tasks spawned by the AI during voice conversations,
//! providing status updates and completion notifications.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

use crate::notifications::NotificationTarget;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TaskStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackgroundTask {
    pub id: String,
    pub description: String,
    pub status: TaskStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub progress: Option<String>,
    pub result_summary: Option<String>,
    /// Where to send a completion/failure notification (None = no notification).
    pub notify_target: Option<NotificationTarget>,
}

/// Maximum number of completed/failed tasks to retain before evicting oldest.
const MAX_FINISHED_TASKS: usize = 500;

pub struct TaskManager {
    tasks: HashMap<String, BackgroundTask>,
}

impl TaskManager {
    pub fn new() -> Self {
        Self {
            tasks: HashMap::new(),
        }
    }

    pub fn create_task(
        &mut self,
        description: &str,
        notify_target: Option<NotificationTarget>,
    ) -> String {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now();
        let task = BackgroundTask {
            id: id.clone(),
            description: description.to_string(),
            status: TaskStatus::Running,
            created_at: now,
            updated_at: now,
            progress: None,
            result_summary: None,
            notify_target,
        };
        self.tasks.insert(id.clone(), task);
        // Evict oldest finished tasks if we exceed the cap
        let finished: Vec<String> = {
            let mut done: Vec<_> = self
                .tasks
                .iter()
                .filter(|(_, t)| {
                    matches!(t.status, TaskStatus::Completed | TaskStatus::Failed)
                })
                .map(|(k, t)| (k.clone(), t.created_at))
                .collect();
            done.sort_by_key(|(_, ts)| *ts);
            let evict_count = done.len().saturating_sub(MAX_FINISHED_TASKS);
            done.into_iter()
                .take(evict_count)
                .map(|(k, _)| k)
                .collect()
        };
        for k in finished {
            self.tasks.remove(&k);
        }
        id
    }

    pub fn update_progress(&mut self, task_id: &str, progress: &str) {
        if let Some(task) = self.tasks.get_mut(task_id) {
            task.progress = Some(progress.to_string());
            task.updated_at = Utc::now();
        }
    }

    pub fn complete_task(&mut self, task_id: &str, summary: &str) {
        if let Some(task) = self.tasks.get_mut(task_id) {
            task.status = TaskStatus::Completed;
            task.result_summary = Some(summary.to_string());
            task.progress = None;
            task.updated_at = Utc::now();
        }
    }

    pub fn fail_task(&mut self, task_id: &str, error: &str) {
        if let Some(task) = self.tasks.get_mut(task_id) {
            task.status = TaskStatus::Failed;
            task.result_summary = Some(format!("Error: {}", error));
            task.progress = None;
            task.updated_at = Utc::now();
        }
    }

    pub fn list_tasks(&self) -> Vec<BackgroundTask> {
        self.tasks.values().cloned().collect()
    }

    pub fn get_task(&self, task_id: &str) -> Option<&BackgroundTask> {
        self.tasks.get(task_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notifications::NotificationTarget;

    #[test]
    fn test_create_task_no_notification() {
        let mut mgr = TaskManager::new();
        let id = mgr.create_task("do something", None);
        let task = mgr.get_task(&id).expect("task exists");
        assert_eq!(task.description, "do something");
        assert_eq!(task.status, TaskStatus::Running);
        assert!(task.notify_target.is_none());
        assert!(task.progress.is_none());
        assert!(task.result_summary.is_none());
    }

    #[test]
    fn test_create_task_with_telegram_target() {
        let mut mgr = TaskManager::new();
        let target = NotificationTarget::Telegram { chat_id: 12345 };
        let id = mgr.create_task("telegram task", Some(target.clone()));
        let task = mgr.get_task(&id).expect("task exists");
        assert_eq!(task.notify_target, Some(target));
    }

    #[test]
    fn test_create_task_with_all_configured_target() {
        let mut mgr = TaskManager::new();
        let id = mgr.create_task("broadcast task", Some(NotificationTarget::AllConfigured));
        let task = mgr.get_task(&id).expect("task exists");
        assert_eq!(task.notify_target, Some(NotificationTarget::AllConfigured));
    }

    #[test]
    fn test_complete_task_updates_status_and_summary() {
        let mut mgr = TaskManager::new();
        let id = mgr.create_task("work to do", Some(NotificationTarget::Gui));
        mgr.complete_task(&id, "All done!");
        let task = mgr.get_task(&id).expect("task exists");
        assert_eq!(task.status, TaskStatus::Completed);
        assert_eq!(task.result_summary.as_deref(), Some("All done!"));
        assert!(task.progress.is_none());
        // notify_target is preserved after completion (caller reads it to dispatch)
        assert_eq!(task.notify_target, Some(NotificationTarget::Gui));
    }

    #[test]
    fn test_fail_task_updates_status_and_formats_error() {
        let mut mgr = TaskManager::new();
        let id = mgr.create_task("risky work", None);
        mgr.fail_task(&id, "connection refused");
        let task = mgr.get_task(&id).expect("task exists");
        assert_eq!(task.status, TaskStatus::Failed);
        assert_eq!(
            task.result_summary.as_deref(),
            Some("Error: connection refused")
        );
    }

    #[test]
    fn test_update_progress() {
        let mut mgr = TaskManager::new();
        let id = mgr.create_task("long task", None);
        mgr.update_progress(&id, "50% done");
        let task = mgr.get_task(&id).expect("task exists");
        assert_eq!(task.progress.as_deref(), Some("50% done"));
        assert_eq!(task.status, TaskStatus::Running);
    }

    #[test]
    fn test_complete_task_clears_progress() {
        let mut mgr = TaskManager::new();
        let id = mgr.create_task("task", None);
        mgr.update_progress(&id, "halfway");
        mgr.complete_task(&id, "finished");
        let task = mgr.get_task(&id).expect("task exists");
        assert!(
            task.progress.is_none(),
            "progress should be cleared on completion"
        );
    }

    #[test]
    fn test_task_ids_are_unique() {
        let mut mgr = TaskManager::new();
        let ids: Vec<String> = (0..10).map(|_| mgr.create_task("t", None)).collect();
        let unique: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(unique.len(), 10, "all task IDs must be unique");
    }

    #[test]
    fn test_list_tasks() {
        let mut mgr = TaskManager::new();
        mgr.create_task("task a", None);
        mgr.create_task("task b", None);
        assert_eq!(mgr.list_tasks().len(), 2);
    }

    #[test]
    fn test_notify_target_serde_roundtrip() {
        let targets = vec![
            NotificationTarget::AllConfigured,
            NotificationTarget::Gui,
            NotificationTarget::Telegram { chat_id: -100123 },
            NotificationTarget::TelegramConfigured,
            NotificationTarget::Discord {
                channel_id: 987654321,
            },
            NotificationTarget::Slack {
                channel_id: "C0ABC123".to_string(),
            },
            NotificationTarget::WhatsApp {
                phone_number: "15551234567".to_string(),
            },
            NotificationTarget::Signal {
                phone_number: "+15557654321".to_string(),
            },
            NotificationTarget::Matrix {
                room_id: "!roomid:matrix.org".to_string(),
            },
            NotificationTarget::Mattermost {
                channel_id: "channel-id".to_string(),
            },
            NotificationTarget::GoogleChat,
            NotificationTarget::BlueBubbles {
                chat_guid: "iMessage;-;+15551234567".to_string(),
            },
            NotificationTarget::Messenger {
                recipient_id: "1234567890".to_string(),
            },
            NotificationTarget::Instagram {
                recipient_id: "17841400000000000".to_string(),
            },
            NotificationTarget::Line {
                user_id: "U1234567890abcdef".to_string(),
            },
            NotificationTarget::Twilio {
                phone_number: "+15551234567".to_string(),
            },
        ];
        for target in targets {
            let json = serde_json::to_string(&target).expect("serialize");
            let back: NotificationTarget = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(target, back, "serde roundtrip failed for {:?}", json);
        }
    }
}
