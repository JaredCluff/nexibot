//! Streaming infrastructure for trait-based tool execution.
//! Provides plumbing to drain progress events from a tool's
//! call_streaming() method and forward them to the ToolLoopObserver.

use crate::tool_registry::ToolProgress;
use tokio::sync::mpsc;

/// Spawns a task to drain progress events from a channel and
/// forward them to the provided callback. Returns a JoinHandle.
pub fn drain_progress<F>(
    mut rx: mpsc::UnboundedReceiver<ToolProgress>,
    tool_name: String,
    tool_id: String,
    on_progress: F,
) -> tokio::task::JoinHandle<()>
where
    F: Fn(String, String, ToolProgress) + Send + 'static,
{
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            on_progress(tool_name.clone(), tool_id.clone(), event);
        }
    })
}

/// Collect all progress events synchronously (for testing).
pub async fn collect_progress(
    mut rx: mpsc::UnboundedReceiver<ToolProgress>,
) -> Vec<ToolProgress> {
    let mut events = Vec::new();
    loop {
        match tokio::time::timeout(
            std::time::Duration::from_millis(100),
            rx.recv()
        ).await {
            Ok(Some(event)) => events.push(event),
            _ => break,
        }
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_drain_progress_collects_events() {
        let (tx, rx) = mpsc::unbounded_channel();
        tx.send(ToolProgress::Stdout("line 1\n".to_string())).unwrap();
        tx.send(ToolProgress::Stderr("err\n".to_string())).unwrap();
        drop(tx);

        let events = collect_progress(rx).await;
        assert_eq!(events.len(), 2);
        assert!(matches!(&events[0], ToolProgress::Stdout(s) if s == "line 1\n"));
        assert!(matches!(&events[1], ToolProgress::Stderr(s) if s == "err\n"));
    }

    #[tokio::test]
    async fn test_drain_progress_handles_empty_channel() {
        let (tx, rx) = mpsc::unbounded_channel::<ToolProgress>();
        drop(tx);
        let events = collect_progress(rx).await;
        assert!(events.is_empty());
    }
}
