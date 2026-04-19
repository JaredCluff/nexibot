//! Matrix E2EE client using matrix-sdk.
use anyhow::Result;
use std::path::PathBuf;

pub fn matrix_store_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("nexibot")
        .join("matrix")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrix_sdk_available() {
        let _ = std::marker::PhantomData::<matrix_sdk::Client>;
    }

    #[test]
    fn matrix_store_path_contains_nexibot_matrix() {
        let p = matrix_store_path();
        let s = p.to_string_lossy();
        assert!(s.contains("nexibot") && s.contains("matrix"), "path: {}", s);
    }
}
