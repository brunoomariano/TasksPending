//! Montagem do snapshot a partir do resultado de cada fonte.

use chrono::{DateTime, TimeZone, Utc};
use pending_core::{
    CardSeverity, PendingCard, SourceBatch, SourceError, SourceItem, SourceOutcome, SourceReport,
    SourceStatus, build_snapshot,
};

fn at(hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 28, hour, 0, 0).unwrap()
}

fn item(section: &str, id: &str, severity: CardSeverity, hour: u32) -> SourceItem {
    SourceItem {
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

fn with_url(mut item: SourceItem, url: &str) -> SourceItem {
    item.card.url = Some(url.to_owned());
    item
}

fn ok(name: &str, lane: &str, items: Vec<SourceItem>) -> SourceReport {
    SourceReport {
        name: name.to_owned(),
        lane: lane.to_owned(),
        outcome: SourceOutcome::Fresh {
            batch: SourceBatch {
                items,
                warnings: Vec::new(),
            },
            refreshed_at: at(11),
        },
    }
}

fn layout(snapshot: &pending_core::DashboardSnapshot) -> Vec<String> {
    snapshot
        .lanes
        .iter()
        .flat_map(|lane| {
            lane.sections.iter().map(move |section| {
                let ids: Vec<&str> = section.cards.iter().map(|c| c.id.as_str()).collect();
                format!("{}/{}: {}", lane.name, section.name, ids.join(","))
            })
        })
        .collect()
}

fn health<'a>(
    snapshot: &'a pending_core::DashboardSnapshot,
    name: &str,
) -> &'a pending_core::SourceHealth {
    snapshot
        .sources
        .iter()
        .find(|s| s.name == name)
        .expect("source health present")
}

/// Cada fonte alimenta a lane que a configuração dela define, e decide a seção
/// de cada card. Lanes e seções aparecem na ordem em que surgiram, para que a
/// ordem das fontes na configuração controle o layout.
#[test]
fn places_cards_in_the_configured_lane_and_source_section() {
    let snapshot = build_snapshot(
        at(12),
        vec![
            ok(
                "github",
                "Work",
                vec![
                    item("Review", "a", CardSeverity::Info, 1),
                    item("Alerts", "c", CardSeverity::Info, 1),
                    item("Review", "d", CardSeverity::Info, 1),
                ],
            ),
            ok(
                "todo",
                "Personal",
                vec![item("Next", "b", CardSeverity::Info, 1)],
            ),
            ok(
                "jira",
                "Work",
                vec![item("Review", "e", CardSeverity::Info, 1)],
            ),
        ],
    );

    assert_eq!(
        layout(&snapshot),
        vec!["Work/Review: a,d,e", "Work/Alerts: c", "Personal/Next: b"]
    );
    assert_eq!(snapshot.generated_at, at(12));
}

/// Dentro de uma seção, o que é mais grave vem primeiro; com a mesma
/// gravidade, o que mudou por último vem primeiro.
#[test]
fn orders_cards_by_severity_then_most_recent_update() {
    let snapshot = build_snapshot(
        at(12),
        vec![ok(
            "github",
            "Work",
            vec![
                item("Review", "old-info", CardSeverity::Info, 1),
                item("Review", "new-info", CardSeverity::Info, 5),
                item("Review", "critical", CardSeverity::Critical, 0),
                item("Review", "warning", CardSeverity::Warning, 3),
            ],
        )],
    );

    assert_eq!(
        layout(&snapshot),
        vec!["Work/Review: critical,warning,new-info,old-info"]
    );
}

/// Uma fonte saudável aparece pronta, com o horário do refresh.
#[test]
fn healthy_source_is_ready() {
    let snapshot = build_snapshot(
        at(12),
        vec![ok(
            "github",
            "Work",
            vec![item("Review", "a", CardSeverity::Info, 1)],
        )],
    );

    let github = health(&snapshot, "github");
    assert_eq!(github.status, SourceStatus::Ready);
    assert_eq!(github.last_refresh_at, Some(at(11)));
    assert_eq!(github.message, None);
}

/// Uma fonte que falha não derruba o dashboard: ela aparece como falha, com o
/// motivo, e as outras fontes continuam visíveis.
#[test]
fn failed_source_is_reported_without_hiding_other_sources() {
    let snapshot = build_snapshot(
        at(12),
        vec![
            SourceReport {
                name: "github".to_owned(),
                lane: "Work".to_owned(),
                outcome: SourceOutcome::Failed(SourceError::new("401 bad credentials")),
            },
            ok(
                "todo",
                "Personal",
                vec![item("Next", "b", CardSeverity::Info, 1)],
            ),
        ],
    );

    assert_eq!(layout(&snapshot), vec!["Personal/Next: b"]);
    let github = health(&snapshot, "github");
    assert_eq!(github.status, SourceStatus::Failed);
    assert_eq!(github.last_refresh_at, None);
    assert_eq!(github.message.as_deref(), Some("401 bad credentials"));
    assert_eq!(health(&snapshot, "todo").status, SourceStatus::Ready);
}

