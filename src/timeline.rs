use std::{collections::BTreeMap, sync::Arc};

use color_eyre::eyre;
use futures::prelude::*;
use ruma::{
    MilliSecondsSinceUnixEpoch, OwnedEventId, OwnedMxcUri, OwnedUserId, RoomId,
    events::{
        AnyFullStateEventContent, AnySyncMessageLikeEvent, AnySyncTimelineEvent, StateEventType,
        room::message::{MessageType, Relation, RoomMessageEventContentWithoutRelation},
    },
    html::RemoveReplyFallback,
};
use serde_json::value::RawValue;

pub async fn build_timeline_event(
    client: &matrix_sdk::Client,
    room_id: &RoomId,
    event: matrix_sdk::deserialized_responses::TimelineEvent,
) -> eyre::Result<TimelineEvent> {
    let event_de = event.raw().deserialize()?;
    let sender = event_de.sender();
    let timestamp = event_de.origin_server_ts();

    let room = client.get_room(room_id);
    let sender_profile = if let Some(ref room) = room {
        let mut profile = room.get_member_no_sync(sender).await?;

        // Fallback to the slow path.
        if profile.is_none() {
            profile = room.get_member(sender).await?;
        }
        profile.as_mut().map(|profile| Profile {
            display_name: profile.display_name().map(ToOwned::to_owned),
            display_name_ambiguous: profile.name_ambiguous(),
            avatar_url: profile.avatar_url().map(ToOwned::to_owned),
        })
    } else {
        None
    };
    let is_room_encrypted = room
        .map(|r| r.encryption_state().is_encrypted())
        .unwrap_or(false);

    let content = build_timeline_item(&event_de).await?;

    Ok(TimelineEvent {
        sender: sender.into(),
        sender_profile,
        timestamp,
        content,
        is_room_encrypted,
        event_id: event.event_id(),
        raw: event.into_raw().into_json(),
    })
}

pub async fn build_timeline_item(
    event: &AnySyncTimelineEvent,
) -> eyre::Result<TimelineItemContent> {
    match event {
        AnySyncTimelineEvent::MessageLike(any_sync_message_like_event) => {
            messagelike_to_content(any_sync_message_like_event).await
        }
        AnySyncTimelineEvent::State(state_event) => {
            Ok(TimelineItemContent::OtherState(OtherState {
                state_key: state_event.state_key().to_string(),
                content: state_event.content(),
            }))
        }
    }
}
async fn messagelike_to_content(
    msg_like: &AnySyncMessageLikeEvent,
) -> eyre::Result<TimelineItemContent> {
    let content = match msg_like {
        AnySyncMessageLikeEvent::RoomMessage(room_message_event) => match room_message_event {
            ruma::events::SyncMessageLikeEvent::Original(original_sync_message_like_event) => {
                let msgtype = original_sync_message_like_event.content.msgtype.clone();
                let message = Message::from_event(
                    msgtype,
                    original_sync_message_like_event
                        .unsigned
                        .relations
                        .replace
                        .as_ref()
                        .and_then(|r| match &r.content.relates_to {
                            Some(Relation::Replacement(r)) => Some(r.new_content.clone()),
                            _ => None,
                        }),
                );

                TimelineItemContent::MsgLike(MsgLikeContent {
                    kind: MsgLikeKind::Message(message),
                    reactions: ReactionsByKeyBySender::default(),
                    in_reply_to: None,
                    thread_root: None,
                })
            }
            ruma::events::SyncMessageLikeEvent::Redacted(_) => {
                TimelineItemContent::MsgLike(MsgLikeContent {
                    kind: MsgLikeKind::Redacted,
                    reactions: ReactionsByKeyBySender::default(),
                    in_reply_to: None,
                    thread_root: None,
                })
            }
        },
        AnySyncMessageLikeEvent::Reaction(_) | AnySyncMessageLikeEvent::RoomRedaction(_) => {
            let reactions = ReactionsByKeyBySender::default();
            TimelineItemContent::MsgLike(MsgLikeContent {
                kind: MsgLikeKind::Hidden,
                reactions,
                in_reply_to: None,
                thread_root: None,
            })
        }
        _ => Err(eyre::eyre!(
            "Unsupported message-like event type {msg_like:?}"
        ))?,
    };
    Ok(content)
}

