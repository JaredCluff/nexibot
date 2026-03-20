//! NexiBot Tauri library entry point.
//!
//! This `lib.rs` exposes the self-contained security primitives for integration
//! tests in `tests/`. The main binary entry point remains `main.rs`.
//!
//! Only the security submodules that have no cross-crate dependencies are
//! exposed here; the full module tree is rooted in `main.rs`.

pub mod security {
    //! Security primitives exposed for integration tests.
    pub mod constant_time;
    pub mod external_content;
    pub mod key_vault;
    pub mod log_redactor;
    pub mod session_encryption;
}
