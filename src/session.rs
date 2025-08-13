use std::path::Path;

use color_eyre::eyre;
use futures::TryFutureExt;
use matrix_sdk::{Client, SessionMeta, SessionTokens, authentication::matrix::MatrixSession};
use ruma::{ServerName, UserId};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, instrument, trace, warn};

use crate::{
    DatabaseConnection, DatabasePool,
    account::{
        config::{AccountDetails, AuthMethod},
        selection::construct_full_user_id,
    },
};

/// The data needed to re-build a client.
#[derive(Debug, Serialize, Deserialize)]
pub struct ClientSession {
    /// The URL of the homeserver of the user.
    pub homeserver: String,

    /// The path of the database.
    /// Relative to the data directory
    pub db_path: String,

    /// The passphrase of the database.
    pub passphrase: String,
}

/// The full session to persist.
#[derive(Debug, Serialize, Deserialize)]
pub struct FullSession {
    /// The data to re-build the client.
    pub client_session: ClientSession,

    /// The Matrix user session.
    pub user_session: MatrixSession,

    /// The latest sync token.
    ///
    /// It is only needed to persist it when using `Client::sync_once()` and we
    /// want to make our syncs faster by not receiving all the initial sync
    /// again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sync_token: Option<String>,
}

/// Insert a new account session into the database.
/// If an entry with the same user_id already exists, it will be updated.
#[instrument(level = "debug", skip(tx, session), fields(user_id = %session.user_session.meta.user_id))]
pub async fn insert_account_session(
    tx: &mut DatabaseConnection,
    session: &FullSession,
) -> eyre::Result<()> {
    debug!(
        user_id = %session.user_session.meta.user_id,
        homeserver = %session.client_session.homeserver,
        device_id = %session.user_session.meta.device_id,
        db_path = %session.client_session.db_path,
        "Inserting account session into database"
    );
    sqlx::query!(
        // language=PostgreSQL
        r#"
        insert into "account"(
            user_id,
            device_id,
            access_token,
            refresh_token,
            db_passphrase,
            homeserver_url,
            db_path,
            next_batch)
            values ($1, $2, $3, $4, $5, $6, $7, $8)
        "#,
        session.user_session.meta.user_id.to_string(),
        session.user_session.meta.device_id.to_string(),
        session.user_session.tokens.access_token,
        session.user_session.tokens.refresh_token,
        session.client_session.passphrase,
        session.client_session.homeserver,
        session.client_session.db_path,
        session.sync_token
    )
    .execute(tx)
    .await?;
    Ok(())
}

/// Load a session from the database, if it exists.
pub async fn load_session_from_db(db: &DatabasePool) -> eyre::Result<Option<FullSession>> {
    let maybe_session = sqlx::query!(
        // language=PostgreSQL
        r#"select user_id,
        device_id,
        access_token,
        refresh_token,
        homeserver_url,
        db_path,
        db_passphrase,
        next_batch
        from "account""#
    )
    .fetch_optional(db)
    .await?;

    if let Some(session_res) = maybe_session {
        Ok(Some(FullSession {
            sync_token: session_res.next_batch,
            client_session: ClientSession {
                homeserver: session_res.homeserver_url,
                db_path: session_res.db_path,
                passphrase: session_res.db_passphrase,
            },
            user_session: MatrixSession {
                meta: SessionMeta {
                    user_id: session_res.user_id.try_into()?,
                    device_id: session_res.device_id.into(),
                },
                tokens: SessionTokens {
                    access_token: session_res.access_token,
                    refresh_token: None,
                },
            },
        }))
    } else {
        Ok(None)
    }
}

/// Match a session to an account configuration
#[instrument(level = "trace", skip(session, account_configs), fields(user_id = %session.user_session.meta.user_id, homeserver = %session.client_session.homeserver))]
pub fn match_session_to_account(
    session: &FullSession,
    account_configs: &[crate::account::config::AccountDetails],
) -> Option<usize> {
    use tracing::{debug, trace};

    for (index, account) in account_configs.iter().enumerate() {
        let matches = session_is_for_account(session, account);

        if matches {
            debug!(
                session_user_id = %session.user_session.meta.user_id,
                account_user_id = %account.user_id,
                "Found matching account configuration"
            );
            return Some(index);
        }
    }

    trace!(
        user_id = %session.user_session.meta.user_id,
        "No matching account configuration found"
    );
    None
}

