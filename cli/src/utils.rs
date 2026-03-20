//! Utility functions for the CLI

use crate::error::CliError;
use std::fs::File;
use std::io::{BufRead, BufReader};

/// Parse JSON Lines format (one JSON object per line)
pub fn parse_jsonl(file_path: &str) -> Result<Vec<serde_json::Value>, CliError> {
    let file = File::open(file_path).map_err(|e| CliError::Io(e))?;
    let reader = BufReader::new(file);

    let mut result = Vec::new();
    for line in reader.lines() {
        let line = line.map_err(|e| CliError::Io(e))?;
        if line.trim().is_empty() {
            continue;
        }
        let value: serde_json::Value = serde_json::from_str(&line)?;
        result.push(value);
    }
    Ok(result)
}

/// Merge multiple JSON objects
pub fn merge_json(values: Vec<serde_json::Value>) -> serde_json::Value {
    let mut result = serde_json::json!({});
    for value in values {
        if let (serde_json::Value::Object(ref mut dest), serde_json::Value::Object(src)) =
            (&mut result, value)
        {
            for (k, v) in src {
                dest.insert(k, v);
            }
        }
    }
    result
}

/// Format duration in human-readable format
pub fn format_duration(secs: u64) -> String {
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else if secs < 86400 {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    } else {
        format!("{}d {}h", secs / 86400, (secs % 86400) / 3600)
    }
}

/// Format size in human-readable format
pub fn format_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit_idx = 0;

    while size >= 1024.0 && unit_idx < UNITS.len() - 1 {
        size /= 1024.0;
        unit_idx += 1;
    }

    if size < 10.0 {
        format!("{:.2} {}", size, UNITS[unit_idx])
    } else {
        format!("{:.0} {}", size, UNITS[unit_idx])
    }
}

/// Check if a string looks like a JSON value
pub fn is_json(s: &str) -> bool {
    s.trim().starts_with('{') || s.trim().starts_with('[')
}

/// Parse key=value arguments into JSON object
pub fn parse_args(args: &[String]) -> Result<serde_json::Value, CliError> {
    let mut result = serde_json::json!({});

    for arg in args {
        let parts: Vec<&str> = arg.splitn(2, '=').collect();
        if parts.len() != 2 {
            return Err(CliError::InvalidArgument(format!(
                "Invalid argument format: '{}' (expected key=value)",
                arg
            )));
        }

        let key = parts[0];
        let value_str = parts[1];

        // Try to parse as JSON value
        let value = if is_json(value_str) {
            serde_json::from_str(value_str)?
        } else {
            serde_json::Value::String(value_str.to_string())
        };

        if let serde_json::Value::Object(ref mut obj) = result {
            obj.insert(key.to_string(), value);
        }
    }

    Ok(result)
}
