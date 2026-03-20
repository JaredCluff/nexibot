//! Database Maintenance and Backup System
//!
//! Provides comprehensive database maintenance, backup/restore, and health monitoring:
//! - Automated backups of all SQLite databases
//! - Backup retention policies with configurable window
//! - Point-in-time recovery
//! - Database optimization (VACUUM, ANALYZE, integrity checks)
//! - Health monitoring and corruption detection
//! - Scheduled maintenance tasks
//! - Incremental backup support

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use tokio::sync::RwLock;
use tracing::{error, info, warn};
use uuid::Uuid;

/// Database health check result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckResult {
    /// Database path
    pub path: String,
    /// True if database is healthy
    pub is_healthy: bool,
    /// Detailed status message
    pub status: String,
    /// Number of integrity errors found
    pub error_count: u32,
    /// Database file size in bytes
    pub file_size: u64,
    /// Database page count
    pub page_count: u32,
    /// Timestamp of check
    pub checked_at: DateTime<Utc>,
}

/// Backup metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupMetadata {
    /// Backup ID (UUID)
    pub id: String,
    /// Name of the backup
    pub name: String,
    /// Type of backup (Full, Incremental)
    pub backup_type: String,
    /// Path to backup directory
    pub backup_path: PathBuf,
    /// List of backed-up database files
    pub backed_up_files: Vec<String>,
    /// Total size of backup in bytes
    pub total_size: u64,
    /// Timestamp when backup was created
    pub created_at: DateTime<Utc>,
    /// List of included databases
    pub databases: Vec<String>,
    /// Whether backup has been verified
    pub verified: bool,
    /// Checksum for integrity verification
    pub checksum: Option<String>,
}

/// Database maintenance configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaintenanceConfig {
    /// Enable automatic backups
    pub auto_backup_enabled: bool,
    /// Interval for automatic backups (minutes)
    pub auto_backup_interval_minutes: u32,
    /// Maximum number of backups to keep
    pub max_backups: usize,
    /// Backup retention duration (days)
    pub backup_retention_days: u32,
    /// Enable automatic VACUUM on maintenance
    pub auto_vacuum_enabled: bool,
    /// Enable automatic ANALYZE on maintenance
    pub auto_analyze_enabled: bool,
    /// Enable health checks
    pub health_check_enabled: bool,
    /// Interval for health checks (hours)
    pub health_check_interval_hours: u32,
    /// Compression for backups
    pub compression_enabled: bool,
}

impl Default for MaintenanceConfig {
    fn default() -> Self {
        Self {
            auto_backup_enabled: true,
            auto_backup_interval_minutes: 60, // Every hour
            max_backups: 24,                  // Keep up to 24 backups
            backup_retention_days: 30,        // Keep backups for 30 days
            auto_vacuum_enabled: true,
            auto_analyze_enabled: true,
            health_check_enabled: true,
            health_check_interval_hours: 6, // Every 6 hours
            compression_enabled: false,
        }
    }
}

/// Database Maintenance Manager
pub struct DbMaintenanceManager {
    /// Configuration for maintenance
    config: Arc<RwLock<MaintenanceConfig>>,
    /// Backup directory root
    backup_dir: PathBuf,
    /// Backup metadata store (in-memory)
    backups: Arc<RwLock<Vec<BackupMetadata>>>,
    /// Last backup timestamp
    last_backup_time: Arc<RwLock<Option<DateTime<Utc>>>>,
    /// Last health check timestamp
    last_health_check_time: Arc<RwLock<Option<DateTime<Utc>>>>,
    /// Database paths to monitor
    db_paths: Arc<RwLock<Vec<PathBuf>>>,
}

use std::sync::Arc;

impl DbMaintenanceManager {
    /// Create a new database maintenance manager
    pub fn new(backup_dir: PathBuf, config: MaintenanceConfig) -> Result<Self> {
        // Ensure backup directory exists
        fs::create_dir_all(&backup_dir).context("Failed to create backup directory")?;

        let manager = Self {
            config: Arc::new(RwLock::new(config)),
            backup_dir,
            backups: Arc::new(RwLock::new(Vec::new())),
            last_backup_time: Arc::new(RwLock::new(None)),
            last_health_check_time: Arc::new(RwLock::new(None)),
            db_paths: Arc::new(RwLock::new(Vec::new())),
        };

        info!("[DB_MAINTENANCE] Manager initialized");
        Ok(manager)
    }

