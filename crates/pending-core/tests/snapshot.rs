//! Dashboard assembly: areas (tabs) → groups per source → columns → cards.

use chrono::{DateTime, TimeZone, Utc};
use pending_core::{
    CardSeverity, DashboardSnapshot, Icon, PendingCard, SourceBatch, SourceError, SourceHealth,
    SourceItem, SourceOutcome, SourceReport, SourceStatus, StackSort, build_snapshot,
};

fn at(hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 28, hour, 0, 0).unwrap()
}

fn item(column: &str, id: &str, severity: CardSeverity, hour: u32) -> SourceItem {
    SourceItem {
        column: column.to_owned(),
        card: PendingCard {
            id: id.to_owned(),
            title: id.to_owned(),
            body: String::new(),
            source: "test".to_owned(),
            url: None,
            due_at: None,
            severity,
            updated_at: at(hour),
        },
    }
}

fn with_url(mut item: SourceItem, url: &str) -> SourceItem {
    item.card.url = Some(url.to_owned());
    item
}

fn report(name: &str, board: &str, columns: &[&str], outcome: SourceOutcome) -> SourceReport {
    SourceReport {
        name: name.to_owned(),
        board: board.to_owned(),
        columns: columns.iter().map(|c| (*c).to_owned()).collect(),
        icon: None,
        sorts: Default::default(),
        outcome,
    }
}

/// A configured icon travels with the source's group, whatever its state.
#[test]
fn groups_carry_their_source_icon() {
    let icon = Icon {
        url: "https://cdn.example/plane.svg".to_owned(),
        dark_url: None,
    };
    let mut ready = report("plane", "Work", &["Mine"], fresh(Vec::new()));
    ready.icon = Some(icon.clone());
    let mut failed = report(
        "todoist",
        "Work",
        &["Today"],
        SourceOutcome::Failed(SourceError::new("401")),
    );
    failed.icon = Some(icon.clone());

    let snapshot = build_snapshot(at(12), vec![ready, failed]);

    let groups = &snapshot.boards[0].groups;
    assert_eq!(groups[0].icon, Some(icon.clone()));
    assert_eq!(groups[1].icon, Some(icon));
}

fn fresh(items: Vec<SourceItem>) -> SourceOutcome {
    SourceOutcome::Fresh {
        batch: SourceBatch {
            items,
            warnings: Vec::new(),
        },
        refreshed_at: at(11),
    }
}

/// `Board/group/column: ids` for each column, in screen order.
fn layout(snapshot: &DashboardSnapshot) -> Vec<String> {
    snapshot
        .boards
        .iter()
        .flat_map(|board| {
            board.groups.iter().flat_map(move |group| {
                group.columns.iter().map(move |column| {
                    let ids: Vec<&str> = column.cards.iter().map(|c| c.id.as_str()).collect();
                    format!(
                        "{}/{}/{}: {}",
                        board.name,
                        group.source,
                        column.name,
                        ids.join(",")
                    )
                })
            })
        })
        .collect()
}

fn health<'a>(snapshot: &'a DashboardSnapshot, name: &str) -> &'a SourceHealth {
    snapshot
        .sources
        .iter()
        .find(|s| s.name == name)
        .expect("source health present")
}

/// Each source becomes a group of columns in the configured area (tab);
/// areas and groups follow config order, and declared columns appear in
/// declared order, even when empty.
#[test]
fn sources_become_column_groups_inside_their_board() {
    let snapshot = build_snapshot(
        at(12),
        vec![
            report(
                "plane",
                "Work",
                &["Mine", "Inbox"],
                fresh(vec![item("Mine", "p1", CardSeverity::Info, 1)]),
            ),
            report(
                "todoist",
                "Personal",
                &["Today"],
                fresh(vec![item("Today", "t1", CardSeverity::Info, 1)]),
            ),
            report(
                "github",
                "Work",
                &["Review"],
                fresh(vec![item("Review", "g1", CardSeverity::Info, 1)]),
            ),
        ],
    );

    assert_eq!(
        layout(&snapshot),
        vec![
            "Work/plane/Mine: p1",
            "Work/plane/Inbox: ",
            "Work/github/Review: g1",
            "Personal/todoist/Today: t1",
        ]
    );
    assert_eq!(snapshot.generated_at, at(12));
}

/// A column the source uses without declaring it is appended to the group.
#[test]
fn undeclared_columns_are_appended() {
    let snapshot = build_snapshot(
        at(12),
        vec![report(
            "github",
            "Work",
            &["Review"],
            fresh(vec![item("Extra", "g1", CardSeverity::Info, 1)]),
        )],
    );

    assert_eq!(
        layout(&snapshot),
        vec!["Work/github/Review: ", "Work/github/Extra: g1"]
    );
}

