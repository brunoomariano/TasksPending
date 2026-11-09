//! Being reminded of a card in a while: the choices, the alerts set and
//! when each one rings, and what the notification says. Alerts last as
//! long as the window: nothing is written anywhere.

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Local};

/// How long until the alert rings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Quarter,
    Half,
    Hour,
    ThreeHours,
}

/// The choices in menu order, each with its label.
pub const CHOICES: [(Choice, &str); 4] = [
    (Choice::Quarter, "in 15 min"),
    (Choice::Half, "in 30 min"),
    (Choice::Hour, "in 1 hour"),
    (Choice::ThreeHours, "in 3 hours"),
];

/// How the time an alert rings is written: `15:45`.
pub const TIME_FORMAT: &str = "%H:%M";

/// How long `choice` waits.
pub fn wait(choice: Choice) -> Duration {
    let minutes = match choice {
        Choice::Quarter => 15,
        Choice::Half => 30,
        Choice::Hour => 60,
        Choice::ThreeHours => 180,
    };
    Duration::from_secs(minutes * 60)
}

/// "Alert at 15:45": the line on a card with an alert set.
pub fn at_text(at: DateTime<Local>) -> String {
    format!("Alert at {}", at.format(TIME_FORMAT))
}

/// An alert set on a card: when it rings, and what to say then (kept
/// here, since the card may be gone from the dashboard by then).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alert {
    pub at: DateTime<Local>,
    pub title: String,
    pub source: String,
}

/// What the desktop notification for `alert` says: the card's title, then
/// its source and the time the alert was set for.
pub fn notification(alert: &Alert) -> (String, String) {
    (
        alert.title.clone(),
        format!(
            "{} · alert set for {}",
            alert.source,
            alert.at.format(TIME_FORMAT)
        ),
    )
}

/// The alerts set, one per card at most.
#[derive(Debug, Default)]
pub struct Alerts {
    by_card: HashMap<String, Alert>,
}

impl Alerts {
    /// Sets `alert` on the card `id`, in place of any it had.
    pub fn set(&mut self, id: &str, alert: Alert) {
        self.by_card.insert(id.to_owned(), alert);
    }

    /// Takes the alert off the card `id`, if it had one.
    pub fn cancel(&mut self, id: &str) -> Option<Alert> {
        self.by_card.remove(id)
    }

    /// Gives the alert on the card `id`, if it has one, the card's new
    /// `title`, so that it rings with the name the card has now.
    pub fn retitle(&mut self, id: &str, title: &str) {
        if let Some(alert) = self.by_card.get_mut(id) {
            alert.title = title.to_owned();
        }
    }

    /// The alerts due at `now`, soonest first, taken off their cards.
    pub fn due(&mut self, now: DateTime<Local>) -> Vec<(String, Alert)> {
        let mut due: Vec<(String, Alert)> = self
            .by_card
            .iter()
            .filter(|(_, alert)| alert.at <= now)
            .map(|(id, alert)| (id.clone(), alert.clone()))
            .collect();
        due.sort_by_key(|(_, alert)| alert.at);
        for (id, _) in &due {
            self.by_card.remove(id);
        }
        due
    }

    /// What the card `id` says about its alert, when it has one.
    pub fn text_for(&self, id: &str) -> Option<String> {
        self.by_card.get(id).map(|alert| at_text(alert.at))
    }
}

/// The alerts that rang while the window was not in front, until it next
/// is: the icon on Omarchy's bar shows a dot for them (see
/// [`crate::omarchy_bar::signal`]). One per card, the latest, in the order
/// they rang.
#[derive(Debug, Default)]
pub struct Missed {
    /// The card's id and its title when the alert rang.
    rung: Vec<(String, String)>,
}

impl Missed {
    /// Notes the alerts `rung`, unless the window was `in_front`, where
    /// they were seen ringing. Says whether that changed anything.
    pub fn rang(&mut self, rung: &[(String, Alert)], in_front: bool) -> bool {
        if in_front || rung.is_empty() {
            return false;
        }
        for (id, alert) in rung {
            self.rung.retain(|(missed, _)| missed != id);
            self.rung.push((id.clone(), alert.title.clone()));
        }
        true
    }

    /// The window came to the front: nothing is missed any more. Says
    /// whether something was.
    pub fn seen(&mut self) -> bool {
        let was = !self.rung.is_empty();
        self.rung.clear();
        was
    }

