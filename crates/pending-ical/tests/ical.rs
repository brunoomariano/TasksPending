//! Eventos do calendário (feed iCal) que ainda não terminaram viram cards.

use chrono::{DateTime, TimeZone, Utc};
use pending_core::{CardSeverity, PendingSource, SourceItem};
use pending_ical::{IcalSource, Window, occurrences};

fn now() -> DateTime<Utc> {
    // Segunda-feira, 28/09/2026, 12:00 UTC.
    Utc.with_ymd_and_hms(2026, 9, 28, 12, 0, 0).unwrap()
}

fn calendar(events: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Google Inc//Google Calendar 70.9054//EN\r\n\
BEGIN:VTIMEZONE\r\nTZID:America/Sao_Paulo\r\nBEGIN:STANDARD\r\nDTSTART:19700101T000000\r\n\
TZOFFSETFROM:-0300\r\nTZOFFSETTO:-0300\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\n{events}END:VCALENDAR\r\n"
    )
}

fn event(uid: &str, lines: &str) -> String {
    format!("BEGIN:VEVENT\r\nUID:{uid}\r\nDTSTAMP:20260901T000000Z\r\n{lines}END:VEVENT\r\n")
}

fn items(ics: &str) -> Vec<SourceItem> {
    occurrences(ics, now(), Window::default(), chrono_tz::UTC, None)
        .expect("valid calendar")
        .items
}

fn summary(items: &[SourceItem]) -> Vec<(String, String)> {
    items
        .iter()
        .map(|item| (item.column.clone(), item.card.title.clone()))
        .collect()
}

/// Um evento de hoje vira card na seção de hoje, com horário, local, início
/// como data do card e um id estável por ocorrência.
#[test]
fn an_event_later_today_becomes_a_card() {
    let ics = calendar(&event(
        "standup@google.com",
        "SUMMARY:Planning\r\nLOCATION:Room 1\r\nDTSTART:20260928T150000Z\r\nDTEND:20260928T160000Z\r\n",
    ));

    let items = items(&ics);

    assert_eq!(
        summary(&items),
        vec![("Today".to_owned(), "Planning".to_owned())]
    );
    let card = &items[0].card;
    assert_eq!(card.id, "ical:standup@google.com:2026-09-28T15:00:00+00:00");
    assert_eq!(card.source, "calendar");
    assert_eq!(
        card.due_at,
        Some(Utc.with_ymd_and_hms(2026, 9, 28, 15, 0, 0).unwrap())
    );
    assert!(card.body.contains("15:00"), "{}", card.body);
    assert!(card.body.contains("16:00"), "{}", card.body);
    assert!(card.body.contains("Room 1"), "{}", card.body);
    assert_eq!(card.severity, CardSeverity::Info);
}

/// Evento em andamento aparece em "Now" como aviso; o que começa em menos de
/// uma hora também é aviso; eventos que já terminaram ficam de fora.
#[test]
fn ongoing_and_imminent_events_are_warnings_and_past_ones_are_hidden() {
    let ics = calendar(
        &[
            event(
                "past",
                "SUMMARY:Past\r\nDTSTART:20260928T080000Z\r\nDTEND:20260928T090000Z\r\n",
            ),
            event(
                "now",
                "SUMMARY:Ongoing\r\nDTSTART:20260928T113000Z\r\nDTEND:20260928T123000Z\r\n",
            ),
            event(
                "soon",
                "SUMMARY:Soon\r\nDTSTART:20260928T124500Z\r\nDTEND:20260928T130000Z\r\n",
            ),
        ]
        .concat(),
    );

    let items = items(&ics);

    assert_eq!(
        summary(&items),
        vec![
            ("Now".to_owned(), "Ongoing".to_owned()),
            ("Today".to_owned(), "Soon".to_owned()),
        ]
    );
    assert!(
        items
            .iter()
            .all(|i| i.card.severity == CardSeverity::Warning)
    );
}

/// Evento de dia inteiro amanhã aparece em "Tomorrow" marcado como dia todo.
#[test]
fn all_day_events_are_marked() {
    let ics = calendar(&event(
        "holiday",
        "SUMMARY:Holiday\r\nDTSTART;VALUE=DATE:20260929\r\nDTEND;VALUE=DATE:20260930\r\n",
    ));

    let items = items(&ics);

    assert_eq!(
        summary(&items),
        vec![("Tomorrow".to_owned(), "Holiday".to_owned())]
    );
    assert!(
        items[0].card.body.contains("all day"),
        "{}",
        items[0].card.body
    );
}