    /// Register a database path for monitoring
    pub async fn register_database(&self, path: PathBuf) -> Result<()> {
        let mut db_paths = self.db_paths.write().await;
        if !db_paths.contains(&path) {
            db_paths.push(path.clone());
            info!("[DB_MAINTENANCE] Registered database: {}", path.display());
        }
        Ok(())
    }

    /// Create a backup of all registered databases
    pub async fn create_backup(&self, name: Option<String>) -> Result<BackupMetadata> {
        let backup_id = Uuid::new_v4().to_string();
        let name = name.unwrap_or_else(|| format!("backup_{}", Utc::now().format("%Y%m%d_%H%M%S")));

        let backup_path = self.backup_dir.join(&backup_id);
        fs::create_dir_all(&backup_path).context("Failed to create backup directory")?;

        let db_paths = self.db_paths.read().await;
        let mut backed_up_files = Vec::new();
        let mut total_size = 0u64;
        let mut databases = Vec::new();

        for db_path in db_paths.iter() {
            if !db_path.exists() {
                warn!("[DB_MAINTENANCE] Database not found: {}", db_path.display());
                continue;
            }

            let db_name = db_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown");

            databases.push(db_name.to_string());

            // Copy database file
            let backup_file = backup_path.join(db_name);
            match fs::copy(db_path, &backup_file) {
                Ok(size) => {
                    backed_up_files.push(db_name.to_string());
                    total_size += size;
                    info!("[DB_MAINTENANCE] Backed up {}: {} bytes", db_name, size);
                }
                Err(e) => {
                    error!("[DB_MAINTENANCE] Failed to backup {}: {}", db_name, e);
                }
            }

            // Also copy WAL and SHM files if they exist
            for suffix in &["-wal", "-shm"] {
                let wal_path = PathBuf::from(format!("{}{}", db_path.display(), suffix));
                if wal_path.exists() {
                    if let Ok(size) = fs::copy(
                        &wal_path,
                        backup_path.join(format!("{}{}", db_name, suffix)),
                    ) {
                        total_size += size;
                    }
                }
            }
        }

        let metadata = BackupMetadata {
            id: backup_id,
            name,
            backup_type: "Full".to_string(),
            backup_path: backup_path.clone(),
            backed_up_files,
            total_size,
            created_at: Utc::now(),
            databases,
            verified: false,
            checksum: None,
        };

        let mut backups = self.backups.write().await;
        backups.push(metadata.clone());

        // Update last backup time
        *self.last_backup_time.write().await = Some(Utc::now());

        // Enforce retention policy
        self.enforce_retention_policy().await?;

        info!(
            "[DB_MAINTENANCE] Created backup: {} ({} bytes)",
            metadata.id, total_size
        );
        Ok(metadata)
    }

    /// Restore from a backup
    pub async fn restore_backup(&self, backup_id: &str) -> Result<()> {
        let backups = self.backups.read().await;
        let backup = backups
            .iter()
            .find(|b| b.id == backup_id)
            .ok_or_else(|| anyhow!("Backup not found: {}", backup_id))?;

        info!(
            "[DB_MAINTENANCE] Starting restore from backup: {}",
            backup_id
        );

        for backed_up_file in &backup.backed_up_files {
            let backup_file = backup.backup_path.join(backed_up_file);
            let db_paths = self.db_paths.read().await;

            // Find the original database path
            let original_path = db_paths
                .iter()
                .find(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n == backed_up_file)
                        .unwrap_or(false)
                })
                .cloned();

