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
use ruma::{events::room::MediaSource, uint};
use tracing::{info, warn};

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
use sqlx::Row;

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

    // Fetch timeline events from the database (fetch limit+1 to check for next page)
    let mut rows: Vec<DbTimelineEvent> = sqlx::query_as(
                    r#"SELECT
                        event.rowid, timeline.rowid as timeline_rowid,
                        event.room_id, event_id, sender, event_type, state_key,
                        timestamp, content::jsonb,
                        unsigned::jsonb, transaction_id, redacted_by, relates_to, relation_type,
                        megolm_session_id, last_edit_rowid
                    FROM timeline
                    JOIN event ON event.rowid = timeline.event_rowid
                    WHERE timeline.user_id = $1 AND timeline.room_id = $2 AND ($3 = 0 OR timeline.rowid <= $3)
                    ORDER BY timeline.rowid DESC
                    LIMIT $4;"#,
                )
                .bind(user_id.as_str())
                .bind(room_id.as_str())
                .bind(last_rowid.unwrap_or(0))
                .bind((limit + 1) as i32)
                .fetch_all(&db)
                .await?;

    let mut hit_end_of_timeline = false;
    let mut next_page = None;
    let mut prev_page = None;

    // If we fetched more than limit, there is a next page
    if rows.len() > limit {
        let next_row = rows.last().expect("Failed to get last row");
        prev_page = Some(format!("/room/{room_id}/{}", next_row.timeline_rowid));
        rows.truncate(limit);
    } else {
        hit_end_of_timeline = true;
    }

    // Only show prev_page if this is not the first page (i.e., last_rowid is Some and not 0)
    if let Some(last_rowid) = last_rowid
        && last_rowid != 0
    {
        let row = sqlx::query(
            r#"SELECT COALESCE(
                    (
                        SELECT timeline.rowid as timeline_rowid
                        FROM timeline
                        WHERE timeline.user_id = $1 AND timeline.room_id = $2 AND timeline.rowid >= $3
                        ORDER BY timeline.rowid ASC
                        OFFSET $4 LIMIT 1
                    ),
                    (
                        SELECT MAX(timeline.rowid) as timeline_rowid
                        FROM timeline
                        WHERE timeline.user_id = $1 AND timeline.room_id = $2
                    )
                ) as timeline_rowid;"#,
        )
        .bind(user_id.as_str())
        .bind(room_id.as_str())
        .bind(last_rowid)
        .bind(limit as i32)
        .fetch_one(&db)
        .await
        .ok();

        if let Some(timeline_rowid) = row.map(|r| r.get::<i32, &str>("timeline_rowid"))
            && timeline_rowid != last_rowid
        {
            next_page = Some(format!("/room/{room_id}/{timeline_rowid}"));
        }
    }

    // Convert database rows to TimelineEvent objects
    for (i, row) in rows.into_iter().enumerate() {
        if let Ok(event) = build_timeline_event_from_db(row).await.inspect_err(|e| {
            warn!(
                error = %e,
                "Failed to build timeline event from database"
            );
        }) {
            if let Some(next) = i.checked_sub(1).and_then(|i| timeline.get_mut(i)) {
                // Check if same sender and within 10 minutes (600,000 milliseconds)
                if (next.sender == event.sender)
                    && (next.timestamp.0.saturating_sub(event.timestamp.0) <= uint!(600_000))
                {
                    next.same_sender = true;
                }
            };
            timeline.push(event);
        }
    }

    timeline.reverse();

    let template = RoomTemplate {
        name: room
            .display_name()
            .await
            .map(|name| name.to_string())
            .unwrap_or("Unknown Room".to_owned()),
        room_id: &room_id,
        hit_end_of_timeline,
        room: &room,
        prev_page,
        next_page,
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

pub async fn serve(primary_client: matrix_sdk::Client, db: DatabasePool) -> eyre::Result<()> {
    info!("Starting web server");

    // Build the router with the primary client
    let app = build_router(primary_client, db);

    // Setup TCP listener with support for systemfd/listenfd
    let listener = setup_tcp_listener().await?;

    info!(
        listener = ?listener,
        "Web server listening"
    );

    // Run the web server with graceful shutdown
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    info!("Web server shut down gracefully");
    Ok(())
}

/// Setup TCP listener with support for systemfd/listenfd
async fn setup_tcp_listener() -> eyre::Result<tokio::net::TcpListener> {
    use listenfd::ListenFd;

    let mut listenfd = ListenFd::from_env();
    let listener = match listenfd.take_tcp_listener(0)? {
        Some(listener) => {
            listener.set_nonblocking(true)?;
            tokio::net::TcpListener::from_std(listener)?
        }
        None => tokio::net::TcpListener::bind("0.0.0.0:3000").await?,
    };

    Ok(listener)
}
