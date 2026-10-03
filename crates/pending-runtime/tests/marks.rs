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

/// A clean refresh that finished a moment from now, i.e. after any mark
/// made so far.
fn fresh(ids: &[&str]) -> SourceOutcome {
    fresh_at(ids, Utc::now() + chrono::Duration::seconds(1))
}

fn fresh_at(ids: &[&str], refreshed_at: chrono::DateTime<Utc>) -> SourceOutcome {
    SourceOutcome::Fresh {
        batch: batch(ids),
        refreshed_at,
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

/// The daemon and the TUI fetch on their own schedules. A process whose
/// last clean refresh is older than the mark has simply not seen the card
/// yet: it must not drop the mark.
#[test]
fn a_refresh_older_than_the_mark_does_not_drop_it() {
    let path = scratch("older-refresh");
    let marks = Marks::load(Some(path.clone()));
    let earlier = Utc::now() - chrono::Duration::minutes(5);
    marks
        .set(
            &snapshot(vec![("github", fresh(&["new-pr"]))]),
            "new-pr",
            true,
        )
        .unwrap();

    let behind = snapshot(vec![("github", fresh_at(&[], earlier))]);
    assert_eq!(marked(&Marks::load(Some(path.clone())), behind), ["new-pr"]);

    // Once that process refreshes cleanly and still lacks it, it is done.
    let caught_up = snapshot(vec![("github", fresh(&[]))]);
    assert!(marked(&Marks::load(Some(path)), caught_up).is_empty());
}

/// A source missing from the snapshot says nothing about its cards: the TUI
/// may run another config, the config file may be briefly absent, or the
/// source may be switched off. Its marks are kept and work again when the
/// source is back; only marks left behind for a month are cleaned up.
#[test]
fn marks_survive_their_source_leaving_the_config() {
    let path = scratch("absent-source");
    let marks = Marks::load(Some(path.clone()));
    marks
        .set(&snapshot(vec![("plane", fresh(&["a"]))]), "a", true)
        .unwrap();

    assert_eq!(
        marked(&marks, snapshot(vec![("sample", fresh(&["x"]))])),
        ["a"]
    );
    assert_eq!(
        marked(&marks, snapshot(vec![("plane", fresh(&["a"]))])),
        ["a"]
    );

    let old = Utc::now() - chrono::Duration::days(40);
    std::fs::write(
        &path,
        format!(
            r#"{{"version":1,"marks":[{{"id":"a","source":"plane","marked_at":"{}"}}]}}"#,
            old.to_rfc3339()
        ),
    )
    .unwrap();
    assert!(marked(&marks, snapshot(vec![("sample", fresh(&["x"]))])).is_empty());
}

/// The file is the truth: deleting it clears the marks, and a file written
/// by a newer version is left alone instead of being overwritten.
#[test]
fn the_file_can_be_deleted_and_newer_files_are_left_alone() {
    let path = scratch("file-truth");
    let marks = Marks::load(Some(path.clone()));
    let board = || snapshot(vec![("plane", fresh(&["a", "b"]))]);
    marks.set(&board(), "a", true).unwrap();

    std::fs::remove_file(&path).unwrap();
    assert!(marked(&marks, board()).is_empty());

    let newer = r#"{"version":99,"marks":[]}"#;
    std::fs::write(&path, newer).unwrap();
    marks.set(&board(), "b", true).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), newer);
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

/// The daemon and the TUI are separate processes sharing one marks file: a
/// mark made by one shows in the other, and neither overwrites the other's
/// marks with its own older copy.
#[test]
fn two_processes_share_the_marks_file() {
    let path = scratch("shared");
    let board = || snapshot(vec![("plane", fresh(&["a", "b", "c"]))]);
    let daemon = Marks::load(Some(path.clone()));
    let tui = Marks::load(Some(path));

    daemon.set(&board(), "a", true).unwrap();
    assert_eq!(marked(&tui, board()), ["a"]);

    tui.set(&board(), "b", true).unwrap();
    assert_eq!(marked(&daemon, board()), ["a", "b"]);

    daemon.set(&board(), "a", false).unwrap();
    assert_eq!(marked(&tui, board()), ["b"]);
}

/// A marked card that a stack's `exclude` pattern now hides is not
/// finished: its source still lists it, so the mark is kept and works again
/// when the pattern goes away. Once the source stops listing the card, the
/// mark is dropped like any other.
#[test]
fn a_mark_on_a_card_hidden_by_exclude_is_kept_until_the_card_is_finished() {
    use std::collections::HashSet;

    let path = scratch("hidden");
    let marks = Marks::load(Some(path));
    marks
        .set(&snapshot(vec![("github", fresh(&["a", "b"]))]), "a", true)
        .unwrap();
    let hidden: HashSet<String> = HashSet::from(["a".to_owned()]);

    // Excluded from every stack: off the boards, still marked.
    let mut excluded = snapshot(vec![("github", fresh(&["b"]))]);
    marks.apply_hiding(&mut excluded, &hidden);
    assert_eq!(excluded.marked, ["a"]);

    // The pattern is removed: the card is back, with its mark.
    assert_eq!(
        marked(&marks, snapshot(vec![("github", fresh(&["a", "b"]))])),
        ["a"]
    );

    // Finished at the source: neither listed nor hidden.
    assert!(marked(&marks, snapshot(vec![("github", fresh(&["b"]))])).is_empty());
}
