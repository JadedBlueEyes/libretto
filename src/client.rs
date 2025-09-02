use color_eyre::eyre::{self, Context, eyre};
use config::{Config, File};
use futures::TryFutureExt;
use futures::future::join_all;
use matrix_sdk::{Client, config::SyncSettings, sync::SyncResponse};
use ruma::UserId;
use ruma::api::client::uiaa::{AuthData, Password, UserIdentifier};
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tokio::time::sleep;
use tracing::{debug, error, info, instrument, warn};

use crate::account::config::AccountDetails;
use crate::config::ConfigFile;
use crate::session::{load_session_from_db, restore_session};
use crate::{DatabaseConnection, DatabasePool, room_list, server};

/// Total number of sync failures allowed before reinitializing the client.
const MAX_SYNC_FAILURES: u32 = 10;

/// Backoff cap for sync retries (in power of 2 seconds).
/// This limits exponential backoff to prevent excessively long wait times.
const MAX_BACKOFF_POWER: u32 = 7;

/// Handles device management for the Matrix client.
pub async fn run(client: &Client, account_config: &AccountDetails) -> eyre::Result<()> {
    let current_session = client.device_id().map(|d| d.to_owned());

    // Delete other devices if requested
    if account_config.delete_other_devices {
        debug!(
            user_id = %client.user_id().expect("Client should be logged in"),
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
            info!(
                user_id = %client.user_id().expect("Client should be logged in"),
                device_count = other_devices.len(),
                "Deleting other devices"
            );

            // Try to delete devices with authentication
            let auth_data = match &account_config.auth_method {
                crate::account::config::AuthMethod::Password(password) => {
                    Some(AuthData::Password(Password::new(
                        UserIdentifier::UserIdOrLocalpart(account_config.user_id.clone()),
                        password.clone(),
                    )))
                }
                _ => {
                    // For other auth methods, prompt for password for UIAA
                    println!(
                        "Type password for the account (characters won't show up as you type them)"
                    );
                    match rpassword::prompt_password("Password: ") {
                        Ok(password) if !password.is_empty() => {
                            Some(AuthData::Password(Password::new(
                                UserIdentifier::UserIdOrLocalpart(account_config.user_id.clone()),
                                password,
                            )))
                        }
                        _ => {
                            warn!(
                                user_id = %client.user_id().expect("Client should be logged in"),
                                "No password provided, cannot delete other devices"
                            );
                            None
                        }
                    }
                }
            };

            if let Some(auth_data) = auth_data {
                match client.delete_devices(&other_devices, Some(auth_data)).await {
                    Ok(_) => {
                        info!(
                            user_id = %client.user_id().expect("Client should be logged in"),
                            device_count = other_devices.len(),
                            "Successfully deleted other devices"
                        );
                    }
                    Err(e) => {
                        warn!(
                            user_id = %client.user_id().expect("Client should be logged in"),
                            error = %e,
                            "Failed to delete other devices"
                        );
                    }
                }
            }
        } else {
            debug!(
                user_id = %client.user_id().expect("Client should be logged in"),
                "No other devices found to delete"
            );
        }
    }

    if account_config.set_device_name {
        if let Some(current_session) = current_session {
            let device_name = account_config.device_name.as_deref().unwrap_or("libretto");
            debug!(
                user_id = %client.user_id().expect("Client should be logged in"),
                device_id = %current_session,
                device_name = %device_name,
                "Setting device name"
            );
            client.rename_device(&current_session, device_name).await?;
        } else {
            warn!(
                user_id = %client.user_id().expect("Client should be logged in"),
                "No device ID found, cannot set device name"
            );
        }
    }

    Ok(())
}

