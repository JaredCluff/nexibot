//! Safe merge utilities that prevent prototype pollution attacks.
#![allow(dead_code)]

/// Keys that must never be accepted in merge operations.
const BLOCKED_KEYS: &[&str] = &["__proto__", "prototype", "constructor"];

/// Check if a key is safe to use in merge operations.
pub fn is_safe_merge_key(key: &str) -> bool {
    !BLOCKED_KEYS.contains(&key)
}

/// Filter a serde_json::Value map, removing prototype-polluting keys.
pub fn sanitize_json_value(value: &mut serde_json::Value) {
    if let serde_json::Value::Object(map) = value {
        map.retain(|k, _| is_safe_merge_key(k));
        for (_, v) in map.iter_mut() {
            sanitize_json_value(v);
        }
    }
    if let serde_json::Value::Array(arr) = value {
        for v in arr.iter_mut() {
            sanitize_json_value(v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_blocks_proto() {
        assert!(!is_safe_merge_key("__proto__"));
        assert!(!is_safe_merge_key("prototype"));
        assert!(!is_safe_merge_key("constructor"));
    }

    #[test]
    fn test_allows_normal_keys() {
        assert!(is_safe_merge_key("name"));
        assert!(is_safe_merge_key("config"));
        assert!(is_safe_merge_key("value"));
    }

    #[test]
    fn test_sanitize_removes_proto() {
        let mut val = json!({
            "name": "test",
            "__proto__": { "admin": true },
            "nested": {
                "constructor": "evil",
                "safe": "value"
            }
        });
        sanitize_json_value(&mut val);
        assert!(val.get("__proto__").is_none());
        assert!(val.get("name").is_some());
        assert!(val["nested"].get("constructor").is_none());
        assert!(val["nested"].get("safe").is_some());
    }

    #[test]
    fn test_sanitize_handles_arrays() {
        let mut val = json!([
            { "__proto__": "bad", "ok": 1 },
            { "prototype": "bad", "fine": 2 }
        ]);
        sanitize_json_value(&mut val);
        let arr = val.as_array().unwrap();
        assert!(arr[0].get("__proto__").is_none());
        assert!(arr[0].get("ok").is_some());
        assert!(arr[1].get("prototype").is_none());
        assert!(arr[1].get("fine").is_some());
    }

    #[test]
    fn test_sanitize_deep_nesting() {
        let mut val = json!({
            "level1": {
                "level2": {
                    "__proto__": "deep_evil",
                    "level3": {
                        "constructor": "very_deep_evil",
                        "data": "safe"
                    }
                }
            }
        });
        sanitize_json_value(&mut val);
        assert!(val["level1"]["level2"].get("__proto__").is_none());
        assert!(val["level1"]["level2"]["level3"]
            .get("constructor")
            .is_none());
        assert_eq!(val["level1"]["level2"]["level3"]["data"], "safe");
    }

    #[test]
    fn test_sanitize_no_change_for_clean_value() {
        let mut val = json!({
            "name": "test",
            "items": [1, 2, 3],
            "nested": { "key": "value" }
        });
        let expected = val.clone();
        sanitize_json_value(&mut val);
        assert_eq!(val, expected);
    }
}
