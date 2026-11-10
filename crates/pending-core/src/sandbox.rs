//! Deterministic simulated dashboard data for screenshots and local demos.

use chrono::{DateTime, TimeZone, Utc};

use crate::config::SourceKind;
use crate::model::{CardSeverity, DashboardSnapshot, PendingCard};
use crate::snapshot::{SourceOutcome, SourceReport, build_snapshot};
use crate::source::{SourceBatch, SourceError, SourceItem};

/// A credential-free snapshot with representative cards across two boards:
/// every kind of source, a stack left empty, a source whose token expired
/// (its cards stay, under a warning), and a few cards changed since the
/// demo's last look.
///
/// This is intentionally separate from [`crate::sample_snapshot`]: the sample
/// source remains the small fallback shown to new installations, while this
/// snapshot exists only for the explicit `tasks-pending sandbox` command.
pub fn sandbox_snapshot() -> DashboardSnapshot {
    let generated_at = at(11, 30);
    build_snapshot(
        generated_at,
        vec![
            report(
                "GitHub demo",
                SourceKind::Github,
                "Work",
                &[
                    "Review requested",
                    "Returned to you",
                    "Ready to merge",
                    "Not approved yet",
                    "Drafts",
                ],
                vec![
                    link(
                        "Review requested",
                        "github:review:214",
                        "Review the release checklist",
                        "acme/dashboard#214 · @mara · review required · checks passing · 2 comments",
                        "https://example.com/acme/dashboard/pull/214",
                        CardSeverity::Warning,
                        11,
                    ),
                    link(
                        "Review requested",
                        "github:review:218",
                        "Confirm the dashboard empty state",
                        "acme/dashboard#218 · @leo · review required · checks pending",
                        "https://example.com/acme/dashboard/pull/218",
                        CardSeverity::Warning,
                        9,
                    ),
                    link(
                        "Review requested",
                        "github:review:231",
                        "Rotate the release signing key",
                        "acme/infra#231 · @sofia · review required · checks passing · 1 comment",
                        "https://example.com/acme/infra/pull/231",
                        CardSeverity::Warning,
                        11,
                    ),
                    link(
                        "Returned to you",
                        "github:pull:220",
                        "Make the installer explain missing tokens",
                        "acme/installer#220 · @you · changes requested · checks failing · 5 comments",
                        "https://example.com/acme/installer/pull/220",
                        CardSeverity::Warning,
                        11,
                    ),
                    link(
                        "Returned to you",
                        "github:pull:226",
                        "Explain the sandbox in the README",
                        "acme/docs#226 · @you · changes requested · checks passing · 3 comments",
                        "https://example.com/acme/docs/pull/226",
                        CardSeverity::Info,
                        10,
                    ),
                    link(
                        "Ready to merge",
                        "github:pull:221",
                        "Cache source icons between restarts",
                        "acme/dashboard#221 · @you · approved · checks passing",
                        "https://example.com/acme/dashboard/pull/221",
                        CardSeverity::Info,
                        10,
                    ),
                    link(
                        "Not approved yet",
                        "github:pull:223",
                        "Document the sandbox command",
                        "acme/docs#223 · @you · review required · checks passing",
                        "https://example.com/acme/docs/pull/223",
                        CardSeverity::Info,
                        8,
                    ),
                    link(
                        "Not approved yet",
                        "github:pull:229",
                        "Pin the GTK version in CI",
                        "acme/dashboard#229 · @you · review required · checks failing",
                        "https://example.com/acme/dashboard/pull/229",
                        CardSeverity::Warning,
                        9,
                    ),
                    link(
                        "Drafts",
                        "github:draft:233",
                        "Sketch the weekly digest",
                        "acme/dashboard#233 · @you · draft · checks passing",
                        "https://example.com/acme/dashboard/pull/233",
                        CardSeverity::Info,
                        7,
                    ),
                ],
            ),
            report(
                "GitLab demo",
                SourceKind::Gitlab,
                "Work",
                &["Merge requests", "Issues", "To-dos"],
                vec![
                    link(
                        "Merge requests",
                        "gitlab:mr:91",
                        "Retry transient 502s from the registry",
                        "platform/ingest!91 · @nina · review requested · pipeline running",
                        "https://example.com/platform/ingest/-/merge_requests/91",
                        CardSeverity::Warning,
                        11,
                    ),
                    link(
                        "Merge requests",
                        "gitlab:mr:88",
                        "Split the ingest job by source",
                        "platform/ingest!88 · @you · pipeline passed · 2 approvals needed",
                        "https://example.com/platform/ingest/-/merge_requests/88",
                        CardSeverity::Info,
                        10,
                    ),
                    link(
                        "Issues",
                        "gitlab:issue:412",
                        "Importer stalls on empty feeds",
                        "platform/ingest#412 · bug · assigned to you · due 2026-10-03",
                        "https://example.com/platform/ingest/-/issues/412",
                        CardSeverity::Warning,
                        11,
                    ),
                    link(
                        "To-dos",
                        "gitlab:todo:7",
                        "You were mentioned in platform/ingest!88",
                        "@nina: can you confirm the retry budget before Friday?",
                        "https://example.com/platform/ingest/-/merge_requests/88#note_7",
                        CardSeverity::Info,
                        9,
                    ),
                ],
            ),
            stale_report(
                "Jira demo",
                SourceKind::Jira,
                "Work",
                &["Sprint", "Waiting on me"],
                vec![
                    link(
                        "Sprint",
                        "jira:DASH-301",
                        "DASH-301",
                        "Add the Jira source to the installer checklist · In Progress · 3 points · due 2026-10-02",
                        "https://example.com/jira/browse/DASH-301",
                        CardSeverity::Warning,
                        9,
                    ),
                    link(
                        "Sprint",
                        "jira:DASH-305",
                        "DASH-305",
                        "Audit the token scopes we document · To Do · 2 points",
                        "https://example.com/jira/browse/DASH-305",
                        CardSeverity::Info,
                        8,
                    ),
                    link(
                        "Waiting on me",
                        "jira:DASH-298",
                        "DASH-298",
                        "Clarify the stale-data notice copy · In Review · reporter @mara",
                        "https://example.com/jira/browse/DASH-298",
                        CardSeverity::Info,
                        9,
                    ),
                ],
                "401 Unauthorized: the token expired, set a new one in the env file",
            ),
            report(
                "Linear demo",
                SourceKind::Linear,
                "Work",
                &["In progress", "Blocked", "To do"],
                vec![
                    link(
                        "In progress",
                        "linear:ENG-142",
                        "ENG-142",
                        "Rate limit the export endpoint · Platform · In Progress · due 2026-10-02",
                        "https://example.com/linear/ENG-142",
                        CardSeverity::Warning,
                        10,
                    ),
                    link(
                        "Blocked",
                        "linear:ENG-147",
                        "ENG-147",
                        "Migrate the export queue to the new broker · Platform · Blocked · waiting on infra",
                        "https://example.com/linear/ENG-147",
                        CardSeverity::Warning,
                        9,
                    ),
                    link(
                        "To do",
                        "linear:ENG-151",
                        "ENG-151",
                        "Add a health page for the importer · Platform · Todo",
                        "https://example.com/linear/ENG-151",
                        CardSeverity::Info,
                        8,
                    ),
                    link(
                        "To do",
                        "linear:ENG-155",
                        "ENG-155",
                        "Trim the onboarding checklist · Growth · Todo",
                        "https://example.com/linear/ENG-155",
                        CardSeverity::Info,
                        9,
                    ),
                    link(
                        "To do",
                        "linear:ENG-160",
                        "ENG-160",
                        "Backfill missing avatars in the digest · Growth · Todo · due 2026-10-07",
                        "https://example.com/linear/ENG-160",
                        CardSeverity::Info,
                        11,
                    ),
                ],
            ),
            report(
                "Plane demo",
                SourceKind::Plane,
                "Work",
                &["In review", "Assigned to me"],
                vec![
                    link(
                        "In review",
                        "plane:OPS-52",
                        "OPS-52",
                        "Map the next dashboard source · Operations · In Review · @mara",
                        "https://example.com/plane/ops/OPS-52",
                        CardSeverity::Info,
                        11,
                    ),
                    link(
                        "Assigned to me",
                        "plane:OPS-38",
                        "OPS-38",
                        "Prepare the workspace access review · Operations · In Progress · due 2026-10-01",
                        "https://example.com/plane/ops/OPS-38",
                        CardSeverity::Warning,
                        8,
                    ),
                    link(
                        "Assigned to me",
                        "plane:OPS-44",
                        "OPS-44",
                        "Document the backup handoff · Operations · Todo",
                        "https://example.com/plane/ops/OPS-44",
                        CardSeverity::Info,
                        9,
                    ),
                    link(
                        "Assigned to me",
                        "plane:OPS-57",
                        "OPS-57",
                        "Rotate the backup encryption key · Operations · Todo · due 2026-10-10",
                        "https://example.com/plane/ops/OPS-57",
                        CardSeverity::Info,
                        11,
                    ),
                ],
            ),
            report(
                "Todoist demo",
                SourceKind::Todoist,
                "Personal",
                &["Overdue", "Today", "Later"],
                vec![
                    item(
                        "Today",
                        "todoist:today:1",
                        "Send the design review notes",
                        "Today - 14:00 - shared with the dashboard team",
                        CardSeverity::Warning,
                        10,
                    ),
                    item(
                        "Today",
                        "todoist:today:2",
                        "Pick a screenshot for the README",
                        "Today - keep the examples clearly simulated",
                        CardSeverity::Info,
                        11,
                    ),
                    item(
                        "Today",
                        "todoist:today:3",
                        "Renew the passport appointment",
                        "Today - 17:30 - bring the old one",
                        CardSeverity::Warning,
                        11,
                    ),
                    item(
                        "Later",
                        "todoist:later:1",
                        "Organize the next release notes",
                        "Friday - release process",
                        CardSeverity::Info,
                        8,
                    ),
                    item(
                        "Later",
                        "todoist:later:2",
                        "Book the dentist",
                        "Next week - any morning",
                        CardSeverity::Info,
                        7,
                    ),
                ],
            ),
            report(
                "Calendar demo",
                SourceKind::Google,
                "Personal",
                &["Today", "Upcoming"],
                vec![
                    event(
                        "Today",
                        "calendar:today:1",
                        "Project sync",
                        "Today - 15:30-16:00 - video call",
                        at(15, 30),
                        CardSeverity::Warning,
                        10,
                    ),
                    event(
                        "Today",
                        "calendar:today:2",
                        "Focus time",
                        "Today - 16:30-17:30 - protect this block",
                        at(16, 30),
                        CardSeverity::Info,
                        9,
                    ),
                    event(
                        "Upcoming",
                        "calendar:upcoming:1",
                        "Weekly planning",
                        "Tomorrow - 10:00-10:45 - planning room",
                        on(1, 10, 0),
                        CardSeverity::Info,
                        8,
                    ),
                    event(
                        "Upcoming",
                        "calendar:upcoming:2",
                        "Dentist",
                        "Thursday - 09:00-09:40 - clinic downtown",
                        on(8, 9, 0),
                        CardSeverity::Info,
                        8,
                    ),
                ],
            ),
            report(
                "iCal demo",
                SourceKind::Ical,
                "Personal",
                &["This week", "Next month"],
                vec![
                    event(
                        "This week",
                        "ical:week:1",
                        "Team offsite",
                        "Friday - 09:00-18:00 - riverside venue",
                        on(2, 9, 0),
                        CardSeverity::Info,
                        10,
                    ),
                    event(
                        "This week",
                        "ical:week:2",
                        "Mara's birthday",
                        "Saturday - all day - send a message",
                        on(3, 0, 0),
                        CardSeverity::Info,
                        6,
                    ),
                    event(
                        "Next month",
                        "ical:month:1",
                        "Hackathon",
                        "24 Oct - 09:00-20:00 - registration closed",
                        on(24, 9, 0),
                        CardSeverity::Info,
                        5,
                    ),
                ],
            ),
        ],
    )
}

