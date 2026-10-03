//! Calendar events (iCal feed) that have not ended yet become cards.

use chrono::{DateTime, TimeZone, Utc};
use pending_core::{CardSeverity, PendingSource, SourceItem};
use pending_ical::{IcalSource, Window, occurrences};

fn now() -> DateTime<Utc> {
    // Segunda-feira, 28/09/2026, 12:00 UTC.
    Utc.with_ymd_and_hms(2026, 9, 28, 12, 0, 0).unwrap()
}

fn calendar(events: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Google Inc//Google Calendar 70.9054//EN\r\n\
BEGIN:VTIMEZONE\r\nTZID:America/Sao_Paulo\r\nBEGIN:STANDARD\r\nDTSTART:19700101T000000\r\n\
TZOFFSETFROM:-0300\r\nTZOFFSETTO:-0300\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\n{events}END:VCALENDAR\r\n"
    )
}

fn event(uid: &str, lines: &str) -> String {
    format!("BEGIN:VEVENT\r\nUID:{uid}\r\nDTSTAMP:20260901T000000Z\r\n{lines}END:VEVENT\r\n")
}

fn items(ics: &str) -> Vec<SourceItem> {
    occurrences(ics, now(), Window::default(), chrono_tz::UTC, None)
        .expect("valid calendar")
        .items
}

fn summary(items: &[SourceItem]) -> Vec<(String, String)> {
    items
        .iter()
        .map(|item| (item.column.clone(), item.card.title.clone()))
        .collect()
}

/// An event today becomes a card in the today section, with time, location,
/// its start as the card date and a stable id per occurrence.
#[test]
fn an_event_later_today_becomes_a_card() {
    let ics = calendar(&event(
        "standup@google.com",
        "SUMMARY:Planning\r\nLOCATION:Room 1\r\nDTSTART:20260928T150000Z\r\nDTEND:20260928T160000Z\r\n",
    ));

    let items = items(&ics);

    assert_eq!(
        summary(&items),
        vec![("Today".to_owned(), "Planning".to_owned())]
    );
    let card = &items[0].card;
    assert_eq!(card.id, "ical:standup@google.com:2026-09-28T15:00:00+00:00");
    assert_eq!(card.source, "calendar");
    assert_eq!(
        card.due_at,
        Some(Utc.with_ymd_and_hms(2026, 9, 28, 15, 0, 0).unwrap())
    );
    assert!(card.body.contains("15:00"), "{}", card.body);
    assert!(card.body.contains("16:00"), "{}", card.body);
    assert!(card.body.contains("Room 1"), "{}", card.body);
    assert_eq!(card.severity, CardSeverity::Info);
}

/// An ongoing event shows under "Now" as a warning; one starting within an
/// hour is also a warning; events already over are left out.
#[test]
fn ongoing_and_imminent_events_are_warnings_and_past_ones_are_hidden() {
    let ics = calendar(
        &[
            event(
                "past",
                "SUMMARY:Past\r\nDTSTART:20260928T080000Z\r\nDTEND:20260928T090000Z\r\n",
            ),
            event(
                "now",
                "SUMMARY:Ongoing\r\nDTSTART:20260928T113000Z\r\nDTEND:20260928T123000Z\r\n",
            ),
            event(
                "soon",
                "SUMMARY:Soon\r\nDTSTART:20260928T124500Z\r\nDTEND:20260928T130000Z\r\n",
            ),
        ]
        .concat(),
    );

    let items = items(&ics);

    assert_eq!(
        summary(&items),
        vec![
            ("Now".to_owned(), "Ongoing".to_owned()),
            ("Today".to_owned(), "Soon".to_owned()),
        ]
    );
    assert!(
        items
            .iter()
            .all(|i| i.card.severity == CardSeverity::Warning)
    );
}

/// An all-day event tomorrow shows under "Tomorrow" marked as all day.
#[test]
fn all_day_events_are_marked() {
    let ics = calendar(&event(
        "holiday",
        "SUMMARY:Holiday\r\nDTSTART;VALUE=DATE:20260929\r\nDTEND;VALUE=DATE:20260930\r\n",
    ));

    let items = items(&ics);

    assert_eq!(
        summary(&items),
        vec![("Tomorrow".to_owned(), "Holiday".to_owned())]
    );
    assert!(
        items[0].card.body.contains("all day"),
        "{}",
        items[0].card.body
    );
}