pub async fn sync_handler(
    tx: &mut DatabaseConnection,
    client: &matrix_sdk::Client,
    user_id: &UserId,
    response: &SyncResponse,
) -> eyre::Result<()> {
    let updated_rooms = response
        .rooms
        .invited
        .keys()
        .chain(response.rooms.joined.keys())
        .chain(response.rooms.knocked.keys())
        .chain(response.rooms.left.keys());

    room_list::update_rooms(
        updated_rooms
            .filter_map(|r| client.get_room(r))
            .collect::<Vec<_>>()
            .as_slice(),
        user_id,
        &mut *tx,
    )
    .await?;

    let timeline_updates: Vec<_> = response
        .rooms
        .joined
        .iter()
        .map(|(id, update)| (id, update.timeline.clone()))
        .chain(
            response
                .rooms
                .left
                .iter()
                .map(|(id, update)| (id, update.timeline.clone())),
        )
        .filter(|(_id, update)| update.prev_batch.is_some() || !update.events.is_empty())
        .collect();

    let state_updates: Vec<_> = response
        .rooms
        .joined
        .iter()
        .map(|(id, update)| (id, update.state.clone()))
        .chain(
            response
                .rooms
                .left
                .iter()
                .map(|(id, update)| (id, update.state.clone())),
        )
        .filter(|(_id, update)| !update.is_empty())
        .collect();

    for (room_id, update) in timeline_updates {
        if update.limited {
            warn!(
                room_id = %room_id,
                "Got limited timeline, clearing existing timeline data"
            );
            sqlx::query!(
                "DELETE FROM timeline WHERE room_id = $1",
                room_id.to_string()
            )
            .execute(&mut *tx)
            .await?;

            if let Some(prev_batch) = &update.prev_batch {
                sqlx::query!(
                    "UPDATE room SET prev_batch = $1 WHERE room_id = $2 AND user_id = $3",
                    prev_batch,
                    room_id.to_string(),
                    user_id.to_string()
                )
                .execute(&mut *tx)
                .await?;
            }
        }

        for event in update.events {
            match event.raw().deserialize() {
                Ok(event_de) => {
                    let sender = event_de.sender();
                    let timestamp = event_de.origin_server_ts();
                    let event_id = event_de.event_id();
                    let event_type = event_de.event_type();

                    // Extract additional fields that may be present
                    let transaction_id = event_de.transaction_id().map(|t| t.to_string());
                    let unsigned = event
                        .raw()
                        .get_field::<serde_json::Value>("unsigned")
                        .unwrap_or_default()
                        .unwrap_or(serde_json::json!({}));
                    let megolm_session_id = event.encryption_info().and_then(|e| e.session_id());
                    let content = event
                        .raw()
                        .get_field::<serde_json::Value>("content")
                        .unwrap_or_default()
                        .unwrap_or(serde_json::json!({}));

                    // Extract relation information from content if present
                    let (relates_to, relation_type) = if let Some(content_obj) = content.as_object()
                    {
                        if let Some(relates_to_obj) =
                            content_obj.get("m.relates_to").and_then(|v| v.as_object())
                        {
                            let relates_to = relates_to_obj
                                .get("event_id")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());
                            let relation_type = relates_to_obj
                                .get("rel_type")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());
                            (relates_to, relation_type)
                        } else {
                            (None, None)
                        }
                    } else {
                        (None, None)
                    };

                    // Extract state_key for state events
                    let state_key = event.raw().get_field::<String>("state_key").ok().flatten();

                    // Insert event into database and get the rowid
                    let event_rowid: i32 = sqlx::query_scalar(
                        r#"
                        INSERT INTO event (
                            user_id, room_id, event_id, sender, timestamp,
                            transaction_id, unsigned, content, event_type,
                            relates_to, relation_type, state_key, megolm_session_id
                        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
                        ON CONFLICT (room_id, user_id, event_id) DO UPDATE SET
                            timestamp = EXCLUDED.timestamp
                        RETURNING rowid
                        "#,
                    )
                    .bind(user_id.to_string())
                    .bind(room_id.to_string())
                    .bind(event_id.to_string())
                    .bind(sender.to_string())
                    .bind({
                        let timestamp_u64: u64 = timestamp.get().into();
                        std::cmp::min(timestamp_u64, i64::MAX as u64) as i64
                    })
                    .bind(transaction_id)
                    .bind(&unsigned)
                    .bind(&content)
                    .bind(event_type.to_string())
                    .bind(relates_to)
                    .bind(relation_type)
                    .bind(state_key)
                    .bind(megolm_session_id)
                    .fetch_one(&mut *tx)
                    .await?;

                    // Extract and store media references
                    let mxc_uris = extract_mxc_uris(&content);
                    for mxc_uri in mxc_uris {
                        // Insert media if it doesn't exist
                        sqlx::query(
                            r#"INSERT INTO media (mxc) VALUES ($1) ON CONFLICT (mxc) DO NOTHING"#,
                        )
                        .bind(&mxc_uri)
                        .execute(&mut *tx)
                        .await?;

                        // Insert media reference
                        sqlx::query(
                            r#"
                            INSERT INTO media_reference (event_rowid, media_mxc)
                            VALUES ($1, $2)
                            ON CONFLICT (event_rowid, media_mxc) DO NOTHING
                            "#,
                        )
                        .bind(event_rowid)
                        .bind(&mxc_uri)
                        .execute(&mut *tx)
                        .await?;
                    }

                    // Insert into timeline
                    sqlx::query(
                        r#"
                        INSERT INTO timeline (room_id, user_id, event_rowid)
                        VALUES ($1, $2, $3)
                        ON CONFLICT (event_rowid) DO NOTHING
                        "#,
                    )
                    .bind(room_id.to_string())
                    .bind(user_id.to_string())
                    .bind(event_rowid)
                    .execute(&mut *tx)
                    .await?;
                }
                Err(e) => {
                    warn!(
                        error = %e,
                        "Failed to deserialize timeline event"
                    );
                }
            }
        }
    }

    // Process state updates (events that come through the state section, not timeline)
    for (room_id, state_events) in state_updates {
        for event in state_events {
            match event.deserialize() {
                Ok(event_de) => {
                    let sender = event_de.sender();
                    let timestamp = event_de.origin_server_ts();
                    let event_id = event_de.event_id();
                    let event_type = event_de.event_type();

                    // Extract additional fields that may be present
                    let transaction_id = event_de.transaction_id().map(|t| t.to_string());
                    let unsigned = event
                        .get_field::<serde_json::Value>("unsigned")
                        .unwrap_or_default()
                        .unwrap_or(serde_json::json!({}));
                    let content = event
                        .get_field::<serde_json::Value>("content")
                        .unwrap_or_default()
                        .unwrap_or(serde_json::json!({}));

                    // Extract state_key for state events (should always be present here)
                    let state_key = event.get_field::<String>("state_key").ok().flatten();
                    let state_key_for_current_state = state_key.clone();

                    // Insert event into database and get the rowid
                    let event_rowid: i32 = sqlx::query_scalar(
                        r#"
                        INSERT INTO event (
                            user_id, room_id, event_id, sender, timestamp,
                            transaction_id, unsigned, content, event_type,
                            state_key
                        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
                        ON CONFLICT (room_id, user_id, event_id) DO UPDATE SET
                            timestamp = EXCLUDED.timestamp
                        RETURNING rowid
                        "#,
                    )
                    .bind(user_id.to_string())
                    .bind(room_id.to_string())
                    .bind(event_id.to_string())
                    .bind(sender.to_string())
                    .bind({
                        let timestamp_u64: u64 = timestamp.get().into();
                        std::cmp::min(timestamp_u64, i64::MAX as u64) as i64
                    })
                    .bind(transaction_id)
                    .bind(&unsigned)
                    .bind(&content)
                    .bind(event_type.to_string())
                    .bind(state_key)
                    .fetch_one(&mut *tx)
                    .await?;

                    // Extract and store media references
                    let mxc_uris = extract_mxc_uris(&content);
                    for mxc_uri in mxc_uris {
                        // Insert media if it doesn't exist
                        sqlx::query(
                            r#"INSERT INTO media (mxc) VALUES ($1) ON CONFLICT (mxc) DO NOTHING"#,
                        )
                        .bind(&mxc_uri)
                        .execute(&mut *tx)
                        .await?;

                        // Insert media reference
                        sqlx::query(
                            r#"
                            INSERT INTO media_reference (event_rowid, media_mxc)
                            VALUES ($1, $2)
                            ON CONFLICT (event_rowid, media_mxc) DO NOTHING
                            "#,
                        )
                        .bind(event_rowid)
                        .bind(&mxc_uri)
                        .execute(&mut *tx)
                        .await?;
                    }

                    // Update current_state table (state events should always have a state_key)
                    if let Some(state_key_value) = &state_key_for_current_state {
                        sqlx::query(
                            r#"
                            INSERT INTO current_state (user_id, room_id, event_type, state_key, event_rowid)
                            VALUES ($1, $2, $3, $4, $5)
                            ON CONFLICT (room_id, user_id, event_type, state_key)
                            DO UPDATE SET event_rowid = EXCLUDED.event_rowid
                            "#,
                        )
                        .bind(user_id.to_string())
                        .bind(room_id.to_string())
                        .bind(event_type.to_string())
                        .bind(state_key_value)
                        .bind(event_rowid)
                        .execute(&mut *tx)
                        .await?;
                    }
                }
                Err(e) => {
                    warn!(
                        error = %e,
                        "Failed to deserialize state event"
                    );
                }
            }
        }
    }

    Ok(())
}

