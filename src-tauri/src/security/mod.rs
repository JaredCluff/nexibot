//! Security hardening module.
//!
//! Centralizes security primitives: SSRF protection, constant-time auth,
//! environment sanitization, binary path validation, credential storage,
//! path traversal prevention, workspace confinement, execution approval,
//! tool policies, rate limiting, and session encryption.

pub mod audit;
pub mod constant_time;
pub mod credentials;
pub mod dangerous_tools;
pub mod env_sanitize;
pub mod exec_approval;
pub mod external_content;
pub mod key_interceptor;
pub mod key_vault;
pub mod log_redactor;
pub mod merge_safety;
pub mod path_validation;
pub mod rate_limit;
pub mod safe_bins;
pub mod session_encryption;
pub mod skill_scanner;
pub mod ssrf;
pub mod tool_audit;
pub mod tool_policy;
pub mod workspace;