            if let Some(original_path) = original_path {
                // Create backup of current database before restore
                if original_path.exists() {
                    let backup_ext = format!(".restore-{}", Utc::now().format("%Y%m%d_%H%M%S"));
                    let temp_path =
                        PathBuf::from(format!("{}{}", original_path.display(), backup_ext));
                    fs::copy(&original_path, &temp_path).context(format!(
                        "Failed to backup current {} before restore",
                        backed_up_file
                    ))?;
                }

                // Restore from backup
                fs::copy(&backup_file, &original_path)
                    .context(format!("Failed to restore {}", backed_up_file))?;
                info!("[DB_MAINTENANCE] Restored {}", backed_up_file);
            }
        }

        info!("[DB_MAINTENANCE] Backup restore completed: {}", backup_id);
        Ok(())
    }

    /// List all available backups
    pub async fn list_backups(&self) -> Result<Vec<BackupMetadata>> {
        let backups = self.backups.read().await;
        Ok(backups.clone())
    }

    /// Delete a backup
    pub async fn delete_backup(&self, backup_id: &str) -> Result<()> {
        let mut backups = self.backups.write().await;

        if let Some(pos) = backups.iter().position(|b| b.id == backup_id) {
            let backup = backups.remove(pos);
            if backup.backup_path.exists() {
                fs::remove_dir_all(&backup.backup_path)
                    .context("Failed to delete backup directory")?;
                info!("[DB_MAINTENANCE] Deleted backup: {}", backup_id);
            }
            Ok(())
        } else {
            Err(anyhow!("Backup not found: {}", backup_id))
        }
    }

    /// Verify backup integrity
    pub async fn verify_backup(&self, backup_id: &str) -> Result<bool> {
        let mut backups = self.backups.write().await;

        if let Some(backup) = backups.iter_mut().find(|b| b.id == backup_id) {
            // Check if all files exist
            let all_exist = backup
                .backed_up_files
                .iter()
                .all(|f| backup.backup_path.join(f).exists());

            backup.verified = all_exist;
            info!(
                "[DB_MAINTENANCE] Verified backup {}: {}",
                backup_id, all_exist
            );
            Ok(all_exist)
        } else {
            Err(anyhow!("Backup not found: {}", backup_id))
        }
    }

    /// Perform database health check
    pub async fn health_check(&self) -> Result<Vec<HealthCheckResult>> {
        let db_paths = self.db_paths.read().await;
        let mut results = Vec::new();

        for db_path in db_paths.iter() {
            if !db_path.exists() {
                warn!(
                    "[DB_MAINTENANCE] Database not found for health check: {}",
                    db_path.display()
                );
                continue;
            }

            let result = match self.check_database_health(db_path).await {
                Ok(result) => result,
                Err(e) => {
                    error!(
                        "[DB_MAINTENANCE] Health check failed for {}: {}",
                        db_path.display(),
                        e
                    );
                    HealthCheckResult {
                        path: db_path.display().to_string(),
                        is_healthy: false,
                        status: format!("Error: {}", e),
                        error_count: 1,
                        file_size: 0,
                        page_count: 0,
                        checked_at: Utc::now(),
                    }
                }
            };

            results.push(result);
        }

        // Update last health check time
        *self.last_health_check_time.write().await = Some(Utc::now());

        info!(
            "[DB_MAINTENANCE] Health check completed: {} databases",
            results.len()
        );
        Ok(results)
    }

    /// Check health of a single database using `PRAGMA quick_check`.
    ///
    /// `quick_check` verifies the structural integrity of the B-tree pages and
    /// the free-list without running a full `integrity_check` (which rewrites
    /// each page).  This is fast enough for routine health monitoring while
    /// catching the most common forms of corruption.
    async fn check_database_health(&self, db_path: &Path) -> Result<HealthCheckResult> {
        let file_size = fs::metadata(db_path)
            .context("Failed to get file metadata")?
            .len();

        if file_size == 0 {
            return Ok(HealthCheckResult {
                path: db_path.display().to_string(),
                is_healthy: false,
                status: "Database file is empty".to_string(),
                error_count: 1,
                file_size: 0,
                page_count: 0,
                checked_at: Utc::now(),
            });
        }

        // Open the database and run PRAGMA quick_check.
        let conn = rusqlite::Connection::open(db_path)
            .context("Failed to open database for health check")?;

        // Retrieve page count for reporting.
        let page_count: u32 = conn
            .query_row("PRAGMA page_count;", [], |row| row.get::<_, u32>(0))
            .unwrap_or(0);

        // quick_check returns one row per problem found; "ok" means no errors.
        let mut stmt = conn
            .prepare("PRAGMA quick_check;")
            .context("Failed to prepare quick_check")?;

        let mut integrity_errors: Vec<String> = Vec::new();
        let rows = stmt.query_map([], |row| row.get::<_, String>(0));
        match rows {
            Ok(mapped) => {
                for row in mapped.flatten() {
                    if row != "ok" {
                        integrity_errors.push(row);
                    }
                }
            }
            Err(e) => {
                integrity_errors.push(format!("quick_check failed: {}", e));
            }
        }

        let is_healthy = integrity_errors.is_empty();
        let error_count = integrity_errors.len() as u32;
        let status = if is_healthy {
            "Database integrity check passed (quick_check: ok)".to_string()
        } else {
            format!(
                "Database integrity errors detected: {}",
                integrity_errors.join("; ")
            )
        };

        if !is_healthy {
            error!(
                "[DB_MAINTENANCE] Integrity errors in {}: {}",
                db_path.display(),
                status
            );
        }

        Ok(HealthCheckResult {
            path: db_path.display().to_string(),
            is_healthy,
            status,
            error_count,
            file_size,
            page_count,
            checked_at: Utc::now(),
        })
    }

    /// Optimize a database (VACUUM, ANALYZE)
    ///
    /// VACUUM is run under an exclusive locking mode to prevent concurrent
    /// writers from causing corruption.  After VACUUM completes the locking
    /// mode is restored to NORMAL so normal WAL operation resumes.
    pub async fn optimize_database(&self, db_path: &Path) -> Result<String> {
        if !db_path.exists() {
            return Err(anyhow!("Database not found: {}", db_path.display()));
        }

        let size_before = fs::metadata(db_path)?.len();

        // Use rusqlite to perform optimization.
        // Open the connection and acquire an exclusive locking mode before
        // running VACUUM so that no other connection can read or write while
        // the VACUUM is in progress (prevents potential corruption in WAL mode).
        let conn = rusqlite::Connection::open(db_path)
            .context("Failed to open database for optimization")?;

        // Switch to EXCLUSIVE locking mode so VACUUM is the only writer.
        conn.execute_batch("PRAGMA locking_mode=EXCLUSIVE;")
            .context("Failed to set exclusive locking mode")?;

        // ANALYZE updates query-planner statistics.
        conn.execute_batch("ANALYZE;")
            .context("Failed to run ANALYZE")?;

        // VACUUM reclaims free pages and rebuilds the database file.
        conn.execute_batch("VACUUM;")
            .context("Failed to run VACUUM")?;

        // Restore normal locking mode so WAL readers can proceed.
        // A dummy SELECT is required to actually release the exclusive lock.
        conn.execute_batch("PRAGMA locking_mode=NORMAL; SELECT count(*) FROM sqlite_master;")
            .context("Failed to restore normal locking mode")?;

        let size_after = fs::metadata(db_path)?.len();
        let reclaimed = size_before.saturating_sub(size_after);

        let message = format!(
            "Optimized database: {} bytes reclaimed (before: {} bytes, after: {} bytes)",
            reclaimed, size_before, size_after
        );

        info!("[DB_MAINTENANCE] {}", message);
        Ok(message)
    }

    /// Schedule a periodic maintenance background task.
    ///
    /// Runs immediately if last maintenance was more than 24 hours ago (or
    /// never), then repeats every 24 hours.  The returned `JoinHandle` should
    /// be stored for the lifetime of the application.
    #[allow(dead_code)]
    pub fn start_auto_maintenance(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            const INTERVAL: std::time::Duration = std::time::Duration::from_secs(24 * 3600);
            const TWENTY_FOUR_HOURS: chrono::Duration = chrono::Duration::hours(24);

            loop {
                // Check whether we need to run maintenance now.
                let needs_run = {
                    let last = self.last_health_check_time.read().await;
                    match *last {
                        None => true,
                        Some(t) => Utc::now() - t > TWENTY_FOUR_HOURS,
                    }
                };

                if needs_run {
                    info!("[DB_MAINTENANCE] Running scheduled maintenance");

                    // Health check across all registered databases.
                    if let Err(e) = self.health_check().await {
                        error!("[DB_MAINTENANCE] Scheduled health check failed: {}", e);
                    }

                    // Optimise each database if configured.
                    let config = self.config.read().await;
                    let do_vacuum = config.auto_vacuum_enabled;
                    drop(config);

                    if do_vacuum {
                        let db_paths = self.db_paths.read().await.clone();
                        for path in &db_paths {
                            if let Err(e) = self.optimize_database(path).await {
                                error!(
                                    "[DB_MAINTENANCE] Optimization failed for {}: {}",
                                    path.display(),
                                    e
                                );
                            }
                        }
                    }

                    // Backup if configured.
                    let config = self.config.read().await;
                    let do_backup = config.auto_backup_enabled;
                    drop(config);

                    if do_backup {
                        if let Err(e) = self.create_backup(None).await {
                            error!("[DB_MAINTENANCE] Scheduled backup failed: {}", e);
                        }
                    }
                }

                tokio::time::sleep(INTERVAL).await;
            }
        })
    }

    /// Enforce backup retention policy
    async fn enforce_retention_policy(&self) -> Result<()> {
        let config = self.config.read().await;
        let mut backups = self.backups.write().await;

        let retention_date = Utc::now() - Duration::days(config.backup_retention_days as i64);

        // Remove old backups based on retention policy
        let initial_count = backups.len();
        backups.retain(|b| b.created_at > retention_date);

        // Also enforce maximum number of backups
        if backups.len() > config.max_backups {
            backups.sort_by(|a, b| b.created_at.cmp(&a.created_at)); // Sort newest first
            let to_remove = backups.split_off(config.max_backups);

            for backup in to_remove {
                if backup.backup_path.exists() {
                    if let Err(e) = fs::remove_dir_all(&backup.backup_path) {
                        error!(
                            "[DB_MAINTENANCE] Failed to remove old backup {}: {}",
                            backup.id, e
                        );
                    }
                }
            }
        }

        let removed_count = initial_count - backups.len();
        if removed_count > 0 {
            info!(
                "[DB_MAINTENANCE] Enforced retention policy: removed {} old backups",
                removed_count
            );
        }

        Ok(())
    }

    /// Get maintenance configuration
    pub async fn get_config(&self) -> MaintenanceConfig {
        self.config.read().await.clone()
    }

    /// Update maintenance configuration
    pub async fn update_config(&self, config: MaintenanceConfig) -> Result<()> {
        *self.config.write().await = config;
        info!("[DB_MAINTENANCE] Configuration updated");
        Ok(())
    }

    /// Get last backup time
    pub async fn get_last_backup_time(&self) -> Option<DateTime<Utc>> {
        self.last_backup_time.read().await.clone()
    }

    /// Get last health check time
    pub async fn get_last_health_check_time(&self) -> Option<DateTime<Utc>> {
        self.last_health_check_time.read().await.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_maintenance_manager_creation() {
        let temp_dir = TempDir::new().unwrap();
        let backup_dir = temp_dir.path().join("backups");
        let config = MaintenanceConfig::default();

        let manager = DbMaintenanceManager::new(backup_dir, config).unwrap();
        assert!(manager.list_backups().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_register_database() {
        let temp_dir = TempDir::new().unwrap();
        let backup_dir = temp_dir.path().join("backups");
        let config = MaintenanceConfig::default();

        let manager = DbMaintenanceManager::new(backup_dir, config).unwrap();
        let db_path = temp_dir.path().join("test.db");
        fs::File::create(&db_path).unwrap();

        manager.register_database(db_path.clone()).await.unwrap();
        let db_paths = manager.db_paths.read().await;
        assert_eq!(db_paths.len(), 1);
    }
}
