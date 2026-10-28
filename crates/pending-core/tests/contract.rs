//! The snapshot JSON is the contract between the daemon and the window, and
//! a window may read an older daemon. `fixtures/snapshot-0.5.0.json` is what
//! the 0.5.0 daemon's sandbox answered on `GET /api/v1/snapshot`, with one
//! card marked and one snoozed; these tests keep today's types able to read
//! it and still sending every field it had, under the same names.

use chrono::{TimeZone, Utc};
use pending_core::{CardSeverity, DashboardSnapshot, SourceStatus};
use serde_json::Value;

const SNAPSHOT_0_5_0: &str = include_str!("fixtures/snapshot-0.5.0.json");

fn snapshot_0_5_0() -> DashboardSnapshot {
    serde_json::from_str(SNAPSHOT_0_5_0).expect("the 0.5.0 snapshot reads")
}

/// Scenario: the window opens against a 0.5.0 daemon. It reads the boards,
/// the sources, the marked, snoozed and changed cards as that daemon sent
/// them, not empty lists from a field it no longer recognises.
#[test]
fn reads_a_snapshot_from_a_0_5_0_daemon() {
    let snapshot = snapshot_0_5_0();

    assert_eq!(snapshot.version, "0.5.0");
    assert_eq!(
        snapshot.generated_at,
        Utc.with_ymd_and_hms(2026, 9, 30, 11, 30, 0).unwrap()
    );
    let boards: Vec<&str> = snapshot.boards.iter().map(|b| b.name.as_str()).collect();
    assert_eq!(boards, ["Work", "Personal"]);
    let sources: Vec<(&str, SourceStatus)> = snapshot
        .sources
        .iter()
        .map(|s| (s.name.as_str(), s.status))
        .collect();
    assert_eq!(
        sources,
        [
            ("GitHub demo", SourceStatus::Ready),
            ("Linear demo", SourceStatus::Ready),
            ("Plane demo", SourceStatus::Ready),
            ("Todoist demo", SourceStatus::Ready),
            ("Calendar demo", SourceStatus::Ready),
        ]
    );
    assert_eq!(snapshot.marked, ["github:review:214"]);
    assert_eq!(snapshot.snoozed.len(), 1);
    assert_eq!(snapshot.snoozed[0].card.id, "github:pull:221");
    assert_eq!(snapshot.snoozed[0].source, "GitHub demo");
    assert_eq!(
        snapshot.snoozed[0].until,
        Some(Utc.with_ymd_and_hms(2099, 1, 1, 0, 0, 0).unwrap())
    );
    assert_eq!(
        snapshot.changed,
        [
            "github:review:214",
            "github:pull:220",
            "plane:OPS-52",
            "todoist:today:2"
        ]
    );
    let github = &snapshot.boards[0].groups[0];
    assert_eq!(github.source, "GitHub demo");
    assert_eq!(
        github.icon.as_ref().map(|icon| icon.url.as_str()),
        Some("https://cdn.jsdelivr.net/gh/homarr-labs/dashboard-icons/svg/github.svg")
    );
    let severities: Vec<CardSeverity> = snapshot
        .boards
        .iter()
        .flat_map(|board| &board.groups)
        .flat_map(|group| &group.columns)
        .flat_map(|column| &column.cards)
        .map(|card| card.severity)
        .collect();
    assert!(severities.contains(&CardSeverity::Warning));
}

/// Scenario: a 0.6.0 window is left open while a newer daemon starts. The
/// newer daemon still sends every field 0.5.0 sent, under the same name and
/// with the same value; it may add fields, never rename or drop one.
#[test]
fn still_sends_every_0_5_0_field() {
    let sent = serde_json::to_value(snapshot_0_5_0()).unwrap();
    let expected: Value = serde_json::from_str(SNAPSHOT_0_5_0).unwrap();
    assert_contains(&sent, &expected, "snapshot");
}

/// Fails unless every key of `expected`, at every depth, is in `actual`
/// with the same value. Arrays are compared item by item.
fn assert_contains(actual: &Value, expected: &Value, path: &str) {
    match (actual, expected) {
        (Value::Object(actual), Value::Object(expected)) => {
            for (key, value) in expected {
                let inner = format!("{path}.{key}");
                match actual.get(key) {
                    Some(found) => assert_contains(found, value, &inner),
                    None => panic!("{inner} is no longer sent"),
                }
            }
        }
        (Value::Array(actual), Value::Array(expected)) => {
            assert_eq!(actual.len(), expected.len(), "{path} has another length");
            for (index, (found, value)) in actual.iter().zip(expected).enumerate() {
                assert_contains(found, value, &format!("{path}[{index}]"));
            }
        }
        _ => assert_eq!(actual, expected, "{path} changed"),
    }
}
