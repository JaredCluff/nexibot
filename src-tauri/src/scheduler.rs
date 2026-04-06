//! Scheduled task runner.
//!
//! Supports simplified schedule formats:
//! - `"daily HH:MM"` — runs once per day at the specified time
//! - `"hourly"` — runs at the top of every hour
//! - `"every Nm"` — runs every N minutes
//! - `"weekly DAY HH:MM"` — runs once per week on the specified day
//! - `"cron EXPR"` — standard 5-field cron expression (minute hour day month weekday)

use anyhow::Result;
use chrono::{DateTime, Datelike, Duration, NaiveDateTime, NaiveTime, Timelike, Utc, Weekday};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::claude::ClaudeClient;
use crate::config::NexiBotConfig;
use crate::notifications::{NotificationDispatcher, NotificationTarget};
use crate::session_overrides::SessionOverrides;

/// Maximum number of recent results to keep.
const MAX_RESULTS: usize = 50;

// ── Cron Expression Support ──────────────────────────────────────────

/// A parsed 5-field cron schedule (minute, hour, day-of-month, month, day-of-week).
#[derive(Debug, Clone, PartialEq)]
pub struct CronSchedule {
    pub minutes: Vec<u32>,  // 0-59
    pub hours: Vec<u32>,    // 0-23
    pub days: Vec<u32>,     // 1-31
    pub months: Vec<u32>,   // 1-12
    pub weekdays: Vec<u32>, // 0-6 (0=Sunday)
}

/// Parse a single cron field into a sorted, deduplicated list of values.
///
/// Supports: specific values (5), ranges (1-5), lists (1,3,5), wildcards (*),
/// and step values (*/5 or 1-10/2).
fn parse_cron_field(field: &str, min: u32, max: u32) -> Result<Vec<u32>> {
    let mut values = Vec::new();

    for part in field.split(',') {
        let part = part.trim();
        if part.is_empty() {
            anyhow::bail!("Empty field component");
        }

        // Check for step: "*/5" or "1-10/2"
        let (range_part, step) = if let Some(idx) = part.find('/') {
            let step_str = &part[idx + 1..];
            let step: u32 = step_str
                .parse()
                .map_err(|_| anyhow::anyhow!("Invalid step value: '{}'", step_str))?;
            if step == 0 {
                anyhow::bail!("Step value must be > 0");
            }
            (&part[..idx], Some(step))
        } else {
            (part, None)
        };

        // Determine the range
        let (start, end) = if range_part == "*" {
            (min, max)
        } else if let Some(dash_idx) = range_part.find('-') {
            let s: u32 = range_part[..dash_idx].parse().map_err(|_| {
                anyhow::anyhow!("Invalid range start: '{}'", &range_part[..dash_idx])
            })?;
            let e: u32 = range_part[dash_idx + 1..].parse().map_err(|_| {
                anyhow::anyhow!("Invalid range end: '{}'", &range_part[dash_idx + 1..])
            })?;
            if s < min || e > max || s > e {
                anyhow::bail!("Range {}-{} out of bounds [{}, {}]", s, e, min, max);
            }
            (s, e)
        } else {
            // Single value
            let v: u32 = range_part
                .parse()
                .map_err(|_| anyhow::anyhow!("Invalid value: '{}'", range_part))?;
            if v < min || v > max {
                anyhow::bail!("Value {} out of bounds [{}, {}]", v, min, max);
            }
            (v, v)
        };

        match step {
            Some(step) => {
                if step == 0 {
                    anyhow::bail!("Cron step value must be > 0");
                }
                let mut v = start;
                while v <= end {
                    values.push(v);
                    match v.checked_add(step) {
                        Some(next) => v = next,
                        None => break, // step would wrap past u32::MAX; no more values
                    }
                }
            }
            None => {
                for v in start..=end {
                    values.push(v);
                }
            }
        }
    }

    values.sort();
    values.dedup();

    if values.is_empty() {
        anyhow::bail!("Cron field produced no values");
    }

    Ok(values)
}

