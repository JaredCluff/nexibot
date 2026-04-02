//! Output formatting for different formats (JSON, table, YAML, plain)

use colored::*;
use serde_json::Value;
use std::collections::BTreeMap;

pub fn format_output(value: &Value, format: &str) -> String {
    match format {
        "json" => format_json(value),
        "yaml" => format_yaml(value),
        "table" => format_table(value),
        _ => format_plain(value),
    }
}

/// Format as JSON with pretty printing
fn format_json(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".to_string())
}

/// Format as YAML
fn format_yaml(value: &Value) -> String {
    match serde_yaml::to_string(value) {
        Ok(s) => s,
        Err(_) => serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".to_string()),
    }
}

/// Format as plain text (key=value pairs)
fn format_plain(value: &Value) -> String {
    fn flatten(value: &Value, prefix: &str, output: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                for (k, v) in map {
                    let new_prefix = if prefix.is_empty() {
                        k.clone()
                    } else {
                        format!("{}.{}", prefix, k)
                    };
                    flatten(v, &new_prefix, output);
                }
            }
            Value::Array(arr) => {
                for (i, v) in arr.iter().enumerate() {
                    let new_prefix = format!("{}[{}]", prefix, i);
                    flatten(v, &new_prefix, output);
                }
            }
            _ => {
                output.push(format!(
                    "{}={}",
                    prefix,
                    value.as_str().unwrap_or(&value.to_string())
                ));
            }
        }
    }

    let mut output = Vec::new();
    flatten(value, "", &mut output);
    output.join("\n")
}

/// Format as ASCII table
fn format_table(value: &Value) -> String {
    if let Value::Array(rows) = value {
        return format_table_rows(rows);
    }

    if let Value::Object(map) = value {
        return format_table_object(map);
    }

    value.to_string()
}

/// Format array of objects as table
fn format_table_rows(rows: &[Value]) -> String {
    if rows.is_empty() {
        return "(empty)".to_string();
    }

    // Collect all keys from all objects
    let mut all_keys: std::collections::HashSet<String> = std::collections::HashSet::new();
    for row in rows {
        if let Value::Object(map) = row {
            for key in map.keys() {
                all_keys.insert(key.clone());
            }
        }
    }

    if all_keys.is_empty() {
        return "(no data)".to_string();
    }

    let mut keys: Vec<String> = all_keys.into_iter().collect();
    keys.sort();

    // Build header
    let mut header = String::new();
    let mut separators = String::new();

    for key in &keys {
        header.push_str(&format!("{:<20} ", key.bold()));
        separators.push_str(&"─".repeat(20).to_string());
        separators.push(' ');
    }

    let mut output = format!("{}\n{}\n", header, separators);

    // Build rows
    for row in rows {
        if let Value::Object(map) = row {
            for key in &keys {
                let value = map
                    .get(key)
                    .map(format_value_short)
                    .unwrap_or_else(|| "-".to_string());
                output.push_str(&format!("{:<20} ", value));
            }
            output.push('\n');
        }
    }

    output
}

/// Format object as table
fn format_table_object(map: &serde_json::Map<String, Value>) -> String {
    let sorted: BTreeMap<_, _> = map.iter().collect();

    let mut output = String::new();
    for (key, value) in sorted.iter() {
        output.push_str(&format!(
            "{}: {}\n",
            key.bold(),
            format_value_multiline(value)
        ));
    }
    output
}

/// Format a value for short display (max 20 chars)
fn format_value_short(value: &Value) -> String {
    let s = match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "(null)".to_string(),
        Value::Array(a) => format!("[{}]", a.len()),
        Value::Object(o) => format!("{{{}}}", o.len()),
    };

    if s.len() > 20 {
        format!("{}...", &s[..17])
    } else {
        s
    }
}

/// Format a value with potential multi-line display
fn format_value_multiline(value: &Value) -> String {
    match value {
        Value::String(s) => {
            if s.len() > 60 || s.contains('\n') {
                format!("\n  {}", s.replace('\n', "\n  "))
            } else {
                s.clone()
            }
        }
        Value::Array(a) => {
            if a.is_empty() {
                "[]".to_string()
            } else if a.len() <= 3 {
                format!(
                    "[{}]",
                    a.iter()
                        .map(|v| v.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            } else {
                format!(
                    "\n  {}",
                    a.iter()
                        .map(|v| v.to_string())
                        .collect::<Vec<_>>()
                        .join("\n  ")
                )
            }
        }
        Value::Object(_) => "...".to_string(),
        _ => value.to_string(),
    }
}

/// Print success message
pub fn success(msg: &str) {
    println!("{} {}", "✓".green(), msg);
}

/// Print error message
pub fn error(msg: &str) {
    eprintln!("{} {}", "✗".red(), msg);
}

/// Print info message
pub fn info(msg: &str) {
    println!("{} {}", "ℹ".cyan(), msg);
}

/// Print warning message
pub fn warn(msg: &str) {
    println!("{} {}", "⚠".yellow(), msg);
}

/// Indent text for nested display
pub fn indent(text: &str, level: usize) -> String {
    let indent = "  ".repeat(level);
    text.lines()
        .map(|line| format!("{}{}", indent, line))
        .collect::<Vec<_>>()
        .join("\n")
}
