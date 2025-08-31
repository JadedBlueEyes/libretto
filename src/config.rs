use crate::account::config::AccountDetails;
use clap::{Parser, Subcommand};
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

    #[clap(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Management utilities
    #[clap(subcommand)]
    Util(UtilCommand),
}

#[derive(Subcommand, Debug)]
pub enum UtilCommand {
    /// Remove the `next_batch` token from one or all accounts.
    /// This will force an initial sync.
    RemoveNextBatch {
        /// The user ID of the account to target. If not provided, all accounts will be targeted.
        #[arg(long)]
        user_id: Option<String>,
    },
    /// Delete the `prev_batch` from all rooms and delete all timelines.
    DeletePrevBatchAndTimelines {
        /// The user ID of the account to target. If not provided, all accounts will be targeted.
        #[arg(long)]
        user_id: Option<String>,
    },
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
    /// Restore all sessions found in the database, even if not configured
    #[serde(default)]
    pub restore_all_sessions: bool,
}