/// A weekly event in the São Paulo zone yields one occurrence per week in the
/// 30-day window, minus the excluded date (EXDATE).
#[test]
fn recurring_events_are_expanded_within_the_window() {
    let ics = calendar(&event(
        "weekly",
        "SUMMARY:1:1\r\nDTSTART;TZID=America/Sao_Paulo:20260901T100000\r\n\
DTEND;TZID=America/Sao_Paulo:20260901T103000\r\nRRULE:FREQ=WEEKLY;BYDAY=TU\r\n\
EXDATE;TZID=America/Sao_Paulo:20261006T100000\r\n",
    ));

    let items = items(&ics);
    let starts: Vec<String> = items
        .iter()
        .map(|i| i.card.due_at.unwrap().to_rfc3339())
        .collect();

    assert_eq!(
        starts,
        vec![
            "2026-09-29T13:00:00+00:00",
            "2026-10-13T13:00:00+00:00",
            "2026-10-20T13:00:00+00:00",
            "2026-10-27T13:00:00+00:00",
        ]
    );
    assert_eq!(items[0].column, "Tomorrow");
}

/// The window stops at 25 events, even if more fit in 30 days.
#[test]
fn at_most_25_events_are_shown() {
    let ics = calendar(&event(
        "daily",
        "SUMMARY:Daily\r\nDTSTART:20260928T130000Z\r\nDTEND:20260928T131500Z\r\nRRULE:FREQ=DAILY\r\n",
    ));

    let items = items(&ics);

    assert_eq!(items.len(), 25);
}

/// A moved occurrence (RECURRENCE-ID) shows at its new time, a cancelled one
/// disappears, and cancelled events are not shown.
#[test]
fn moved_and_cancelled_occurrences_are_respected() {
    let ics = calendar(&[
        event(
            "series",
            "SUMMARY:Sync\r\nDTSTART:20260929T140000Z\r\nDTEND:20260929T150000Z\r\nRRULE:FREQ=DAILY;COUNT=3\r\n",
        ),
        event(
            "series",
            "SUMMARY:Sync (moved)\r\nRECURRENCE-ID:20260930T140000Z\r\n\
DTSTART:20260930T170000Z\r\nDTEND:20260930T180000Z\r\n",
        ),
        event(
            "series",
            "SUMMARY:Sync\r\nRECURRENCE-ID:20261001T140000Z\r\nSTATUS:CANCELLED\r\n\
DTSTART:20261001T140000Z\r\nDTEND:20261001T150000Z\r\n",
        ),
        event(
            "gone",
            "SUMMARY:Cancelled\r\nSTATUS:CANCELLED\r\nDTSTART:20260929T100000Z\r\nDTEND:20260929T110000Z\r\n",
        ),
    ]
    .concat());

    let items = items(&ics);
    let titles: Vec<(String, String)> = items
        .iter()
        .map(|i| (i.card.title.clone(), i.card.due_at.unwrap().to_rfc3339()))
        .collect();

    assert_eq!(
        titles,
        vec![
            ("Sync".to_owned(), "2026-09-29T14:00:00+00:00".to_owned()),
            (
                "Sync (moved)".to_owned(),
                "2026-09-30T17:00:00+00:00".to_owned()
            ),
        ]
    );
}

/// Folded lines and escaped text in the iCal format are read correctly.
#[test]
fn folded_lines_and_escaped_text_are_decoded() {
    let ics = calendar(&event(
        "folded",
        "SUMMARY:Review\\, plan and\r\n  ship\r\nDTSTART:20260928T150000Z\r\nDTEND:20260928T160000Z\r\n",
    ));

    let items = items(&ics);

    assert_eq!(items[0].card.title, "Review, plan and ship");
}

/// In a Google feed, the card links to the day in Google Calendar.
#[test]
fn google_feeds_link_to_the_day() {
    let ics = calendar(&event(
        "g",
        "SUMMARY:Planning\r\nDTSTART:20260928T150000Z\r\nDTEND:20260928T160000Z\r\n",
    ));

    let batch = occurrences(
        &ics,
        now(),
        Window::default(),
        chrono_tz::UTC,
        Some("https://calendar.google.com/calendar/ical/x/private-y/basic.ics"),
    )
    .unwrap();

    assert_eq!(
        batch.items[0].card.url.as_deref(),
        Some("https://calendar.google.com/calendar/r/day/2026/9/28")
    );
}

/// Without the URL in the environment, the source says which variable to set.
#[tokio::test]
async fn missing_url_names_the_variable() {
    let error = IcalSource::from_env(&|_| None)
        .refresh()
        .await
        .expect_err("not configured");

    assert!(
        error.to_string().contains("TASKS_PENDING_ICAL_URL"),
        "{error}"
    );
}

