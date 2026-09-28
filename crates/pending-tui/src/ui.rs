//! Rendering of the dashboard state.

use pending_core::{CardSeverity, SourceHealth, SourceStatus};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, List, ListItem, ListState, Paragraph, StatefulWidget, Widget,
};

use crate::app::App;

const TIMESTAMP_FORMAT: &str = "%Y-%m-%d %H:%M UTC";
const HELP: &str = "j/k move · enter open · r refresh · q quit";

/// `header` describes where the config came from.
pub fn render(area: Rect, buf: &mut Buffer, app: &App, header: &str) {
    let snapshot = app.snapshot();
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(snapshot.sources.len().max(1) as u16 + 2),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(area);

    let title = Line::from(vec![
        Span::styled(
            "TasksPending",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(
            "  {}  ",
            snapshot.generated_at.format(TIMESTAMP_FORMAT)
        )),
        Span::styled(header, Style::default().fg(Color::DarkGray)),
    ]);
    Paragraph::new(title)
        .block(Block::default().borders(Borders::ALL))
        .render(layout[0], buf);

    // One line per source, clipped, so a long failure message never pushes
    // the other sources out of the panel.
    let sources: Vec<Line> = if snapshot.sources.is_empty() {
        vec![Line::from(Span::styled(
            "No sources configured: add [[sources]] to the config file.",
            Style::default().fg(Color::Yellow),
        ))]
    } else {
        snapshot.sources.iter().map(source_line).collect()
    };
    Paragraph::new(sources)
        .block(Block::default().title("Sources").borders(Borders::ALL))
        .render(layout[1], buf);

    render_lanes(layout[2], buf, app);

    let footer = match app.notice() {
        Some(notice) => Span::styled(notice, Style::default().fg(Color::Red)),
        None => Span::styled(HELP, Style::default().fg(Color::DarkGray)),
    };
    Paragraph::new(footer).render(layout[3], buf);
}

fn source_line(source: &SourceHealth) -> Line<'_> {
    let (label, color) = match source.status {
        SourceStatus::Ready => ("ready", Color::Green),
        SourceStatus::Refreshing => ("refreshing", Color::Blue),
        SourceStatus::Degraded => ("degraded", Color::Yellow),
        SourceStatus::Failed => ("failed", Color::Red),
    };
    let mut spans = vec![
        Span::raw(format!("{} ", source.name)),
        Span::styled(label, Style::default().fg(color)),
    ];
    if let Some(message) = &source.message {
        spans.push(Span::raw(format!(": {message}")));
    }
    if let Some(at) = source.last_refresh_at {
        spans.push(Span::styled(
            format!("  ({})", at.format(TIMESTAMP_FORMAT)),
            Style::default().fg(Color::DarkGray),
        ));
    }
    Line::from(spans)
}

