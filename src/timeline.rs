use std::{collections::BTreeMap, sync::Arc};

use color_eyre::eyre;
use matrix_sdk::ruma::{
    MilliSecondsSinceUnixEpoch, OwnedEventId, OwnedMxcUri, OwnedUserId,
    events::{
        StateEventType,
        room::message::{MessageType, Relation, RoomMessageEventContentWithoutRelation},
        sticker::StickerEventContent,
    },
    html::RemoveReplyFallback,
};
use ruma::events::{AnyMessageLikeEventContent, AnyStateEvent, EventContentFromType};
use serde::Deserialize;
use serde_json::{json, value::RawValue};
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
            TimelineItemContent::MsgLike(ref boxed_content) if matches!(
                boxed_content.kind,
                MsgLikeKind::Hidden
            )
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
    MsgLike(Box<MsgLikeContent>),

    /// A state event.
    StateEvent(Box<AnyStateEvent>),

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

    /// A sticker message
    Sticker(Box<StickerEventContent>),

    /// A reaction to another message
    Reaction(String, OwnedEventId), // (emoji, event_id)

    /// Call events
    CallInvite(String), // call_id
    CallAnswer(String),          // call_id
    CallHangup(String, String),  // (call_id, reason)
    CallReject(String),          // call_id
    CallNegotiate(String),       // call_id
    CallCandidates(String),      // call_id
    CallNotify(String),          // call_id
    CallSelectAnswer(String),    // call_id
    CallMetadataChanged(String), // call_id

    /// Poll events
    PollStart(String, Vec<(String, String)>), // (question, answers: Vec<(id, text)>)
    PollResponse(Vec<String>, OwnedEventId), // (answers, poll_event_id)
    PollEnd(String),                         // result summary

    /// Key verification events
    KeyVerificationReady(String), // from_device
    KeyVerificationStart(String),          // from_device
    KeyVerificationCancel(String, String), // (reason, code)
    KeyVerificationAccept,
    KeyVerificationKey,
    KeyVerificationMac,
    KeyVerificationDone,

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
        let state_event = json!({
            "event_id": event_id,
            "room_id": evt.room_id,
            "type": evt.event_type,
            "content": &evt.raw_content,
            "sender": sender,
            "origin_server_ts": timestamp,
            "state_key": state_key,
            "unsigned": &evt.raw_unsigned
        });
        let state_event_de = AnyStateEvent::deserialize(state_event);
        let timeline_content = match state_event_de {
            Ok(state_event) => TimelineItemContent::StateEvent(Box::new(state_event)),
            Err(err) => {
                error!(
                    event_type = %evt.event_type,
                    state_key = %state_key,
                    error = %err,
                    "Failed to parse state event content"
                );
                TimelineItemContent::FailedToParseState {
                    event_type: StateEventType::from(evt.event_type),
                    state_key,
                    error: err.into(),
                }
            }
        };

        // TODO: handle redacted state events

        // dbg!(&state_content);
        Ok(TimelineEvent {
            event_id,
            sender,
            sender_profile: None, // We don't have profile info in the database
            timestamp,
            content: timeline_content,
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
                content: TimelineItemContent::MsgLike(Box::new(MsgLikeContent {
                    kind: MsgLikeKind::Redacted,
                    reactions: ReactionsByKeyBySender::default(),
                    in_reply_to: None,
                    thread_root: None,
                })),
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
                    error!(
                        event_type = %evt.event_type,
                        error = %error,
                        "Failed to parse message-like event content"
                    );
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
            TimelineItemContent::MsgLike(Box::new(MsgLikeContent {
                kind: MsgLikeKind::Message(message),
                reactions: ReactionsByKeyBySender::default(),
                in_reply_to: None,
                thread_root: None,
            }))
        }
        AnyMessageLikeEventContent::Sticker(sticker_content) => {
            let (in_reply_to, thread_root) = match &sticker_content.relates_to {
                Some(Relation::Reply { in_reply_to }) => (
                    Some(InReplyToDetails {
                        event_id: in_reply_to.event_id.clone(),
                        event: None,
                    }),
                    None,
                ),
                Some(Relation::Thread(thread)) => (
                    thread.in_reply_to.as_ref().map(|reply| InReplyToDetails {
                        event_id: reply.event_id.clone(),
                        event: None,
                    }),
                    Some(thread.event_id.clone()),
                ),
                _ => (None, None),
            };

            TimelineItemContent::MsgLike(Box::new(MsgLikeContent {
                kind: MsgLikeKind::Sticker(Box::new(sticker_content)),
                reactions: ReactionsByKeyBySender::default(),
                in_reply_to,
                thread_root,
            }))
        }
        AnyMessageLikeEventContent::Reaction(reaction_content) => {
            TimelineItemContent::MsgLike(Box::new(MsgLikeContent {
                kind: MsgLikeKind::Reaction(
                    reaction_content.relates_to.key.clone(),
                    reaction_content.relates_to.event_id.clone(),
                ),
                reactions: ReactionsByKeyBySender::default(),
                in_reply_to: None,
                thread_root: None,
            }))
        }
        AnyMessageLikeEventContent::CallInvite(call_invite_content) => {
            TimelineItemContent::MsgLike(Box::new(MsgLikeContent {
                kind: MsgLikeKind::CallInvite(call_invite_content.call_id.to_string()),
                reactions: ReactionsByKeyBySender::default(),
                in_reply_to: None,
                thread_root: None,
            }))
        }
        AnyMessageLikeEventContent::CallAnswer(call_answer_content) => {
            TimelineItemContent::MsgLike(Box::new(MsgLikeContent {
                kind: MsgLikeKind::CallAnswer(call_answer_content.call_id.to_string()),
                reactions: ReactionsByKeyBySender::default(),
                in_reply_to: None,
                thread_root: None,
            }))
        }
        AnyMessageLikeEventContent::CallHangup(call_hangup_content) => {
            TimelineItemContent::MsgLike(Box::new(MsgLikeContent {
                kind: MsgLikeKind::CallHangup(
                    call_hangup_content.call_id.to_string(),
                    format!("{:?}", call_hangup_content.reason),
                ),
                reactions: ReactionsByKeyBySender::default(),
                in_reply_to: None,
                thread_root: None,
            }))
        }
        AnyMessageLikeEventContent::CallReject(call_reject_content) => {
            TimelineItemContent::MsgLike(Box::new(MsgLikeContent {
                kind: MsgLikeKind::CallReject(call_reject_content.call_id.to_string()),
                reactions: ReactionsByKeyBySender::default(),
                in_reply_to: None,
                thread_root: None,
            }))
        }
        AnyMessageLikeEventContent::CallNegotiate(call_negotiate_content) => {
            TimelineItemContent::MsgLike(Box::new(MsgLikeContent {
                kind: MsgLikeKind::CallNegotiate(call_negotiate_content.call_id.to_string()),
                reactions: ReactionsByKeyBySender::default(),
                in_reply_to: None,
                thread_root: None,
            }))
        }
        AnyMessageLikeEventContent::CallCandidates(call_candidates_content) => {
            TimelineItemContent::MsgLike(Box::new(MsgLikeContent {
                kind: MsgLikeKind::CallCandidates(call_candidates_content.call_id.to_string()),
                reactions: ReactionsByKeyBySender::default(),
                in_reply_to: None,
                thread_root: None,
            }))
        }
        AnyMessageLikeEventContent::CallSelectAnswer(call_select_answer_content) => {
            TimelineItemContent::MsgLike(Box::new(MsgLikeContent {
                kind: MsgLikeKind::CallSelectAnswer(call_select_answer_content.call_id.to_string()),
                reactions: ReactionsByKeyBySender::default(),
                in_reply_to: None,
                thread_root: None,
            }))
        }
        AnyMessageLikeEventContent::CallSdpStreamMetadataChanged(call_sdp_content) => {
            TimelineItemContent::MsgLike(Box::new(MsgLikeContent {
                kind: MsgLikeKind::CallMetadataChanged(call_sdp_content.call_id.to_string()),
                reactions: ReactionsByKeyBySender::default(),
                in_reply_to: None,
                thread_root: None,
            }))
        }
        AnyMessageLikeEventContent::CallNotify(call_notify_content) => {
            TimelineItemContent::MsgLike(Box::new(MsgLikeContent {
                kind: MsgLikeKind::CallNotify(call_notify_content.call_id.to_string()),
                reactions: ReactionsByKeyBySender::default(),
                in_reply_to: None,
                thread_root: None,
            }))
        }
        AnyMessageLikeEventContent::UnstablePollStart(poll_start_content) => {
            let (question, answers) = match poll_start_content {
                ruma::events::poll::unstable_start::UnstablePollStartEventContent::New(
                    new_poll,
                ) => {
                    let question = new_poll.poll_start.question.text.clone();
                    let answers = new_poll
                        .poll_start
                        .answers
                        .iter()
                        .map(|answer| (answer.id.clone(), answer.text.clone()))
                        .collect();
                    (question, answers)
                }
                ruma::events::poll::unstable_start::UnstablePollStartEventContent::Replacement(
                    replacement_poll,
                ) => {
                    let question = replacement_poll
                        .poll_start
                        .as_ref()
                        .map(|ps| ps.question.text.clone())
                        .unwrap_or_else(|| "Poll question unavailable".to_string());
                    let answers = replacement_poll
                        .poll_start
                        .as_ref()
                        .map(|ps| {
                            ps.answers
                                .iter()
                                .map(|answer| (answer.id.clone(), answer.text.clone()))
                                .collect()
                        })
                        .unwrap_or_else(Vec::new);
                    (question, answers)
                }
                _ => ("Unknown poll type".to_string(), Vec::new()),
            };
            TimelineItemContent::MsgLike(Box::new(MsgLikeContent {
                kind: MsgLikeKind::PollStart(question, answers),
                reactions: ReactionsByKeyBySender::default(),
                in_reply_to: None,
                thread_root: None,
            }))
        }
        AnyMessageLikeEventContent::UnstablePollResponse(poll_response_content) => {
            let answers = poll_response_content.poll_response.answers.clone();
            let poll_event_id = poll_response_content.relates_to.event_id.clone();
            TimelineItemContent::MsgLike(Box::new(MsgLikeContent {
                kind: MsgLikeKind::PollResponse(answers, poll_event_id),
                reactions: ReactionsByKeyBySender::default(),
                in_reply_to: None,
                thread_root: None,
            }))
        }
        AnyMessageLikeEventContent::UnstablePollEnd(poll_end_content) => {
            TimelineItemContent::MsgLike(Box::new(MsgLikeContent {
                kind: MsgLikeKind::PollEnd(poll_end_content.text.clone()),
                reactions: ReactionsByKeyBySender::default(),
                in_reply_to: None,
                thread_root: None,
            }))
        }
        AnyMessageLikeEventContent::KeyVerificationReady(verification_ready_content) => {
            TimelineItemContent::MsgLike(Box::new(MsgLikeContent {
                kind: MsgLikeKind::KeyVerificationReady(
                    verification_ready_content.from_device.to_string(),
                ),
                reactions: ReactionsByKeyBySender::default(),
                in_reply_to: None,
                thread_root: None,
            }))
        }
        AnyMessageLikeEventContent::KeyVerificationStart(verification_start_content) => {
            TimelineItemContent::MsgLike(Box::new(MsgLikeContent {
                kind: MsgLikeKind::KeyVerificationStart(
                    verification_start_content.from_device.to_string(),
                ),
                reactions: ReactionsByKeyBySender::default(),
                in_reply_to: None,
                thread_root: None,
            }))
        }
        AnyMessageLikeEventContent::KeyVerificationCancel(verification_cancel_content) => {
            TimelineItemContent::MsgLike(Box::new(MsgLikeContent {
                kind: MsgLikeKind::KeyVerificationCancel(
                    verification_cancel_content.reason.clone(),
                    format!("{:?}", verification_cancel_content.code),
                ),
                reactions: ReactionsByKeyBySender::default(),
                in_reply_to: None,
                thread_root: None,
            }))
        }
        AnyMessageLikeEventContent::KeyVerificationAccept(_) => {
            TimelineItemContent::MsgLike(Box::new(MsgLikeContent {
                kind: MsgLikeKind::KeyVerificationAccept,
                reactions: ReactionsByKeyBySender::default(),
                in_reply_to: None,
                thread_root: None,
            }))
        }
        AnyMessageLikeEventContent::KeyVerificationKey(_) => {
            TimelineItemContent::MsgLike(Box::new(MsgLikeContent {
                kind: MsgLikeKind::KeyVerificationKey,
                reactions: ReactionsByKeyBySender::default(),
                in_reply_to: None,
                thread_root: None,
            }))
        }
        AnyMessageLikeEventContent::KeyVerificationMac(_) => {
            TimelineItemContent::MsgLike(Box::new(MsgLikeContent {
                kind: MsgLikeKind::KeyVerificationMac,
                reactions: ReactionsByKeyBySender::default(),
                in_reply_to: None,
                thread_root: None,
            }))
        }
        AnyMessageLikeEventContent::KeyVerificationDone(_) => {
            TimelineItemContent::MsgLike(Box::new(MsgLikeContent {
                kind: MsgLikeKind::KeyVerificationDone,
                reactions: ReactionsByKeyBySender::default(),
                in_reply_to: None,
                thread_root: None,
            }))
        }
        AnyMessageLikeEventContent::RoomRedaction(_) => {
            TimelineItemContent::MsgLike(Box::new(MsgLikeContent {
                kind: MsgLikeKind::Hidden,
                reactions: ReactionsByKeyBySender::default(),
                in_reply_to: None,
                thread_root: None,
            }))
        }
        AnyMessageLikeEventContent::RoomEncrypted(_) => {
            TimelineItemContent::MsgLike(Box::new(MsgLikeContent {
                kind: MsgLikeKind::UnableToDecrypt,
                reactions: ReactionsByKeyBySender::default(),
                in_reply_to: None,
                thread_root: None,
            }))
        }
        _ => {
            warn!("Unsupported message like event type: {:?}", msg_like);
            TimelineItemContent::MsgLike(Box::new(MsgLikeContent {
                kind: MsgLikeKind::UnsupportedType,
                reactions: ReactionsByKeyBySender::default(),
                in_reply_to: None,
                thread_root: None,
            }))
        }
    };
    Ok(content)
}
