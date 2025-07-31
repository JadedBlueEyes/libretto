use axum::{
    Router, extract,
    http::header,
    response::{Html, IntoResponse},
    routing::get,
};
use color_eyre::eyre;
use matrix_sdk::{
    Client,
    media::{MediaFormat, MediaThumbnailSettings},
};
use ruma::events::room::MediaSource;
use tracing_log::log::warn;

use crate::timeline::TimelineEvent;
use crate::{DatabasePool, error::AppError};
use crate::{
    assets::Dist,
    room_list::{RoomList, room_to_list_entry},
};
use crate::{
    room_to_html::{RoomListTemplate, RoomTemplate},
    timeline::{DbTimelineEvent, build_timeline_event_from_db},
};
use askama::Template;

#[derive(Clone, extract::FromRef)]
struct AppState {
    client: Client,
    db: DatabasePool,
}

/// Sets up the Axum router with all routes and state.
pub fn build_router(client: Client, db: DatabasePool) -> Router {
    Router::new()
        .route("/room/{room_id}/{page}", get(room_page))
        .route("/room/{room_id}", get(room))
        .route("/media/plain/{dimensions}/{media_id}", get(load_media_file))
        .route("/", get(index))
        .fallback(get(crate::assets::static_service::<Dist>))
        .with_state(AppState { db, client })
}

/// Handler for the index route, listing all rooms.
pub async fn index(
    extract::State(client): extract::State<Client>,
) -> Result<impl IntoResponse, AppError> {
    let mut list = RoomList::new();
    for room in client.joined_rooms() {
        if let Ok(room_entry) = room_to_list_entry(&room).await {
            list.add_room(room_entry);
        }
    }

    list.sort_by_display_names();

    let template = RoomListTemplate { rooms: list.rooms };

    Ok(Html(template.render().map_err(|e| eyre::eyre!(e))?).into_response())
}
//

/// Handler for the room route, showing timeline for a room.
pub async fn room(
    extract::State(client): extract::State<Client>,
    extract::State(db): extract::State<DatabasePool>,
    extract::Path(room_id): extract::Path<String>,
) -> Result<impl IntoResponse, AppError> {
    room_internal(client, db, room_id, None).await
}
pub async fn room_page(
    extract::State(client): extract::State<Client>,
    extract::State(db): extract::State<DatabasePool>,
    extract::Path((room_id, last_rowid)): extract::Path<(String, Option<i32>)>,
) -> Result<impl IntoResponse, AppError> {
    room_internal(client, db, room_id, last_rowid).await
}
/// Handler for the room route, showing timeline for a room.
pub async fn room_internal(
    client: Client,
    db: DatabasePool,
    room_id: String,
    last_rowid: Option<i32>,
) -> Result<impl IntoResponse, AppError> {
    dbg!(&room_id, &last_rowid);
    use matrix_sdk::ruma::{OwnedRoomId, RoomAliasId};

    let room_id: OwnedRoomId = if let Ok(alias) = <&RoomAliasId>::try_from(room_id.as_str()) {
        client.resolve_room_alias(alias).await?.room_id
    } else {
        OwnedRoomId::try_from(room_id.as_str())
            .map_err(AppError::from)
            .expect("Room ID was not a valid ID or alias!")
    };

    // client
    //     .encryption()
    //     .backups()
    //     .download_room_keys_for_room(&room_id)
    //     .await
    //     .inspect_err(|e| {
    //         tracing::error!("Failed to download room keys for room {room_id}: {e}");
    //     })?;

    let room = client
        .get_room(&room_id)
        .ok_or_else(|| eyre::eyre!("Failed to get room"))?;

    // Get the current user ID
    let user_id = client
        .user_id()
        .ok_or_else(|| eyre::eyre!("User not logged in"))?
        .to_owned();

    let mut timeline: Vec<TimelineEvent> = Vec::new();
    let limit = 250;

    // Fetch timeline events from the database
    let rows: Vec<DbTimelineEvent> = sqlx::query_as(
                    r#"SELECT
                        event.rowid, timeline.rowid as timeline_rowid,
                        event.room_id, event_id, sender, event_type, state_key,
                        timestamp, content::jsonb,
                        unsigned::jsonb, transaction_id, redacted_by, relates_to, relation_type,
                        megolm_session_id, last_edit_rowid
                    FROM timeline
                    JOIN event ON event.rowid = timeline.event_rowid
                    WHERE timeline.user_id = $1 AND timeline.room_id = $2 AND ($3 = 0 OR timeline.rowid < $3)
                    ORDER BY timeline.rowid DESC
                    LIMIT $4"#,
                )
                .bind(user_id.as_str())
                .bind(room_id.as_str())
                .bind(last_rowid.unwrap_or(0))
                .bind(limit as i32)
                .fetch_all(&db)
                .await?;

    // let is_room_encrypted = room.encryption_state().is_encrypted();

    let hit_end_of_timeline = rows.len() < limit;

    // Convert database rows to TimelineEvent objects
    for row in rows {
        if let Ok(event) = build_timeline_event_from_db(row)
            .await
            .inspect_err(|e| warn!("Error building timeline: {e}"))
        {
            timeline.push(event);
        }
    }

    let template = RoomTemplate {
        name: room
            .display_name()
            .await
            .map(|name| name.to_string())
            .unwrap_or("Unknown Room".to_owned()),
        room_id: &room_id,
        hit_end_of_timeline,
        room: &room,
        prev_page: Some(format!("/room/{room_id}/{limit}")),
        events: timeline,
    };

    Ok(Html(template.render().map_err(|e| eyre::eyre!(e))?).into_response())
}

pub async fn load_media_file(
    extract::State(client): extract::State<Client>,
    extract::Path((dimensions, media_id)): extract::Path<(String, String)>,
) -> Result<impl IntoResponse, AppError> {
    let request = matrix_sdk::media::MediaRequestParameters {
        source: MediaSource::Plain(media_id.into()),
        format: match dimensions.as_str() {
            "file" | "full" => MediaFormat::File,
            dimension_string if dimension_string.starts_with("thumbnail:") => {
                let (width, height) = dimension_string[10..].split_once('x').unwrap_or_default();
                MediaFormat::Thumbnail(MediaThumbnailSettings {
                    width: width.parse().unwrap_or_default(),
                    height: height.parse().unwrap_or_default(),
                    method: ruma::media::Method::Crop,
                    animated: true,
                })
            }
            _ => return Err(eyre::format_err!("Invalid dimension format").into()),
        },
    };
    let media = client.media().get_media_content(&request, true).await?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (header::CONTENT_DISPOSITION, "attachment"),
            (header::CACHE_CONTROL, "max-age=31536000"),
        ],
        media.into_response(),
    ))
}

/// Handles graceful shutdown signals (Ctrl+C, SIGTERM).
pub async fn shutdown_signal() {
    use tokio::signal;

    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
