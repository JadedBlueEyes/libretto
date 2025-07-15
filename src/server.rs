use axum::{
    Router, extract,
    http::header,
    response::{Html, IntoResponse},
    routing::get,
};
use color_eyre::eyre;
use futures::StreamExt;
use matrix_sdk::{
    Client,
    media::{MediaFormat, MediaThumbnailSettings},
};
use ruma::events::room::MediaSource;

use crate::error::AppError;
use crate::room_to_html::{RoomListTemplate, RoomTemplate};
use crate::timeline::build_timeline_event;
use crate::{
    assets::Dist,
    room_list::{RoomList, room_to_list_entry},
};
use askama::Template;

/// Sets up the Axum router with all routes and state.
pub fn build_router(client: Client) -> Router {
    Router::new()
        .route("/room/{room_id}", get(room))
        .route("/media/plain/{dimensions}/{media_id}", get(load_media_file))
        .route("/", get(index))
        .fallback(get(crate::assets::static_service::<Dist>))
        .with_state(client)
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

/// Handler for the room route, showing timeline for a room.
pub async fn room(
    extract::State(client): extract::State<Client>,
    extract::Path(room_id): extract::Path<String>,
) -> Result<impl IntoResponse, AppError> {
    use futures::{TryStreamExt, stream};
    use matrix_sdk::ruma::{OwnedRoomId, RoomAliasId, assign};

    let room_id: OwnedRoomId = if let Ok(alias) = <&RoomAliasId>::try_from(room_id.as_str()) {
        client.resolve_room_alias(alias).await?.room_id
    } else {
        OwnedRoomId::try_from(room_id.as_str())
            .map_err(AppError::from)
            .expect("Room ID was not a valid ID or alias!")
    };

    client
        .encryption()
        .backups()
        .download_room_keys_for_room(&room_id)
        .await
        .inspect_err(|e| {
            tracing::error!("Failed to download room keys for room {room_id}: {e}");
        })?;

    let room = client
        .get_room(&room_id)
        .ok_or_else(|| eyre::eyre!("Failed to get room"))?;

    let messages = room
        .messages(assign!(matrix_sdk::room::MessagesOptions::backward(), {limit: 100u8.into()}))
        .await?;
    let mut events = messages.chunk;
    let token = messages.end;
    events.reverse();

    let timeline = stream::iter(events)
        .then(|i| build_timeline_event(&client, &room_id, i))
        .try_collect::<Vec<_>>()
        .await?;

    let template = RoomTemplate {
        name: room
            .display_name()
            .await
            .map(|name| name.to_string())
            .unwrap_or("Unknown Room".to_owned()),
        room_id: &room_id,
        hit_end_of_timeline: token.is_none(),
        room: &room,
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
