//! Where the config is looked up, how it is read, and which sources it enables.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use pending_core::StackSort;
use pending_runtime::config::{
    ConfigLocation, LoadError, Origin, load_plan, load_plan_with, locate,
};

fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
    move |key| {
        vars.iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| OsString::from(*v))
    }
}

fn scratch(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("config-tests")
        .join(test);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The path comes, in this order, from: the command-line flag,
/// `TASKS_PENDING_CONFIG`, `$XDG_CONFIG_HOME/tasks-pending/config.toml` and
/// `~/.config/tasks-pending/config.toml`. Only the first two are explicit.
#[test]
fn config_path_follows_flag_env_xdg_home_precedence() {
    let all = [
        ("TASKS_PENDING_CONFIG", "/env/config.toml"),
        ("XDG_CONFIG_HOME", "/xdg"),
        ("HOME", "/home/me"),
    ];

    assert_eq!(
        locate(Some(PathBuf::from("/flag.toml")), &env(&all)),
        Some(ConfigLocation {
            path: PathBuf::from("/flag.toml"),
            explicit: true
        })
    );
    assert_eq!(
        locate(None, &env(&all)),
        Some(ConfigLocation {
            path: PathBuf::from("/env/config.toml"),
            explicit: true
        })
    );
    assert_eq!(
        locate(None, &env(&all[1..])),
        Some(ConfigLocation {
            path: PathBuf::from("/xdg/tasks-pending/config.toml"),
            explicit: false
        })
    );
    assert_eq!(
        locate(None, &env(&all[2..])),
        Some(ConfigLocation {
            path: PathBuf::from("/home/me/.config/tasks-pending/config.toml"),
            explicit: false
        })
    );
    assert_eq!(locate(None, &env(&[])), None);
}

/// On first run, with no file at the default location, the app starts with
/// the sample source and says where it looked.
#[test]
fn missing_default_config_starts_with_the_sample_source() {
    let dir = scratch("missing-default");
    let xdg = dir.to_str().unwrap();

    let (plan, origin) = load_plan(None, &env(&[("XDG_CONFIG_HOME", xdg)])).expect("sample plan");

    assert_eq!(
        origin,
        Origin::SampleDefault {
            searched: Some(dir.join("tasks-pending/config.toml"))
        }
    );
    assert_eq!(plan.specs.len(), 1);
    assert_eq!(plan.specs[0].name, "sample");
}

/// An explicitly requested path that doesn't exist is an error naming the
/// path, instead of silently starting with sample data.
#[test]
fn missing_explicit_config_is_an_error() {
    let path = scratch("missing-explicit").join("nope.toml");

    let error = load_plan(Some(path.clone()), &env(&[])).expect_err("missing file");

    assert!(
        matches!(error, LoadError::Missing(ref p) if *p == path),
        "{error}"
    );
    assert!(error.to_string().contains("nope.toml"), "{error}");
}

/// Syntax and rule errors point at the file, so the user knows what to
/// fix.
#[test]
fn invalid_config_errors_name_the_file() {
    let dir = scratch("invalid");
    let broken = dir.join("broken.toml");
    std::fs::write(&broken, "refresh_seconds = \"soon\"").unwrap();
    let zero = dir.join("zero.toml");
    std::fs::write(&zero, "refresh_seconds = 0").unwrap();

    for path in [broken, zero] {
        let error = load_plan(Some(path.clone()), &env(&[])).expect_err("invalid");
        assert!(
            error.to_string().contains(path.to_str().unwrap()),
            "{error}"
        );
    }
}

/// The config enables the enabled sources, each in its configured lane and
/// interval, with the global timeout.
#[test]
fn config_file_turns_enabled_sources_into_scheduled_specs() {
    let path = scratch("valid").join("config.toml");
    std::fs::write(
        &path,
        r#"
        refresh_seconds = 120
        timeout_seconds = 10

        [[sources]]
        name = "sample"
        kind = "sample"
        lane = "Work"
        refresh_seconds = 45
        icon = "https://cdn.example/sample.svg"

        [[sources]]
        name = "off"
        kind = "sample"
        enabled = false
        "#,
    )
    .unwrap();

    let (plan, origin) = load_plan(Some(path.clone()), &env(&[])).expect("valid plan");

    assert_eq!(origin, Origin::File(path));
    assert_eq!(plan.timeout.as_secs(), 10);
    assert_eq!(plan.specs.len(), 1);
    assert_eq!(plan.specs[0].board, "Work");
    assert_eq!(plan.specs[0].interval.as_secs(), 45);
    assert_eq!(
        plan.specs[0].icon.as_ref().map(|icon| icon.url.as_str()),
        Some("https://cdn.example/sample.svg")
    );
}

/// The example checked into the repo is a valid config; if the format
/// changes without updating the example, the gate fails.
#[test]
fn committed_example_config_is_valid() {
    let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config.example.toml");

    let (plan, _) = load_plan(Some(example), &env(&[])).expect("example config loads");

    assert!(!plan.specs.is_empty());
}

/// Empty or relative-path variables don't count: the lookup moves on to the
/// next option, as the XDG spec says.
#[test]
fn empty_and_relative_env_paths_are_ignored() {
    assert_eq!(
        locate(
            None,
            &env(&[
                ("TASKS_PENDING_CONFIG", ""),
                ("XDG_CONFIG_HOME", "relative/xdg"),
                ("HOME", "/home/me"),
            ])
        ),
        Some(ConfigLocation {
            path: PathBuf::from("/home/me/.config/tasks-pending/config.toml"),
            explicit: false
        })
    );
    assert_eq!(locate(None, &env(&[("HOME", "relative-home")])), None);
}

/// A `github` source in the config becomes a scheduled source with the
/// configured name and lane; the token comes from the environment, never the file.
#[test]
fn github_sources_are_scheduled_with_the_token_from_the_environment() {
    let path = scratch("github").join("config.toml");
    std::fs::write(
        &path,
        r#"
        [[sources]]
        name = "work-github"
        kind = "github"
        lane = "Work"
        "#,
    )
    .unwrap();

    let (plan, _) =
        load_plan(Some(path), &env(&[("GITHUB_TOKEN", "t")])).expect("github is supported");

    assert_eq!(plan.specs.len(), 1);
    assert_eq!(plan.specs[0].name, "work-github");
    assert_eq!(plan.specs[0].board, "Work");
}

/// A GitHub stack may list repositories whose security alerts it shows.
#[test]
fn github_alert_stacks_are_accepted() {
    let path = scratch("github-alerts").join("config.toml");
    std::fs::write(
        &path,
        r#"
        [[sources]]
        name = "github"
        kind = "github"

          [[sources.stacks]]
          name = "Security"
          alerts = ["acme/api", "acme/web.site"]
        "#,
    )
    .unwrap();

    let (plan, _) =
        load_plan(Some(path), &env(&[("GITHUB_TOKEN", "t")])).expect("alerts are supported");

    assert_eq!(plan.specs[0].source.columns(), ["Security"]);
}

/// A stack's `sort` travels with the scheduled source, for any source kind,
/// so the dashboard can order that stack oldest first.
#[test]
fn stack_sort_order_reaches_the_scheduled_source() {
    let path = scratch("stack-sort").join("config.toml");
    std::fs::write(
        &path,
        r#"
        [[sources]]
        name = "github"
        kind = "github"

          [[sources.stacks]]
          name = "Stale PRs"
          query = "is:open is:pr author:@me"
          sort = "oldest"

          [[sources.stacks]]
          name = "Recent PRs"
          query = "is:open is:pr author:@me"
        "#,
    )
    .unwrap();

    let (plan, _) =
        load_plan(Some(path), &env(&[("GITHUB_TOKEN", "t")])).expect("sort is accepted");

    let sorts: Vec<(&str, StackSort)> = plan.specs[0]
        .sorts
        .iter()
        .map(|(name, sort)| (name.as_str(), *sort))
        .collect();
    assert_eq!(
        sorts,
        [
            ("Recent PRs", StackSort::Newest),
            ("Stale PRs", StackSort::Oldest)
        ]
    );
}

/// Without a token in the environment, `gh` is asked only once, even with
/// several GitHub sources, so startup waits don't add up.
#[test]
fn gh_cli_is_asked_for_a_token_once_for_all_github_sources() {
    let path = scratch("github-gh").join("config.toml");
    std::fs::write(
        &path,
        r#"
        [[sources]]
        name = "work"
        kind = "github"

        [[sources]]
        name = "oss"
        kind = "github"
        "#,
    )
    .unwrap();
    let calls = std::cell::Cell::new(0);
    let gh = || {
        calls.set(calls.get() + 1);
        Some("from-gh".to_owned())
    };

    let (plan, _) = load_plan_with(Some(path), &env(&[]), &gh).expect("valid plan");

    assert_eq!(plan.specs.len(), 2);
    assert_eq!(calls.get(), 1);
}

/// The cache lives at `$XDG_STATE_HOME/tasks-pending/cache.json`, or at
/// `~/.local/state/...`; relative paths are ignored.
#[test]
fn cache_path_follows_xdg_state_home() {
    use pending_runtime::config::cache_path;

    assert_eq!(
        cache_path(&env(&[("XDG_STATE_HOME", "/state"), ("HOME", "/home/me")])),
        Some(PathBuf::from("/state/tasks-pending/cache.json"))
    );
    assert_eq!(
        cache_path(&env(&[("XDG_STATE_HOME", "rel"), ("HOME", "/home/me")])),
        Some(PathBuf::from(
            "/home/me/.local/state/tasks-pending/cache.json"
        ))
    );
    assert_eq!(cache_path(&env(&[])), None);
}

/// A `plane` source in the config becomes a scheduled source; credentials
/// come from the environment, never the file.
#[test]
fn plane_sources_are_scheduled() {
    let path = scratch("plane").join("config.toml");
    std::fs::write(
        &path,
        r#"
        [[sources]]
        name = "plane"
        kind = "plane"
        lane = "Work"
        "#,
    )
    .unwrap();

    let (plan, _) = load_plan_with(Some(path), &env(&[]), &|| None).expect("plane is supported");

    assert_eq!(plan.specs.len(), 1);
    assert_eq!(plan.specs[0].name, "plane");
}

/// An `ical` source in the config becomes a scheduled source; the secret URL
/// comes from the environment, never the file.
#[test]
fn ical_sources_are_scheduled() {
    let path = scratch("ical").join("config.toml");
    std::fs::write(
        &path,
        r#"
        [[sources]]
        name = "calendar"
        kind = "ical"
        "#,
    )
    .unwrap();

    let (plan, _) = load_plan_with(Some(path), &env(&[]), &|| None).expect("ical is supported");

    assert_eq!(plan.specs[0].name, "calendar");
}

/// Each source declares its columns in the config, and the area (tab) where
/// it appears; `lane` is still accepted as the old name for `board`.
#[test]
fn sources_declare_board_and_columns() {
    let path = scratch("columns").join("config.toml");
    std::fs::write(
        &path,
        r#"
        [[sources]]
        name = "plane"
        kind = "plane"
        board = "Work"

          [[sources.stacks]]
          name = "Mine"

          [[sources.stacks]]
          name = "Unassigned inbox"
          assignee = "none"
          state = ["Inbox"]

        [[sources]]
        name = "todo"
        kind = "todoist"
        lane = "Personal"

          [[sources.stacks]]
          name = "Today"
          filter = "today | overdue"

        [[sources]]
        name = "gh"
        kind = "github"
        board = "Contributions"

          [[sources.stacks]]
          name = "OSS reviews"
          query = "is:open is:pr review-requested:@me -org:SejaSenfio"
          severity = "warning"
        "#,
    )
    .unwrap();

    let (plan, _) = load_plan_with(Some(path), &env(&[("GITHUB_TOKEN", "t")]), &|| None)
        .expect("valid columns");

    let summary: Vec<(String, String, Vec<String>)> = plan
        .specs
        .iter()
        .map(|spec| (spec.name.clone(), spec.board.clone(), spec.source.columns()))
        .collect();
    assert_eq!(
        summary,
        vec![
            (
                "plane".to_owned(),
                "Work".to_owned(),
                vec!["Mine".to_owned(), "Unassigned inbox".to_owned()]
            ),
            (
                "todo".to_owned(),
                "Personal".to_owned(),
                vec!["Today".to_owned()]
            ),
            (
                "gh".to_owned(),
                "Contributions".to_owned(),
                vec!["OSS reviews".to_owned()]
            ),
        ]
    );
}

/// A filter with a wrong key or invalid value is rejected naming the source
/// and column, instead of becoming a column that never shows anything.
#[test]
fn invalid_column_filters_name_the_source_and_column() {
    let dir = scratch("bad-columns");
    for (name, body) in [
        ("typo.toml", "assigne = \"me\""),
        ("value.toml", "assignee = \"somebody\""),
        ("group.toml", "state_group = [\"in_progress\"]"),
        ("priority.toml", "priority = [\"p1\"]"),
    ] {
        let path = dir.join(name);
        std::fs::write(
            &path,
            format!(
                "[[sources]]\nname = \"plane\"\nkind = \"plane\"\n[[sources.stacks]]\nname = \"Mine\"\n{body}\n"
            ),
        )
        .unwrap();

        let error = load_plan_with(Some(path), &env(&[]), &|| None).expect_err(name);
        let message = error.to_string();
        assert!(
            message.contains("plane") && message.contains("Mine"),
            "{message}"
        );
    }
}

/// A GitHub column needs exactly one origin: a search (`query`), the
/// notifications inbox (`notifications`) or security alerts (`alerts`, a
/// non-empty list of `owner/repo`).
#[test]
fn github_columns_need_a_query_or_notifications() {
    let dir = scratch("gh-columns");
    for (name, body) in [
        ("neither.toml", ""),
        ("both.toml", "query = \"is:pr\"\nnotifications = \"all\""),
        ("bad-mode.toml", "notifications = \"done\""),
        (
            "alerts-and-query.toml",
            "query = \"is:pr\"\nalerts = [\"o/api\"]",
        ),
        ("no-repos.toml", "alerts = []"),
        ("not-a-repo.toml", "alerts = [\"api\"]"),
        ("repo-with-path.toml", "alerts = [\"o/api/x\"]"),
    ] {
        let path = dir.join(name);
        std::fs::write(
            &path,
            format!("[[sources]]\nname = \"gh\"\nkind = \"github\"\n[[sources.stacks]]\nname = \"N\"\n{body}\n"),
        )
        .unwrap();

        let error =
            load_plan_with(Some(path), &env(&[("GITHUB_TOKEN", "t")]), &|| None).expect_err(name);
        assert!(error.to_string().contains("stack `N`"), "{name}: {error}");
    }
}

/// A `google` source uses the GNOME Online Accounts account; columns filter
/// time ranges and calendars.
#[test]
fn google_sources_are_scheduled_with_columns() {
    let path = scratch("google").join("config.toml");
    std::fs::write(
        &path,
        r#"
        [[sources]]
        name = "agenda"
        kind = "google"
        board = "Personal"

          [[sources.stacks]]
          name = "Today"
          when = ["now", "today"]

          [[sources.stacks]]
          name = "Team"
          calendar = ["Team"]
        "#,
    )
    .unwrap();

    let (plan, _) = load_plan_with(Some(path), &env(&[]), &|| None).expect("google is supported");

    assert_eq!(plan.specs[0].source.columns(), vec!["Today", "Team"]);
}
