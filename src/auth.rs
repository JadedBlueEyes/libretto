use color_eyre::eyre::{self};
use matrix_sdk::Client;
use matrix_sdk::encryption::Encryption;
use matrix_sdk::{SessionMeta, SessionTokens, authentication::matrix::MatrixSession};
use rand::RngExt;
use rand::distr::Alphanumeric;
use rpassword::prompt_password;
use tracing::{debug, error, info, instrument, warn};

use crate::account::config::{AccountDetails, AuthMethod};
use crate::session::{ClientSession, FullSession};
use ruma::UserId;

/// Login to a Matrix account using the provided configuration.
/// Creates a new client database from scratch.
#[instrument(level = "info", skip(data_dir, cache_dir, config), fields(user_id = %config.user_id, homeserver = ?config.homeserver))]
pub async fn login(
    data_dir: &std::path::Path,
    cache_dir: &std::path::Path,
    config: &AccountDetails,
) -> eyre::Result<(Client, FullSession)> {
    info!(
        user_id = %config.user_id,
        homeserver = %config.homeserver.as_deref().unwrap_or("<from user_id>"),
        "Starting authentication"
    );
    // Generate a random passphrase.

    let (db_path, passphrase): (String, String) = {
        let mut rng = rand::rng();
        (
            (&mut rng)
                .sample_iter(Alphanumeric)
                .take(7)
                .map(char::from)
                .collect(),
            (&mut rng)
                .sample_iter(Alphanumeric)
                .take(32)
                .map(char::from)
                .collect(),
        )
    };

    // Get homeserver URL or name
    let homeserver_url = if let Some(homeserver) = &config.homeserver {
        debug!(
            homeserver = %homeserver,
            "Using explicitly configured homeserver"
        );
        homeserver.clone()
    } else {
        let uid = UserId::parse(&config.user_id)
            .map_err(|e| eyre::eyre!("Invalid user ID format and no homeserver provided: {}", e))?;
        // Extract homeserver from user ID
        let server = uid.server_name().to_string();
        debug!(
            server = %server,
            "Extracted homeserver from user_id"
        );
        server
    };

    let client = Client::builder()
        .server_name_or_homeserver_url(&homeserver_url)
        .sqlite_store_with_cache_path(
            data_dir.join(&db_path),
            cache_dir.join(&db_path),
            Some(&passphrase),
        )
        .build()
        .await
        .map_err(|e| {
            eyre::eyre!(
                "Failed to build Matrix client for {}: {}",
                config.user_id,
                e
            )
        })?;

    debug!(
        user_id = %config.user_id,
        "Matrix client built successfully"
    );
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
                    info!(
                        user_id = %r.user_id,
                        device_id = %r.device_id,
                        "Password authentication successful"
                    );
                }
                Err(error) => {
                    error!(
                        user_id = %config.user_id,
                        error = %error,
                        "Password authentication failed"
                    );
                    return Err(eyre::eyre!(
                        "Password login failed for {}: {}",
                        config.user_id,
                        error
                    ));
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
                .await
                .map_err(|e| {
                    eyre::eyre!(
                        "Failed to restore session with access token for {}: {}",
                        config.user_id,
                        e
                    )
                })?;
            let device_id = client.device_id().expect("client id on logged in session");
            info!(
                user_id = %config.user_id,
                device_id = %device_id,
                "Access token authentication successful"
            );
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
                    info!(
                        user_id = %r.user_id,
                        device_id = %r.device_id,
                        "Interactive password authentication successful"
                    );
                }
                Err(error) => {
                    error!(
                        user_id = %config.user_id,
                        error = %error,
                        "Interactive password authentication failed"
                    );
                    return Err(eyre::eyre!(
                        "Interactive password login failed for {}: {}",
                        config.user_id,
                        error
                    ));
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
    debug!(
        user_id = %config.user_id,
        "Authentication complete, session ready for persistence"
    );
    let session = FullSession {
        client_session,
        user_session,
        sync_token: None,
    };

    Ok((client, session))
}

#[instrument(level = "debug", skip(encryption, recovery_key))]
pub async fn verify_device(
    encryption: Encryption,
    recovery_key: Option<String>,
) -> eyre::Result<()> {
    let device = encryption
        .get_own_device()
        .await?
        .expect("to have a device");

    if device.is_verified_with_cross_signing() {
        debug!(
            user_id = %device.user_id(),
            device_id = %device.device_id(),
            "Device is verified with cross-signing"
        );
    } else {
        warn!(
            user_id = %device.user_id(),
            device_id = %device.device_id(),
            "Device is not verified with cross-signing"
        );
        let recovery_key = recovery_key.or_else(|| {
            println!("Type recovery key for the bot (characters won't show up as you type them)");
            prompt_password("Recovery Key: ").ok()
        });
        if let Some(recovery_key) = recovery_key {
            debug!("Attempting device recovery with recovery key");
            let _ = encryption
                .recovery()
                .recover(&recovery_key)
                .await
                .inspect_err(|e| {
                    error!(
                        error = %e,
                        "Device recovery failed"
                    );
                });
        }
    }
    encryption.wait_for_e2ee_initialization_tasks().await;
    Ok(())
}