fn report(
    name: &str,
    kind: SourceKind,
    board: &str,
    columns: &[&str],
    items: Vec<SourceItem>,
) -> SourceReport {
    let refreshed_at = at(11, 28);
    SourceReport {
        name: name.to_owned(),
        board: board.to_owned(),
        columns: columns.iter().map(|column| (*column).to_owned()).collect(),
        icon: kind.default_icon(),
        sorts: Default::default(),
        outcome: SourceOutcome::Fresh {
            batch: SourceBatch {
                items,
                warnings: Vec::new(),
            },
            refreshed_at,
        },
    }
}

/// A source whose last refresh failed: its cards are the ones from the
/// refresh before, shown under a warning, and Settings says why.
fn stale_report(
    name: &str,
    kind: SourceKind,
    board: &str,
    columns: &[&str],
    items: Vec<SourceItem>,
    error: &str,
) -> SourceReport {
    let mut report = report(name, kind, board, columns, items);
    let SourceOutcome::Fresh { batch, .. } = report.outcome else {
        unreachable!("a report is fresh until made stale");
    };
    report.outcome = SourceOutcome::Stale {
        batch,
        refreshed_at: at(9, 40),
        error: SourceError::new(error),
    };
    report
}

fn item(
    column: &str,
    id: &str,
    title: &str,
    body: &str,
    severity: CardSeverity,
    updated_hour: u32,
) -> SourceItem {
    SourceItem {
        column: column.to_owned(),
        card: PendingCard {
            id: id.to_owned(),
            title: title.to_owned(),
            body: body.to_owned(),
            source: "sandbox".to_owned(),
            url: None,
            due_at: None,
            severity,
            updated_at: at(updated_hour, 0),
        },
    }
}

/// A card that links somewhere, as the provider's cards do.
fn link(
    column: &str,
    id: &str,
    title: &str,
    body: &str,
    url: &str,
    severity: CardSeverity,
    updated_hour: u32,
) -> SourceItem {
    let mut item = item(column, id, title, body, severity, updated_hour);
    item.card.url = Some(url.to_owned());
    item
}

/// A card with a start time, as the calendars' cards have.
fn event(
    column: &str,
    id: &str,
    title: &str,
    body: &str,
    starts_at: DateTime<Utc>,
    severity: CardSeverity,
    updated_hour: u32,
) -> SourceItem {
    let mut item = item(column, id, title, body, severity, updated_hour);
    item.card.due_at = Some(starts_at);
    item
}

/// The demo's day, 30 September 2026, at `hour:minute` UTC.
fn at(hour: u32, minute: u32) -> DateTime<Utc> {
    on(0, hour, minute)
}

/// `days` after the demo's day, at `hour:minute` UTC.
fn on(days: i64, hour: u32, minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 30, hour, minute, 0)
        .single()
        .expect("fixed sandbox time")
        + chrono::Duration::days(days)
}
