use crate::account::config::AccountDetails;
use clap::Parser;
use std::path::PathBuf;

// mod account;

/// Application configuration parsed from CLI arguments and environment variables.
#[derive(Parser, Debug)]
pub struct CommandConfig {
    /// Path to the configuration file. This contains account settings and other configuration options.
    #[arg(short, long, env = "CONFIG_FILE")]
    pub config_file: PathBuf,

    /// Connection string for the PostgreSQL database
    #[arg(long, env = "DATABASE_URL")]
    pub database_url: String,

    /// Matrix SDK data directory
    #[arg(long, env = "MATRIX_DATA_DIR")]
    pub data_dir: Option<PathBuf>,
    /// Matrix SDK cache directory
    #[arg(long, env = "MATRIX_CACHE_DIR")]
    pub cache_dir: Option<PathBuf>,

    #[clap(flatten)]
    pub(crate) verbose: clap_verbosity_flag::Verbosity,
}

use serde::{Deserialize, Serialize};

/// Account details for Matrix bots
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ConfigFile {
    /// Primary user ID to use. If unset, uses the first account.
    /// If set, must match an account's user_id (raw string or parsed form).
    pub primary_user_id: Option<String>,
    /// List of accounts to be managed by the application
    pub accounts: Vec<AccountDetails>,
}
