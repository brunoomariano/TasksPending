//! Cards snoozed by the user: hidden until a time or until the item changes.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use pending_core::{
    CardSeverity, DashboardSnapshot, PendingCard, SourceBatch, SourceError, SourceItem,
    SourceOutcome, SourceReport, build_snapshot,
};
use pending_runtime::snoozes::{SnoozeError, Snoozes};

fn scratch(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("snooze-tests")
        .join(test);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("snoozes.json")
}

/// A fixed `updated_at`, well in the past.
fn base() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-01-05T10:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

/// Cards `(id, updated_at)` in two stacks; the first card is in both.
fn batch(cards: &[(&str, DateTime<Utc>)]) -> SourceBatch {
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
    SourceBatch {
        items,
        warnings: Vec::new(),
    }
}

fn snapshot_of(outcome: SourceOutcome) -> DashboardSnapshot {
    build_snapshot(
        Utc::now(),
        vec![SourceReport {
            name: "plane".to_owned(),
            board: "Work".to_owned(),
            columns: vec!["Mine".to_owned(), "Also".to_owned()],
            icon: None,
            sorts: Default::default(),
            outcome,
        }],
    )
}

/// A clean refresh that finished a moment from now.
fn fresh(cards: &[(&str, DateTime<Utc>)]) -> DashboardSnapshot {
    snapshot_of(SourceOutcome::Fresh {
        batch: batch(cards),
        refreshed_at: Utc::now() + Duration::seconds(1),
    })
}

fn visible(snapshot: &DashboardSnapshot) -> Vec<String> {
    let mut ids: Vec<String> = snapshot
        .boards
        .iter()
        .flat_map(|board| &board.groups)
        .flat_map(|group| &group.columns)
        .flat_map(|column| &column.cards)
        .map(|card| card.id.clone())
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

fn snoozed(snapshot: &DashboardSnapshot) -> Vec<String> {
    snapshot
        .snoozed
        .iter()
        .map(|entry| entry.card.id.clone())
        .collect()
}

fn applied(snoozes: &Snoozes, mut snapshot: DashboardSnapshot) -> DashboardSnapshot {
    snoozes.apply(&mut snapshot);
    snapshot
}

/// A snoozed card leaves every stack that held it and is listed once among
/// the snoozed cards, with its source and until when; it survives a
/// restart, and waking it brings it back.
#[test]
fn a_snoozed_card_is_hidden_and_listed_until_woken() {
    let path = scratch("hidden");
    let cards = [("a", base()), ("b", base())];
    let snoozes = Snoozes::load(Some(path.clone()));
    let until = Utc::now() + Duration::hours(1);

    snoozes
        .snooze(&fresh(&cards), "a", Some(until))
        .expect("card is on the dashboard");

    let shown = applied(&Snoozes::load(Some(path)), fresh(&cards));
    assert_eq!(visible(&shown), ["b"]);
    assert_eq!(snoozed(&shown), ["a"]);
    assert_eq!(shown.snoozed[0].source, "plane");
    assert_eq!(shown.snoozed[0].until, Some(until));

    snoozes.wake("a");
    let shown = applied(&snoozes, fresh(&cards));
    assert_eq!(visible(&shown), ["a", "b"]);
    assert!(shown.snoozed.is_empty());
}

/// Only a card on the dashboard can be snoozed, and not into the past.
#[test]
fn snoozing_needs_a_card_and_a_future_time() {
    let snoozes = Snoozes::load(None);
    let board = fresh(&[("a", base())]);

    assert_eq!(
        snoozes.snooze(&board, "ghost", None),
        Err(SnoozeError::UnknownCard)
    );
    assert_eq!(
        snoozes.snooze(&board, "a", Some(Utc::now() - Duration::minutes(1))),
        Err(SnoozeError::PastTime)
    );
    assert_eq!(visible(&applied(&snoozes, board)), ["a"]);
}

/// A snooze ends by itself when its time passes or when the item changes
/// (its `updated_at` moves): the card is back and the snooze is forgotten.
#[test]
fn snoozes_end_when_time_passes_or_the_item_changes() {
    let path = scratch("ends");
    let snoozes = Snoozes::load(Some(path.clone()));
    let cards = [("timed", base()), ("changed", base()), ("quiet", base())];
    let board = fresh(&cards);
    snoozes.snooze(&board, "changed", None).unwrap();
    snoozes.snooze(&board, "quiet", None).unwrap();
    // A snooze so short that it has passed by the next look.
    snoozes
        .snooze(
            &board,
            "timed",
            Some(Utc::now() + Duration::milliseconds(30)),
        )
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(60));

    let later = fresh(&[
        ("timed", base()),
        ("changed", base() + Duration::minutes(30)),
        ("quiet", base()),
    ]);
    let shown = applied(&snoozes, later);

    assert_eq!(visible(&shown), ["changed", "timed"]);
    assert_eq!(snoozed(&shown), ["quiet"]);
    // Forgotten for good: an unchanged card does not hide again.
    let again = applied(&Snoozes::load(Some(path)), fresh(&cards));
    assert_eq!(snoozed(&again), ["quiet"]);
}

/// Like marks: a snooze whose card is gone after a clean refresh is dropped
/// (the item was finished), while a failing source keeps it.
#[test]
fn snoozes_of_finished_cards_are_dropped() {
    let path = scratch("finished");
    let snoozes = Snoozes::load(Some(path.clone()));
    snoozes
        .snooze(&fresh(&[("a", base()), ("b", base())]), "a", None)
        .unwrap();

    let failing = snapshot_of(SourceOutcome::Failed(SourceError::new("401")));
    assert!(snoozed(&applied(&snoozes, failing)).is_empty());
    // Still remembered: back on a later good refresh that has the card.
    let back = applied(&snoozes, fresh(&[("a", base()), ("b", base())]));
    assert_eq!(snoozed(&back), ["a"]);

    let gone = applied(&snoozes, fresh(&[("b", base())]));
    assert!(snoozed(&gone).is_empty());
    let reloaded = applied(
        &Snoozes::load(Some(path)),
        fresh(&[("a", base()), ("b", base())]),
    );
    assert_eq!(visible(&reloaded), ["a", "b"]);
}
