use color_eyre::eyre;
use matrix_sdk::Client;

use matrix_sdk::sync::SyncResponse;
use ruma::UserId;
use ruma::api::client::uiaa::{AuthData, Password, UserIdentifier};
use tracing::{info, trace, warn};

use crate::account::config::AccountDetails;
use crate::config::CommandConfig;
use crate::{DatabaseConnection, room_list};

/// Handles device management for the Matrix client.
pub async fn run(
    client: &Client,
    _config: &CommandConfig,
    account_config: &AccountDetails,
) -> eyre::Result<()> {
    let current_session = client.device_id().map(|d| d.to_owned());

    // Delete other devices if requested
    if account_config.delete_other_devices {
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
                            warn!("No password provided, cannot delete other devices");
                            None
                        }
                    }
                }
            };

            if let Some(auth_data) = auth_data {
                match client.delete_devices(&other_devices, Some(auth_data)).await {
                    Ok(_) => {
                        info!("Successfully deleted {} other devices", other_devices.len());
                    }
                    Err(e) => {
                        warn!("Failed to delete other devices: {}", e);
                    }
                }
            }
        } else {
            info!("No other devices found to delete");
        }
    }

    if account_config.set_device_name {
        if let Some(current_session) = current_session {
            let device_name = account_config.device_name.as_deref().unwrap_or("libretto");
            info!(
                current_session = format!("{current_session:?}"),
                "Renaming device to {}", device_name
            );
            client.rename_device(&current_session, device_name).await?;
        } else {
            warn!("No device ID found, cannot name device");
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

    for (room_id, update) in timeline_updates {
        if update.limited {
            warn!("Got limited timeline from update {room_id}");
            sqlx::query!(
                "DELETE FROM timeline WHERE room_id = $1",
                room_id.to_string()
            )
            .execute(&mut *tx)
            .await?;
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
                    warn!("Failed to deserialize event: {:?}", e);
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