    /// The titles of the cards whose alerts were missed, in the order
    /// they rang.
    pub fn titles(&self) -> Vec<&str> {
        self.rung.iter().map(|(_, title)| title.as_str()).collect()
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn local(hour: u32, minute: u32) -> DateTime<Local> {
        Local
            .with_ymd_and_hms(2026, 10, 7, hour, minute, 0)
            .unwrap()
    }

    fn alert(at: DateTime<Local>) -> Alert {
        Alert {
            at,
            title: "Review the release checklist".to_owned(),
            source: "github".to_owned(),
        }
    }

    /// The menu offers a quarter of an hour, half an hour, an hour and
    /// three hours, in that order.
    #[test]
    fn offers_four_waits() {
        assert_eq!(
            CHOICES.map(|(_, label)| label),
            ["in 15 min", "in 30 min", "in 1 hour", "in 3 hours"]
        );
        assert_eq!(
            CHOICES.map(|(choice, _)| wait(choice).as_secs() / 60),
            [15, 30, 60, 180]
        );
    }

    /// A card with an alert says when it rings; a card without says
    /// nothing. Setting an alert again replaces the first, and cancelling
    /// takes it off.
    #[test]
    fn one_alert_per_card() {
        let mut alerts = Alerts::default();
        assert_eq!(alerts.text_for("gh:1"), None);
        alerts.set("gh:1", alert(local(15, 45)));
        assert_eq!(alerts.text_for("gh:1"), Some("Alert at 15:45".to_owned()));
        alerts.set("gh:1", alert(local(16, 0)));
        assert_eq!(alerts.text_for("gh:1"), Some("Alert at 16:00".to_owned()));
        assert_eq!(alerts.cancel("gh:1"), Some(alert(local(16, 0))));
        assert_eq!(alerts.text_for("gh:1"), None);
        assert_eq!(alerts.cancel("gh:1"), None);
    }

    /// At each look, the alerts whose time has come ring once, soonest
    /// first, and the others wait; one set for this very minute rings now.
    #[test]
    fn rings_each_alert_once_when_its_time_comes() {
        let mut alerts = Alerts::default();
        alerts.set("gh:1", alert(local(16, 0)));
        alerts.set("gh:2", alert(local(15, 45)));
        alerts.set("gh:3", alert(local(17, 0)));
        assert!(alerts.due(local(15, 44)).is_empty());
        let rung: Vec<_> = alerts
            .due(local(16, 0))
            .into_iter()
            .map(|(id, alert)| (id, alert.at))
            .collect();
        assert_eq!(
            rung,
            [
                ("gh:2".to_owned(), local(15, 45)),
                ("gh:1".to_owned(), local(16, 0)),
            ]
        );
        assert!(alerts.due(local(16, 0)).is_empty(), "rung once");
        assert_eq!(alerts.text_for("gh:3"), Some("Alert at 17:00".to_owned()));
    }

    /// Alerts that ring while the window is not in front are missed until
    /// it comes to the front, one per card (the latest), in the order
    /// they rang; ringing in front, they were seen, not missed.
    #[test]
    fn misses_the_alerts_that_ring_away_from_the_window() {
        let mut missed = Missed::default();
        let rung = |id: &str, title: &str| {
            (
                id.to_owned(),
                Alert {
                    title: title.to_owned(),
                    ..alert(local(15, 45))
                },
            )
        };
        assert!(!missed.rang(&[rung("gh:1", "Review")], true), "in front");
        assert!(missed.titles().is_empty());
        assert!(!missed.rang(&[], false), "nothing rang");
        assert!(missed.rang(&[rung("gh:1", "Review"), rung("gh:2", "Deploy")], false));
        assert!(missed.rang(&[rung("gh:1", "Review again")], false));
        assert_eq!(missed.titles(), ["Deploy", "Review again"]);
        assert!(missed.seen(), "something was missed");
        assert!(missed.titles().is_empty());
        assert!(!missed.seen(), "nothing left");
    }

    /// The notification names the card, and says where it comes from and
    /// the time the alert was set for.
    #[test]
    fn says_which_card_and_when() {
        assert_eq!(
            notification(&alert(local(15, 45))),
            (
                "Review the release checklist".to_owned(),
                "github · alert set for 15:45".to_owned()
            )
        );
    }
}
