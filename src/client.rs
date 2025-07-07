use color_eyre::eyre;
use matrix_sdk::ruma::api::client::uiaa::{AuthData, Password, UserIdentifier};
use matrix_sdk::{Client, config::SyncSettings};
use tracing::{info, trace, warn};

use crate::DatabasePool;
use crate::config::Config;
use crate::session::persist_sync_token;

/// Handles initial sync and device management for the Matrix client.
pub async fn run(
    client: &Client,
    initial_sync_token: Option<String>,
    database: DatabasePool,
    config: &Config,
) -> eyre::Result<()> {
    info!("Launching a first sync...");

    // Enable room members lazy-loading, it will speed up the initial sync a lot
    let filter = matrix_sdk::ruma::api::client::filter::FilterDefinition::with_lazy_loading();
    let mut sync_settings = SyncSettings::default().filter(filter.into());

    // Restore the sync where we left off, if available.
    if let Some(sync_token) = initial_sync_token {
        sync_settings = sync_settings.token(sync_token);
    }

    // Ignore messages before the program was launched.
    loop {
        match client.sync_once(sync_settings.clone()).await {
            Ok(response) => {
                persist_sync_token(
                    database,
                    client.user_id().expect("to be logged in"),
                    response.next_batch,
                )
                .await?;
                break;
            }
            Err(error) => {
                warn!("An error occurred during initial sync: {error}");
            }
        }
    }
    info!("Initial sync done");

    let current_session = client.device_id().map(|d| d.to_owned());
    if let Some(account_config) = &config.account_config
        && account_config.delete_other_devices
    {
        info!(
            current_session = format!("{current_session:?}"),
            "Checking for other devices to delete"
        );
        let other_devices: Vec<_> = client
            .devices()
            .await?
            .devices
            .iter()
            .filter(|device| Some(&device.device_id) != current_session.as_ref())
            .map(|device| device.device_id.clone())
            .collect();
        if !other_devices.is_empty() {
            trace!(
                current_session = format!("{current_session:?}"),
                other_devices = format!("{other_devices:?}"),
                "Deleting other devices"
            );
            client
                .delete_devices(
                    &other_devices,
                    Some(AuthData::Password(Password::new(
                        UserIdentifier::UserIdOrLocalpart(account_config.username.as_ref().expect("UIAA requires username/password").clone()),
                        account_config.password.clone().unwrap_or_else(|| {
                            println!(
                                "Type password for the account (characters won't show up as you type them)"
                            );
                            rpassword::prompt_password("Password: ").unwrap_or_default()
                        }),
                    ))),
                )
                .await?;
        }
    }

    if let Some(account_config) = &config.account_config
        && account_config.set_device_name
    {
        if let Some(current_session) = current_session {
            info!(
                current_session = format!("{current_session:?}"),
                "Renaming device to {}", &account_config.device_name
            );
            client
                .rename_device(&current_session, &account_config.device_name)
                .await?;
        } else {
            warn!("No device ID found, cannot name device");
        }
    }

    Ok(())
}
