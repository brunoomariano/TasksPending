//! The source contract: every source is polled through the same port.

use pending_core::{PendingSource, SampleSource, SourceStatus, sample_snapshot, sandbox_snapshot};

/// The aggregator keeps different sources in one list and asks each for a
/// refresh without knowing which provider it comes from. The sample source
/// honors the contract and delivers cards with a defined section.
#[tokio::test]
async fn sources_are_refreshed_through_a_shared_port() {
    let sources: Vec<Box<dyn PendingSource>> = vec![Box::new(SampleSource)];

    for source in &sources {
        let batch = source.refresh().await.expect("sample never fails");
        assert!(!batch.items.is_empty());
        assert!(batch.items.iter().all(|item| !item.column.is_empty()));
        assert!(batch.warnings.is_empty());
    }
}

/// The sample snapshot served by the API and the TUI is built through the
/// same path as a real source, and the source shows as ready.
#[test]
fn sample_snapshot_is_built_from_the_sample_source() {
    let snapshot = sample_snapshot();

    assert!(!snapshot.boards.is_empty());
    let sample = snapshot
        .sources
        .iter()
        .find(|s| s.name == "sample")
        .expect("sample source health");
    assert_eq!(sample.status, SourceStatus::Ready);
}

/// The documentation sandbox is deterministic, needs no credentials, and
/// shows enough varied cards to exercise the dashboard's normal layout.
#[test]
fn sandbox_snapshot_has_simulated_cards_across_boards() {
    let snapshot = sandbox_snapshot();

    assert_eq!(snapshot.boards.len(), 2);
    assert!(snapshot.boards.iter().any(|board| board.name == "Work"));
    assert!(snapshot.boards.iter().any(|board| board.name == "Personal"));
    assert!(
        snapshot
            .boards
            .iter()
            .flat_map(|board| &board.groups)
            .flat_map(|group| &group.columns)
            .flat_map(|column| &column.cards)
            .any(|card| card.title == "Review the release checklist")
    );
    assert!(
        snapshot
            .sources
            .iter()
            .all(|source| source.status == SourceStatus::Ready)
    );

    let github = snapshot
        .boards
        .iter()
        .flat_map(|board| &board.groups)
        .find(|group| group.source == "GitHub demo")
        .expect("GitHub demo group");
    assert_eq!(
        github.icon.as_ref().map(|icon| icon.url.as_str()),
        Some("https://cdn.jsdelivr.net/gh/homarr-labs/dashboard-icons/svg/github.svg")
    );
    assert_eq!(
        github
            .icon
            .as_ref()
            .and_then(|icon| icon.dark_url.as_deref()),
        Some("https://cdn.jsdelivr.net/gh/homarr-labs/dashboard-icons/svg/github-light.svg")
    );
    assert!(
        snapshot
            .boards
            .iter()
            .flat_map(|board| &board.groups)
            .filter(|group| group.source != "GitHub demo")
            .all(|group| group.icon.is_some())
    );
}

/// The sandbox shows the dashboard as a real setup looks: GitHub's stacks
/// named by next action with review and check state on the cards, issue
/// references as titles for trackers, and a group for a newer provider with
/// its own logo. Card ids are unique, so marking or snoozing one card in
/// the demo never touches another.
#[test]
fn sandbox_shows_the_current_dashboard_features() {
    let snapshot = sandbox_snapshot();
    let group = |name: &str| {
        snapshot
            .boards
            .iter()
            .flat_map(|board| &board.groups)
            .find(|group| group.source == name)
            .unwrap_or_else(|| panic!("{name} group"))
    };

    let github: Vec<&str> = group("GitHub demo")
        .columns
        .iter()
        .map(|column| column.name.as_str())
        .collect();
    assert_eq!(
        github,
        [
            "Review requested",
            "Returned to you",
            "Ready to merge",
            "Not approved yet"
        ]
    );
    let bodies: Vec<&str> = group("GitHub demo")
        .columns
        .iter()
        .flat_map(|column| &column.cards)
        .map(|card| card.body.as_str())
        .collect();
    assert!(bodies.iter().any(|body| body.contains("changes requested")));
    assert!(bodies.iter().any(|body| body.contains("checks failing")));
    assert!(bodies.iter().any(|body| body.contains("checks passing")));

    let linear = group("Linear demo");
    assert_eq!(
        linear.icon.as_ref().map(|icon| icon.url.as_str()),
        Some("https://cdn.jsdelivr.net/gh/homarr-labs/dashboard-icons/svg/linear.svg")
    );
    assert!(linear.columns[0].cards[0].title.starts_with("ENG-"));
    assert!(
        group("Plane demo").columns[0].cards[0]
            .title
            .starts_with("OPS-")
    );

    let mut ids: Vec<&str> = snapshot
        .boards
        .iter()
        .flat_map(|board| &board.groups)
        .flat_map(|group| &group.columns)
        .flat_map(|column| &column.cards)
        .map(|card| card.id.as_str())
        .collect();
    let total = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), total);
    assert!(total >= 14);

    // The cards `scripts/screenshots.mjs` marks, snoozes and opens.
    let ids: Vec<&str> = snapshot
        .boards
        .iter()
        .flat_map(|board| &board.groups)
        .flat_map(|group| &group.columns)
        .flat_map(|column| &column.cards)
        .map(|card| card.id.as_str())
        .collect();
    for id in [
        "github:pull:220",
        "linear:ENG-142",
        "github:review:214",
        "github:review:218",
        "todoist:later:1",
    ] {
        assert!(ids.contains(&id), "the screenshots need the card {id}");
    }
}
