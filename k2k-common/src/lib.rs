//! K2K (Knowledge-to-Knowledge) Protocol Common Library
//!
//! Shared models, client utilities, and JWT helpers for K2K federation protocol.
//!
//! This crate contains the core protocol types used by all K2K implementations.
//! Enterprise management types (managed policies, heartbeat, instance registration)
//! live in the `nexibot-connect` crate.

pub mod client;
pub mod jwt;
pub mod models;

/// K2K protocol version string.  Incremented when the wire format changes.
pub const PROTOCOL_VERSION: &str = "1.1";

pub use client::{generate_rsa_keypair, K2KClient};
pub use jwt::*;
pub use models::*;
