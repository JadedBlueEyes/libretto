use icu::calendar::{AnyCalendar, AnyCalendarKind, Iso};
use icu::datetime::fieldsets;
use icu::datetime::input::{Date as IcuDate, DateTime as IcuDateTime, Time as IcuTime};
use icu::datetime::scaffold::ConvertCalendar;
use icu::{datetime::DateTimeFormatter, locale::locale};
use jiff::Timestamp;
use matrix_sdk::ruma::MilliSecondsSinceUnixEpoch;
use matrix_sdk::ruma::events::room::MediaSource;
use matrix_sdk::ruma::events::room::message::{FormattedBody, MessageType};
use matrix_sdk::ruma::events::sticker::StickerMediaSource;
use ruma::events::OriginalStateEvent;
use ruma::events::room::member::{MembershipChange, RoomMemberEventContent};
use ruma::events::{AnyStateEvent, StateEvent};

use crate::timeline::{MsgLikeKind, TimelineEvent, TimelineItemContent};

#[derive(askama::Template)]
#[template(path = "room_list.html.j2")]
pub struct RoomListTemplate {
    pub rooms: Vec<crate::room_list::RoomListEntry>,
}

#[derive(askama::Template)]
#[template(path = "room.html.j2")]
pub struct RoomTemplate<'a> {
    pub room_id: &'a matrix_sdk::ruma::RoomId,
    pub name: String,
    pub events: Vec<TimelineEvent>,
    pub hit_end_of_timeline: bool,
    pub prev_page: Option<String>,
    pub next_page: Option<String>,
    pub room: &'a matrix_sdk::room::Room,
}

fn html_body(formatted_body: &FormattedBody) -> Option<&str> {
    if formatted_body.format == ruma::events::room::message::MessageFormat::Html {
        Some(&formatted_body.body)
    } else {
        None
    }
}
pub(crate) fn message_formatted_body(message: &MessageType) -> Option<&FormattedBody> {
    match message {
        MessageType::Audio(audio_message_event_content) => {
            audio_message_event_content.formatted_caption()
        }
        MessageType::Emote(emote_message_event_content) => {
            emote_message_event_content.formatted.as_ref()
        }
        MessageType::File(file_message_event_content) => {
            file_message_event_content.formatted_caption()
        }
        MessageType::Image(image_message_event_content) => {
            image_message_event_content.formatted_caption()
        }
        MessageType::Location(_location_message_event_content) => None,
        MessageType::Notice(notice_message_event_content) => {
            notice_message_event_content.formatted.as_ref()
        }
        MessageType::ServerNotice(_server_notice_message_event_content) => None,
        MessageType::Text(text_message_event_content) => {
            text_message_event_content.formatted.as_ref()
        }
        MessageType::Video(video_message_event_content) => {
            video_message_event_content.formatted_caption()
        }
        MessageType::VerificationRequest(_key_verification_request_event_content) => None,
        _ => None,
    }
}

pub(crate) fn timestamp_to_string(ts: &MilliSecondsSinceUnixEpoch) -> String {
    milliseconds_since_unix_epoch_to_string(ts.0.into())
}
pub(crate) fn timestamp_to_format_string(ts: &MilliSecondsSinceUnixEpoch) -> String {
    milliseconds_since_unix_epoch_to_format_string(ts.0.into())
}

pub(crate) fn milliseconds_since_unix_epoch_to_string(milliseconds: i64) -> String {
    Timestamp::from_millisecond(milliseconds)
        .map_or_else(|_| "Unknown Time".to_string(), |ts| ts.to_string())
}

pub(crate) fn milliseconds_since_unix_epoch_to_format_string(milliseconds: i64) -> String {
    let field_set_with_options = fieldsets::YMD::long().with_time_hm();
    let locale = locale!("en-GB");
    let calendar = AnyCalendar::new(AnyCalendarKind::new(locale.clone().into()));

    let formatter: DateTimeFormatter<_> =
        DateTimeFormatter::try_new(locale.into(), field_set_with_options)
            .expect("Failed to create DateTimeFormatter");
    Timestamp::from_millisecond(milliseconds).map_or_else(
        |_| "Unknown Time".to_string(),
        |ts| {
            formatter
                .format(
                    &convert_from_datetime(
                        ts.in_tz("UTC")
                            .expect("Failed to convert to UTC")
                            .datetime(),
                    )
                    .to_calendar(&calendar),
                )
                .to_string()
        },
    )
}

