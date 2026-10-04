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

    snoozes.wake("a").expect("saved");
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

/// A snoozed card can be snoozed again, from a snapshot that already lists
/// it as snoozed (what the page and the TUI hold): the new time replaces
/// the old one, and the card is still listed once.
#[test]
fn snoozing_a_snoozed_card_replaces_its_time() {
    let snoozes = Snoozes::load(Some(scratch("again")));
    let cards = [("a", base()), ("b", base())];
    let first = Utc::now() + Duration::hours(1);
    let second = Utc::now() + Duration::days(2);
    snoozes.snooze(&fresh(&cards), "a", Some(first)).unwrap();
    let shown = applied(&snoozes, fresh(&cards));
    assert_eq!(visible(&shown), ["b"]);

    snoozes
        .snooze(&shown, "a", Some(second))
        .expect("a snoozed card is still a card of the dashboard");

    let shown = applied(&snoozes, fresh(&cards));
    assert_eq!(snoozed(&shown), ["a"]);
    assert_eq!(shown.snoozed[0].until, Some(second));
    assert_eq!(shown.snoozed[0].source, "plane");
    // Still not a way to snooze something that is nowhere.
    assert_eq!(
        snoozes.snooze(&shown, "ghost", None),
        Err(SnoozeError::UnknownCard)
    );
}

/// A snoozed card that an `exclude` pattern now hides is still open at
/// its source: the snooze is kept, although a clean refresh no longer
/// shows the card, and applies again when the pattern goes away.
#[test]
fn a_snooze_on_a_card_hidden_by_exclude_is_kept() {
    let snoozes = Snoozes::load(Some(scratch("excluded")));
    let cards = [("a", base()), ("b", base())];
    snoozes.snooze(&fresh(&cards), "a", None).unwrap();

    let mut excluding = fresh(&[("b", base())]);
    snoozes.apply_hiding(&mut excluding, &["a".to_owned()].into());
    assert!(snoozed(&excluding).is_empty());
    assert_eq!(visible(&excluding), ["b"]);

    let shown = applied(&snoozes, fresh(&cards));
    assert_eq!(snoozed(&shown), ["a"]);
}

/// A snooze whose source is not on the dashboard (switched off, or absent
/// from the config in use) is kept for a month, then forgotten.
#[test]
fn snoozes_of_an_absent_source_are_kept_for_a_month() {
    let path = scratch("absent");
    let entry = |id: &str, days: i64| {
        serde_json::json!({
            "id": id,
            "source": "switched-off",
            "snoozed_at": Utc::now() - Duration::days(days),
            "until": null,
            "updated_at": base(),
        })
    };
    let file = serde_json::json!({
        "version": 1,
        "snoozes": [entry("recent", 29), entry("old", 31)],
    });
    std::fs::write(&path, file.to_string()).unwrap();
    let snoozes = Snoozes::load(Some(path.clone()));

    applied(&snoozes, fresh(&[("b", base())]));

    let kept: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let ids: Vec<&str> = kept["snoozes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|snooze| snooze["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["recent"]);
}

/// A snooze that cannot be written is an error, not a silent success: the
/// card would stay on the board while the page said it was snoozed.
#[test]
fn a_snooze_that_cannot_be_saved_is_an_error() {
    let cards = [("a", base())];
    // The state directory cannot be created: a file is in its place.
    let blocker = scratch("unsaved").with_file_name("blocker");
    std::fs::write(&blocker, "not a directory").unwrap();
    let snoozes = Snoozes::load(Some(blocker.join("snoozes.json")));
    assert_eq!(
        snoozes.snooze(&fresh(&cards), "a", None),
        Err(SnoozeError::NotSaved)
    );
    assert_eq!(snoozes.wake("a"), Err(SnoozeError::NotSaved));

    // A file from a newer version is left alone, and says so.
    let path = scratch("newer");
    let newer = r#"{"version":99,"snoozes":[]}"#;
    std::fs::write(&path, newer).unwrap();
    let snoozes = Snoozes::load(Some(path.clone()));
    assert_eq!(
        snoozes.snooze(&fresh(&cards), "a", None),
        Err(SnoozeError::NotSaved)
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), newer);
    assert_eq!(visible(&applied(&snoozes, fresh(&cards))), ["a"]);
}