/// A URL that responds with an error fails with the status, without exposing
/// the secret URL.
#[tokio::test]
async fn http_errors_do_not_leak_the_secret_url() {
    let app = axum::Router::new().route(
        "/calendar/ical/secret-token/basic.ics",
        axum::routing::get(|| async { (axum::http::StatusCode::NOT_FOUND, "gone") }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://{}/calendar/ical/secret-token/basic.ics",
        listener.local_addr().unwrap()
    );
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let error = IcalSource::from_env(&|key| (key == "TASKS_PENDING_ICAL_URL").then(|| url.clone()))
        .refresh()
        .await
        .expect_err("404");

    let message = error.to_string();
    assert!(message.contains("404"), "{message}");
    assert!(!message.contains("secret-token"), "{message}");
}

/// Series ending (UNTIL) on a date, as Google exports all-day events, or at a
/// floating time, are expanded up to the end; they neither vanish nor leave
/// the source degraded.
#[test]
fn series_ending_on_a_date_or_floating_time_are_expanded() {
    let ics = calendar(
        &[
            event(
                "allday",
                "SUMMARY:Gym\r\nDTSTART;VALUE=DATE:20260901\r\nDTEND;VALUE=DATE:20260902\r\n\
RRULE:FREQ=WEEKLY;UNTIL=20261013\r\n",
            ),
            event(
                "floating",
                "SUMMARY:Class\r\nDTSTART:20260930T100000\r\nDTEND:20260930T110000\r\n\
RRULE:FREQ=WEEKLY;UNTIL=20261014T100000\r\n",
            ),
        ]
        .concat(),
    );

    let batch = occurrences(&ics, now(), Window::default(), chrono_tz::UTC, None).unwrap();
    let starts: Vec<(String, String)> = batch
        .items
        .iter()
        .map(|i| (i.card.title.clone(), i.card.due_at.unwrap().to_rfc3339()))
        .collect();

    assert!(batch.warnings.is_empty(), "{:?}", batch.warnings);
    assert_eq!(
        starts,
        vec![
            ("Gym".to_owned(), "2026-09-29T00:00:00+00:00".to_owned()),
            ("Class".to_owned(), "2026-09-30T10:00:00+00:00".to_owned()),
            ("Gym".to_owned(), "2026-10-06T00:00:00+00:00".to_owned()),
            ("Class".to_owned(), "2026-10-07T10:00:00+00:00".to_owned()),
            ("Gym".to_owned(), "2026-10-13T00:00:00+00:00".to_owned()),
            ("Class".to_owned(), "2026-10-14T10:00:00+00:00".to_owned()),
        ]
    );
}

/// An all-day series spans the whole local day, even on the occurrence where
/// the clocks change (a 25-hour day): it stays on screen until local
/// midnight.
#[test]
fn all_day_events_last_the_whole_local_day_across_dst() {
    let zone = chrono_tz::America::New_York;
    let ics = calendar(&event(
        "fallback",
        "SUMMARY:Long day\r\nDTSTART;VALUE=DATE:20261025\r\nDTEND;VALUE=DATE:20261026\r\n\
RRULE:FREQ=WEEKLY\r\n",
    ));
    // 23:30 on 11/01 in New York (04:30 UTC on 11/02), still the same local day.
    let late = Utc.with_ymd_and_hms(2026, 11, 2, 4, 30, 0).unwrap();

    let batch = occurrences(&ics, late, Window::default(), zone, None).unwrap();

    // The 11/01 occurrence (midnight EDT = 04:00 UTC) is still on screen.
    assert_eq!(
        batch.items[0].card.due_at.map(|at| at.to_rfc3339()),
        Some("2026-11-01T04:00:00+00:00".to_owned()),
        "still today at 23:30 local"
    );
}

/// In a zone where midnight does not exist on the DST change day, the all-day
/// event starts at the first instant of the day.
#[test]
fn all_day_events_on_a_day_without_midnight_are_read() {
    let zone = chrono_tz::America::Santiago;
    let ics = calendar(&event(
        "spring",
        "SUMMARY:Spring forward\r\nDTSTART;VALUE=DATE:20260906\r\nDTEND;VALUE=DATE:20260907\r\n",
    ));
    let morning = Utc.with_ymd_and_hms(2026, 9, 6, 12, 0, 0).unwrap();

    let batch = occurrences(&ics, morning, Window::default(), zone, None).unwrap();

    assert!(batch.warnings.is_empty(), "{:?}", batch.warnings);
    assert_eq!(batch.items.len(), 1);
}

/// An event without an end shows only its start time.
#[test]
fn events_without_an_end_show_only_the_start() {
    let ics = calendar(&event(
        "instant",
        "SUMMARY:Reminder\r\nDTSTART:20260928T150000Z\r\n",
    ));

    let items = items(&ics);

    assert!(
        items[0].card.body.contains("15:00"),
        "{}",
        items[0].card.body
    );
    assert!(
        !items[0].card.body.contains("15:00–"),
        "{}",
        items[0].card.body
    );
}

/// The series end (UNTIL) is read in the series' zone: a date UNTIL in an
/// offset zone includes the last day, a floating UNTIL with a start in another
/// zone uses the start's zone, and an UNTIL at a nonexistent hour (DST change)
/// does not drop the series.
#[test]
fn until_is_read_in_the_series_zone() {
    let sp = chrono_tz::America::Sao_Paulo;
    let ics = calendar(
        &[
            event(
                "date-until",
                "SUMMARY:Gym\r\nDTSTART;VALUE=DATE:20260929\r\nDTEND;VALUE=DATE:20260930\r\n\
RRULE:until=20261013;freq=weekly\r\n",
            ),
            event(
                "ny",
                "SUMMARY:NY call\r\nDTSTART;TZID=America/New_York:20260930T090000\r\n\
DTEND;TZID=America/New_York:20260930T093000\r\nRRULE:FREQ=WEEKLY;UNTIL=20261014T090000\r\n",
            ),
            event(
                "gap",
                "SUMMARY:Gap\r\nDTSTART;TZID=America/New_York:20260930T020000\r\n\
DTEND;TZID=America/New_York:20260930T030000\r\nRRULE:FREQ=WEEKLY;UNTIL=20270314T023000\r\n",
            ),
        ]
        .concat(),
    );

    let batch = occurrences(&ics, now(), Window::default(), sp, None).unwrap();
    let count = |title: &str| batch.items.iter().filter(|i| i.card.title == title).count();

    assert!(batch.warnings.is_empty(), "{:?}", batch.warnings);
    assert_eq!(count("Gym"), 3, "29/09, 06/10 and the UNTIL day 13/10");
    assert_eq!(
        count("NY call"),
        3,
        "30/09, 07/10 and 14/10 at 09:00 New York"
    );
    assert!(
        count("Gap") > 0,
        "a non-existent UNTIL time keeps the series"
    );
}

/// Configured columns group the time buckets: "Today" holds what is ongoing
/// and the rest of the day, "Week" what comes later; a bucket no column
/// lists is not shown.
#[test]
fn configured_columns_group_time_buckets() {
    use pending_ical::{IcalColumn, When, assign_columns};

    let ics = calendar(
        &[
            event(
                "now",
                "SUMMARY:Ongoing\r\nDTSTART:20260928T113000Z\r\nDTEND:20260928T123000Z\r\n",
            ),
            event(
                "today",
                "SUMMARY:Afternoon\r\nDTSTART:20260928T150000Z\r\nDTEND:20260928T160000Z\r\n",
            ),
            event(
                "tomorrow",
                "SUMMARY:Tomorrow\r\nDTSTART:20260929T150000Z\r\nDTEND:20260929T160000Z\r\n",
            ),
            event(
                "later",
                "SUMMARY:Friday\r\nDTSTART:20261002T150000Z\r\nDTEND:20261002T160000Z\r\n",
            ),
        ]
        .concat(),
    );
    let columns = vec![
        IcalColumn {
            name: "Today".to_owned(),
            when: vec![When::Now, When::Today],
        },
        IcalColumn {
            name: "Week".to_owned(),
            when: vec![When::Later],
        },
    ];

    let batch = occurrences(&ics, now(), Window::default(), chrono_tz::UTC, None).unwrap();
    let batch = assign_columns(&columns, batch);

    assert_eq!(
        summary(&batch.items),
        vec![
            ("Today".to_owned(), "Ongoing".to_owned()),
            ("Today".to_owned(), "Afternoon".to_owned()),
            ("Week".to_owned(), "Friday".to_owned()),
        ]
    );
}

/// An event without LAST-MODIFIED has no known update time: its card must
/// carry the same one on every refresh, not the time of the refresh.
/// Otherwise the event would look changed each time, waking a snoozed card
/// and flagging it as new.
#[test]
fn events_without_a_modification_time_keep_a_stable_update_time() {
    let ics = calendar(&event(
        "stable@example",
        "DTSTART:20260928T150000Z\r\nDTEND:20260928T160000Z\r\nSUMMARY:Review\r\n",
    ));
    let at = |now: DateTime<Utc>| {
        occurrences(&ics, now, Window::default(), chrono_tz::UTC, None)
            .expect("valid calendar")
            .items[0]
            .card
            .updated_at
    };

    assert_eq!(at(now()), at(now() + chrono::Duration::minutes(5)));

    let modified = calendar(&event(
        "edited@example",
        "DTSTART:20260928T150000Z\r\nDTEND:20260928T160000Z\r\nSUMMARY:Review\r\n\
LAST-MODIFIED:20260920T101500Z\r\n",
    ));
    let card = &items(&modified)[0].card;
    assert_eq!(
        card.updated_at,
        Utc.with_ymd_and_hms(2026, 9, 20, 10, 15, 0).unwrap()
    );
}