/// Um evento semanal no fuso de São Paulo gera uma ocorrência por semana na
/// janela de 30 dias, sem a data excluída (EXDATE).
#[test]
fn recurring_events_are_expanded_within_the_window() {
    let ics = calendar(&event(
        "weekly",
        "SUMMARY:1:1\r\nDTSTART;TZID=America/Sao_Paulo:20260901T100000\r\n\
DTEND;TZID=America/Sao_Paulo:20260901T103000\r\nRRULE:FREQ=WEEKLY;BYDAY=TU\r\n\
EXDATE;TZID=America/Sao_Paulo:20261006T100000\r\n",
    ));

    let items = items(&ics);
    let starts: Vec<String> = items
        .iter()
        .map(|i| i.card.due_at.unwrap().to_rfc3339())
        .collect();

    assert_eq!(
        starts,
        vec![
            "2026-09-29T13:00:00+00:00",
            "2026-10-13T13:00:00+00:00",
            "2026-10-20T13:00:00+00:00",
            "2026-10-27T13:00:00+00:00",
        ]
    );
    assert_eq!(items[0].column, "Tomorrow");
}

/// A janela para em 25 eventos, mesmo que caibam mais em 30 dias.
#[test]
fn at_most_25_events_are_shown() {
    let ics = calendar(&event(
        "daily",
        "SUMMARY:Daily\r\nDTSTART:20260928T130000Z\r\nDTEND:20260928T131500Z\r\nRRULE:FREQ=DAILY\r\n",
    ));

    let items = items(&ics);

    assert_eq!(items.len(), 25);
}

/// Uma ocorrência movida (RECURRENCE-ID) aparece no novo horário, uma
/// cancelada some, e eventos cancelados não aparecem.
#[test]
fn moved_and_cancelled_occurrences_are_respected() {
    let ics = calendar(&[
        event(
            "series",
            "SUMMARY:Sync\r\nDTSTART:20260929T140000Z\r\nDTEND:20260929T150000Z\r\nRRULE:FREQ=DAILY;COUNT=3\r\n",
        ),
        event(
            "series",
            "SUMMARY:Sync (moved)\r\nRECURRENCE-ID:20260930T140000Z\r\n\
DTSTART:20260930T170000Z\r\nDTEND:20260930T180000Z\r\n",
        ),
        event(
            "series",
            "SUMMARY:Sync\r\nRECURRENCE-ID:20261001T140000Z\r\nSTATUS:CANCELLED\r\n\
DTSTART:20261001T140000Z\r\nDTEND:20261001T150000Z\r\n",
        ),
        event(
            "gone",
            "SUMMARY:Cancelled\r\nSTATUS:CANCELLED\r\nDTSTART:20260929T100000Z\r\nDTEND:20260929T110000Z\r\n",
        ),
    ]
    .concat());

    let items = items(&ics);
    let titles: Vec<(String, String)> = items
        .iter()
        .map(|i| (i.card.title.clone(), i.card.due_at.unwrap().to_rfc3339()))
        .collect();

    assert_eq!(
        titles,
        vec![
            ("Sync".to_owned(), "2026-09-29T14:00:00+00:00".to_owned()),
            (
                "Sync (moved)".to_owned(),
                "2026-09-30T17:00:00+00:00".to_owned()
            ),
        ]
    );
}

/// Linhas dobradas e texto escapado do formato iCal são lidos corretamente.
#[test]
fn folded_lines_and_escaped_text_are_decoded() {
    let ics = calendar(&event(
        "folded",
        "SUMMARY:Review\\, plan and\r\n  ship\r\nDTSTART:20260928T150000Z\r\nDTEND:20260928T160000Z\r\n",
    ));

    let items = items(&ics);

    assert_eq!(items[0].card.title, "Review, plan and ship");
}

/// Num feed do Google, o card aponta para o dia no Google Calendar.
#[test]
fn google_feeds_link_to_the_day() {
    let ics = calendar(&event(
        "g",
        "SUMMARY:Planning\r\nDTSTART:20260928T150000Z\r\nDTEND:20260928T160000Z\r\n",
    ));

    let batch = occurrences(
        &ics,
        now(),
        Window::default(),
        chrono_tz::UTC,
        Some("https://calendar.google.com/calendar/ical/x/private-y/basic.ics"),
    )
    .unwrap();

    assert_eq!(
        batch.items[0].card.url.as_deref(),
        Some("https://calendar.google.com/calendar/r/day/2026/9/28")
    );
}

