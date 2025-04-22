use std::collections::HashMap;

use icu::{calendar::Gregorian, datetime::TypedDateTimeFormatter, locid::locale};
use jiff::Timestamp;
use matrix_sdk::{
    deserialized_responses::{TimelineEvent, TimelineEventKind},
    ruma::{
        MilliSecondsSinceUnixEpoch,
        events::{AnySyncMessageLikeEvent, AnySyncTimelineEvent, SyncMessageLikeEvent},
    },
};

#[derive(askama::Template)] // this will generate the code...
#[template(path = "room.html.j2")] // using the template in this path, relative
// to the `templates` dir in the crate root
pub struct RoomTemplate<'a> {
    // the name of the struct can be anything
    pub room_id: &'a matrix_sdk::ruma::RoomId,
    pub name: String,
    pub members: HashMap<&'a matrix_sdk::ruma::UserId, &'a matrix_sdk::room::RoomMember>,
    pub events: Vec<TimelineEvent>,
    pub hit_end_of_timeline: bool,
    pub room: &'a matrix_sdk::room::Room,
}

pub(crate) fn timestamp_to_string(ts: MilliSecondsSinceUnixEpoch) -> String {
    milliseconds_since_unix_epoch_to_string(ts.0.into())
}
pub(crate) fn timestamp_to_format_string(ts: MilliSecondsSinceUnixEpoch) -> String {
    milliseconds_since_unix_epoch_to_format_string(ts.0.into())
}

pub(crate) fn milliseconds_since_unix_epoch_to_string(milliseconds: i64) -> String {
    Timestamp::from_millisecond(milliseconds)
        .map_or_else(|e| "Unknown Time".to_string(), |ts| ts.to_string())
}

pub(crate) fn milliseconds_since_unix_epoch_to_format_string(milliseconds: i64) -> String {
    let formatter =
        TypedDateTimeFormatter::try_new(&locale!("en-GB").into(), Default::default()).unwrap();
    Timestamp::from_millisecond(milliseconds).map_or_else(
        |e| "Unknown Time".to_string(),
        |ts| {
            formatter
                .format(
                    &convert_from_datetime(ts.in_tz("UTC").unwrap().datetime())
                        .to_calendar(Gregorian),
                )
                .to_string()
        },
    )
}

use icu::calendar::{Date as IcuDate, DateTime as IcuDateTime, Iso, Time as IcuTime};

fn convert_from_datetime(v: jiff::civil::DateTime) -> IcuDateTime<Iso> {
    let date: IcuDate<Iso> = convert_from_date(v.date());
    let time: IcuTime = convert_from_time(v.time());
    IcuDateTime::new(date, time)
}

fn convert_from_date(v: jiff::civil::Date) -> IcuDate<Iso> {
    let year = i32::from(v.year());
    let month = v.month().unsigned_abs();
    let day = v.day().unsigned_abs();
    // All Jiff civil dates are valid ICU4X dates.
    IcuDate::try_new_iso_date(year, month, day).unwrap()
}
fn convert_from_time(v: jiff::civil::Time) -> IcuTime {
    let hour = v.hour().unsigned_abs();
    let minute = v.minute().unsigned_abs();
    let second = v.second().unsigned_abs();
    let subsec = v.subsec_nanosecond().unsigned_abs();
    // All Jiff civil times are valid ICU4X times.
    IcuTime::try_new(hour, minute, second, subsec).unwrap()
}