/// Parse a 5-field cron expression string: "minute hour day month weekday".
///
/// # Examples
/// - `"*/5 * * * *"` — every 5 minutes
/// - `"0 9 * * 1-5"` — 9:00 AM on weekdays
/// - `"30 2 1 * *"` — 2:30 AM on the 1st of each month
pub fn parse_cron_expression(expr: &str) -> Result<CronSchedule> {
    let fields: Vec<&str> = expr.split_whitespace().collect();
    if fields.len() != 5 {
        anyhow::bail!(
            "Cron expression must have exactly 5 fields (minute hour day month weekday), got {}",
            fields.len()
        );
    }

    let minutes = parse_cron_field(fields[0], 0, 59)?;
    let hours = parse_cron_field(fields[1], 0, 23)?;
    let days = parse_cron_field(fields[2], 1, 31)?;
    let months = parse_cron_field(fields[3], 1, 12)?;
    let weekdays = parse_cron_field(fields[4], 0, 6)?;

    Ok(CronSchedule {
        minutes,
        hours,
        days,
        months,
        weekdays,
    })
}

impl CronSchedule {
    /// Check if a given datetime matches this cron schedule.
    pub fn matches(&self, dt: &DateTime<Utc>) -> bool {
        let minute = dt.minute();
        let hour = dt.hour();
        let day = dt.day();
        let month = dt.month();
        // chrono weekday: Mon=0..Sun=6, cron weekday: Sun=0..Sat=6
        let weekday = match dt.weekday() {
            Weekday::Sun => 0,
            Weekday::Mon => 1,
            Weekday::Tue => 2,
            Weekday::Wed => 3,
            Weekday::Thu => 4,
            Weekday::Fri => 5,
            Weekday::Sat => 6,
        };

        // POSIX cron: when both day-of-month and day-of-week are restricted
        // (neither is a full wildcard), either condition matching fires the job.
        let days_wildcard = self.days.len() == 31;    // * expands to 1..=31
        let weekdays_wildcard = self.weekdays.len() == 7; // * expands to 0..=6
        let day_cond = if days_wildcard || weekdays_wildcard {
            self.days.contains(&day) && self.weekdays.contains(&weekday)
        } else {
            self.days.contains(&day) || self.weekdays.contains(&weekday)
        };

        self.minutes.contains(&minute)
            && self.hours.contains(&hour)
            && self.months.contains(&month)
            && day_cond
    }

    /// Find the next datetime after `after` that matches this cron schedule.
    /// Searches up to 366 days ahead. Returns None if no match is found.
    pub fn next_occurrence(&self, after: &DateTime<Utc>) -> Option<DateTime<Utc>> {
        // Start from the next minute after `after`
        let start = *after + Duration::minutes(1);
        // Zero out seconds/nanos
        let start_naive = NaiveDateTime::new(
            start.date_naive(),
            NaiveTime::from_hms_opt(start.hour(), start.minute(), 0)?,
        );
        let limit = start_naive + Duration::days(366);

        let mut current_date = start_naive.date();
        let start_time_on_first_day = start_naive.time();

        while NaiveDateTime::new(current_date, NaiveTime::from_hms_opt(23, 59, 0)?) <= limit {
            let month = current_date.month();
            let day = current_date.day();
            let weekday_num = match current_date.weekday() {
                Weekday::Sun => 0,
                Weekday::Mon => 1,
                Weekday::Tue => 2,
                Weekday::Wed => 3,
                Weekday::Thu => 4,
                Weekday::Fri => 5,
                Weekday::Sat => 6,
            };

            // Check if this date matches month, day, and weekday constraints.
            // POSIX cron: if both day-of-month and day-of-week are restricted,
            // either matching fires the job (OR semantics).
            let days_wildcard = self.days.len() == 31;
            let weekdays_wildcard = self.weekdays.len() == 7;
            let day_cond = if days_wildcard || weekdays_wildcard {
                self.days.contains(&day) && self.weekdays.contains(&weekday_num)
            } else {
                self.days.contains(&day) || self.weekdays.contains(&weekday_num)
            };

            if self.months.contains(&month) && day_cond {
                // Find the earliest matching time on this date
                for &hour in &self.hours {
                    for &minute in &self.minutes {
                        let candidate_time = NaiveTime::from_hms_opt(hour, minute, 0)?;
                        if current_date == start_naive.date()
                            && candidate_time < start_time_on_first_day
                        {
                            continue;
                        }
                        let candidate = NaiveDateTime::new(current_date, candidate_time);
                        let dt = DateTime::<Utc>::from_naive_utc_and_offset(candidate, Utc);
                        // Final verification (should always pass, but belt-and-suspenders)
                        if self.matches(&dt) {
                            return Some(dt);
                        }
                    }
                }
            }

            current_date = current_date.succ_opt()?;
        }

        None
    }
}