pub fn session_is_for_account(
    session: &FullSession,
    account: &crate::account::config::AccountDetails,
) -> bool {
    let session_user_id = &session.user_session.meta.user_id;
    let session_homeserver_url = &session.client_session.homeserver;

    let homeserver = account
        .homeserver
        .as_ref()
        .and_then(|s| <&ServerName>::try_from(s.as_str()).ok());

    let user_id = construct_full_user_id(&account.user_id, homeserver).ok();

    Some(session_user_id) == user_id.as_ref()
        || (session_user_id.localpart() == account.user_id
            && account.homeserver.as_ref() == Some(session_homeserver_url))
}

/// Load all sessions from the database
#[instrument(level = "debug", skip(db))]
pub async fn load_all_sessions_from_db(db: &DatabasePool) -> eyre::Result<Vec<FullSession>> {
    let all_sessions = sqlx::query!(
        // language=PostgreSQL
        r#"select user_id,
        device_id,
        access_token,
        refresh_token,
        homeserver_url,
        db_path,
        db_passphrase,
        next_batch
        from "account""#
    )
    .fetch_all(db)
    .await?;

    let mut sessions = Vec::new();

    for session_res in all_sessions {
        sessions.push(FullSession {
            sync_token: session_res.next_batch,
            client_session: ClientSession {
                homeserver: session_res.homeserver_url,
                db_path: session_res.db_path,
                passphrase: session_res.db_passphrase,
            },
            user_session: MatrixSession {
                meta: SessionMeta {
                    user_id: session_res.user_id.try_into()?,
                    device_id: session_res.device_id.into(),
                },
                tokens: SessionTokens {
                    access_token: session_res.access_token,
                    refresh_token: None,
                },
            },
        });
    }

    Ok(sessions)
}

/// Restore a previous session from the session data.
pub async fn restore_session(
    session: &FullSession,
    data_dir: &Path,
    cache_dir: &Path,
) -> eyre::Result<Client> {
    let FullSession {
        client_session,
        user_session,
        sync_token: _,
    } = session;

    // Build the client with the previous settings from the session.
    let client = Client::builder()
        .homeserver_url(&client_session.homeserver)
        .sqlite_store_with_cache_path(
            data_dir.join(&client_session.db_path),
            cache_dir.join(&client_session.db_path),
            Some(&client_session.passphrase),
        )
        .build()
        .await?;

    // Restore the Matrix user session.
    client.restore_session(user_session.to_owned()).await?;

    Ok(client)
}

/// Persist the sync token for a future session.
/// Note that this is needed only when using `sync_once`. Other sync methods get
/// the sync token from the store.
pub async fn persist_sync_token(
    tx: &mut DatabaseConnection,
    user_id: &UserId,
    sync_token: String,
) -> eyre::Result<()> {
    sqlx::query!(
        r#"UPDATE "account" SET next_batch = $1 WHERE user_id = $2"#,
        sync_token,
        user_id.to_string()
    )
    .execute(tx)
    .await?;

    Ok(())
}

/// Process all sessions according to configuration
#[instrument(level = "info", skip(db, config_file, data_dir, cache_dir))]
pub async fn process_sessions(
    db: &DatabasePool,
    config_file: &crate::config::ConfigFile,
    data_dir: &std::path::Path,
    cache_dir: &std::path::Path,
) -> eyre::Result<Vec<crate::client::ClientSession>> {
    info!("Processing sessions according to configuration");

    let all_sessions = load_all_sessions_from_db(db).await?;
    info!(
        session_count = all_sessions.len(),
        "Loaded sessions from database"
    );

    let (sessions_with_config, sessions_without_config, accounts_without_sessions) =
        match_sessions_to_config(all_sessions, config_file);

    let mut sessions = login_accounts(&accounts_without_sessions, db, data_dir, cache_dir).await?;

    for (session, account) in sessions_with_config {
        let client = restore_session(&session, data_dir, cache_dir).await?;
        sessions.push(crate::client::ClientSession {
            client,
            sync_token: session.sync_token,
            account_config: account,
        });
    }

    if config_file.restore_all_sessions {
        for session in sessions_without_config {
            let client = restore_session(&session, data_dir, cache_dir).await?;
            sessions.push(crate::client::ClientSession {
                sync_token: session.sync_token,
                account_config: AccountDetails {
                    user_id: client.user_id().unwrap().to_string(),
                    auth_method: AuthMethod::None,
                    recovery_key: None,
                    homeserver: None,
                    enable_encryption: false,
                    device_name: None,
                    set_device_name: false,
                    delete_other_devices: false,
                },
                client,
            });
        }
    }

    for session in sessions.iter() {
        if let Err(e) = setup_client(&session.client, &session.account_config).await {
            warn!(
                user_id = %session.account_config.user_id,
                error = %e,
                "Failed to set up session"
            );
        }
    }

    info!("Successfully processed client sessions");
    Ok(sessions)
}

