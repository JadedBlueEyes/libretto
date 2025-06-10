// filepath: /Users/jade/Code/libretto/src/room_list.rs
use matrix_sdk::{Room, RoomDisplayName, RoomState};
use ruma::{OwnedRoomId, RoomId};
use serde::{Deserialize, Serialize};

use crate::AppError;

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
        state: room.state(),
    })
}
