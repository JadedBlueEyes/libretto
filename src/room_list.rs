use color_eyre::eyre::{self, Context};
// filepath: /Users/jade/Code/libretto/src/room_list.rs
use matrix_sdk::ruma::{OwnedRoomId, RoomId};
use matrix_sdk::{Room, RoomCreateWithCreatorEventContent, RoomDisplayName};
use ruma::events::room::tombstone::RoomTombstoneEventContent;
use ruma::{OwnedRoomAliasId, OwnedUserId, UserId};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use sqlx::types::Json as SqlxJson;

use crate::DatabasePool;
use crate::error::AppError;

/// Represents a room in the room list with additional metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomListEntry {
    /// The Matrix room ID
    pub id: OwnedRoomId,

    /// The human-readable name of the room
    pub name: RoomDisplayName,

    /// The room's avatar URL if available
    pub avatar_url: Option<String>,

    /// Whether the room is encrypted
    pub is_encrypted: bool,

    /// Whether the room is a direct message room
    pub is_direct: bool,

    /// Number of unread messages or notifications
    pub unread_count: u64,

    /// The room's join state (joined, invited, left)
    pub state: RoomState,
}

impl RoomListEntry {
    /// Get the first letter of the room name for avatar placeholder
    pub fn name_initial(&self) -> String {
        let name = self.name.to_string();
        name.chars()
            .next()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "?".to_string())
    }

    /// Check if the room has unread messages
    pub fn has_unread(&self) -> bool {
        self.unread_count > 0
    }
}

/// A collection of rooms organized by category
#[derive(Debug, Serialize, Deserialize)]
pub struct RoomList {
    /// All rooms in the list
    pub rooms: Vec<RoomListEntry>,
}

impl RoomList {
    /// Create a new empty room list
    pub fn new() -> Self {
        Self { rooms: Vec::new() }
    }

    /// Add a room to the list
    pub fn add_room(&mut self, room: RoomListEntry) {
        self.rooms.push(room);
    }

    /// Get a room by its ID
    pub fn get_room(&self, room_id: &RoomId) -> Option<&RoomListEntry> {
        self.rooms.iter().find(|room| room.id == *room_id)
    }

    /// Sort rooms by display names alphabetically
    pub fn sort_by_display_names(&mut self) {
        self.rooms.sort_by(|a, b| {
            // Convert both room names to strings and compare them
            let a_name = a.name.to_string().to_lowercase();
            let b_name = b.name.to_string().to_lowercase();

            // Sort alphabetically, case-insensitive
            a_name.cmp(&b_name)
        });
    }
}

/// Represents a row in the `room` table in the database.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct RoomDbEntry {
    #[sqlx(try_from = "String")]
    pub room_id: OwnedRoomId,
    #[sqlx(try_from = "String")]
    pub user_id: OwnedUserId,

    pub room_type: Option<String>,
    #[sqlx(json(nullable))]
    pub creation_content: Option<RoomCreateWithCreatorEventContent>,
    #[sqlx(json(nullable))]
    pub tombstone_content: Option<RoomTombstoneEventContent>,

    pub room_state: RoomState,

    #[sqlx(json(nullable))]
    pub name: Option<RoomDisplayName>,
    pub avatar: Option<String>,

    pub topic: Option<String>,
    #[sqlx(try_from = "OptionAliasDbParser")]
    pub canonical_alias: Option<OwnedRoomAliasId>,

    pub encryption_state: Option<bool>,

    // #[sqlx(try_from = "Option<i32>")]
    pub last_event_timestamp: Option<i64>,

    pub unread_highlight_count: i32,
    pub unread_notification_count: i32,

    pub prev_batch: Option<String>,
}

/// Enum keeping track of the membership of our user in a room.
/// Differs from the matrix_sdk enum in that it includes the 'forgotten' state.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "room_membership_state", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum RoomState {
    Joined,
    Left,
    Invited,
    Knocked,
    Banned,
    Forgotten,
}

impl From<matrix_sdk::RoomState> for RoomState {
    fn from(state: matrix_sdk::RoomState) -> Self {
        match state {
            matrix_sdk::RoomState::Joined => RoomState::Joined,
            matrix_sdk::RoomState::Left => RoomState::Left,
            matrix_sdk::RoomState::Invited => RoomState::Invited,
            matrix_sdk::RoomState::Knocked => RoomState::Knocked,
            matrix_sdk::RoomState::Banned => RoomState::Banned,
        }
    }
}

#[derive(sqlx::Type)]
#[sqlx(transparent)]
struct OptionAliasDbParser(Option<String>);

impl TryFrom<OptionAliasDbParser> for Option<OwnedRoomAliasId> {
    type Error = ruma::IdParseError;

    fn try_from(value: OptionAliasDbParser) -> Result<Self, Self::Error> {
        value.0.map(OwnedRoomAliasId::try_from).transpose()
    }
}