/// Extract MXC URIs from event content JSON
fn extract_mxc_uris(content: &serde_json::Value) -> Vec<String> {
    let mut mxc_uris = Vec::new();
    extract_mxc_uris_recursive(content, &mut mxc_uris);
    mxc_uris
}

/// Recursively search for MXC URIs in JSON values
fn extract_mxc_uris_recursive(value: &serde_json::Value, mxc_uris: &mut Vec<String>) {
    match value {
        serde_json::Value::String(s) => {
            if s.starts_with("mxc://") {
                mxc_uris.push(s.clone());
            }
            // TODO: Inline image URLs (custom emoji)
        }
        serde_json::Value::Object(obj) => {
            for v in obj.values() {
                extract_mxc_uris_recursive(v, mxc_uris);
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr {
                extract_mxc_uris_recursive(v, mxc_uris);
            }
        }
        _ => {}
    }
}

/// Represents a client with its associated account configuration and sync state.
#[derive(Debug)]
pub struct ClientSession {
    /// The Matrix SDK client instance
    pub client: Client,
    /// Account configuration; may be used to recreate the client if needed
    pub account_config: AccountDetails,
    /// Current sync token
    pub sync_token: Option<String>,
}

/// Result of processing sessions - contains clients ready for sync
/// Run sync tasks for all clients
pub async fn run_sync_tasks(
    // client_sessions: Vec<ClientSession>,
    config_file: std::path::PathBuf,
    db: &DatabasePool,
    data_dir: &std::path::Path,
    cache_dir: &std::path::Path,
) -> eyre::Result<()> {
    loop {
        // Load config file
        let config_file: ConfigFile = Config::builder()
            .add_source(File::from(config_file.clone()))
            .build()
            .context("Failed to load config file")?
            .try_deserialize()
            .context("Failed to deserialize config file")?;

        // Process sessions according to configuration
        let client_sessions =
            crate::session::process_sessions(db, &config_file, data_dir, cache_dir).await?;

        let primary_account = crate::account::selection::select_primary_account(
            &config_file.accounts,
            config_file.primary_user_id.as_deref(),
        )?;
        let primary_client = client_sessions
            .iter()
            .find(|cs| cs.account_config.user_id == primary_account.user_id)
            .map(|cs| cs.client.clone())
            .ok_or_else(|| {
                eyre::eyre!(
                    "No client found for primary account: {}",
                    primary_account.user_id
                )
            })?;
        let _ = server::CLIENT.set(RwLock::new(primary_client));

        if client_sessions.is_empty() {
            warn!("No clients available for sync tasks");
            return Ok(());
        }

        // Spawn sync tasks for all clients
        let sync_tasks: Vec<_> = client_sessions
            .into_iter()
            .map(|client_session| spawn_sync_task(client_session, db.clone(), data_dir, cache_dir))
            .collect();

        info!(sync_task_count = sync_tasks.len(), "Starting sync tasks");

        // Wait for all sync tasks to complete
        let result = join_all(sync_tasks).await;
        debug!("All sync tasks finished");

        if result.iter().all(|r| r.is_err()) {
            error!("Sync tasks failed, restarting");
        } else {
            info!("Sync tasks completed successfully");
            break;
        }
    }

    Ok(())
}

/// Spawn a sync task for a given client session
fn spawn_sync_task(
    mut client_session: ClientSession,
    db: DatabasePool,
    data_dir: &std::path::Path,
    cache_dir: &std::path::Path,
) -> tokio::task::JoinHandle<()> {
    let data_dir = data_dir.to_owned();
    let cache_dir = cache_dir.to_owned();
    tokio::spawn(async move {
        let user_id = client_session
            .client
            .user_id()
            .expect("Client should be logged in");
        let user_id_string = user_id.to_string();
        info!(
            user_id = %user_id_string,
            "Starting sync task"
        );

        loop {
            if let Err(e) = perform_initial_room_update(&client_session.client, &db).await {
                warn!(
                    user_id = %user_id_string,
                    error = %e,
                    "Failed initial room update"
                );
            }

            if let Err(e) =
                run_sync_loop(client_session.client, &db, client_session.sync_token).await
            {
                match load_session_from_db(&db, &user_id_string)
                    .and_then(async |session| {
                        if let Some(session) = session {
                            restore_session(&session, &data_dir, &cache_dir)
                                .await
                                .map(|c| (c, session))
                        } else {
                            Err(eyre!("Failed to find session"))
                        }
                    })
                    .await
                {
                    Ok((client, session)) => {
                        client_session = crate::client::ClientSession {
                            client,
                            sync_token: session.sync_token,
                            account_config: client_session.account_config,
                        };
                    }
                    Err(_) => {
                        error!(
                            user_id = %user_id_string,
                            error = %e,
                            "Failed to restore session"
                        );
                        break;
                    }
                };
            } else {
                break;
            }
        }

        debug!(
            user_id = %user_id_string,
            "Sync task finished"
        );
    })
}

/// Perform initial room update for a client
#[instrument(level = "info", skip(client, db))]
async fn perform_initial_room_update(client: &Client, db: &DatabasePool) -> eyre::Result<()> {
    use futures::TryFutureExt;

    let user_id = client.user_id().expect("Client should be logged in");
    let rooms: Vec<_> = client.rooms().into_iter().collect();

    db.acquire()
        .map_err(|e| e.into())
        .and_then(async |mut tx| crate::room_list::update_rooms(&rooms, user_id, &mut tx).await)
        .await
}

/// Run the sync loop for a client
/// Will return an error if the sync fails enough times in a row
async fn run_sync_loop(
    client: Client,
    db: &DatabasePool,
    sync_token: Option<String>,
) -> eyre::Result<()> {
    let user_id = client.user_id().expect("Client should be logged in");
    let mut last_sync_time: Option<Instant> = None;

    // Setup sync settings
    let filter = matrix_sdk::ruma::api::client::filter::FilterDefinition::with_lazy_loading();
    let mut sync_settings = SyncSettings::default().filter(filter.into());
    let mut backoff = None;

    if let Some(token) = sync_token {
        sync_settings = sync_settings.token(token);
    }

    let sync_loop = async {
        loop {
            let result = perform_sync_once(&client, db, user_id, &sync_settings).await;

            match result {
                Ok(next_batch) => {
                    sync_settings = sync_settings.token(next_batch);
                    backoff = None;
                }
                Err(err) => {
                    let backoff_seconds = 2u64.pow(backoff.unwrap_or(0).min(MAX_BACKOFF_POWER));
                    error!(
                        user_id = %user_id,
                        error = %err,
                        "Sync error occurred"
                    );

                    backoff = Some(backoff.unwrap_or(0) + 1);

                    // If we've had too many consecutive failures, return an error
                    let count = backoff.unwrap_or(0);
                    if count >= MAX_SYNC_FAILURES {
                        return Err(eyre::eyre!(
                            "Sync failed {count} consecutive times, last error: {}",
                            err
                        ));
                    }

                    warn!(
                        user_id = %user_id,
                        backoff_seconds = backoff_seconds,
                        "Backing off before retry"
                    );

                    sleep(Duration::from_secs(backoff_seconds)).await;
                    continue;
                }
            }

            // Rate limiting
            rate_limit_sync(&mut last_sync_time).await;
        }
    };

    // Run sync loop with shutdown signal
    tokio::select! {
        result = sync_loop => {
            match result {
                Ok(()) => debug!(
                    user_id = %user_id,
                    "Sync loop finished"
                ),
                Err(e) => {
                    error!(
                        user_id = %user_id,
                        error = %e,
                        "Sync loop failed with error"
                    );
                    return Err(e);
                }
            }
        },
        _ = server::shutdown_signal() => debug!(
            user_id = %user_id,
            "Sync shutdown requested"
        ),
    }

    Ok(())
}

/// Perform a single sync operation
#[instrument(level = "debug", skip(client, db, sync_settings), fields(user_id = %user_id))]
async fn perform_sync_once(
    client: &Client,
    db: &DatabasePool,
    user_id: &ruma::UserId,
    sync_settings: &SyncSettings,
) -> eyre::Result<String> {
    use futures::TryFutureExt;

    client
        .sync_once(sync_settings.clone())
        .map_err(|e| e.into())
        .and_then(async |response: SyncResponse| {
            let tx = db.begin().await?;
            Ok((response, tx))
        })
        .and_then(async |(response, mut tx)| {
            sync_handler(&mut tx, client, user_id, &response).await?;
            crate::session::persist_sync_token(&mut tx, user_id, response.next_batch.clone())
                .await?;
            Ok((response, tx))
        })
        .and_then(async |(response, tx)| {
            tx.commit().await?;
            Ok(response.next_batch)
        })
        .await
}

/// Apply rate limiting to sync operations
#[instrument(level = "trace", skip(last_sync_time))]
async fn rate_limit_sync(last_sync_time: &mut Option<Instant>) -> () {
    let now = Instant::now();

    if let Some(last_time) = *last_sync_time {
        let duration = now - last_time;
        if duration <= Duration::from_secs(1) {
            sleep(Duration::from_secs(1) - duration).await;
        }
    }

    *last_sync_time = Some(now);
}
