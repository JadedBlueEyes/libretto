use std::{collections::BTreeMap, sync::Arc};

use color_eyre::eyre;
use futures::prelude::*;
use ruma::{
    MilliSecondsSinceUnixEpoch, OwnedEventId, OwnedMxcUri, OwnedUserId,
    events::{
        AnyFullStateEventContent, AnySyncMessageLikeEvent, AnySyncTimelineEvent, StateEventType,
        room::message::{MessageType, Relation, RoomMessageEventContentWithoutRelation},
    },
    html::RemoveReplyFallback,
};
use serde_json::value::RawValue;


pub async fn build_timeline_item(
    event: matrix_sdk::deserialized_responses::TimelineEvent,
) -> eyre::Result<TimelineItem> {
    let event_id = event.event_id().to_owned();

    match event.kind {
        matrix_sdk::deserialized_responses::TimelineEventKind::PlainText { event: raw_event } => {
            match raw_event.deserialize() {
                Ok(AnySyncTimelineEvent::MessageLike(msg_like)) => Ok(TimelineItem {
                    event_id,
                    content: messagelike_to_content(&msg_like).await?,
                    raw: raw_event.into_json(),
                }),
                Ok(AnySyncTimelineEvent::State(state_event)) => Ok(TimelineItem {
                    event_id,
                    content: TimelineItemContent::OtherState(OtherState {
                        state_key: state_event.state_key().to_string(),
                        content: state_event.content(),
                    }),
                    raw: raw_event.into_json(),
                }),
                Err(e) => Ok(TimelineItem {
                    event_id,
                    content: TimelineItemContent::FailedToParseMessageLike { error: Arc::new(e) },
                    raw: raw_event.into_json(),
                }),
            }
        }
        matrix_sdk::deserialized_responses::TimelineEventKind::Decrypted(decrypted_room_event) => {
            todo!()
        }
        matrix_sdk::deserialized_responses::TimelineEventKind::UnableToDecrypt {
            event,
            utd_info,
        } => todo!(),
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
pub struct TimelineItem {
    /// The ID of the event.
    pub event_id: Option<OwnedEventId>,

    /// The content of the event.
    pub content: TimelineItemContent,

    /// The JSON serialization of the event.
    pub raw: Box<RawValue>,
}

#[derive(Clone, Debug)]
pub struct TimelineEvent {
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
