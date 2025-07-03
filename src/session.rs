use std::path::Path;

use color_eyre::eyre;
use matrix_sdk::{Client, authentication::matrix::MatrixSession};
use ruma::UserId;
use serde::{Deserialize, Serialize};

use crate::DatabasePool;

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

/// Restore a previous session from a file.
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
    database: DatabasePool,
    user_id: &UserId,
    sync_token: String,
) -> eyre::Result<()> {
    sqlx::query!(
        r#"UPDATE "account" SET next_batch = $1 WHERE user_id = $2"#,
        sync_token,
        user_id.to_string()
    )
    .execute(&database)
    .await?;

    Ok(())
}
