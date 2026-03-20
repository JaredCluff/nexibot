//! Tauri commands for advanced memory features

use crate::memory_advanced::{Importance, RelationType};
use serde_json::json;
use tauri::State;
use tracing::info;

#[tauri::command]
pub async fn add_advanced_memory(
    advanced_memory: State<'_, std::sync::Arc<crate::memory_advanced::AdvancedMemoryManager>>,
    content: String,
    importance_score: f32,
    source: String,
    confidence: f32,
) -> Result<String, String> {
    info!(
        "[MEMORY_ADVANCED_CMD] Adding memory with importance {}",
        importance_score
    );

    let importance = Importance::new(importance_score);
    advanced_memory
        .add_memory(content, importance, source, confidence, None)
        .await
        .map_err(|e| format!("Failed to add memory: {}", e))
}

#[tauri::command]
pub async fn link_memories(
    advanced_memory: State<'_, std::sync::Arc<crate::memory_advanced::AdvancedMemoryManager>>,
    source_id: String,
    target_id: String,
    relation_type: String,
) -> Result<(), String> {
    let rel_type = match relation_type.as_str() {
        "related" => RelationType::Related,
        "supersedes" => RelationType::Supersedes,
        "complements" => RelationType::Complements,
        "contradicts" => RelationType::Contradicts,
        "references" => RelationType::References,
        _ => RelationType::Related,
    };

    advanced_memory
        .link_memories(&source_id, &target_id, rel_type)
        .await
        .map_err(|e| format!("Failed to link memories: {}", e))
}

#[tauri::command]
pub async fn find_similar_memories(
    advanced_memory: State<'_, std::sync::Arc<crate::memory_advanced::AdvancedMemoryManager>>,
    content: String,
    threshold: f32,
) -> Result<Vec<String>, String> {
    advanced_memory
        .find_similar(&content, threshold)
        .await
        .map_err(|e| format!("Failed to find similar: {}", e))
}

#[tauri::command]
pub async fn verify_memory(
    advanced_memory: State<'_, std::sync::Arc<crate::memory_advanced::AdvancedMemoryManager>>,
    memory_id: String,
) -> Result<(), String> {
    advanced_memory
        .verify_memory(&memory_id)
        .await
        .map_err(|e| format!("Failed to verify memory: {}", e))
}

#[tauri::command]
pub async fn set_memory_importance(
    advanced_memory: State<'_, std::sync::Arc<crate::memory_advanced::AdvancedMemoryManager>>,
    memory_id: String,
    importance_score: f32,
) -> Result<(), String> {
    let importance = Importance::new(importance_score);
    advanced_memory
        .set_importance(&memory_id, importance)
        .await
        .map_err(|e| format!("Failed to set importance: {}", e))
}

#[tauri::command]
pub async fn get_related_memories(
    advanced_memory: State<'_, std::sync::Arc<crate::memory_advanced::AdvancedMemoryManager>>,
    memory_id: String,
) -> Result<Vec<serde_json::Value>, String> {
    match advanced_memory.get_related_memories(&memory_id).await {
        Ok(memories) => {
            let result = memories
                .iter()
                .map(|m| {
                    json!({
                        "id": m.id,
                        "content": m.content,
                        "importance": m.importance.0,
                        "verified": m.verified,
                        "source": m.source,
                        "confidence": m.confidence,
                    })
                })
                .collect();
            Ok(result)
        }
        Err(e) => Err(format!("Failed to get related memories: {}", e)),
    }
}

#[tauri::command]
pub async fn cleanup_expired_memories(
    advanced_memory: State<'_, std::sync::Arc<crate::memory_advanced::AdvancedMemoryManager>>,
) -> Result<usize, String> {
    advanced_memory
        .cleanup_expired()
        .await
        .map_err(|e| format!("Cleanup failed: {}", e))
}

#[tauri::command]
pub async fn get_memory_analytics(
    advanced_memory: State<'_, std::sync::Arc<crate::memory_advanced::AdvancedMemoryManager>>,
) -> Result<serde_json::Value, String> {
    match advanced_memory.calculate_analytics().await {
        Ok(analytics) => {
            let mut dist = serde_json::Map::new();
            for (k, v) in analytics.importance_distribution {
                dist.insert(k, json!(v));
            }

            Ok(json!({
                "total_memories": analytics.total_memories,
                "importance_distribution": dist,
                "access_frequency": analytics.access_frequency,
                "average_age_days": analytics.average_age_days,
                "redundancy_score": analytics.redundancy_score,
                "predicted_eviction_count": analytics.predicted_eviction_count,
                "retention_rate": analytics.retention_rate,
            }))
        }
        Err(e) => Err(format!("Failed to calculate analytics: {}", e)),
    }
}

#[tauri::command]
pub async fn export_memories(
    advanced_memory: State<'_, std::sync::Arc<crate::memory_advanced::AdvancedMemoryManager>>,
    include_relationships: bool,
) -> Result<serde_json::Value, String> {
    match advanced_memory.export_memories(include_relationships).await {
        Ok(export) => Ok(json!({
            "timestamp": export.timestamp,
            "memory_count": export.memories.len(),
            "relationship_count": export.relationships.len(),
        })),
        Err(e) => Err(format!("Export failed: {}", e)),
    }
}

#[tauri::command]
pub async fn search_advanced_memories(
    advanced_memory: State<'_, std::sync::Arc<crate::memory_advanced::AdvancedMemoryManager>>,
    query: String,
    min_importance: Option<f32>,
    include_unverified: bool,
) -> Result<Vec<serde_json::Value>, String> {
    match advanced_memory
        .search(&query, min_importance, include_unverified)
        .await
    {
        Ok(memories) => {
            let result = memories
                .iter()
                .map(|m| {
                    json!({
                        "id": m.id,
                        "content": m.content,
                        "importance": m.importance.0,
                        "verified": m.verified,
                        "source": m.source,
                        "confidence": m.confidence,
                        "created_at": m.created_at,
                    })
                })
                .collect();
            Ok(result)
        }
        Err(e) => Err(format!("Search failed: {}", e)),
    }
}