/// Result of a scheduled task execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskExecutionResult {
    pub task_id: String,
    pub task_name: String,
    pub response: String,
    pub timestamp: DateTime<Utc>,
    pub success: bool,
}

/// The scheduler manages periodic task execution.
pub struct Scheduler {
    config: Arc<RwLock<NexiBotConfig>>,
    claude_client: Arc<RwLock<ClaudeClient>>,
    notification_dispatcher: Arc<NotificationDispatcher>,
    results: Arc<RwLock<VecDeque<TaskExecutionResult>>>,
}

impl Scheduler {
    pub fn new(
        config: Arc<RwLock<NexiBotConfig>>,
        claude_client: Arc<RwLock<ClaudeClient>>,
        notification_dispatcher: Arc<NotificationDispatcher>,
    ) -> Self {
        Self {
            config,
            claude_client,
            notification_dispatcher,
            results: Arc::new(RwLock::new(VecDeque::new())),
        }
    }

    /// Get recent execution results.
    pub async fn get_results(&self) -> Vec<TaskExecutionResult> {
        self.results.read().await.iter().cloned().collect()
    }

    /// Execute a specific task by sending its prompt to Claude.
    pub async fn execute_task(&self, task_id: &str) -> Result<TaskExecutionResult, String> {
        let (task_name, prompt) = {
            let config = self.config.read().await;
            let task = config
                .scheduled_tasks
                .tasks
                .iter()
                .find(|t| t.id == task_id)
                .ok_or_else(|| format!("Task not found: {}", task_id))?;
            (task.name.clone(), task.prompt.clone())
        };

        info!("[SCHEDULER] Executing task '{}': {}", task_name, task_id);

        let claude_client = self.claude_client.read().await;
        let default_overrides = SessionOverrides::default();
        let result = match claude_client
            .send_message_with_tools(&prompt, &[], &default_overrides)
            .await
        {
            Ok(r) => TaskExecutionResult {
                task_id: task_id.to_string(),
                task_name: task_name.clone(),
                response: r.text,
                timestamp: Utc::now(),
                success: true,
            },
            Err(e) => TaskExecutionResult {
                task_id: task_id.to_string(),
                task_name: task_name.clone(),
                response: format!("Error: {}", e),
                timestamp: Utc::now(),
                success: false,
            },
        };

        // Update last_run
        {
            let mut config = self.config.write().await;
            let previous_config = config.clone();
            if let Some(task) = config
                .scheduled_tasks
                .tasks
                .iter_mut()
                .find(|t| t.id == task_id)
            {
                task.last_run = Some(Utc::now());
            }
            // config.save() does synchronous file I/O; use block_in_place to avoid
            // blocking the async executor thread.
            let save_result = tokio::task::block_in_place(|| config.save());
            if let Err(e) = save_result {
                *config = previous_config;
                warn!(
                    "[SCHEDULER] Failed to persist last_run for task '{}': {}",
                    task_id, e
                );
            }
        }

        // Store result in ring buffer (O(1) push/pop with VecDeque)
        {
            let mut results = self.results.write().await;
            if results.len() >= MAX_RESULTS {
                results.pop_front();
            }
            results.push_back(result.clone());
        }

        info!(
            "[SCHEDULER] Task '{}' completed (success: {})",
            task_name, result.success
        );

        // Route scheduled-task completion alerts through the shared dispatcher
        // so delivery behavior is consistent with background task notifications.
        let notify_msg = if result.success {
            format!(
                "\u{2705} Scheduled task *{}* completed successfully.",
                result.task_name
            )
        } else {
            format!(
                "\u{274c} Scheduled task *{}* failed: {}",
                result.task_name, result.response
            )
        };
        let attempted = self
            .notification_dispatcher
            .dispatch(&NotificationTarget::AllConfigured, &notify_msg)
            .await;
        if !attempted {
            warn!(
                "[SCHEDULER] No delivery targets configured for task '{}' ({}) completion alert",
                task_name, task_id
            );
        }

        Ok(result)
    }