fn render_lanes(area: Rect, buf: &mut Buffer, app: &App) {
    let snapshot = app.snapshot();
    if snapshot.lanes.is_empty() {
        Paragraph::new("No pending cards.")
            .block(Block::default().borders(Borders::ALL))
            .render(area, buf);
        return;
    }

    let lane_count = snapshot.lanes.len();
    let lanes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(vec![Constraint::Ratio(1, lane_count as u32); lane_count])
        .split(area);

    for (lane, area) in snapshot.lanes.iter().zip(lanes.iter()) {
        let mut items = Vec::new();
        let mut state = ListState::default();
        for section in &lane.sections {
            items.push(ListItem::new(Line::from(Span::styled(
                section.name.as_str(),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ))));

            for card in &section.cards {
                let marker = match card.severity {
                    CardSeverity::Info => Style::default().fg(Color::Blue),
                    CardSeverity::Warning => Style::default().fg(Color::Yellow),
                    CardSeverity::Critical => Style::default().fg(Color::Red),
                };
                let title_style = if app.is_selected(card) {
                    // Scrolls the lane so the selected card stays visible.
                    state.select(Some(items.len()));
                    Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                items.push(ListItem::new(vec![
                    Line::from(vec![
                        Span::styled("▍ ", marker),
                        Span::styled(card.title.as_str(), title_style),
                    ]),
                    Line::from(Span::styled(
                        format!(
                            "  {} · {}",
                            card.body,
                            card.updated_at.format(TIMESTAMP_FORMAT)
                        ),
                        Style::default().fg(Color::DarkGray),
                    )),
                ]));
            }
        }

        let list = List::new(items).block(
            Block::default()
                .title(lane.name.as_str())
                .borders(Borders::ALL),
        );
        StatefulWidget::render(list, *area, buf, &mut state);
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use pending_core::{
        CardSeverity, PendingCard, SourceBatch, SourceError, SourceItem, SourceOutcome,
        SourceReport, build_snapshot,
    };
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    use super::*;
    use crate::app::App;

    fn text(app: &App, header: &str) -> String {
        let area = Rect::new(0, 0, 100, 20);
        let mut buf = Buffer::empty(area);
        render(area, &mut buf, app, header);
        (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn app() -> App {
        let at = Utc.with_ymd_and_hms(2026, 9, 28, 12, 0, 0).unwrap();
        App::new(build_snapshot(
            at,
            vec![
                SourceReport {
                    name: "github".to_owned(),
                    lane: "Work".to_owned(),
                    outcome: SourceOutcome::Fresh {
                        batch: SourceBatch {
                            items: vec![SourceItem {
                                section: "Review requested".to_owned(),
                                card: PendingCard {
                                    id: "github:o/api#7".to_owned(),
                                    title: "Add cache".to_owned(),
                                    body: "o/api#7 · @octocat".to_owned(),
                                    source: "github".to_owned(),
                                    url: None,
                                    severity: CardSeverity::Warning,
                                    updated_at: at,
                                },
                            }],
                            warnings: Vec::new(),
                        },
                        refreshed_at: at,
                    },
                },
                SourceReport {
                    name: "jira".to_owned(),
                    lane: "Work".to_owned(),
                    outcome: SourceOutcome::Failed(SourceError::new("401 bad credentials")),
                },
                SourceReport {
                    name: "slow".to_owned(),
                    lane: "Work".to_owned(),
                    outcome: SourceOutcome::Pending,
                },
            ],
        ))
    }

    /// A tela mostra lanes, seções e cards, e a saúde de cada fonte com o
    /// motivo das falhas, para o usuário saber o que não está sendo mostrado.
    #[test]
    fn dashboard_shows_cards_and_source_health() {
        let screen = text(&app(), "config: ~/.config/tasks-pending/config.toml");

        for expected in [
            "Work",
            "Review requested",
            "Add cache",
            "o/api#7",
            "github ready",
            "jira failed: 401 bad credentials",
            "slow refreshing",
            "config: ~/.config/tasks-pending/config.toml",
            "r refresh",
        ] {
            assert!(
                screen.contains(expected),
                "missing {expected:?} in\n{screen}"
            );
        }
    }

    /// Quando uma ação falha (ex.: abrir o link sem navegador disponível), o
    /// rodapé mostra o motivo no lugar da ajuda.
    #[test]
    fn footer_shows_the_last_notice() {
        let mut app = app();
        app.set_notice("could not open link: xdg-open not found");

        let screen = text(&app, "");

        assert!(
            screen.contains("could not open link: xdg-open not found"),
            "{screen}"
        );
    }

    fn screen(app: &App, width: u16, height: u16) -> String {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        render(area, &mut buf, app, "");
        (0..height)
            .map(|y| (0..width).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn many_cards(count: usize) -> App {
        let at = Utc.with_ymd_and_hms(2026, 9, 28, 12, 0, 0).unwrap();
        let items = (0..count)
            .map(|i| SourceItem {
                section: "Review".to_owned(),
                card: PendingCard {
                    id: format!("card-{i}"),
                    title: format!("Card number {i}"),
                    body: String::new(),
                    source: "github".to_owned(),
                    url: None,
                    severity: CardSeverity::Info,
                    updated_at: at,
                },
            })
            .collect();
        App::new(build_snapshot(
            at,
            vec![SourceReport {
                name: "github".to_owned(),
                lane: "Work".to_owned(),
                outcome: SourceOutcome::Fresh {
                    batch: SourceBatch {
                        items,
                        warnings: Vec::new(),
                    },
                    refreshed_at: at,
                },
            }],
        ))
    }

    /// Com mais cards do que cabem na tela, a lane rola para manter o card
    /// selecionado visível; o Enter nunca abre algo que o usuário não vê.
    #[test]
    fn the_selected_card_stays_visible_in_long_lanes() {
        let mut app = many_cards(30);
        for _ in 0..29 {
            app.handle_key(crossterm::event::KeyEvent::from(
                crossterm::event::KeyCode::Char('j'),
            ));
        }

        let screen = screen(&app, 80, 20);

        assert!(screen.contains("Card number 29"), "{screen}");
        assert!(!screen.contains("Card number 0 "), "{screen}");
    }

    /// Uma mensagem de falha longa não empurra as outras fontes para fora do
    /// painel: cada fonte ocupa uma linha.
    #[test]
    fn long_source_messages_do_not_hide_other_sources() {
        let at = Utc.with_ymd_and_hms(2026, 9, 28, 12, 0, 0).unwrap();
        let failed = |name: &str, message: &str| SourceReport {
            name: name.to_owned(),
            lane: "Work".to_owned(),
            outcome: SourceOutcome::Failed(SourceError::new(message)),
        };
        let app = App::new(build_snapshot(
            at,
            vec![
                failed("first", &"very long failure reason ".repeat(10)),
                failed("second", "401"),
                failed("third", "503"),
            ],
        ));

        let screen = screen(&app, 80, 20);

        for name in ["first", "second failed: 401", "third failed: 503"] {
            assert!(screen.contains(name), "missing {name:?} in\n{screen}");
        }
    }

    /// Sem nenhuma fonte habilitada, a tela diz isso em vez de parecer que não
    /// há pendências.
    #[test]
    fn no_sources_is_explained() {
        let at = Utc.with_ymd_and_hms(2026, 9, 28, 12, 0, 0).unwrap();
        let app = App::new(build_snapshot(at, Vec::new()));

        let screen = screen(&app, 80, 20);

        assert!(screen.contains("No sources configured"), "{screen}");
    }
}