/// Uma fonte que trouxe só parte dos dados (ex.: um repo sem permissão) mostra
/// o que conseguiu e fica degradada com o aviso.
#[test]
fn source_warnings_degrade_the_source_but_keep_its_cards() {
    let mut report = ok(
        "github",
        "Work",
        vec![item("Review", "a", CardSeverity::Info, 1)],
    );
    if let SourceOutcome::Fresh { batch, .. } = &mut report.outcome {
        batch.warnings.push("o/private: 403".to_owned());
    }

    let snapshot = build_snapshot(at(12), vec![report]);

    assert_eq!(layout(&snapshot), vec!["Work/Review: a"]);
    let github = health(&snapshot, "github");
    assert_eq!(github.status, SourceStatus::Degraded);
    assert_eq!(github.message.as_deref(), Some("o/private: 403"));
}

/// Duas fontes que emitem o mesmo id tornariam ações sobre o card ambíguas. O
/// primeiro card fica, o repetido é descartado e a fonte dele fica degradada
/// nomeando o id; o resto do dashboard continua.
#[test]
fn duplicate_card_ids_keep_the_first_and_degrade_the_later_source() {
    let snapshot = build_snapshot(
        at(12),
        vec![
            ok(
                "github",
                "Work",
                vec![item("Review", "dup", CardSeverity::Info, 1)],
            ),
            ok(
                "mirror",
                "Personal",
                vec![
                    item("Next", "dup", CardSeverity::Info, 2),
                    item("Next", "b", CardSeverity::Info, 2),
                ],
            ),
        ],
    );

    assert_eq!(
        layout(&snapshot),
        vec!["Work/Review: dup", "Personal/Next: b"]
    );
    assert_eq!(health(&snapshot, "github").status, SourceStatus::Ready);
    let mirror = health(&snapshot, "mirror");
    assert_eq!(mirror.status, SourceStatus::Degraded);
    assert!(
        mirror.message.as_deref().unwrap_or("").contains("dup"),
        "{mirror:?}"
    );
}

/// O link do card vira `href` no frontend; um esquema como `javascript:` vindo
/// de uma fonte executaria código ao clicar. Esse card é descartado e a fonte
/// fica degradada nomeando o card; links http(s) passam, sem diferenciar
/// maiúsculas no esquema.
#[test]
fn cards_with_non_http_urls_are_dropped_and_degrade_the_source() {
    let snapshot = build_snapshot(
        at(12),
        vec![ok(
            "github",
            "Work",
            vec![
                with_url(
                    item("Review", "good-link", CardSeverity::Info, 1),
                    "HTTPS://github.com/o/r/pull/1",
                ),
                with_url(
                    item("Review", "bad-link", CardSeverity::Info, 1),
                    "javascript:alert(1)",
                ),
            ],
        )],
    );

    assert_eq!(layout(&snapshot), vec!["Work/Review: good-link"]);
    let github = health(&snapshot, "github");
    assert_eq!(github.status, SourceStatus::Degraded);
    assert!(
        github.message.as_deref().unwrap_or("").contains("bad-link"),
        "{github:?}"
    );
}

/// Enquanto a primeira consulta de uma fonte não termina, ela aparece como
/// atualizando, sem cards e sem horário de refresh.
#[test]
fn pending_source_shows_as_refreshing() {
    let snapshot = build_snapshot(
        at(12),
        vec![SourceReport {
            name: "github".to_owned(),
            lane: "Work".to_owned(),
            outcome: SourceOutcome::Pending,
        }],
    );

    assert!(snapshot.lanes.is_empty());
    let github = health(&snapshot, "github");
    assert_eq!(github.status, SourceStatus::Refreshing);
    assert_eq!(github.last_refresh_at, None);
}

/// Quando um refresh falha depois de um sucesso, o dashboard continua mostrando
/// os últimos cards conhecidos; a fonte fica degradada, diz que o dado é antigo
/// e por quê, e o horário é o do último sucesso.
#[test]
fn stale_source_keeps_last_known_cards_and_explains_the_failure() {
    let snapshot = build_snapshot(
        at(12),
        vec![SourceReport {
            name: "github".to_owned(),
            lane: "Work".to_owned(),
            outcome: SourceOutcome::Stale {
                batch: SourceBatch {
                    items: vec![item("Review", "a", CardSeverity::Info, 1)],
                    warnings: vec!["o/private: 403".to_owned()],
                },
                refreshed_at: at(9),
                error: SourceError::new("timed out"),
            },
        }],
    );

    assert_eq!(layout(&snapshot), vec!["Work/Review: a"]);
    let github = health(&snapshot, "github");
    assert_eq!(github.status, SourceStatus::Degraded);
    assert_eq!(github.last_refresh_at, Some(at(9)));
    let message = github.message.as_deref().unwrap_or("");
    assert!(message.contains("timed out"), "{message}");
    assert!(message.contains("stale"), "{message}");
    assert!(message.contains("o/private: 403"), "{message}");
}