    /// Start the scheduler loop. Should be spawned as a tokio task.
    pub async fn run_loop(self: Arc<Self>, app_handle: tauri::AppHandle) {
        info!("[SCHEDULER] Scheduler loop started (checking every 60s)");

        // Check for missed tasks on startup
        self.check_missed_tasks(&app_handle).await;

        loop {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;

            let (enabled, tasks) = {
                let config = self.config.read().await;
                (
                    config.scheduled_tasks.enabled,
                    config.scheduled_tasks.tasks.clone(),
                )
            };

            if !enabled {
                continue;
            }

            let now = Utc::now();

            for task in &tasks {
                if !task.enabled {
                    continue;
                }

                if should_run_now(task, now) {
                    let result = self.execute_task(&task.id).await;

                    // Emit event to frontend
                    if let Ok(result) = &result {
                        use tauri::Emitter;
                        if let Err(e) = app_handle.emit("scheduler:task-complete", result) {
                            warn!("[SCHEDULER] Failed to emit scheduler:task-complete: {}", e);
                        }
                    }
                }
            }
        }
    }

    /// Run the scheduler loop without a Tauri AppHandle (headless / container mode).
    ///
    /// Task completion events are logged instead of being emitted to the frontend.
    pub async fn run_headless_loop(self: Arc<Self>) {
        info!("[SCHEDULER] Headless scheduler loop started (checking every 60s)");
        self.check_missed_tasks_headless().await;
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            let (enabled, tasks) = {
                let config = self.config.read().await;
                (
                    config.scheduled_tasks.enabled,
                    config.scheduled_tasks.tasks.clone(),
                )
            };
            if !enabled {
                continue;
            }
            let now = Utc::now();
            for task in &tasks {
                if !task.enabled {
                    continue;
                }
                if should_run_now(task, now) {
                    match self.execute_task(&task.id).await {
                        Ok(r) => info!(
                            "[SCHEDULER] Task '{}' completed: success={}",
                            task.id, r.success
                        ),
                        Err(e) => tracing::warn!("[SCHEDULER] Task '{}' failed: {}", task.id, e),
                    }
                }
            }
        }
    }

    async fn check_missed_tasks_headless(&self) {
        let tasks = {
            let config = self.config.read().await;
            if !config.scheduled_tasks.enabled {
                return;
            }
            config.scheduled_tasks.tasks.clone()
        };
        let now = Utc::now();
        for task in &tasks {
            if !task.enabled || !task.run_if_missed {
                continue;
            }
            if let Some(last_run) = task.last_run {
                if should_have_run_since(task, last_run, now) {
                    let jitter_ms = (rand::random::<u32>() % 3000) as u64;
                    tokio::time::sleep(std::time::Duration::from_millis(jitter_ms)).await;

                    info!("[SCHEDULER] Missed task '{}', running now", task.name);
                    match self.execute_task(&task.id).await {
                        Ok(r) => info!(
                            "[SCHEDULER] Missed task '{}' completed: success={}",
                            task.id, r.success
                        ),
                        Err(e) => {
                            tracing::warn!("[SCHEDULER] Missed task '{}' failed: {}", task.id, e)
                        }
                    }
                }
            }
        }
    }

    /// Check for missed tasks that should have run while the app was closed.
    async fn check_missed_tasks(&self, app_handle: &tauri::AppHandle) {
        let tasks = {
            let config = self.config.read().await;
            if !config.scheduled_tasks.enabled {
                return;
            }
            config.scheduled_tasks.tasks.clone()
        };

        let now = Utc::now();

        for task in &tasks {
            if !task.enabled || !task.run_if_missed {
                continue;
            }

            if let Some(last_run) = task.last_run {
                if should_have_run_since(task, last_run, now) {
                    // Jitter up to 3s to prevent thundering herd when multiple
                    // tasks are missed simultaneously (e.g., app was offline).
                    let jitter_ms = (rand::random::<u32>() % 3000) as u64;
                    tokio::time::sleep(std::time::Duration::from_millis(jitter_ms)).await;

                    info!("[SCHEDULER] Missed task '{}', running now", task.name);
                    let result = self.execute_task(&task.id).await;
                    if let Ok(result) = &result {
                        use tauri::Emitter;
                        if let Err(e) = app_handle.emit("scheduler:task-complete", result) {
                            warn!("[SCHEDULER] Failed to emit scheduler:task-complete: {}", e);
                        }
                    }
                }
            }
        }
    }
}