#[derive(Clone, Debug)]
pub struct TimelineEvent {
    /// The ID of the event.
    pub event_id: Option<OwnedEventId>,
    /// The sender of the event.
    pub sender: OwnedUserId,
    /// The sender's profile of the event.
    pub sender_profile: Option<Profile>,
    /// The timestamp of the event.
    pub timestamp: MilliSecondsSinceUnixEpoch,
    /// The content of the event.
    pub content: TimelineItemContent,
    /// Whether or not the event belongs to an encrypted room.
    ///
    /// May be false when we don't know about the room encryption status yet.
    pub is_room_encrypted: bool,

    /// The JSON serialization of the event.
    pub raw: Box<RawValue>,
}

/// The display name and avatar URL of a room member.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Profile {
    pub display_name: Option<String>,

    /// True if the display name is not unique in the room.
    pub display_name_ambiguous: bool,

    pub avatar_url: Option<OwnedMxcUri>,
}

/// The content of an [`EventTimelineItem`][super::EventTimelineItem].
#[derive(Clone, Debug)]
pub enum TimelineItemContent {
    MsgLike(MsgLikeContent),

    /// A room membership change.
    // MembershipChange(RoomMembershipChange),

    /// A room member profile change.
    // ProfileChange(MemberProfileChange),

    /// Another state event.
    OtherState(OtherState),

    /// A message-like event that failed to deserialize.
    FailedToParseMessageLike {
        /// The deserialization error.
        error: Arc<serde_json::Error>,
    },

    /// A state event that failed to deserialize.
    FailedToParseState {
        /// The event `type`.
        event_type: StateEventType,

        /// The state key.
        state_key: String,

        /// The deserialization error.
        error: Arc<serde_json::Error>,
    },
}

/// A state event that doesn't have its own variant.
#[derive(Clone, Debug)]
pub struct OtherState {
    pub state_key: String,
    pub content: AnyFullStateEventContent,
}

/// A special kind of [`super::TimelineItemContent`] that groups together
/// different room message types with their respective reactions and thread
/// information.
#[derive(Clone, Debug)]
pub struct MsgLikeContent {
    pub kind: MsgLikeKind,
    pub reactions: ReactionsByKeyBySender,
    /// The event this message is replying to, if any.
    pub in_reply_to: Option<InReplyToDetails>,
    /// Event ID of the thread root, if this is a message in a thread.
    pub thread_root: Option<OwnedEventId>,
}
/// Details about an event being replied to.
#[derive(Clone, Debug)]
pub struct InReplyToDetails {
    /// The ID of the event.
    pub event_id: OwnedEventId,

    /// The details of the event.
    /// Fetch if not there
    pub event: Option<Box<RepliedToEvent>>,
}

#[derive(Clone, Debug)]
pub struct RepliedToEvent {
    content: TimelineItemContent,
    sender: OwnedUserId,
    sender_profile: Option<Profile>,
}

#[derive(Clone, Debug)]
pub enum MsgLikeKind {
    /// An `m.room.message` event or extensible event, including edits.
    Message(Message),
    Hidden,

    Redacted,

    UnableToDecrypt,
}
#[derive(Clone, Debug)]
pub struct Message {
    pub msgtype: MessageType,
    pub edited: bool,
}

impl Message {
    pub fn from_event(
        mut msgtype: MessageType,
        edit: Option<RoomMessageEventContentWithoutRelation>,
    ) -> Self {
        msgtype.sanitize(
            ruma::html::HtmlSanitizerMode::Compat,
            RemoveReplyFallback::Yes,
        );
        let mut msg = Self {
            msgtype,
            edited: false,
        };
        if let Some(edit) = edit {
            msg.apply_edit(edit);
        }
        msg
    }
    pub fn apply_edit(&mut self, mut new_content: RoomMessageEventContentWithoutRelation) {
        self.edited = true;
        new_content.msgtype.sanitize(
            ruma::html::HtmlSanitizerMode::Compat,
            RemoveReplyFallback::No,
        );
        self.msgtype = new_content.msgtype;
    }
}

// reaction -> sender -> details
#[derive(Debug, Clone, Default)]
pub struct ReactionsByKeyBySender(pub BTreeMap<String, BTreeMap<OwnedUserId, ReactionInfo>>);

/// Information about a single reaction stored in [`ReactionsByKeyBySender`].
#[derive(Clone, Debug)]
pub struct ReactionInfo {
    pub timestamp: MilliSecondsSinceUnixEpoch,
}
