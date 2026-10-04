//! Day-only due dates, as issue trackers keep them.

use chrono::{DateTime, Local, NaiveDate, Utc};

/// The due day of an item and whether it has passed. Shared by the sources
/// so that "overdue" means the same thing on every card: the day is over
/// in the local time zone, and the item is still open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DueDay {
    day: NaiveDate,
    overdue: bool,
}

impl DueDay {
    /// A due day seen on `today` (the local date): overdue from the day
    /// after it.
    pub fn new(day: NaiveDate, today: NaiveDate) -> Self {
        Self {
            day,
            overdue: day < today,
        }
    }

    /// Reads `YYYY-MM-DD`; anything else is no due day.
    pub fn parse(text: &str, today: NaiveDate) -> Option<Self> {
        NaiveDate::parse_from_str(text, "%Y-%m-%d")
            .ok()
            .map(|day| Self::new(day, today))
    }

    /// The same day for an item that is finished or cancelled: nothing is
    /// left to be late, so it is never overdue.
    pub fn of_closed_item(self) -> Self {
        Self {
            overdue: false,
            ..self
        }
    }

    pub fn is_overdue(self) -> bool {
        self.overdue
    }

    /// `due 2026-10-05` or `overdue since 2026-10-01`, for the card details.
    pub fn label(self) -> String {
        let prefix = if self.overdue { "overdue since" } else { "due" };
        format!("{prefix} {}", self.day)
    }

    /// The start of the day in the local time zone, matching how "overdue"
    /// is decided; `None` for a midnight that a clock change skips.
    pub fn at(self) -> Option<DateTime<Utc>> {
        self.day
            .and_hms_opt(0, 0, 0)
            .and_then(|midnight| midnight.and_local_timezone(Local).earliest())
            .map(|midnight| midnight.with_timezone(&Utc))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(text: &str) -> NaiveDate {
        NaiveDate::parse_from_str(text, "%Y-%m-%d").unwrap()
    }

    /// An item is overdue from the day after its due day, and says since
    /// when; on the day itself and before, it is just due.
    #[test]
    fn a_due_day_is_overdue_once_it_has_passed() {
        let today = day("2026-10-03");

        let past = DueDay::parse("2026-10-02", today).expect("a date");
        assert!(past.is_overdue());
        assert_eq!(past.label(), "overdue since 2026-10-02");

        for text in ["2026-10-03", "2026-10-04"] {
            let due = DueDay::parse(text, today).expect("a date");
            assert!(!due.is_overdue(), "{text}");
            assert_eq!(due.label(), format!("due {text}"));
        }
        assert_eq!(DueDay::new(day("2026-10-02"), today), past);
    }

    /// A finished item keeps its due day but is never overdue.
    #[test]
    fn a_closed_item_is_never_overdue() {
        let closed = DueDay::new(day("2026-10-01"), day("2026-10-03")).of_closed_item();

        assert!(!closed.is_overdue());
        assert_eq!(closed.label(), "due 2026-10-01");
    }

    /// The due time is the local start of the day; text that is not a
    /// date gives no due day at all.
    #[test]
    fn the_due_time_is_local_midnight() {
        let due = DueDay::parse("2026-10-05", day("2026-10-03")).expect("a date");
        let at = due.at().expect("midnight exists").with_timezone(&Local);

        assert_eq!(at.date_naive(), day("2026-10-05"));
        assert_eq!(at.format("%H:%M:%S").to_string(), "00:00:00");
        for text in ["", "tomorrow", "2026-13-01", "2026-10-05T10:00:00Z"] {
            assert_eq!(DueDay::parse(text, day("2026-10-03")), None, "{text}");
        }
    }
}
