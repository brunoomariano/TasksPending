//! What changed since the user last looked at the dashboard.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use pending_core::{
    CardSeverity, DashboardSnapshot, PendingCard, SourceBatch, SourceItem, SourceOutcome,
    SourceReport, build_snapshot,
};
use pending_runtime::looks::Looks;

fn scratch(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("looks-tests")
        .join(test);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("looks.json")
}

/// Nine in the morning; the tests count minutes from here.
fn at(minutes: i64) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-10-05T09:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
        + Duration::minutes(minutes)
}

/// A board with cards `(id, updated_at)`; the first one is in two stacks.
fn board(cards: &[(&str, DateTime<Utc>)]) -> DashboardSnapshot {
    let item = |column: &str, (id, updated_at): &(&str, DateTime<Utc>)| SourceItem {
        column: column.to_owned(),
        card: PendingCard {
            id: (*id).to_owned(),
            title: (*id).to_owned(),
            body: String::new(),
            source: "test".to_owned(),
            url: None,
            due_at: None,
            severity: CardSeverity::Info,
            updated_at: *updated_at,
        },
    };
    let mut items: Vec<SourceItem> = cards.iter().map(|card| item("Mine", card)).collect();
    if let Some(first) = cards.first() {
        items.push(item("Also", first));
    }
    build_snapshot(
        Utc::now(),
        vec![SourceReport {
            name: "plane".to_owned(),
            board: "Work".to_owned(),
            columns: vec!["Mine".to_owned(), "Also".to_owned()],
            icon: None,
            sorts: Default::default(),
            outcome: SourceOutcome::Fresh {
                batch: SourceBatch {
                    items,
                    warnings: Vec::new(),
                },
                refreshed_at: Utc::now(),
            },
        }],
    )
}

/// The flagged card ids, sorted.
fn changed(looks: &Looks, mut snapshot: DashboardSnapshot) -> Vec<String> {
    looks.apply(&mut snapshot);
    let mut ids = snapshot.changed;
    ids.sort();
    ids
}

/// Before anyone looked there is nothing to compare with, and the first
/// look starts counting from that moment: nothing is flagged.
#[test]
fn nothing_is_flagged_before_or_at_the_first_look() {
    let looks = Looks::load(None);
    let cards = [("old", at(-600)), ("recent", at(-1))];
    assert!(changed(&looks, board(&cards)).is_empty());

    looks.look_at(at(0));

    assert!(changed(&looks, board(&cards)).is_empty());
}

/// Coming back after a while away, the cards that changed since the last
/// activity are flagged, once each; older ones are not. The flags stay
/// while the user keeps looking, and survive a restart.
#[test]
fn cards_changed_while_away_are_flagged_on_return() {
    let path = scratch("return");
    let looks = Looks::load(Some(path.clone()));
    looks.look_at(at(0));
    looks.look_at(at(5));
    let cards = [("during", at(40)), ("before", at(3)), ("ancient", at(-600))];

    // Away from 9:05 to 10:00.
    looks.look_at(at(60));

    assert_eq!(changed(&looks, board(&cards)), ["during"]);
    looks.look_at(at(62));
    looks.look_at(at(69));
    assert_eq!(changed(&Looks::load(Some(path)), board(&cards)), ["during"]);
}

/// What changes while the user is looking is flagged too; the next return
/// after time away starts over from the end of this sitting.
#[test]
fn the_next_return_starts_over() {
    let looks = Looks::load(None);
    looks.look_at(at(0));
    looks.look_at(at(60));
    let cards = [("first", at(40)), ("live", at(63)), ("second", at(100))];
    looks.look_at(at(65));
    assert_eq!(changed(&looks, board(&cards[..2])), ["first", "live"]);

    // Away from 10:05 to 11:00.
    looks.look_at(at(120));

    assert_eq!(changed(&looks, board(&cards)), ["second"]);
}

/// Short pauses are not time away: the flags of this sitting stay.
#[test]
fn a_short_pause_keeps_the_flags() {
    let looks = Looks::load(None);
    looks.look_at(at(0));
    looks.look_at(at(60));
    let cards = [("during", at(40))];

    looks.look_at(at(69));

    assert_eq!(changed(&looks, board(&cards)), ["during"]);
}
