//! Libretto - A Matrix client library and application
//!
//! This library provides functionality for Matrix client operations,
//! account management, and configuration handling.

pub mod account;
pub mod config;

// Re-export commonly used types for convenience
pub use account::selection::select_primary_account;
pub use config::{CommandConfig, ConfigFile};
