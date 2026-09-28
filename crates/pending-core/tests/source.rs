//! The source contract: every source is polled through the same port.

use pending_core::{PendingSource, SampleSource, SourceStatus, sample_snapshot};

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