/// Sem a URL no ambiente, a fonte explica qual variável definir.
#[tokio::test]
async fn missing_url_names_the_variable() {
    let error = IcalSource::from_env(&|_| None)
        .refresh()
        .await
        .expect_err("not configured");

    assert!(
        error.to_string().contains("TASKS_PENDING_ICAL_URL"),
        "{error}"
    );
}

/// Uma URL que responde erro falha com o status, sem expor a URL secreta.
#[tokio::test]
async fn http_errors_do_not_leak_the_secret_url() {
    let app = axum::Router::new().route(
        "/calendar/ical/secret-token/basic.ics",
        axum::routing::get(|| async { (axum::http::StatusCode::NOT_FOUND, "gone") }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://{}/calendar/ical/secret-token/basic.ics",
        listener.local_addr().unwrap()
    );
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let error = IcalSource::from_env(&|key| (key == "TASKS_PENDING_ICAL_URL").then(|| url.clone()))
        .refresh()
        .await
        .expect_err("404");

    let message = error.to_string();
    assert!(message.contains("404"), "{message}");
    assert!(!message.contains("secret-token"), "{message}");
}

/// Séries com fim (UNTIL) em data, como o Google exporta eventos de dia
/// inteiro, ou em horário sem fuso, são expandidas até o fim; não somem nem
/// deixam a fonte degradada.
#[test]
fn series_ending_on_a_date_or_floating_time_are_expanded() {
    let ics = calendar(
        &[
            event(
                "allday",
                "SUMMARY:Gym\r\nDTSTART;VALUE=DATE:20260901\r\nDTEND;VALUE=DATE:20260902\r\n\
RRULE:FREQ=WEEKLY;UNTIL=20261013\r\n",
            ),
            event(
                "floating",
                "SUMMARY:Class\r\nDTSTART:20260930T100000\r\nDTEND:20260930T110000\r\n\
RRULE:FREQ=WEEKLY;UNTIL=20261014T100000\r\n",
            ),
        ]
        .concat(),
    );

    let batch = occurrences(&ics, now(), Window::default(), chrono_tz::UTC, None).unwrap();
    let starts: Vec<(String, String)> = batch
        .items
        .iter()
        .map(|i| (i.card.title.clone(), i.card.due_at.unwrap().to_rfc3339()))
        .collect();

    assert!(batch.warnings.is_empty(), "{:?}", batch.warnings);
    assert_eq!(
        starts,
        vec![
            ("Gym".to_owned(), "2026-09-29T00:00:00+00:00".to_owned()),
            ("Class".to_owned(), "2026-09-30T10:00:00+00:00".to_owned()),
            ("Gym".to_owned(), "2026-10-06T00:00:00+00:00".to_owned()),
            ("Class".to_owned(), "2026-10-07T10:00:00+00:00".to_owned()),
            ("Gym".to_owned(), "2026-10-13T00:00:00+00:00".to_owned()),
            ("Class".to_owned(), "2026-10-14T10:00:00+00:00".to_owned()),
        ]
    );
}

/// Uma série de dia inteiro ocupa o dia local todo, mesmo na ocorrência do
/// dia em que o relógio muda (dia de 25 horas): continua na tela até a
/// meia-noite local.
#[test]
fn all_day_events_last_the_whole_local_day_across_dst() {
    let zone = chrono_tz::America::New_York;
    let ics = calendar(&event(
        "fallback",
        "SUMMARY:Long day\r\nDTSTART;VALUE=DATE:20261025\r\nDTEND;VALUE=DATE:20261026\r\n\
RRULE:FREQ=WEEKLY\r\n",
    ));
    // 23:30 de 01/11 em Nova York (04:30 UTC de 02/11), ainda no mesmo dia local.
    let late = Utc.with_ymd_and_hms(2026, 11, 2, 4, 30, 0).unwrap();

    let batch = occurrences(&ics, late, Window::default(), zone, None).unwrap();

    // A ocorrência de 01/11 (meia-noite EDT = 04:00 UTC) ainda está na tela.
    assert_eq!(
        batch.items[0].card.due_at.map(|at| at.to_rfc3339()),
        Some("2026-11-01T04:00:00+00:00".to_owned()),
        "still today at 23:30 local"
    );
}

/// Num fuso em que a meia-noite não existe no dia da mudança de horário, o
/// evento de dia inteiro começa no primeiro instante do dia.
#[test]
fn all_day_events_on_a_day_without_midnight_are_read() {
    let zone = chrono_tz::America::Santiago;
    let ics = calendar(&event(
        "spring",
        "SUMMARY:Spring forward\r\nDTSTART;VALUE=DATE:20260906\r\nDTEND;VALUE=DATE:20260907\r\n",
    ));
    let morning = Utc.with_ymd_and_hms(2026, 9, 6, 12, 0, 0).unwrap();

    let batch = occurrences(&ics, morning, Window::default(), zone, None).unwrap();

    assert!(batch.warnings.is_empty(), "{:?}", batch.warnings);
    assert_eq!(batch.items.len(), 1);
}

/// Evento sem fim mostra só o horário de início.
#[test]
fn events_without_an_end_show_only_the_start() {
    let ics = calendar(&event(
        "instant",
        "SUMMARY:Reminder\r\nDTSTART:20260928T150000Z\r\n",
    ));

    let items = items(&ics);

    assert!(
        items[0].card.body.contains("15:00"),
        "{}",
        items[0].card.body
    );
    assert!(
        !items[0].card.body.contains("15:00–"),
        "{}",
        items[0].card.body
    );
}

/// O fim da série (UNTIL) é lido no fuso dela: UNTIL em data num fuso com
/// deslocamento inclui o último dia, e UNTIL sem fuso com início em outro fuso
/// usa o fuso do início; UNTIL numa hora que não existe (mudança de horário)
/// não descarta a série.
#[test]
fn until_is_read_in_the_series_zone() {
    let sp = chrono_tz::America::Sao_Paulo;
    let ics = calendar(
        &[
            event(
                "date-until",
                "SUMMARY:Gym\r\nDTSTART;VALUE=DATE:20260929\r\nDTEND;VALUE=DATE:20260930\r\n\
RRULE:until=20261013;freq=weekly\r\n",
            ),
            event(
                "ny",
                "SUMMARY:NY call\r\nDTSTART;TZID=America/New_York:20260930T090000\r\n\
DTEND;TZID=America/New_York:20260930T093000\r\nRRULE:FREQ=WEEKLY;UNTIL=20261014T090000\r\n",
            ),
            event(
                "gap",
                "SUMMARY:Gap\r\nDTSTART;TZID=America/New_York:20260930T020000\r\n\
DTEND;TZID=America/New_York:20260930T030000\r\nRRULE:FREQ=WEEKLY;UNTIL=20270314T023000\r\n",
            ),
        ]
        .concat(),
    );

    let batch = occurrences(&ics, now(), Window::default(), sp, None).unwrap();
    let count = |title: &str| batch.items.iter().filter(|i| i.card.title == title).count();

    assert!(batch.warnings.is_empty(), "{:?}", batch.warnings);
    assert_eq!(count("Gym"), 3, "29/09, 06/10 and the UNTIL day 13/10");
    assert_eq!(
        count("NY call"),
        3,
        "30/09, 07/10 and 14/10 at 09:00 New York"
    );
    assert!(
        count("Gap") > 0,
        "a non-existent UNTIL time keeps the series"
    );
}

/// Colunas configuradas agrupam as faixas de tempo: "Hoje" com o que está
/// acontecendo e o resto do dia, "Depois" com amanhã e adiante; uma faixa que
/// nenhuma coluna lista não aparece.
#[test]
fn configured_columns_group_time_buckets() {
    use pending_ical::{IcalColumn, When, assign_columns};

    let ics = calendar(
        &[
            event(
                "now",
                "SUMMARY:Ongoing\r\nDTSTART:20260928T113000Z\r\nDTEND:20260928T123000Z\r\n",
            ),
            event(
                "today",
                "SUMMARY:Afternoon\r\nDTSTART:20260928T150000Z\r\nDTEND:20260928T160000Z\r\n",
            ),
            event(
                "tomorrow",
                "SUMMARY:Tomorrow\r\nDTSTART:20260929T150000Z\r\nDTEND:20260929T160000Z\r\n",
            ),
            event(
                "later",
                "SUMMARY:Friday\r\nDTSTART:20261002T150000Z\r\nDTEND:20261002T160000Z\r\n",
            ),
        ]
        .concat(),
    );
    let columns = vec![
        IcalColumn {
            name: "Hoje".to_owned(),
            when: vec![When::Now, When::Today],
        },
        IcalColumn {
            name: "Semana".to_owned(),
            when: vec![When::Later],
        },
    ];

    let batch = occurrences(&ics, now(), Window::default(), chrono_tz::UTC, None).unwrap();
    let batch = assign_columns(&columns, batch);

    assert_eq!(
        summary(&batch.items),
        vec![
            ("Hoje".to_owned(), "Ongoing".to_owned()),
            ("Hoje".to_owned(), "Afternoon".to_owned()),
            ("Semana".to_owned(), "Friday".to_owned()),
        ]
    );
}
