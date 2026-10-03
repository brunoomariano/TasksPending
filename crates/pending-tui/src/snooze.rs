//! Hiding a card for a while: the choices and when each one ends. The same
//! four choices, with the same times, as the web page.

use chrono::{DateTime, Datelike, Duration, Local, NaiveTime, TimeZone, Utc};

/// How long a snoozed card stays away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Hour,
    Tomorrow,
    NextWeek,
    UntilChange,
}

/// The choices in menu order, each with its label.
pub const CHOICES: [(Choice, &str); 4] = [
    (Choice::Hour, "1 hour"),
    (Choice::Tomorrow, "Tomorrow"),
    (Choice::NextWeek, "Next week"),
    (Choice::UntilChange, "Until it changes"),
];

/// How a time a card comes back is written: `Thu 8 Oct 08:00`.
pub const TIME_FORMAT: &str = "%a %-d %b %H:%M";

/// Hour of the morning a snoozed card comes back, local time.
const MORNING_HOUR: u32 = 8;

/// When a card snoozed at `now` comes back: in an hour, tomorrow morning, or
/// next Monday morning (in `now`'s time zone). `None` waits for the item to
/// change. A card always comes back early when its item changes.
pub fn until<Tz: TimeZone>(choice: Choice, now: &DateTime<Tz>) -> Option<DateTime<Tz>> {
    let morning = |days_ahead: i64| {
        let day = now.date_naive() + Duration::days(days_ahead);
        let time = NaiveTime::from_hms_opt(MORNING_HOUR, 0, 0).unwrap_or_default();
        now.timezone()
            .from_local_datetime(&day.and_time(time))
            .earliest()
            // A clock change skipped that hour: the same time of day then.
            .unwrap_or_else(|| now.clone() + Duration::days(days_ahead))
    };
    match choice {
        Choice::Hour => Some(now.clone() + Duration::hours(1)),
        Choice::Tomorrow => Some(morning(1)),
        Choice::NextWeek => {
            // Days to the next Monday; a Monday goes to the following one.
            let since_monday = i64::from(now.weekday().num_days_from_monday());
            Some(morning(7 - since_monday))
        }
        Choice::UntilChange => None,
    }
}

/// "until Thu 8 Oct 08:00" (local time) for a time, "until it changes"
/// without one.
pub fn until_text(until: Option<DateTime<Utc>>) -> String {
    until.map_or_else(
        || "until it changes".to_owned(),
        |at| {
            let at = at.with_timezone(&Local);
            format!("until {}", at.format(TIME_FORMAT))
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Wednesday afternoon, local time.
    fn now() -> DateTime<Local> {
        local(2026, 10, 7, 15, 30)
    }

    fn local(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Local> {
        Local
            .with_ymd_and_hms(year, month, day, hour, minute, 0)
            .unwrap()
    }

    /// The menu offers an hour, tomorrow, next week, or waiting for a change.
    #[test]
    fn offers_four_choices() {
        assert_eq!(
            CHOICES.map(|(choice, _)| choice),
            [
                Choice::Hour,
                Choice::Tomorrow,
                Choice::NextWeek,
                Choice::UntilChange
            ]
        );
        assert_eq!(
            CHOICES.map(|(_, label)| label),
            ["1 hour", "Tomorrow", "Next week", "Until it changes"]
        );
    }

    /// "1 hour" is an hour from now.
    #[test]
    fn an_hour_from_now() {
        assert_eq!(
            until(Choice::Hour, &now()),
            Some(local(2026, 10, 7, 16, 30))
        );
    }

    /// "Tomorrow" is the next day at 8 in the morning, local time, also
    /// across the end of a month.
    #[test]
    fn tomorrow_morning() {
        assert_eq!(
            until(Choice::Tomorrow, &now()),
            Some(local(2026, 10, 8, 8, 0))
        );
        assert_eq!(
            until(Choice::Tomorrow, &local(2026, 10, 31, 23, 59)),
            Some(local(2026, 11, 1, 8, 0))
        );
        // Before 8 in the morning it is still the next day, not today.
        assert_eq!(
            until(Choice::Tomorrow, &local(2026, 10, 7, 6, 0)),
            Some(local(2026, 10, 8, 8, 0))
        );
    }

    /// "Next week" is the next Monday at 8: from a weekday, from the weekend,
    /// and from a Monday, which goes to the following one.
    #[test]
    fn next_monday_morning() {
        let monday = Some(local(2026, 10, 12, 8, 0));

        assert_eq!(until(Choice::NextWeek, &now()), monday, "a Wednesday");
        assert_eq!(
            until(Choice::NextWeek, &local(2026, 10, 5, 9, 0)),
            monday,
            "a Monday goes to the following one"
        );
        assert_eq!(
            until(Choice::NextWeek, &local(2026, 10, 10, 12, 0)),
            monday,
            "a Saturday"
        );
        assert_eq!(
            until(Choice::NextWeek, &local(2026, 10, 11, 23, 0)),
            monday,
            "a Sunday night"
        );
    }

    /// "Until it changes" has no time.
    #[test]
    fn until_it_changes_has_no_time() {
        assert_eq!(until(Choice::UntilChange, &now()), None);
    }

    /// The snoozed list says when each card comes back, in local time.
    #[test]
    fn describes_when_a_card_comes_back() {
        assert_eq!(until_text(None), "until it changes");
        assert_eq!(
            until_text(Some(local(2026, 10, 8, 8, 0).with_timezone(&Utc))),
            "until Thu 8 Oct 08:00"
        );
    }
}
