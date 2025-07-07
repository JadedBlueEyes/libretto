use clap::Parser;
use std::path::PathBuf;

/// Application configuration parsed from CLI arguments and environment variables.
#[derive(Parser, Debug)]
pub struct Config {
    #[clap(flatten)]
    pub account_config: Option<AccountConfig>,

    #[arg(long, env = "DATABASE_URL")]
    pub database_url: String,

    #[clap(flatten)]
    pub(crate) verbose: clap_verbosity_flag::Verbosity,
}

/// Matrix account configuration.
#[derive(Parser, Debug)]
pub struct AccountConfig {
    /// URL of the homeserver to connect to
    #[arg(short, long, env = "MATRIX_SERVER")]
    pub server: Option<String>,

    /// Username of the bot
    #[arg(short, long, env = "MATRIX_USERNAME")]
    pub username: Option<String>,

    /// Password of the bot
    #[arg(short, long, env = "MATRIX_PASSWORD")]
    pub password: Option<String>,

    /// Delete devices other than the one being used by this instance
    #[arg(long)]
    pub delete_other_devices: bool,

    /// Device name to set, if it doesn't exist
    #[arg(long, default_value_t = String::from("libretto client"), env = "MATRIX_CLIENT_NAME")]
    pub device_name: String,

    /// Set the device name, even if it already exists
    #[arg(long, default_value_t = false)]
    pub set_device_name: bool,

    /// Account recovery key
    #[arg(short, long, env = "MATRIX_ACCOUNT_RECOVERY_KEY")]
    pub recovery_key: Option<String>,

    /// Account data directory
    #[arg(short, long, env = "MATRIX_ACCOUNT_DATA_DIR")]
    pub data_dir: Option<PathBuf>,
}
