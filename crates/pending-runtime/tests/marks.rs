//! Cards marked as in progress, remembered across restarts.

use std::path::{Path, PathBuf};

use chrono::Utc;
use pending_core::{
    CardSeverity, DashboardSnapshot, PendingCard, SourceBatch, SourceError, SourceItem,
    SourceOutcome, SourceReport, build_snapshot,
};
use pending_runtime::marks::{MarkError, Marks};

fn scratch(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("marks-tests")
        .join(test);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("marks.json")
}

fn batch(ids: &[&str]) -> SourceBatch {
    SourceBatch {
        items: ids
            .iter()
            .map(|id| SourceItem {
                column: "Mine".to_owned(),
                card: PendingCard {
                    id: (*id).to_owned(),
                    title: (*id).to_owned(),
                    body: String::new(),
                    source: "test".to_owned(),
                    url: None,
                    due_at: None,
                    severity: CardSeverity::Info,
                    updated_at: Utc::now(),
                },
            })
            .collect(),
        warnings: Vec::new(),
    }
}

fn fresh(ids: &[&str]) -> SourceOutcome {
    SourceOutcome::Fresh {
        batch: batch(ids),
        refreshed_at: Utc::now(),
    }
}

fn stale(ids: &[&str]) -> SourceOutcome {
    SourceOutcome::Stale {
        batch: batch(ids),
        refreshed_at: Utc::now(),
        error: SourceError::new("503"),
    }
}

fn snapshot(sources: Vec<(&str, SourceOutcome)>) -> DashboardSnapshot {
    build_snapshot(
        Utc::now(),
        sources
            .into_iter()
            .map(|(name, outcome)| SourceReport {
                name: name.to_owned(),
                board: "Work".to_owned(),
                columns: vec!["Mine".to_owned()],
                icon: None,
                sorts: Default::default(),
                outcome,
            })
            .collect(),
    )
}

fn marked(marks: &Marks, mut snapshot: DashboardSnapshot) -> Vec<String> {
    marks.apply(&mut snapshot);
    snapshot.marked
}

/// Marked cards are listed in the snapshot in the order they were marked,
/// any number of them, and survive a restart; unmarking removes one, and
/// marking twice does not duplicate it.
#[test]
fn marks_are_kept_in_order_across_restarts() {
    let path = scratch("restart");
    let board = || snapshot(vec![("plane", fresh(&["a", "b", "c"]))]);
    let marks = Marks::load(Some(path.clone()));
    let on_screen = board();

    for id in ["c", "a", "b", "a"] {
        marks.set(&on_screen, id, true).expect("card is on screen");
    }
    assert_eq!(marked(&marks, board()), ["c", "a", "b"]);

    marks.set(&on_screen, "a", false).expect("unmark");
    let reloaded = Marks::load(Some(path));
    assert_eq!(marked(&reloaded, board()), ["c", "b"]);
}

/// Only a card on screen can be marked; unmarking an unknown card is fine.
#[test]
fn unknown_cards_cannot_be_marked() {
    let marks = Marks::load(None);
    let on_screen = snapshot(vec![("plane", fresh(&["a"]))]);

    assert_eq!(
        marks.set(&on_screen, "ghost", true),
        Err(MarkError::UnknownCard)
    );
    assert_eq!(marks.set(&on_screen, "ghost", false), Ok(()));
    assert!(marked(&marks, on_screen).is_empty());
}

/// A marked card that is gone after a clean refresh of its source was
/// finished elsewhere: its mark is dropped for good. While the source is
/// failing or showing old data, the mark stays.
#[test]
fn marks_of_finished_cards_are_dropped() {
    let path = scratch("finished");
    let marks = Marks::load(Some(path.clone()));
    marks
        .set(&snapshot(vec![("plane", fresh(&["a", "b"]))]), "a", true)
        .unwrap();

    // Source in trouble and the card missing: keep the mark.
    assert_eq!(
        marked(&marks, snapshot(vec![("plane", stale(&["b"]))])),
        ["a"]
    );
    assert_eq!(
        marked(
            &marks,
            snapshot(vec![(
                "plane",
                SourceOutcome::Failed(SourceError::new("401"))
            )])
        ),
        ["a"]
    );

    // Clean refresh without the card: it is done.
    assert!(marked(&marks, snapshot(vec![("plane", fresh(&["b"]))])).is_empty());
    let reloaded = Marks::load(Some(path));
    assert!(marked(&reloaded, snapshot(vec![("plane", fresh(&["a", "b"]))])).is_empty());
}

/// Marks of a source that was removed from the config are dropped.
#[test]
fn marks_of_removed_sources_are_dropped() {
    let marks = Marks::load(None);
    marks
        .set(&snapshot(vec![("plane", fresh(&["a"]))]), "a", true)
        .unwrap();

    assert!(marked(&marks, snapshot(vec![("github", fresh(&["a"]))])).is_empty());
}

/// An unreadable marks file starts empty instead of failing, and the file
/// holding the marks is private to the user.
#[test]
fn a_broken_file_starts_empty_and_saved_files_are_private() {
    let path = scratch("broken");
    std::fs::write(&path, "not json").unwrap();
    let marks = Marks::load(Some(path.clone()));
    let on_screen = snapshot(vec![("plane", fresh(&["a"]))]);
    assert!(marked(&marks, on_screen.clone()).is_empty());

    marks.set(&on_screen, "a", true).unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
    assert_eq!(marked(&Marks::load(Some(path)), on_screen), ["a"]);
}