/// Within a column, the most severe comes first; then items with a date,
/// soonest first; finally, the most recently updated.
#[test]
fn cards_are_ordered_by_severity_due_time_and_recency() {
    let due = |mut item: SourceItem, hour: u32| {
        item.card.due_at = Some(at(hour));
        item
    };
    let snapshot = build_snapshot(
        at(12),
        vec![report(
            "calendar",
            "Work",
            &["Today"],
            fresh(vec![
                item("Today", "old-info", CardSeverity::Info, 1),
                item("Today", "new-info", CardSeverity::Info, 5),
                due(item("Today", "late", CardSeverity::Info, 1), 18),
                due(item("Today", "soon", CardSeverity::Info, 1), 13),
                item("Today", "critical", CardSeverity::Critical, 0),
                due(item("Today", "warning", CardSeverity::Warning, 1), 20),
            ]),
        )],
    );

    assert_eq!(
        layout(&snapshot),
        vec!["Work/calendar/Today: critical,warning,soon,late,new-info,old-info"]
    );
}

/// A stack sorted oldest first puts the least recently updated cards on top,
/// to surface stale work; severity and due time still come first, and other
/// stacks of the same source keep newest first.
#[test]
fn oldest_first_stacks_surface_the_least_recently_updated_cards() {
    let due = |mut item: SourceItem, hour: u32| {
        item.card.due_at = Some(at(hour));
        item
    };
    let cards = |column: &str| {
        vec![
            item(column, "new-info", CardSeverity::Info, 5),
            item(column, "old-info", CardSeverity::Info, 1),
            item(column, "mid-info", CardSeverity::Info, 3),
            due(item(column, "due", CardSeverity::Info, 4), 18),
            item(column, "new-warning", CardSeverity::Warning, 6),
            item(column, "old-warning", CardSeverity::Warning, 2),
        ]
    };
    let mut report = report(
        "github",
        "Work",
        &["Stale", "Recent"],
        fresh([cards("Stale"), cards("Recent")].concat()),
    );
    report.sorts.insert("Stale".to_owned(), StackSort::Oldest);

    let snapshot = build_snapshot(at(12), vec![report]);

    assert_eq!(
        layout(&snapshot),
        vec![
            "Work/github/Stale: old-warning,new-warning,due,old-info,mid-info,new-info",
            "Work/github/Recent: new-warning,old-warning,due,new-info,mid-info,old-info",
        ]
    );
}

/// Columns are independent filters: the same item can appear in several
/// columns and in different sources; repeated within the same column, it is
/// kept once and the source warns.
#[test]
fn an_item_may_appear_in_several_columns_but_once_per_column() {
    let snapshot = build_snapshot(
        at(12),
        vec![
            report(
                "plane",
                "Work",
                &["Mine", "Urgent"],
                fresh(vec![
                    item("Mine", "same", CardSeverity::Info, 1),
                    item("Urgent", "same", CardSeverity::Critical, 1),
                    item("Mine", "same", CardSeverity::Info, 2),
                ]),
            ),
            report(
                "mirror",
                "Work",
                &["Mine"],
                fresh(vec![item("Mine", "same", CardSeverity::Info, 1)]),
            ),
        ],
    );

    assert_eq!(
        layout(&snapshot),
        vec![
            "Work/plane/Mine: same",
            "Work/plane/Urgent: same",
            "Work/mirror/Mine: same",
        ]
    );
    let plane = health(&snapshot, "plane");
    assert_eq!(plane.status, SourceStatus::Degraded);
    assert!(
        plane.message.as_deref().unwrap_or("").contains("same"),
        "{plane:?}"
    );
    assert_eq!(health(&snapshot, "mirror").status, SourceStatus::Ready);
}

/// A healthy source shows as ready, with the refresh time.
#[test]
fn healthy_source_is_ready() {
    let snapshot = build_snapshot(
        at(12),
        vec![report(
            "github",
            "Work",
            &["Review"],
            fresh(vec![item("Review", "a", CardSeverity::Info, 1)]),
        )],
    );

    let github = health(&snapshot, "github");
    assert_eq!(github.status, SourceStatus::Ready);
    assert_eq!(github.last_refresh_at, Some(at(11)));
    assert_eq!(github.message, None);
}

/// A failing source doesn't bring down the dashboard: it shows as failed,
/// with the reason and empty columns; the other sources stay visible.
#[test]
fn failed_source_keeps_empty_columns_and_other_sources() {
    let snapshot = build_snapshot(
        at(12),
        vec![
            report(
                "github",
                "Work",
                &["Review"],
                SourceOutcome::Failed(SourceError::new("401 bad credentials")),
            ),
            report(
                "todoist",
                "Personal",
                &["Today"],
                fresh(vec![item("Today", "b", CardSeverity::Info, 1)]),
            ),
        ],
    );

    assert_eq!(
        layout(&snapshot),
        vec!["Work/github/Review: ", "Personal/todoist/Today: b"]
    );
    let github = health(&snapshot, "github");
    assert_eq!(github.status, SourceStatus::Failed);
    assert_eq!(github.last_refresh_at, None);
    assert_eq!(github.message.as_deref(), Some("401 bad credentials"));
}