/// Filter sessions based on account configs and restore_all_sessions setting
#[instrument(level = "debug", skip(all_sessions, config_file), fields(session_count = all_sessions.len(), restore_all_sessions = config_file.restore_all_sessions))]
fn match_sessions_to_config(
    all_sessions: Vec<FullSession>,
    config_file: &crate::config::ConfigFile,
) -> (
    Vec<(FullSession, AccountDetails)>,
    Vec<FullSession>,
    Vec<AccountDetails>,
) {
    use tracing::debug;

    let mut sessions_with_config = Vec::new();
    let mut sessions_without_config = Vec::new();
    let mut accounts_without_sessions = config_file.accounts.clone();

    debug!(
        restore_all_sessions = config_file.restore_all_sessions,
        session_count = all_sessions.len(),
        "Filtering sessions by configuration"
    );

    for session in all_sessions {
        let matching_account_index = match_session_to_account(&session, &accounts_without_sessions);
        if let Some(index) = matching_account_index {
            sessions_with_config.push((session, accounts_without_sessions.remove(index)))
        } else {
            sessions_without_config.push(session);
        }
    }

    debug!(
        sessions_with_config = sessions_with_config.len(),
        sessions_without_config = sessions_without_config.len(),
        accounts_without_sessions = accounts_without_sessions.len(),
        "Session filtering completed"
    );

    (
        sessions_with_config,
        sessions_without_config,
        accounts_without_sessions,
    )
}

/// Login for account configs that don't have existing sessions
#[instrument(level = "debug", skip_all)]
async fn login_accounts(
    account_configs: &[crate::account::config::AccountDetails],
    db: &DatabasePool,
    data_dir: &std::path::Path,
    cache_dir: &std::path::Path,
) -> eyre::Result<Vec<crate::client::ClientSession>> {
    use tracing::{debug, error};
    let mut client_sessions = Vec::new();

    for account_config in account_configs {
        match crate::auth::login(data_dir, cache_dir, account_config).await {
            Ok((client, session)) => {
                // Log the actual user ID returned by the server
                debug!(
                    requested_user_id = %session.user_session.meta.user_id,
                    assigned_user_id = %session.user_session.meta.user_id,
                    "Login successful"
                );

                // Insert new session to database
                save_session_to_db(db, &session).await?;

                client_sessions.push(crate::client::ClientSession {
                    client,
                    account_config: account_config.clone(),
                    sync_token: session.sync_token,
                });
            }
            Err(e) => {
                error!(
                    user_id = %account_config.user_id,
                    homeserver = %account_config.homeserver.as_deref().unwrap_or("<none>"),
                    error = %e,
                    "Account login failed"
                );

                return Err(eyre::eyre!(
                    "Failed to login for account {} with homeserver {}: {}",
                    account_config.user_id,
                    account_config.homeserver.as_deref().unwrap_or("<none>"),
                    e
                ));
            }
        }
    }
    Ok(client_sessions)
}

/// Setup a client (event cache subscription and client configuration)
async fn setup_client(
    client: &matrix_sdk::Client,
    account_config: &crate::account::config::AccountDetails,
) -> eyre::Result<()> {
    client.event_cache().subscribe()?;
    crate::client::run(client, account_config).await?;
    Ok(())
}

