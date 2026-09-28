//! Montagem do snapshot a partir de cards já posicionados em lane e seção.

use chrono::{DateTime, TimeZone, Utc};
use pending_core::{AssembleError, CardSeverity, PendingCard, PlacedCard, assemble};

fn at(hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 28, hour, 0, 0).unwrap()
}

fn placed(lane: &str, section: &str, id: &str, severity: CardSeverity, hour: u32) -> PlacedCard {
    PlacedCard {
        lane: lane.to_owned(),
        section: section.to_owned(),
        card: PendingCard {
            id: id.to_owned(),
            title: id.to_owned(),
            body: String::new(),
            source: "test".to_owned(),
            url: None,
            severity,
            updated_at: at(hour),
        },
    }
}

/// Cards de várias fontes chegam misturados; o dashboard os agrupa por lane e
/// seção, preservando a ordem em que cada lane e cada seção apareceu pela
/// primeira vez, para que a configuração controle o layout.
#[test]
fn groups_cards_by_lane_and_section_in_first_seen_order() {
    let snapshot = assemble(
        at(12),
        [
            placed("Work", "Review", "a", CardSeverity::Info, 1),
            placed("Personal", "Next", "b", CardSeverity::Info, 1),
            placed("Work", "Alerts", "c", CardSeverity::Info, 1),
            placed("Work", "Review", "d", CardSeverity::Info, 1),
        ],
        Vec::new(),
    )
    .expect("unique ids");

    let layout: Vec<String> = snapshot
        .lanes
        .iter()
        .flat_map(|lane| {
            lane.sections.iter().map(move |section| {
                let ids: Vec<&str> = section.cards.iter().map(|c| c.id.as_str()).collect();
                format!("{}/{}: {}", lane.name, section.name, ids.join(","))
            })
        })
        .collect();

    assert_eq!(
        layout,
        vec!["Work/Review: a,d", "Work/Alerts: c", "Personal/Next: b"]
    );
    assert_eq!(snapshot.generated_at, at(12));
}

/// Dentro de uma seção, o que é mais grave vem primeiro; com a mesma
/// gravidade, o que mudou por último vem primeiro.
#[test]
fn orders_cards_by_severity_then_most_recent_update() {
    let snapshot = assemble(
        at(12),
        [
            placed("Work", "Review", "old-info", CardSeverity::Info, 1),
            placed("Work", "Review", "new-info", CardSeverity::Info, 5),
            placed("Work", "Review", "critical", CardSeverity::Critical, 0),
            placed("Work", "Review", "warning", CardSeverity::Warning, 3),
        ],
        Vec::new(),
    )
    .expect("unique ids");

    let ids: Vec<&str> = snapshot.lanes[0].sections[0]
        .cards
        .iter()
        .map(|c| c.id.as_str())
        .collect();
    assert_eq!(ids, vec!["critical", "warning", "new-info", "old-info"]);
}

/// Duas fontes que emitem o mesmo id tornariam ações sobre o card ambíguas;
/// a montagem recusa o snapshot e nomeia o id repetido.
#[test]
fn rejects_duplicate_card_ids() {
    let result = assemble(
        at(12),
        [
            placed("Work", "Review", "dup", CardSeverity::Info, 1),
            placed("Personal", "Next", "dup", CardSeverity::Info, 2),
        ],
        Vec::new(),
    );

    assert_eq!(
        result,
        Err(AssembleError::DuplicateCardId("dup".to_owned()))
    );
}
