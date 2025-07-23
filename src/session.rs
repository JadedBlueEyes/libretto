use std::path::Path;

use color_eyre::eyre;
use matrix_sdk::{Client, SessionMeta, SessionTokens, authentication::matrix::MatrixSession};
use ruma::UserId;
use serde::{Deserialize, Serialize};

use crate::{DatabaseConnection, DatabasePool};

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
pub async fn insert_account_session(
    tx: &mut DatabaseConnection,
    session: &FullSession,
) -> eyre::Result<()> {
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

/// Restore a previous session from the session data.
pub async fn restore_session(
    session: FullSession,
    data_dir: &Path,
) -> eyre::Result<(Client, Option<String>)> {
    let FullSession {
        client_session,
        user_session,
        sync_token,
    } = session;

    // Build the client with the previous settings from the session.
    let client = Client::builder()
        .homeserver_url(client_session.homeserver)
        .sqlite_store(
            data_dir.join(&client_session.db_path),
            Some(&client_session.passphrase),
        )
        .build()
        .await?;

    tracing::info!("Restoring session for {}…", user_session.meta.user_id);

    // Restore the Matrix user session.
    client.restore_session(user_session).await?;

    Ok((client, sync_token))
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
