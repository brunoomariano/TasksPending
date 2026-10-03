//! Deterministic simulated dashboard data for screenshots and local demos.

use chrono::{DateTime, TimeZone, Utc};

use crate::config::SourceKind;
use crate::model::{CardSeverity, DashboardSnapshot, PendingCard};
use crate::snapshot::{SourceOutcome, SourceReport, build_snapshot};
use crate::source::{SourceBatch, SourceItem};

/// A credential-free snapshot with representative cards across two boards.
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
                ],
                vec![
                    item(
                        "Review requested",
                        "github:review:214",
                        "Review the release checklist",
                        "acme/dashboard#214 · @mara · review required · checks passing · 2 comments",
                        CardSeverity::Warning,
                        11,
                    ),
                    item(
                        "Review requested",
                        "github:review:218",
                        "Confirm the dashboard empty state",
                        "acme/dashboard#218 · @leo · review required · checks pending",
                        CardSeverity::Warning,
                        9,
                    ),
                    item(
                        "Returned to you",
                        "github:pull:220",
                        "Make the installer explain missing tokens",
                        "acme/installer#220 · @you · changes requested · checks failing · 5 comments",
                        CardSeverity::Warning,
                        11,
                    ),
                    item(
                        "Ready to merge",
                        "github:pull:221",
                        "Cache source icons between restarts",
                        "acme/dashboard#221 · @you · approved · checks passing",
                        CardSeverity::Info,
                        10,
                    ),
                    item(
                        "Not approved yet",
                        "github:pull:223",
                        "Document the sandbox command",
                        "acme/docs#223 · @you · review required · checks passing",
                        CardSeverity::Info,
                        8,
                    ),
                ],
            ),
            report(
                "Linear demo",
                SourceKind::Linear,
                "Work",
                &["In progress", "To do"],
                vec![
                    item(
                        "In progress",
                        "linear:ENG-142",
                        "ENG-142",
                        "Rate limit the export endpoint · Platform · In Progress · due 2026-10-02",
                        CardSeverity::Warning,
                        10,
                    ),
                    item(
                        "To do",
                        "linear:ENG-151",
                        "ENG-151",
                        "Add a health page for the importer · Platform · Todo",
                        CardSeverity::Info,
                        8,
                    ),
                    item(
                        "To do",
                        "linear:ENG-155",
                        "ENG-155",
                        "Trim the onboarding checklist · Growth · Todo",
                        CardSeverity::Info,
                        9,
                    ),
                ],
            ),
            report(
                "Plane demo",
                SourceKind::Plane,
                "Work",
                &["In review", "Assigned to me"],
                vec![
                    item(
                        "In review",
                        "plane:OPS-52",
                        "OPS-52",
                        "Map the next dashboard source · Operations · In Review · @mara",
                        CardSeverity::Info,
                        11,
                    ),
                    item(
                        "Assigned to me",
                        "plane:OPS-38",
                        "OPS-38",
                        "Prepare the workspace access review · Operations · In Progress · due 2026-10-01",
                        CardSeverity::Warning,
                        8,
                    ),
                    item(
                        "Assigned to me",
                        "plane:OPS-44",
                        "OPS-44",
                        "Document the backup handoff · Operations · Todo",
                        CardSeverity::Info,
                        9,
                    ),
                ],
            ),
            report(
                "Todoist demo",
                SourceKind::Todoist,
                "Personal",
                &["Today", "Later"],
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
                        "Later",
                        "todoist:later:1",
                        "Organize the next release notes",
                        "Friday - release process",
                        CardSeverity::Info,
                        8,
                    ),
                ],
            ),
            report(
                "Calendar demo",
                SourceKind::Google,
                "Personal",
                &["Today", "Upcoming"],
                vec![
                    item(
                        "Today",
                        "calendar:today:1",
                        "Project sync",
                        "Today - 15:30-16:00 - video call",
                        CardSeverity::Warning,
                        10,
                    ),
                    item(
                        "Today",
                        "calendar:today:2",
                        "Focus time",
                        "Today - 16:30-17:30 - protect this block",
                        CardSeverity::Info,
                        9,
                    ),
                    item(
                        "Upcoming",
                        "calendar:upcoming:1",
                        "Weekly planning",
                        "Tomorrow - 10:00-10:45 - planning room",
                        CardSeverity::Info,
                        8,
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

fn at(hour: u32, minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 30, hour, minute, 0)
        .single()
        .expect("fixed sandbox time")
}