/// Parse schedule string and check if a task should run now.
pub fn should_run_now(task: &crate::config::ScheduledTask, now: DateTime<Utc>) -> bool {
    // If last_run is within this minute, skip (already ran)
    if let Some(last_run) = task.last_run {
        let diff = now.signed_duration_since(last_run);
        if diff.num_seconds() < 60 {
            return false;
        }
    }

    parse_and_check_schedule(&task.schedule, now)
}

/// Check if a task should have run between last_run and now.
fn should_have_run_since(
    task: &crate::config::ScheduledTask,
    last_run: DateTime<Utc>,
    now: DateTime<Utc>,
) -> bool {
    let elapsed = now.signed_duration_since(last_run);
    let schedule = task.schedule.to_lowercase();

    if schedule == "hourly" {
        return elapsed.num_hours() >= 1;
    }

    if schedule.starts_with("every ") {
        if let Some(minutes) = parse_every_minutes(&schedule) {
            return elapsed.num_minutes() >= minutes as i64;
        }
    }

    if schedule.starts_with("daily ") {
        return elapsed.num_hours() >= 24;
    }

    if schedule.starts_with("weekly ") {
        return elapsed.num_days() >= 7;
    }

    // "cron EXPR" — check if any occurrence was missed between last_run and now
    if schedule.starts_with("cron ") {
        let expr = schedule.trim_start_matches("cron ").trim();
        if let Ok(cron) = parse_cron_expression(expr) {
            // If there was a scheduled occurrence between last_run and now, we missed it
            if let Some(next) = cron.next_occurrence(&last_run) {
                return next <= now;
            }
        }
    }

    false
}

/// Parse and check schedule format against current time.
fn parse_and_check_schedule(schedule: &str, now: DateTime<Utc>) -> bool {
    let schedule = schedule.to_lowercase();

    // "hourly" — run at minute 0
    if schedule == "hourly" {
        return now.minute() == 0;
    }

    // "every Nm" — run every N minutes
    if schedule.starts_with("every ") {
        if let Some(minutes) = parse_every_minutes(&schedule) {
            return (now.minute() as usize).is_multiple_of(minutes);
        }
    }

    // "daily HH:MM"
    if schedule.starts_with("daily ") {
        let time_part = schedule.trim_start_matches("daily ").trim();
        if let Some((h, m)) = parse_time(time_part) {
            return now.hour() == h && now.minute() == m;
        }
    }

    // "weekly DAY HH:MM"
    if schedule.starts_with("weekly ") {
        let parts: Vec<&str> = schedule
            .trim_start_matches("weekly ")
            .split_whitespace()
            .collect();
        if parts.len() >= 2 {
            if let Some(weekday) = parse_weekday(parts[0]) {
                if let Some((h, m)) = parse_time(parts[1]) {
                    return now.weekday() == weekday && now.hour() == h && now.minute() == m;
                }
            }
        }
    }

    // "cron EXPR" — standard 5-field cron expression
    if schedule.starts_with("cron ") {
        let expr = schedule.trim_start_matches("cron ").trim();
        if let Ok(cron) = parse_cron_expression(expr) {
            return cron.matches(&now);
        }
    }

    false
}

