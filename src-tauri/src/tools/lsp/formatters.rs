//! Format LSP responses as human-readable text for the LLM.

use serde_json::Value;

pub fn format_locations(operation: &str, locations: &Value, base_dir: &str) -> String {
    let locs = if let Some(arr) = locations.as_array() {
        arr.clone()
    } else if locations.is_object() {
        vec![locations.clone()]
    } else {
        return format!("{}: No results found.", operation);
    };

    if locs.is_empty() {
        return format!("{}: No results found.", operation);
    }

    let mut lines = vec![format!("{} ({} result{}):", operation, locs.len(),
        if locs.len() == 1 { "" } else { "s" })];

    for loc in &locs {
        if let Some(formatted) = format_location(loc, base_dir) {
            lines.push(formatted);
        }
    }
    lines.join("\n")
}

pub fn format_location(loc: &Value, base_dir: &str) -> Option<String> {
    let uri = loc["uri"].as_str()?;
    let path = uri.strip_prefix("file://").unwrap_or(uri);
    let display_path = path.strip_prefix(base_dir).unwrap_or(path).trim_start_matches('/');
    let line = loc["range"]["start"]["line"].as_u64().unwrap_or(0) + 1;
    let col = loc["range"]["start"]["character"].as_u64().unwrap_or(0) + 1;
    Some(format!("  {}:{}:{}", display_path, line, col))
}

pub fn format_hover(hover: &Value) -> String {
    let content = &hover["contents"];
    let text = if let Some(arr) = content.as_array() {
        arr.iter()
            .filter_map(|v| v.as_str().or(v["value"].as_str()))
            .collect::<Vec<_>>()
            .join("\n")
    } else if let Some(s) = content.as_str() {
        s.to_string()
    } else if let Some(val) = content["value"].as_str() {
        val.to_string()
    } else {
        return "No hover information available.".to_string();
    };
    if text.is_empty() { "No hover information available.".to_string() } else { text }
}

pub fn format_symbols(symbols: &Value) -> String {
    let arr = match symbols.as_array() {
        Some(a) if !a.is_empty() => a,
        _ => return "No symbols found.".to_string(),
    };
    let mut lines = Vec::new();
    for sym in arr { format_symbol(sym, 0, &mut lines); }
    lines.join("\n")
}

fn format_symbol(sym: &Value, depth: usize, lines: &mut Vec<String>) {
    let name = sym["name"].as_str().unwrap_or("?");
    let kind = sym["kind"].as_u64().unwrap_or(0);
    let kind_str = symbol_kind_name(kind);
    let line = sym["range"]["start"]["line"].as_u64().unwrap_or(0) + 1;
    lines.push(format!("{}{}:{} (line {})", "  ".repeat(depth), kind_str, name, line));
    if let Some(children) = sym["children"].as_array() {
        for child in children { format_symbol(child, depth + 1, lines); }
    }
}

fn symbol_kind_name(kind: u64) -> &'static str {
    match kind {
        1 => "File", 2 => "Module", 3 => "Namespace", 4 => "Package",
        5 => "Class", 6 => "Method", 7 => "Property", 8 => "Field",
        9 => "Constructor", 10 => "Enum", 11 => "Interface", 12 => "Function",
        13 => "Variable", 14 => "Constant", 15 => "String", 16 => "Number",
        17 => "Boolean", 18 => "Array", 23 => "Struct", 24 => "Event",
        25 => "Operator", 26 => "TypeParameter", _ => "Symbol",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_locations_empty() {
        let result = format_locations("goToDefinition", &serde_json::json!([]), "/tmp");
        assert!(result.contains("No results found"));
    }

    #[test]
    fn test_format_locations_single() {
        let loc = serde_json::json!([{
            "uri": "file:///tmp/project/src/main.rs",
            "range": {"start": {"line": 41, "character": 9}, "end": {"line": 41, "character": 15}}
        }]);
        let result = format_locations("goToDefinition", &loc, "/tmp/project");
        assert!(result.contains("src/main.rs:42:10"));
    }

    #[test]
    fn test_format_hover_string_content() {
        let hover = serde_json::json!({"contents": "fn main() -> i32"});
        assert_eq!(format_hover(&hover), "fn main() -> i32");
    }

    #[test]
    fn test_format_hover_empty_returns_message() {
        let hover = serde_json::json!({"contents": ""});
        assert!(format_hover(&hover).contains("No hover"));
    }

    #[test]
    fn test_format_symbols_empty() {
        let result = format_symbols(&serde_json::json!([]));
        assert!(result.contains("No symbols found"));
    }

    #[test]
    fn test_symbol_kind_name() {
        assert_eq!(symbol_kind_name(12), "Function");
        assert_eq!(symbol_kind_name(5), "Class");
        assert_eq!(symbol_kind_name(999), "Symbol");
    }
}