fn convert_from_datetime(v: jiff::civil::DateTime) -> IcuDateTime<Iso> {
    let date: IcuDate<Iso> = convert_from_date(v.date());
    let time: IcuTime = convert_from_time(v.time());
    IcuDateTime { date, time }
}

fn convert_from_date(v: jiff::civil::Date) -> IcuDate<Iso> {
    let year = i32::from(v.year());
    let month = v.month().unsigned_abs();
    let day = v.day().unsigned_abs();
    // All Jiff civil dates are valid ICU4X dates.
    IcuDate::try_new_iso(year, month, day).expect("Failed to create IcuDate")
}
fn convert_from_time(v: jiff::civil::Time) -> IcuTime {
    let hour = v.hour().unsigned_abs();
    let minute = v.minute().unsigned_abs();
    let second = v.second().unsigned_abs();
    let subsec = v.subsec_nanosecond().unsigned_abs();
    // All Jiff civil times are valid ICU4X times.
    IcuTime::try_new(hour, minute, second, subsec).expect("Failed to create IcuTime")
}

pub(crate) fn render_member_event(event: &OriginalStateEvent<RoomMemberEventContent>) -> String {
    membership_change_description(
        &event.membership_change(),
        event.state_key.as_str(),
        event.sender.as_str(),
    )
}
pub(crate) fn membership_change_description(
    new_membership: &MembershipChange,
    state_key: &str,
    sender: &str,
) -> String {
    let target_user = state_key;
    let acting_user = sender;

    // Check if user is acting on themselves
    let is_self_action = target_user == acting_user;
    match new_membership {
        MembershipChange::None => format!("{acting_user} made no change"),
        MembershipChange::Error => "An error occurred during membership change".to_string(),
        MembershipChange::Joined => {
            if is_self_action {
                format!("{target_user} joined the room")
            } else {
                format!("{acting_user} added {target_user} to the room")
            }
        }
        MembershipChange::Left => {
            if is_self_action {
                format!("{target_user} left the room")
            } else {
                format!("{acting_user} removed {target_user} from the room")
            }
        }
        MembershipChange::Banned => {
            format!("{acting_user} banned {target_user}")
        }
        MembershipChange::Unbanned => {
            format!("{acting_user} unbanned {target_user}")
        }
        MembershipChange::Kicked => {
            format!("{acting_user} kicked {target_user}")
        }
        MembershipChange::Invited => {
            format!("{acting_user} invited {target_user}")
        }
        MembershipChange::KickedAndBanned => {
            format!("{acting_user} kicked and banned {target_user}")
        }
        MembershipChange::InvitationAccepted => {
            format!("{target_user} accepted the invitation and joined the room")
        }
        MembershipChange::InvitationRejected => {
            format!("{target_user} declined the invitation")
        }
        MembershipChange::InvitationRevoked => {
            format!("{acting_user} revoked the invitation for {target_user}")
        }
        MembershipChange::Knocked => {
            format!("{target_user} requested to join the room")
        }
        MembershipChange::KnockAccepted => {
            format!("{acting_user} accepted {target_user}'s request to join")
        }
        MembershipChange::KnockRetracted => {
            format!("{target_user} withdrew their request to join")
        }
        MembershipChange::KnockDenied => {
            format!("{acting_user} denied {target_user}'s request to join")
        }
        MembershipChange::ProfileChanged {
            displayname_change: _,
            avatar_url_change: _,
        } => {
            format!("{target_user} updated their profile")
        }
        MembershipChange::NotImplemented => "Membership change not implemented".to_string(),
        _ => "Unknown membership change".to_string(),
    }
}

