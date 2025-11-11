use serde::{Deserialize, Serialize};

/// Account details for Matrix bots
#[derive(Debug, Deserialize, Serialize, Clone)]
#[non_exhaustive]
pub struct AccountDetails {
    /// The Matrix user ID (@user:example.com)
    /// This *must* be a full, valid Matrix user ID, not a 3pid
    pub user_id: String,
    #[serde(default)]
    pub auth_method: AuthMethod,
    /// Recovery key for E2EE sessions
    /// WARNING: If this is not set and encryption is enabled, the account will be unable to start.
    #[serde(default)]
    pub recovery_key: Option<String>,
    /// Homeserver URL. Defaults to resolving user ID's homeserver
    #[serde(default)]
    pub homeserver: Option<String>,

    /// Enable encryption for account
    #[serde(default = "default_enable_encryption")]
    pub enable_encryption: bool,

    // Device management
    /// Device name to set, if it doesn't exist
    pub device_name: Option<String>,

    /// Set the device name, even if it already exists
    #[serde(default)]
    pub set_device_name: bool,

    /// Delete devices other than the one being used by this instance
    #[serde(default)]
    pub delete_other_devices: bool,
}

const fn default_enable_encryption() -> bool {
    true
}

#[non_exhaustive]
pub struct DefaultAccountConfig {
    /// Device name to set, if it doesn't exist
    pub device_name: String,
    /// Set the device name, even if it already exists
    pub set_device_name: bool,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub enum AuthMethod {
    /// Password for legacy authentication
    Password(String),
    /// Access token
    AccessToken(String),
    /// Rely on existing credentials
    #[default]
    None,
}
