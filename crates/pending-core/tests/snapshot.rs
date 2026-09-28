//! Montagem do dashboard: áreas (abas) → grupos por fonte → colunas → cards.

use chrono::{DateTime, TimeZone, Utc};
use pending_core::{
    CardSeverity, DashboardSnapshot, PendingCard, SourceBatch, SourceError, SourceHealth,
    SourceItem, SourceOutcome, SourceReport, SourceStatus, build_snapshot,
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
        outcome,
    }
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

/// `Board/grupo/coluna: ids` para cada coluna, na ordem da tela.
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

/// Cada fonte vira um grupo de colunas na área (aba) configurada; as áreas e
/// os grupos seguem a ordem da configuração, e as colunas declaradas aparecem
/// na ordem declarada, mesmo vazias.
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

/// Uma coluna que a fonte usa sem ter declarado é acrescentada no fim do grupo.
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

/// Dentro de uma coluna, o mais grave vem primeiro; depois, o que tem data,
/// do mais próximo ao mais distante; por fim, o mais recentemente atualizado.
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

/// Colunas são filtros independentes: o mesmo item pode aparecer em várias
/// colunas e em fontes diferentes; repetido dentro da mesma coluna, fica só
/// uma vez e a fonte avisa.
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

/// Uma fonte saudável aparece pronta, com o horário do refresh.
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

/// Uma fonte que falha não derruba o dashboard: aparece como falha, com o
/// motivo, com as colunas vazias; as outras fontes continuam visíveis.
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

/// Avisos da fonte (dados parciais) deixam a fonte degradada, com os cards.
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

/// Links que não são http(s) nunca chegam à tela: o card é descartado e a
/// fonte fica degradada nomeando-o.
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

/// Enquanto a primeira consulta não termina, a fonte aparece como atualizando,
/// com as colunas vazias.
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

/// Falha depois de um sucesso mantém os últimos cards, avisando que o dado é
/// antigo e por quê, com o horário do último sucesso.
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

/// Na subida, a fonte com dados da execução anterior mostra esses cards e fica
/// como atualizando até a primeira consulta.
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