/// Helper function to create a RoomListEntry from a matrix-sdk Room
pub async fn room_to_list_entry(room: &Room) -> Result<RoomListEntry, AppError> {
    let room_id = room.room_id().to_owned();
    let is_direct = room.is_direct().await?;

    Ok(RoomListEntry {
        id: room_id,
        name: room.display_name().await?,
        avatar_url: room.avatar_url().map(|url| url.to_string()),
        is_encrypted: room.encryption_state().is_encrypted(),
        is_direct,
        unread_count: room.unread_notification_counts().notification_count,
        state: room.state().into(),
    })
}

async fn get_room_db(
    user_id: &UserId,
    room_id: &OwnedRoomId,
    db: &DatabasePool,
) -> eyre::Result<RoomDbEntry> {
    sqlx::query(
        // language=PostgreSQL
        r#"select *
        from "room"
        where user_id = $1 and room_id = $2"#,
    )
    .bind(user_id.as_str())
    .bind(room_id.as_str())
    .map(|row| RoomDbEntry::from_row(&row).wrap_err("Failed to deserialize Room"))
    .fetch_one(db)
    .await
    .wrap_err("Failed to fetch Room")?
}

pub async fn update_rooms(
    rooms: &[matrix_sdk::Room],
    user_id: &UserId,
    db: &DatabasePool,
) -> eyre::Result<()> {
    let mut tx = db.begin().await?;
    let forgotten_rooms = room_ids_from_db(user_id, db)
        .await?
        .into_iter()
        .filter(|room_id| rooms.iter().any(|room| room.room_id() == room_id))
        .map(|room_id| room_id.into())
        .collect::<Vec<String>>();

    sqlx::query!(
        // language=PostgreSQL
        r#"update "room"
        set room_state = 'forgotten'
        where user_id = $1 and room_id = any($2)"#,
        user_id.as_str(),
        &forgotten_rooms
    )
    .execute(&mut *tx)
    .await
    .wrap_err("Failed to mark forgotten rooms")?;

    for room in rooms {
        let encryption = match room.encryption_state() {
            matrix_sdk::EncryptionState::Encrypted => Some(true),
            matrix_sdk::EncryptionState::NotEncrypted => Some(false),
            matrix_sdk::EncryptionState::Unknown => None,
        };
        let counts = room.unread_notification_counts();

        // Upsert rooms we still have knowledge of
        sqlx::query!(
            // language=PostgreSQL
            r#"insert into "room" (
                user_id,
                room_id,
                room_state,
                room_type,
                creation_content,
                tombstone_content,
                encryption_state,
                unread_highlight_count,
                unread_notification_count,
                name,
                avatar,
                topic,
                canonical_alias
            )
            values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
            on conflict (user_id, room_id) do update set
            room_type                 = COALESCE(EXCLUDED.room_type, room.room_type),
            creation_content          = COALESCE(EXCLUDED.creation_content, room.creation_content),
            tombstone_content         = COALESCE(EXCLUDED.tombstone_content, room.tombstone_content),
            room_state                = COALESCE(EXCLUDED.room_state, room.room_state),
            name                      = COALESCE(EXCLUDED.name, room.name),
            avatar                    = COALESCE(EXCLUDED.avatar, room.avatar),
            topic                     = COALESCE(EXCLUDED.topic, room.topic),
            canonical_alias           = COALESCE(EXCLUDED.canonical_alias, room.canonical_alias),
            encryption_state          = COALESCE(EXCLUDED.encryption_state, room.encryption_state),
            unread_highlight_count    = COALESCE(EXCLUDED.unread_highlight_count, room.unread_highlight_count),
            unread_notification_count = COALESCE(EXCLUDED.unread_notification_count, room.unread_notification_count)
            "#,
            user_id.as_str(),
            room.room_id().as_str(),
            RoomState::from(room.state()) as RoomState,
            room.room_type().map(|i| i.to_string()),
            room.create_content().map(|i| SqlxJson(i)) as _,
            room.tombstone_content().map(|i| SqlxJson(i)) as _,
            encryption,
            counts.highlight_count as i32,
            counts.notification_count as i32,
            SqlxJson(room.display_name().await?) as _,
            room.avatar_url().map(|i| i.to_string()),
            room.topic(),
            room.canonical_alias().map(|i| i.to_string())
        )
        .execute(&mut *tx)
        .await
        .wrap_err("Failed to upsert room")?;
    }

    tx.commit().await?;
    Ok(())
}

pub async fn room_ids_from_db(
    user_id: &UserId,
    db: &DatabasePool,
) -> eyre::Result<Vec<OwnedRoomId>> {
    sqlx::query_scalar!(
        // language=PostgreSQL
        r#"select room_id
        from "room"
        where user_id = $1"#,
        user_id.as_str()
    )
    .fetch_all(db)
    .await?
    .into_iter()
    .map(|r| r.try_into().wrap_err("Failed to convert room ID"))
    .collect()
}
