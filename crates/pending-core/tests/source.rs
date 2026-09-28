//! O contrato de fonte: qualquer fonte é consultada pela mesma porta.

use pending_core::{PendingSource, SampleSource, SourceStatus, sample_snapshot};

/// O agregador guarda fontes diferentes numa mesma lista e pede refresh a cada
/// uma sem saber de qual provedor ela vem. A fonte de exemplo cumpre o
/// contrato e entrega cards com seção definida.
#[tokio::test]
async fn sources_are_refreshed_through_a_shared_port() {
    let sources: Vec<Box<dyn PendingSource>> = vec![Box::new(SampleSource)];

    for source in &sources {
        let batch = source.refresh().await.expect("sample never fails");
        assert!(!batch.items.is_empty());
        assert!(batch.items.iter().all(|item| !item.section.is_empty()));
        assert!(batch.warnings.is_empty());
    }
}

/// O snapshot de exemplo servido pela API e pela TUI é montado pelo mesmo
/// caminho de uma fonte real, e a fonte aparece pronta.
#[test]
fn sample_snapshot_is_built_from_the_sample_source() {
    let snapshot = sample_snapshot();

    assert!(!snapshot.lanes.is_empty());
    let sample = snapshot
        .sources
        .iter()
        .find(|s| s.name == "sample")
        .expect("sample source health");
    assert_eq!(sample.status, SourceStatus::Ready);
}