/// Generate initials from a display name or user ID
pub(crate) fn get_initials(name: &str) -> String {
    if name.is_empty() {
        return "?".to_string();
    }

    // Remove @ symbol if present (for Matrix user IDs)
    let clean_name = name.strip_prefix('@').unwrap_or(name);

    // For Matrix user IDs, split by colon first to get the local part
    let local_part = if clean_name.contains(':') {
        clean_name.split(':').next().unwrap_or(clean_name)
    } else {
        clean_name
    };

    // Split by spaces, dots, underscores, or hyphens
    let words: Vec<&str> = local_part
        .split(&[' ', '.', '_', '-'][..])
        .filter(|word| !word.is_empty())
        .collect();

    if words.is_empty() {
        return "?".to_string();
    }

    if words.len() == 1 {
        // Single word: take first two characters
        words[0].chars().take(2).collect::<String>().to_uppercase()
    } else {
        // Multiple words: take first character of first two words
        let first = words[0].chars().next().unwrap_or('?');
        let second = words[1].chars().next().unwrap_or('?');
        format!("{first}{second}").to_uppercase()
    }
}

/// Generate a consistent color class based on a string (user ID or room ID)
pub(crate) fn get_avatar_color_class(id: &str) -> String {
    // Simple hash function to get consistent color assignment
    let mut hash: u32 = 0;
    for byte in id.bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(byte as u32);
    }

    // Map to one of 10 color classes
    let color_index = (hash % 10) + 1;
    format!("avatar-color-{color_index}")
}

/// Extract initials from user ID, preferring display name if available
pub(crate) fn user_initials(user_id: &str, display_name: Option<&str>) -> String {
    match display_name {
        Some(name) if !name.trim().is_empty() => get_initials(name),
        _ => get_initials(user_id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_initials() {
        // Test basic cases
        assert_eq!(get_initials("John Doe"), "JD");
        assert_eq!(get_initials("Alice Smith"), "AS");
        assert_eq!(get_initials("alice"), "AL");
        assert_eq!(get_initials("Bob"), "BO");

        // Test Matrix user IDs
        assert_eq!(get_initials("@user:matrix.org"), "US");
        assert_eq!(get_initials("@alice:example.com"), "AL");

        // Test edge cases
        assert_eq!(get_initials(""), "?");
        assert_eq!(get_initials("   "), "?");
        assert_eq!(get_initials("@"), "?");

        // Test with separators
        assert_eq!(get_initials("first.last"), "FL");
        assert_eq!(get_initials("user_name"), "UN");
        assert_eq!(get_initials("foo-bar"), "FB");

        // Test multiple spaces
        assert_eq!(get_initials("John   Doe"), "JD");
        assert_eq!(get_initials("A B C"), "AB");

        // Test single character names
        assert_eq!(get_initials("A"), "A");
        assert_eq!(get_initials("X Y"), "XY");
    }

    #[test]
    fn test_get_avatar_color_class() {
        // Test that same input gives same output
        let class1 = get_avatar_color_class("@user:matrix.org");
        let class2 = get_avatar_color_class("@user:matrix.org");
        assert_eq!(class1, class2);

        // Test that different inputs give potentially different outputs
        let class_a = get_avatar_color_class("@alice:matrix.org");
        let class_b = get_avatar_color_class("@bob:matrix.org");
        // They might be the same due to hash collisions, but that's OK

        // Test format
        assert!(class_a.starts_with("avatar-color-"));
        assert!(class_b.starts_with("avatar-color-"));

        // Test that color numbers are in valid range (1-10)
        for id in &["@test1", "@test2", "@test3", "room1", "room2"] {
            let class = get_avatar_color_class(id);
            assert!(class.starts_with("avatar-color-"));
            let number_part = class.strip_prefix("avatar-color-").unwrap();
            let number: u32 = number_part.parse().unwrap();
            assert!((1..=10).contains(&number));
        }
    }

    #[test]
    fn test_user_initials() {
        // Test with display name
        assert_eq!(user_initials("@user:matrix.org", Some("John Doe")), "JD");
        assert_eq!(
            user_initials("@alice:matrix.org", Some("Alice Smith")),
            "AS"
        );

        // Test without display name
        assert_eq!(user_initials("@user:matrix.org", None), "US");
        assert_eq!(user_initials("@alice:matrix.org", None), "AL");

        // Test with empty display name (should fall back to user ID)
        assert_eq!(user_initials("@user:matrix.org", Some("")), "US");
        assert_eq!(user_initials("@alice:matrix.org", Some("")), "AL");

        // Test with whitespace-only display name
        assert_eq!(user_initials("@user:matrix.org", Some("   ")), "US");
    }
}
