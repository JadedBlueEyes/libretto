use color_eyre::eyre::{self};
use matrix_sdk::Client;
use matrix_sdk::encryption::Encryption;
use matrix_sdk::{SessionMeta, SessionTokens, authentication::matrix::MatrixSession};
use rand::Rng;
use rand::distr::Alphanumeric;
use rpassword::prompt_password;
use tracing::{error, info};

use crate::account::config::{AccountDetails, AuthMethod};
use crate::session::{ClientSession, FullSession};
use ruma::UserId;

pub async fn login(
    data_dir: &std::path::Path,
    cache_dir: &std::path::Path,
    config: &AccountDetails,
) -> eyre::Result<(Client, FullSession)> {
    info!("Logging in to new session…");
    let mut rng = rand::rng();

    // Generate a random passphrase.
    let passphrase: String = (&mut rng)
        .sample_iter(Alphanumeric)
        .take(32)
        .map(char::from)
        .collect();

    let db_path: String = (&mut rng)
        .sample_iter(Alphanumeric)
        .take(7)
        .map(char::from)
        .collect();

    // Get homeserver URL or name
    let homeserver_url = if let Some(homeserver) = &config.homeserver {
        homeserver.clone()
    } else {
        let uid = UserId::parse(&config.user_id)
            .map_err(|e| eyre::eyre!("Invalid user ID format and no homeserver provided: {}", e))?;
        // Extract homeserver from user ID
        uid.server_name().to_string()
    };

    let client = Client::builder()
        .server_name_or_homeserver_url(&homeserver_url)
        .sqlite_store_with_cache_path(
            data_dir.join(&db_path),
            cache_dir.join(&db_path),
            Some(&passphrase),
        )
        .build()
        .await?;

    let client_session = ClientSession {
        homeserver: homeserver_url,
        db_path,
        passphrase,
    };
    let matrix_auth = client.matrix_auth();

    match &config.auth_method {
        AuthMethod::Password(password) => {
            match matrix_auth
                .login_username(&config.user_id, password)
                .initial_device_display_name(config.device_name.as_deref().unwrap_or("libretto"))
                .await
            {
                Ok(r) => {
                    info!("Logged in as {} ({})", r.user_id, r.device_id);
                }
                Err(error) => {
                    error!("Error logging in: {error}");
                    return Err(error.into());
                }
            }
        }
        AuthMethod::AccessToken(token) => {
            let uid = UserId::parse(&config.user_id)
                .map_err(|e| eyre::eyre!("Non-full MXID used with access token login {e}"))?;
            client
                .restore_session(MatrixSession {
                    meta: SessionMeta {
                        user_id: uid,
                        device_id: "UNKNOWN".into(), // Will be updated after first sync
                    },
                    tokens: SessionTokens {
                        access_token: token.clone(),
                        refresh_token: None,
                    },
                })
                .await?;
            let device_id = client.device_id().expect("client id on logged in session");
            info!("Restored session for {} ({})", &config.user_id, device_id);
        }
        AuthMethod::None => {
            // Try to prompt for password
            let username = &config.user_id;
            println!("Type password for {username} (characters won't show up as you type them)");
            let password = match prompt_password("Password: ") {
                Ok(p) => p,
                Err(err) => {
                    return Err(eyre::eyre!("Failed to get password: {err}"));
                }
            };

            match matrix_auth
                .login_username(username, &password)
                .initial_device_display_name(config.device_name.as_deref().unwrap_or("libretto"))
                .await
            {
                Ok(r) => {
                    info!("Logged in as {} ({})", r.user_id, r.device_id);
                }
                Err(error) => {
                    error!("Error logging in: {error}");
                    return Err(error.into());
                }
            }
        }
    }

    if config.enable_encryption {
        verify_device(client.encryption(), config.recovery_key.clone()).await?;
    }

    // Persist the session to reuse it later.
    let user_session = matrix_auth
        .session()
        .expect("A logged-in client should have a session");
    let session = FullSession {
        client_session,
        user_session,
        sync_token: None,
    };

    Ok((client, session))
}

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