/// Parse "every Nm" format, returning N.
fn parse_every_minutes(schedule: &str) -> Option<usize> {
    let rest = schedule.trim_start_matches("every ").trim();
    let num_str = rest.trim_end_matches('m').trim();
    num_str.parse::<usize>().ok().filter(|&n| n > 0)
}

/// Parse "HH:MM" format.
fn parse_time(s: &str) -> Option<(u32, u32)> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() == 2 {
        let h = parts[0].parse::<u32>().ok()?;
        let m = parts[1].parse::<u32>().ok()?;
        if h < 24 && m < 60 {
            return Some((h, m));
        }
    }
    None
}

/// Parse day name to chrono Weekday.
fn parse_weekday(s: &str) -> Option<Weekday> {
    match s.to_lowercase().as_str() {
        "monday" | "mon" => Some(Weekday::Mon),
        "tuesday" | "tue" => Some(Weekday::Tue),
        "wednesday" | "wed" => Some(Weekday::Wed),
        "thursday" | "thu" => Some(Weekday::Thu),
        "friday" | "fri" => Some(Weekday::Fri),
        "saturday" | "sat" => Some(Weekday::Sat),
        "sunday" | "sun" => Some(Weekday::Sun),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn test_parse_time() {
        assert_eq!(parse_time("09:00"), Some((9, 0)));
        assert_eq!(parse_time("23:59"), Some((23, 59)));
        assert_eq!(parse_time("24:00"), None);
        assert_eq!(parse_time("abc"), None);
    }

    #[test]
    fn test_parse_every_minutes() {
        assert_eq!(parse_every_minutes("every 30m"), Some(30));
        assert_eq!(parse_every_minutes("every 5m"), Some(5));
        assert_eq!(parse_every_minutes("every 0m"), None);
        assert_eq!(parse_every_minutes("every abcm"), None);
    }

    #[test]
    fn test_parse_weekday() {
        assert_eq!(parse_weekday("monday"), Some(Weekday::Mon));
        assert_eq!(parse_weekday("fri"), Some(Weekday::Fri));
        assert_eq!(parse_weekday("xyz"), None);
    }

    // ── Cron expression parsing tests ────────────────────────────────

    #[test]
    fn test_parse_cron_every_5_minutes() {
        // "*/5 * * * *" — every 5 minutes
        let cron = parse_cron_expression("*/5 * * * *").unwrap();
        assert_eq!(
            cron.minutes,
            vec![0, 5, 10, 15, 20, 25, 30, 35, 40, 45, 50, 55]
        );
        assert_eq!(cron.hours.len(), 24); // 0-23
        assert_eq!(cron.days.len(), 31); // 1-31
        assert_eq!(cron.months.len(), 12); // 1-12
        assert_eq!(cron.weekdays.len(), 7); // 0-6
    }

    #[test]
    fn test_parse_cron_9am_weekdays() {
        // "0 9 * * 1-5" — 9:00 AM on weekdays (Mon-Fri)
        let cron = parse_cron_expression("0 9 * * 1-5").unwrap();
        assert_eq!(cron.minutes, vec![0]);
        assert_eq!(cron.hours, vec![9]);
        assert_eq!(cron.days.len(), 31);
        assert_eq!(cron.months.len(), 12);
        assert_eq!(cron.weekdays, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_parse_cron_monthly_2_30am() {
        // "30 2 1 * *" — 2:30 AM on the 1st of each month
        let cron = parse_cron_expression("30 2 1 * *").unwrap();
        assert_eq!(cron.minutes, vec![30]);
        assert_eq!(cron.hours, vec![2]);
        assert_eq!(cron.days, vec![1]);
        assert_eq!(cron.months.len(), 12);
        assert_eq!(cron.weekdays.len(), 7);
    }

    #[test]
    fn test_parse_cron_lists() {
        // "0,15,30,45 8,12,18 * * *" — specific minutes at specific hours
        let cron = parse_cron_expression("0,15,30,45 8,12,18 * * *").unwrap();
        assert_eq!(cron.minutes, vec![0, 15, 30, 45]);
        assert_eq!(cron.hours, vec![8, 12, 18]);
    }

    #[test]
    fn test_parse_cron_step_with_range() {
        // "1-30/5 * * * *" — minutes 1, 6, 11, 16, 21, 26
        let cron = parse_cron_expression("1-30/5 * * * *").unwrap();
        assert_eq!(cron.minutes, vec![1, 6, 11, 16, 21, 26]);
    }

    #[test]
    fn test_parse_cron_invalid_field_count() {
        assert!(parse_cron_expression("* * *").is_err());
        assert!(parse_cron_expression("* * * * * *").is_err());
    }

    #[test]
    fn test_parse_cron_invalid_values() {
        // Minute out of range
        assert!(parse_cron_expression("60 * * * *").is_err());
        // Hour out of range
        assert!(parse_cron_expression("* 25 * * *").is_err());
        // Day 0 (days are 1-31)
        assert!(parse_cron_expression("* * 0 * *").is_err());
        // Month 13
        assert!(parse_cron_expression("* * * 13 *").is_err());
        // Weekday 7
        assert!(parse_cron_expression("* * * * 7").is_err());
    }

    #[test]
    fn test_parse_cron_invalid_step_zero() {
        assert!(parse_cron_expression("*/0 * * * *").is_err());
    }

    // ── CronSchedule::matches tests ─────────────────────────────────

    #[test]
    fn test_cron_matches_every_5_min() {
        let cron = parse_cron_expression("*/5 * * * *").unwrap();

        // 2025-01-15 10:00 UTC (Wednesday) — minute 0 matches
        let dt = Utc.with_ymd_and_hms(2025, 1, 15, 10, 0, 0).unwrap();
        assert!(cron.matches(&dt));

        // 2025-01-15 10:05 UTC — minute 5 matches
        let dt = Utc.with_ymd_and_hms(2025, 1, 15, 10, 5, 0).unwrap();
        assert!(cron.matches(&dt));

        // 2025-01-15 10:03 UTC — minute 3 does NOT match
        let dt = Utc.with_ymd_and_hms(2025, 1, 15, 10, 3, 0).unwrap();
        assert!(!cron.matches(&dt));
    }

    #[test]
    fn test_cron_matches_9am_weekdays() {
        let cron = parse_cron_expression("0 9 * * 1-5").unwrap();

        // 2025-01-13 09:00 UTC (Monday) — matches
        let dt = Utc.with_ymd_and_hms(2025, 1, 13, 9, 0, 0).unwrap();
        assert!(cron.matches(&dt));

        // 2025-01-17 09:00 UTC (Friday) — matches
        let dt = Utc.with_ymd_and_hms(2025, 1, 17, 9, 0, 0).unwrap();
        assert!(cron.matches(&dt));

        // 2025-01-18 09:00 UTC (Saturday) — does NOT match
        let dt = Utc.with_ymd_and_hms(2025, 1, 18, 9, 0, 0).unwrap();
        assert!(!cron.matches(&dt));

        // 2025-01-19 09:00 UTC (Sunday) — does NOT match
        let dt = Utc.with_ymd_and_hms(2025, 1, 19, 9, 0, 0).unwrap();
        assert!(!cron.matches(&dt));

        // 2025-01-13 10:00 UTC (Monday but wrong hour) — does NOT match
        let dt = Utc.with_ymd_and_hms(2025, 1, 13, 10, 0, 0).unwrap();
        assert!(!cron.matches(&dt));
    }

    #[test]
    fn test_cron_matches_monthly() {
        let cron = parse_cron_expression("30 2 1 * *").unwrap();

        // 2025-02-01 02:30 UTC (Saturday) — matches
        let dt = Utc.with_ymd_and_hms(2025, 2, 1, 2, 30, 0).unwrap();
        assert!(cron.matches(&dt));

        // 2025-02-02 02:30 UTC — day doesn't match
        let dt = Utc.with_ymd_and_hms(2025, 2, 2, 2, 30, 0).unwrap();
        assert!(!cron.matches(&dt));
    }

    // ── CronSchedule::next_occurrence tests ─────────────────────────

    #[test]
    fn test_cron_next_occurrence_every_5_min() {
        let cron = parse_cron_expression("*/5 * * * *").unwrap();

        // From 2025-01-15 10:02 UTC → next should be 10:05
        let after = Utc.with_ymd_and_hms(2025, 1, 15, 10, 2, 0).unwrap();
        let next = cron.next_occurrence(&after).unwrap();
        assert_eq!(next, Utc.with_ymd_and_hms(2025, 1, 15, 10, 5, 0).unwrap());
    }

    #[test]
    fn test_cron_next_occurrence_9am_weekdays() {
        let cron = parse_cron_expression("0 9 * * 1-5").unwrap();

        // From 2025-01-17 09:00 (Friday 9am) → next should be Monday 2025-01-20 09:00
        let after = Utc.with_ymd_and_hms(2025, 1, 17, 9, 0, 0).unwrap();
        let next = cron.next_occurrence(&after).unwrap();
        assert_eq!(next, Utc.with_ymd_and_hms(2025, 1, 20, 9, 0, 0).unwrap());
    }

    #[test]
    fn test_cron_next_occurrence_monthly() {
        let cron = parse_cron_expression("30 2 1 * *").unwrap();

        // From 2025-01-01 02:30 → next should be 2025-02-01 02:30
        let after = Utc.with_ymd_and_hms(2025, 1, 1, 2, 30, 0).unwrap();
        let next = cron.next_occurrence(&after).unwrap();
        assert_eq!(next, Utc.with_ymd_and_hms(2025, 2, 1, 2, 30, 0).unwrap());
    }

    #[test]
    fn test_cron_next_occurrence_wraps_hour() {
        let cron = parse_cron_expression("0 * * * *").unwrap();

        // From 2025-01-15 10:30 → next should be 11:00
        let after = Utc.with_ymd_and_hms(2025, 1, 15, 10, 30, 0).unwrap();
        let next = cron.next_occurrence(&after).unwrap();
        assert_eq!(next, Utc.with_ymd_and_hms(2025, 1, 15, 11, 0, 0).unwrap());
    }

    #[test]
    fn test_cron_next_occurrence_wraps_day() {
        let cron = parse_cron_expression("0 0 * * *").unwrap();

        // From 2025-01-15 23:30 → next should be 2025-01-16 00:00
        let after = Utc.with_ymd_and_hms(2025, 1, 15, 23, 30, 0).unwrap();
        let next = cron.next_occurrence(&after).unwrap();
        assert_eq!(next, Utc.with_ymd_and_hms(2025, 1, 16, 0, 0, 0).unwrap());
    }

    // ── Integration: schedule parsing with "cron" prefix ────────────

    #[test]
    fn test_parse_and_check_schedule_cron() {
        // "cron 0 9 * * 1-5" should match Monday 9:00
        let dt = Utc.with_ymd_and_hms(2025, 1, 13, 9, 0, 0).unwrap(); // Monday
        assert!(parse_and_check_schedule("cron 0 9 * * 1-5", dt));

        // Should NOT match Saturday 9:00
        let dt = Utc.with_ymd_and_hms(2025, 1, 18, 9, 0, 0).unwrap(); // Saturday
        assert!(!parse_and_check_schedule("cron 0 9 * * 1-5", dt));
    }

    #[test]
    fn test_parse_and_check_schedule_cron_case_insensitive() {
        // The existing code lowercases the schedule, so "Cron" should also work
        let dt = Utc.with_ymd_and_hms(2025, 1, 15, 10, 0, 0).unwrap();
        assert!(parse_and_check_schedule("Cron */5 * * * *", dt));
    }
}
