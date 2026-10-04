//! A stack's `exclude` patterns leave matching cards out of that stack.

use chrono::Utc;
use pending_core::{CardSeverity, PendingCard, SourceBatch, SourceConfig, SourceItem};
use pending_runtime::exclude::Excludes;

/// The excludes of a Plane source with these `[[stacks]]`.
fn excludes(stacks: &str) -> Result<Excludes, String> {
    let source: SourceConfig =
        toml::from_str(&format!("name = \"plane\"\nkind = \"plane\"\n{stacks}"))
            .expect("valid toml");
    Excludes::from_stacks(&source.stacks).map_err(|error| error.to_string())
}

fn item(column: &str, id: &str, title: &str, body: &str) -> SourceItem {
    SourceItem {
        column: column.to_owned(),
        card: PendingCard {
            id: id.to_owned(),
            title: title.to_owned(),
            body: body.to_owned(),
            source: "plane".to_owned(),
            url: None,
            due_at: None,
            severity: CardSeverity::Info,
            updated_at: Utc::now(),
        },
    }
}

fn batch(items: Vec<SourceItem>) -> SourceBatch {
    SourceBatch {
        items,
        warnings: Vec::new(),
    }
}

fn ids(batch: &SourceBatch) -> Vec<(&str, &str)> {
    batch
        .items
        .iter()
        .map(|item| (item.column.as_str(), item.card.id.as_str()))
        .collect()
}

/// A card is left out when any pattern is found anywhere in its title or in
/// its body, whatever the letter case; the other cards keep their order.
#[test]
fn a_card_matching_any_pattern_in_title_or_body_is_left_out() {
    let excludes = excludes(
        r#"
        [[stacks]]
        name = "Review"
        exclude = ["dependabot\\[bot\\]", "wip"]
        "#,
    )
    .expect("valid patterns");
    let mut batch = batch(vec![
        item("Review", "1", "Fix login", "acme/web · alice"),
        item("Review", "2", "Bump serde", "acme/web · Dependabot[bot]"),
        item("Review", "3", "[WIP] new parser", ""),
        item("Review", "4", "Add cache", "acme/api · bob"),
    ]);

    excludes.apply(&mut batch);

    assert_eq!(ids(&batch), [("Review", "1"), ("Review", "4")]);
    assert!(batch.warnings.is_empty());
}

/// Patterns are regular expressions: `^` and `$` anchor to the start and end
/// of the title or of the body, not to each line of a longer body.
#[test]
fn anchors_match_the_start_of_the_title_or_the_body() {
    let excludes = excludes(
        r#"
        [[stacks]]
        name = "Review"
        exclude = ["^chore\\(deps\\)"]
        "#,
    )
    .expect("valid patterns");
    let mut batch = batch(vec![
        item("Review", "1", "chore(deps): bump serde", ""),
        item("Review", "2", "Revert chore(deps): bump serde", ""),
        item("Review", "3", "Update", "CHORE(deps) batch\nsecond line"),
        item("Review", "4", "Update", "first line\nchore(deps) batch"),
    ]);

    excludes.apply(&mut batch);

    assert_eq!(ids(&batch), [("Review", "2"), ("Review", "4")]);
}

/// A stack's patterns apply to that stack only: the same card stays in the
/// source's other stacks, and a disabled stack's patterns do nothing.
#[test]
fn patterns_only_affect_their_own_stack() {
    let excludes = excludes(
        r#"
        [[stacks]]
        name = "Review"
        exclude = ["bot"]

        [[stacks]]
        name = "Everything"

        [[stacks]]
        name = "Off"
        enabled = false
        exclude = ["."]
        "#,
    )
    .expect("valid patterns");
    let mut batch = batch(vec![
        item("Review", "1", "Bot update", ""),
        item("Everything", "1", "Bot update", ""),
        item("Review", "2", "Robot arm", ""),
        item("Off", "3", "Anything", ""),
    ]);

    let hidden = excludes.apply(&mut batch);

    assert_eq!(ids(&batch), [("Everything", "1"), ("Off", "3")]);
    // Card 1 is still on the dashboard; card 2 is in no stack any more.
    assert_eq!(hidden.into_iter().collect::<Vec<_>>(), ["2"]);
}

/// A pattern that is not a valid regular expression, or is empty (it would
/// hide every card), is rejected naming the stack and the pattern, even in
/// a disabled stack.
#[test]
fn invalid_and_empty_patterns_are_rejected() {
    let error = excludes(
        "[[stacks]]\nname = \"Fine\"\nexclude = [\"ok\"]\n[[stacks]]\nname = \"Off\"\nenabled = false\nexclude = [\"ok\", \"(unclosed\"]",
    )
    .expect_err("invalid regex");
    assert!(error.contains("stack `Off`"), "{error}");
    assert!(error.contains("`(unclosed`"), "{error}");
    assert!(!error.contains('\n'), "{error}");

    let error = excludes("[[stacks]]\nname = \"Blank\"\nexclude = [\" \"]").expect_err("empty");
    assert!(error.contains("stack `Blank`"), "{error}");
    assert!(error.contains("empty"), "{error}");
}

/// A pattern that matches the empty text would hide cards whatever they
/// say (a trailing `|`, `.*`, a bare anchor): it is rejected like a typo.
#[test]
fn patterns_matching_the_empty_text_are_rejected() {
    for pattern in ["bot|", ".*", "^$", "(wip)?", "^"] {
        let error = excludes(&format!(
            "[[stacks]]\nname = \"Review\"\nexclude = ['{pattern}']"
        ))
        .expect_err(pattern);
        assert!(error.contains("stack `Review`"), "{error}");
        assert!(error.contains(&format!("`{pattern}`")), "{error}");
        assert!(error.contains("empty text"), "{error}");
    }

    // Patterns that need at least one character are fine.
    excludes("[[stacks]]\nname = \"Review\"\nexclude = ['bot|wip', '.+', '^chore']")
        .expect("valid patterns");
}