/// Source warnings (partial data) leave the source degraded, with its cards.
#[test]
fn source_warnings_degrade_the_source_but_keep_its_cards() {
    let mut outcome = fresh(vec![item("Review", "a", CardSeverity::Info, 1)]);
    if let SourceOutcome::Fresh { batch, .. } = &mut outcome {
        batch.warnings.push("o/private: 403".to_owned());
    }

    let snapshot = build_snapshot(at(12), vec![report("github", "Work", &["Review"], outcome)]);

    assert_eq!(layout(&snapshot), vec!["Work/github/Review: a"]);
    let github = health(&snapshot, "github");
    assert_eq!(github.status, SourceStatus::Degraded);
    assert_eq!(github.message.as_deref(), Some("o/private: 403"));
}

/// Non-http(s) links never reach the screen: the card is dropped and the
/// source is degraded, naming it.
#[test]
fn cards_with_non_http_urls_are_dropped_and_degrade_the_source() {
    let snapshot = build_snapshot(
        at(12),
        vec![report(
            "github",
            "Work",
            &["Review"],
            fresh(vec![
                with_url(
                    item("Review", "good-link", CardSeverity::Info, 1),
                    "HTTPS://github.com/o/r/pull/1",
                ),
                with_url(
                    item("Review", "bad-link", CardSeverity::Info, 1),
                    "javascript:alert(1)",
                ),
            ]),
        )],
    );

    assert_eq!(layout(&snapshot), vec!["Work/github/Review: good-link"]);
    let github = health(&snapshot, "github");
    assert_eq!(github.status, SourceStatus::Degraded);
    assert!(
        github.message.as_deref().unwrap_or("").contains("bad-link"),
        "{github:?}"
    );
}

/// Until the first poll finishes, the source shows as refreshing, with empty
/// columns.
#[test]
fn pending_source_shows_as_refreshing() {
    let snapshot = build_snapshot(
        at(12),
        vec![report(
            "github",
            "Work",
            &["Review"],
            SourceOutcome::Pending,
        )],
    );

    assert_eq!(layout(&snapshot), vec!["Work/github/Review: "]);
    let github = health(&snapshot, "github");
    assert_eq!(github.status, SourceStatus::Refreshing);
    assert_eq!(github.last_refresh_at, None);
}

/// A failure after a success keeps the last cards, warning that the data is
/// stale and why, with the time of the last success.
#[test]
fn stale_source_keeps_last_known_cards_and_explains_the_failure() {
    let snapshot = build_snapshot(
        at(12),
        vec![report(
            "github",
            "Work",
            &["Review"],
            SourceOutcome::Stale {
                batch: SourceBatch {
                    items: vec![item("Review", "a", CardSeverity::Info, 1)],
                    warnings: vec!["o/private: 403".to_owned()],
                },
                refreshed_at: at(9),
                error: SourceError::new("timed out"),
            },
        )],
    );

    assert_eq!(layout(&snapshot), vec!["Work/github/Review: a"]);
    let github = health(&snapshot, "github");
    assert_eq!(github.status, SourceStatus::Degraded);
    assert_eq!(github.last_refresh_at, Some(at(9)));
    let message = github.message.as_deref().unwrap_or("");
    assert!(message.contains("timed out"), "{message}");
    assert!(message.contains("stale"), "{message}");
    assert!(message.contains("o/private: 403"), "{message}");
}

/// At startup, a source with data from the previous run shows those cards
/// and stays refreshing until the first poll.
#[test]
fn cached_source_shows_previous_cards_while_refreshing() {
    let snapshot = build_snapshot(
        at(12),
        vec![report(
            "github",
            "Work",
            &["Review"],
            SourceOutcome::Cached {
                batch: SourceBatch {
                    items: vec![item("Review", "a", CardSeverity::Info, 1)],
                    warnings: Vec::new(),
                },
                refreshed_at: at(8),
            },
        )],
    );

    assert_eq!(layout(&snapshot), vec!["Work/github/Review: a"]);
    let github = health(&snapshot, "github");
    assert_eq!(github.status, SourceStatus::Refreshing);
    assert_eq!(github.last_refresh_at, Some(at(8)));
    assert!(
        github
            .message
            .as_deref()
            .unwrap_or("")
            .contains("previous run"),
        "{github:?}"
    );
}
