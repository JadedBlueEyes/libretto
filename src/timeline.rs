use std::{collections::BTreeMap, sync::Arc};

use color_eyre::eyre;
use matrix_sdk::ruma::{
    MilliSecondsSinceUnixEpoch, OwnedEventId, OwnedMxcUri, OwnedUserId,
    events::{
        StateEventType,
        room::message::{MessageType, Relation, RoomMessageEventContentWithoutRelation},
    },
    html::RemoveReplyFallback,
};
use ruma::events::{AnyMessageLikeEventContent, AnyStateEventContent, EventContentFromType};
use serde_json::value::RawValue;
use tracing::{error, warn};

#[derive(Clone, Debug)]
pub struct TimelineEvent {
    /// The ID of the event.
    pub event_id: OwnedEventId,
    /// The sender of the event.
    pub sender: OwnedUserId,
    /// The sender's profile of the event.
    pub sender_profile: Option<Profile>,
    /// The timestamp of the event.
    pub timestamp: MilliSecondsSinceUnixEpoch,
    /// The content of the event.
    pub content: TimelineItemContent,

    /// The JSON serialization of the event.
    pub raw_content: Box<RawValue>,
}

impl TimelineEvent {
    pub(crate) fn is_hidden_event(&self) -> bool {
        matches!(
            self.content,
            TimelineItemContent::MsgLike(MsgLikeContent {
                kind: MsgLikeKind::Hidden,
                ..
            })
        )
    }
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
    OtherState(Box<OtherState>),

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
    pub content: AnyStateEventContent,
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

#[derive(Clone, Debug, PartialEq)]
pub enum MsgLikeKind {
    /// An `m.room.message` event or extensible event, including edits.
    Message(Message),
    Hidden,

    Redacted,

    UnableToDecrypt,

    UnsupportedType,
}
#[derive(Clone, Debug)]
pub struct Message {
    pub msgtype: MessageType,
    pub edited: bool,
}

impl PartialEq for Message {
    fn eq(&self, other: &Self) -> bool {
        self.msgtype.msgtype() == other.msgtype.msgtype()
            && self.edited == other.edited
            && self.msgtype.body() == other.msgtype.body()
    }
}

impl Message {
    pub fn from_event(
        mut msgtype: MessageType,
        edit: Option<RoomMessageEventContentWithoutRelation>,
    ) -> Self {
        msgtype.sanitize(
            matrix_sdk::ruma::html::HtmlSanitizerMode::Compat,
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

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct DbTimelineEvent {
    pub rowid: i32,
    pub timeline_rowid: i32,
    pub room_id: String,
    /// The ID of the event.
    pub event_id: String,
    /// The sender of the event.
    pub sender: String,
    /// The timestamp of the event.
    pub timestamp: i64,

    pub event_type: String,

    pub state_key: Option<String>,

    pub relation_type: Option<String>,
    pub relates_to: Option<String>,

    /// The JSON serialization of the content of the event.
    #[sqlx(rename = "content")]
    pub raw_content: sqlx::types::Json<Box<serde_json::value::RawValue>>,
    #[sqlx(rename = "unsigned")]
    pub raw_unsigned: sqlx::types::Json<Box<serde_json::value::RawValue>>,

    pub redacted_by: Option<String>,
    pub last_edit_rowid: Option<i32>,

    pub megolm_session_id: Option<String>,

    pub transaction_id: Option<String>,
}

/// Build a TimelineEvent from a database row
pub async fn build_timeline_event_from_db(evt: DbTimelineEvent) -> eyre::Result<TimelineEvent> {
    // Extract data from the row
    let event_id: OwnedEventId = evt.event_id.try_into()?;
    let sender: OwnedUserId = evt.sender.try_into()?;
    let timestamp = MilliSecondsSinceUnixEpoch(evt.timestamp.try_into()?);
    // (*evt.content).des

    if let Some(state_key) = evt.state_key {
        // let state_raw: Raw::<AnyStateEventContent> = Raw::from_json(evt.raw_content.0);

        // In place of deserialize_with_type, use the EventContentFromType trait
        let state_content =
            AnyStateEventContent::from_parts(evt.event_type.as_str(), &evt.raw_content.0);

        // TODO: handle redacted state events

        // dbg!(&state_content);
        Ok(TimelineEvent {
            event_id,
            sender,
            sender_profile: None, // We don't have profile info in the database
            timestamp,
            content: match state_content {
                Ok(content) => {
                    TimelineItemContent::OtherState(Box::new(OtherState { state_key, content }))
                }
                Err(error) => {
                    error!("Failed to parse message content: {}", error);
                    TimelineItemContent::FailedToParseState {
                        event_type: StateEventType::from(evt.event_type),
                        state_key,
                        error: error.into(),
                    }
                }
            },
            raw_content: evt.raw_content.0,
        })
    } else {
        // let message_raw: Raw::<AnyMessageLikeEventContent> = Raw::from_json(evt.raw_content.0);
        if evt.redacted_by.is_some() {
            return Ok(TimelineEvent {
                event_id,
                sender,
                sender_profile: None, // We don't have profile info in the database
                timestamp,
                content: TimelineItemContent::MsgLike(MsgLikeContent {
                    kind: MsgLikeKind::Redacted,
                    reactions: ReactionsByKeyBySender::default(),
                    in_reply_to: None,
                    thread_root: None,
                }),
                raw_content: evt.raw_content.0,
            });
        }
        let message_content =
            AnyMessageLikeEventContent::from_parts(evt.event_type.as_str(), &evt.raw_content.0);
        // dbg!(&message_content);

        Ok(TimelineEvent {
            event_id,
            sender,
            sender_profile: None, // We don't have profile info in the database
            timestamp,
            content: match message_content {
                Ok(content) => messagelike_to_content(content).await?,
                Err(error) => {
                    error!("Failed to parse message content: {}", error);
                    TimelineItemContent::FailedToParseMessageLike {
                        error: error.into(),
                    }
                }
            },
            raw_content: evt.raw_content.0,
        })
    }
}

async fn messagelike_to_content(
    msg_like: AnyMessageLikeEventContent,
) -> eyre::Result<TimelineItemContent> {
    let content = match msg_like {
        AnyMessageLikeEventContent::RoomMessage(room_message_event_content) => {
            let message = Message::from_event(
                room_message_event_content.msgtype.clone(),
                room_message_event_content
                    .relates_to
                    .as_ref()
                    .and_then(|rel| match rel {
                        Relation::Replacement(r) => Some(r.new_content.clone()),
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
        AnyMessageLikeEventContent::RoomRedaction(_) | AnyMessageLikeEventContent::Reaction(_) => {
            TimelineItemContent::MsgLike(MsgLikeContent {
                kind: MsgLikeKind::Hidden,
                reactions: ReactionsByKeyBySender::default(),
                in_reply_to: None,
                thread_root: None,
            })
        }
        AnyMessageLikeEventContent::RoomEncrypted(_) => {
            TimelineItemContent::MsgLike(MsgLikeContent {
                kind: MsgLikeKind::UnableToDecrypt,
                reactions: ReactionsByKeyBySender::default(),
                in_reply_to: None,
                thread_root: None,
            })
        }
        _ => {
            warn!("Unsupported message like event type: {:?}", msg_like);
            TimelineItemContent::MsgLike(MsgLikeContent {
                kind: MsgLikeKind::UnsupportedType,
                reactions: ReactionsByKeyBySender::default(),
                in_reply_to: None,
                thread_root: None,
            })
        }
    };
    Ok(content)
}