/// Save a session to the database
#[instrument(level = "debug", skip(db, session), fields(user_id = %session.user_session.meta.user_id))]
async fn save_session_to_db(db: &DatabasePool, session: &FullSession) -> eyre::Result<()> {
    db.acquire()
        .map_err(|e| e.into())
        .and_then(async |mut tx| insert_account_session(&mut tx, session).await)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::config::{AccountDetails, AuthMethod};
    use matrix_sdk::authentication::matrix::MatrixSession;
    use matrix_sdk::{SessionMeta, SessionTokens};
    use ruma::UserId;

    fn create_test_account(user_id: &str, homeserver: Option<&str>) -> AccountDetails {
        AccountDetails {
            user_id: user_id.to_string(),
            homeserver: homeserver.map(|s| s.to_string()),
            auth_method: AuthMethod::None,
            recovery_key: None,
            enable_encryption: true,
            device_name: None,
            set_device_name: false,
            delete_other_devices: false,
        }
    }

    fn create_test_session(
        user_id: &str,
        device_id: &str,
        homeserver: &str,
        access_token: &str,
        sync_token: Option<&str>,
    ) -> FullSession {
        FullSession {
            sync_token: sync_token.map(|s| s.to_string()),
            client_session: ClientSession {
                homeserver: homeserver.to_string(),
                db_path: format!("{device_id}_db"),
                passphrase: "test_passphrase".to_string(),
            },
            user_session: MatrixSession {
                meta: SessionMeta {
                    user_id: UserId::parse(user_id).unwrap().to_owned(),
                    device_id: device_id.into(),
                },
                tokens: SessionTokens {
                    access_token: access_token.to_string(),
                    refresh_token: None,
                },
            },
        }
    }

    #[test]
    fn test_account_matching_logic_full_mxid() {
        // Test exact match for full MXID
        let account = create_test_account("@jade:ellis.link", None);
        let accounts = vec![account];

        let session = create_test_session(
            "@jade:ellis.link",
            "DEVICE_ID",
            "https://matrix.ellis.link",
            "token123",
            None,
        );

        // This should match exactly
        let matched = match_session_to_account(&session, &accounts);
        assert!(matched.is_some());
        assert_eq!(accounts[matched.unwrap()].user_id, "@jade:ellis.link");

        // Test with different user - should not match
        let different_account = create_test_account("@bob:ellis.link", None);
        let different_accounts = vec![different_account];
        let matched = match_session_to_account(&session, &different_accounts);
        assert!(matched.is_none());
    }

    #[test]
    fn test_account_matching_logic_localpart_server_name() {
        // Test localpart + server name construction
        let account = create_test_account("jade", Some("ellis.link"));
        let accounts = vec![account];

        let session = create_test_session(
            "@jade:ellis.link",
            "DEVICE_ID",
            "https://matrix.ellis.link",
            "token123",
            None,
        );

        // This should match via localpart + server name construction
        let matched = match_session_to_account(&session, &accounts);
        assert!(matched.is_some());
        assert_eq!(accounts[matched.unwrap()].user_id, "jade");
        assert_eq!(
            accounts[matched.unwrap()].homeserver.as_ref().unwrap(),
            "ellis.link"
        );
    }

    #[test]
    fn test_account_matching_logic_localpart_url() {
        // Test localpart + URL matching
        let account = create_test_account("jade", Some("https://matrix.ellis.link"));
        let accounts = vec![account];

        let session = create_test_session(
            "@jade:ellis.link",
            "DEVICE_ID",
            "https://matrix.ellis.link",
            "token123",
            None,
        );

        // This should match via localpart + URL matching
        let matched = match_session_to_account(&session, &accounts);
        assert!(matched.is_some());
        assert_eq!(accounts[matched.unwrap()].user_id, "jade");
        assert_eq!(
            accounts[matched.unwrap()].homeserver.as_ref().unwrap(),
            "https://matrix.ellis.link"
        );
    }

    #[test]
    fn test_comprehensive_account_matching_scenarios() {
        // Create a typical session
        let session = create_test_session(
            "@jade:ellis.link",
            "ABCDEF",
            "https://matrix.ellis.link",
            "token123",
            None,
        );

        // Scenario 1: Full MXID in config matches database MXID exactly
        let account1 = create_test_account("@jade:ellis.link", None);
        let accounts1 = vec![account1];
        let matched1 = match_session_to_account(&session, &accounts1);
        assert!(matched1.is_some());
        assert_eq!(accounts1[matched1.unwrap()].user_id, "@jade:ellis.link");

        // Scenario 2: Localpart + server name in config constructs matching MXID
        let account2 = create_test_account("jade", Some("ellis.link"));
        let accounts2 = vec![account2];
        let matched2 = match_session_to_account(&session, &accounts2);
        assert!(matched2.is_some());
        assert_eq!(accounts2[matched2.unwrap()].user_id, "jade");

        // Scenario 3: Localpart + URL in config matches database localpart + URL
        let account3 = create_test_account("jade", Some("https://matrix.ellis.link"));
        let accounts3 = vec![account3];
        let matched3 = match_session_to_account(&session, &accounts3);
        assert!(matched3.is_some());
        assert_eq!(accounts3[matched3.unwrap()].user_id, "jade");

        // Scenario 4: No match
        let account4 = create_test_account("different", Some("other.com"));
        let accounts4 = vec![account4];
        let matched4 = match_session_to_account(&session, &accounts4);
        assert!(matched4.is_none());
    }
}
