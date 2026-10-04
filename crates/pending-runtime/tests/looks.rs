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
    board_refreshed(cards, Utc::now())
}

/// [`board`] whose source was last refreshed at `refreshed_at`.
fn board_refreshed(
    cards: &[(&str, DateTime<Utc>)],
    refreshed_at: DateTime<Utc>,
) -> DashboardSnapshot {
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
                refreshed_at,
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

/// A change is on screen only once its source was refreshed. A card that
/// changed shortly before the user left, but was fetched after, was never
/// seen: it is flagged on return although it is older than the last
/// activity. What was already on screen is not.
#[test]
fn a_change_fetched_after_the_user_left_is_flagged() {
    let looks = Looks::load(None);
    looks.look_at_board(at(0), &board_refreshed(&[], at(-4)));
    // 9:05, the last activity: the source was last refreshed at 9:01.
    looks.look_at_board(at(5), &board_refreshed(&[], at(1)));

    // Back at 10:00. "unseen" changed at 9:03, after the 9:01 refresh but
    // before the user left at 9:05; "seen" changed at 9:00:30.
    let cards = [
        ("unseen", at(3)),
        ("seen", at(0) + Duration::seconds(30)),
        ("during", at(40)),
    ];
    let back = board_refreshed(&cards, at(58));
    looks.look_at_board(at(60), &back);

    assert_eq!(changed(&looks, back), ["during", "unseen"]);

    // Without knowing what was on screen, the last activity decides.
    let plain = Looks::load(None);
    plain.look_at(at(0));
    plain.look_at(at(5));
    plain.look_at(at(60));
    assert_eq!(changed(&plain, board(&cards)), ["during"]);
}

/// Ten minutes without activity is time away; one second less is not.
#[test]
fn ten_minutes_without_activity_start_a_new_sitting() {
    let cards = [("during", at(40))];

    let looks = Looks::load(None);
    looks.look_at(at(0));
    looks.look_at(at(60));
    looks.look_at(at(70) - Duration::seconds(1));
    assert_eq!(changed(&looks, board(&cards)), ["during"], "same sitting");

    let looks = Looks::load(None);
    looks.look_at(at(0));
    looks.look_at(at(60));
    looks.look_at(at(70));
    assert!(changed(&looks, board(&cards)).is_empty(), "a new sitting");
}

/// A clock that goes back is not time away and does not move the latest
/// activity back: the sitting goes on from where it was.
#[test]
fn a_clock_that_goes_back_keeps_the_sitting() {
    let looks = Looks::load(None);
    looks.look_at(at(0));
    looks.look_at(at(60));
    let cards = [("during", at(40))];

    looks.look_at(at(20));
    assert_eq!(changed(&looks, board(&cards)), ["during"]);
    // Nine minutes after the latest activity (10:00), not fifty after 9:20.
    looks.look_at(at(69));
    assert_eq!(changed(&looks, board(&cards)), ["during"]);
}

/// What the looks file holds.
fn file(path: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

/// Activity within a sitting is written at most every 30 seconds, to spare
/// the disk; a new sitting is written at once.
#[test]
fn activity_is_written_at_most_every_thirty_seconds() {
    let path = scratch("throttle");
    let looks = Looks::load(Some(path.clone()));
    let last = |path: &Path| file(path)["looks"][0]["last"].clone();
    looks.look_at(at(0));
    let first = last(&path);

    looks.look_at(at(0) + Duration::seconds(29));
    assert_eq!(last(&path), first, "too soon to write again");

    looks.look_at(at(0) + Duration::seconds(30));
    let second = last(&path);
    assert_ne!(second, first, "written after 30 s");

    // A new sitting is recorded whatever the last write was.
    looks.look_at(at(20));
    assert_ne!(last(&path), second);
    assert_eq!(file(&path)["looks"][0]["since"], second);
}

/// The web page and the TUI are two processes on one file: activity in
/// either one counts for both.
#[test]
fn two_processes_share_the_looks() {
    let path = scratch("shared");
    let daemon = Looks::load(Some(path.clone()));
    let tui = Looks::load(Some(path));
    daemon.look_at(at(0));
    tui.look_at(at(55));
    let cards = [("early", at(40)), ("late", at(58))];

    // The daemon's last activity was 9:00, but the TUI was used at 9:55:
    // this is the same sitting, and it started at the first look.
    daemon.look_at(at(60));

    assert_eq!(changed(&daemon, board(&cards)), ["early", "late"]);
    // Away from 10:00 to 11:00, counted from the latest activity of either.
    tui.look_at(at(120));
    assert!(changed(&daemon, board(&cards)).is_empty());
}
