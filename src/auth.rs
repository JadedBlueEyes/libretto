use color_eyre::eyre::{self};
use matrix_sdk::Client;
#[cfg(false)]
use matrix_sdk::encryption::Encryption;
use rand::Rng;
use rand::distr::Alphanumeric;
use rpassword::prompt_password;
use tracing::{error, info};

use crate::config::AccountConfig;
use crate::session::{ClientSession, FullSession};

pub async fn login(
    data_dir: &std::path::Path,
    session_file: &std::path::Path,
    config: &AccountConfig,
) -> eyre::Result<Client> {
    info!("No previous session found, logging in…");
    let mut rng = rand::rng();

    // Generate a random passphrase.
    let passphrase: String = (&mut rng)
        .sample_iter(Alphanumeric)
        .take(32)
        .map(char::from)
        .collect();

    let db_subfolder: String = (&mut rng)
        .sample_iter(Alphanumeric)
        .take(7)
        .map(char::from)
        .collect();
    let db_path = data_dir.join(db_subfolder);

    let client = Client::builder()
        .homeserver_url(&config.server)
        // .sqlite_store(&db_path, Some(&passphrase))
        .build()
        .await?;

    let client_session = ClientSession {
        homeserver: config.server.clone(),
        db_path,
        passphrase,
    };
    let matrix_auth = client.matrix_auth();

    loop {
        let username = &config.username;
        let password = config.password.clone().unwrap_or_else(|| {
            println!("Type password for the bot (characters won't show up as you type them)");
            match prompt_password("Password: ") {
                Ok(p) => p,
                Err(err) => {
                    panic!("FATAL: failed to get password: {err}");
                }
            }
        });

        match matrix_auth
            .login_username(username, &password)
            .initial_device_display_name(&config.device_name)
            .await
        {
            Ok(_) => {
                info!("Logged in as {username}");
                break;
            }
            Err(error) => {
                error!("Error logging in: {error}");
                if config.password.is_some() {
                    return Err(error.into());
                }
            }
        }
    }

    #[cfg(false)]
    verify_device(client.encryption(), config.recovery_key.clone()).await?;

    // Persist the session to reuse it later.
    let user_session = matrix_auth
        .session()
        .expect("A logged-in client should have a session");
    let serialized_session = serde_json::to_string(&FullSession {
        client_session,
        user_session,
        sync_token: None,
    })?;
    tokio::fs::write(session_file, serialized_session).await?;

    info!("Session persisted in {}", session_file.to_string_lossy());

    Ok(client)
}

#[cfg(false)]
pub async fn verify_device(
    encryption: Encryption,
    recovery_key: Option<String>,
) -> eyre::Result<()> {
    let device = encryption
        .get_own_device()
        .await?
        .expect("to have a device");

    if device.is_verified_with_cross_signing() {
        info!(
            "Device {} of user {} is verified",
            device.device_id(),
            device.user_id(),
        );
    } else {
        info!(
            "Device {} of user {} is not verified",
            device.device_id(),
            device.user_id(),
        );
        let recovery_key = recovery_key.or_else(|| {
            println!("Type recovery key for the bot (characters won't show up as you type them)");
            prompt_password("Recovery Key: ").ok()
        });
        if let Some(recovery_key) = recovery_key {
            info!("Trying to recover device");
            let _ = encryption
                .recovery()
                .recover(&recovery_key)
                .await
                .inspect_err(|e| {
                    error!("Failed to recover device: {e}");
                });
        }
    }
    encryption.wait_for_e2ee_initialization_tasks().await;
    Ok(())
}
