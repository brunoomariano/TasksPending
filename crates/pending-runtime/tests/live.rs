//! The dashboard rereads the config on refresh and when the file changes.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use pending_runtime::live::{Env, Live};

fn scratch(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("live-tests")
        .join(test);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn config(name: &str, board: &str) -> String {
    format!("[[sources]]\nname = \"{name}\"\nkind = \"sample\"\nboard = \"{board}\"\n")
}

fn start(path: &Path) -> Live {
    let env: Env = Arc::new(|_| None);
    let (live, _origin) = Live::start(Some(path.to_owned()), env, None).expect("valid config");
    live
}

fn source_names(live: &Live) -> Vec<String> {
    live.snapshot()
        .sources
        .iter()
        .map(|s| s.name.clone())
        .collect()
}

async fn settle() {
    tokio::time::sleep(Duration::from_millis(20)).await;
}

/// File changed and refresh requested: the new config applies immediately.
#[tokio::test]
async fn refresh_reloads_a_changed_config() {
    let path = scratch("refresh").join("config.toml");
    std::fs::write(&path, config("first", "Work")).unwrap();
    let live = start(&path);
    settle().await;
    assert_eq!(source_names(&live), vec!["first"]);

    std::fs::write(&path, config("second", "Home")).unwrap();
    live.refresh_now();
    settle().await;

    assert_eq!(source_names(&live), vec!["second"]);
    assert_eq!(live.snapshot().boards[0].name, "Home");
    assert_eq!(live.snapshot().config_error, None);
}

/// A refresh with an unchanged config doesn't rebuild the sources (keeping
/// their backoff and rate limits).
#[tokio::test]
async fn refresh_with_an_unchanged_config_keeps_the_sources() {
    let path = scratch("unchanged").join("config.toml");
    std::fs::write(&path, config("first", "Work")).unwrap();
    let live = start(&path);
    let before = live.generation();

    live.refresh_now();
    settle().await;

    assert_eq!(live.generation(), before);
}

/// A typo saved to the file breaks nothing: the previous config stays in
/// effect and the error shows in the dashboard; once the file is fixed, the
/// error goes away.
#[tokio::test]
async fn an_invalid_edit_keeps_the_previous_config_and_reports_it() {
    let path = scratch("invalid").join("config.toml");
    std::fs::write(&path, config("first", "Work")).unwrap();
    let live = start(&path);

    std::fs::write(&path, "[[sources]]\nname = \"broken\"\nkind = \"nope\"\n").unwrap();
    live.refresh_now();
    settle().await;
    let snapshot = live.snapshot();
    assert_eq!(source_names(&live), vec!["first"]);
    let error = snapshot.config_error.expect("error shown");
    assert!(error.contains("config.toml"), "{error}");

    std::fs::write(&path, config("fixed", "Work")).unwrap();
    live.refresh_now();
    settle().await;
    assert_eq!(source_names(&live), vec!["fixed"]);
    assert_eq!(live.snapshot().config_error, None);
}

/// Hands off: saving the file is enough, the change is picked up within a
/// few seconds.
#[tokio::test(start_paused = true)]
async fn saving_the_file_reloads_it_automatically() {
    let path = scratch("watch").join("config.toml");
    std::fs::write(&path, config("first", "Work")).unwrap();
    let live = start(&path);
    let _watch = live.watch(Duration::from_secs(2));

    std::fs::write(&path, config("watched", "Work")).unwrap();
    tokio::time::sleep(Duration::from_secs(3)).await;

    assert_eq!(source_names(&live), vec!["watched"]);
}

/// A card on the running dashboard can be marked as in progress: the mark
/// shows in the snapshot, survives a config reload, and is stored next to
/// the cache. A card that is not on the dashboard cannot be marked.
#[tokio::test]
async fn cards_are_marked_through_the_running_dashboard() {
    let dir = scratch("marks");
    let path = dir.join("config.toml");
    std::fs::write(&path, config("first", "Work")).unwrap();
    let env: Env = Arc::new(|_| None);
    let (live, _) = Live::start(
        Some(path.clone()),
        env,
        Some(dir.join("state").join("cache.json")),
    )
    .expect("valid config");
    settle().await;
    let id = live
        .snapshot()
        .boards
        .iter()
        .flat_map(|board| &board.groups)
        .flat_map(|group| &group.columns)
        .flat_map(|column| &column.cards)
        .map(|card| card.id.clone())
        .next()
        .expect("the sample source has cards");

    live.set_mark(&id, true).expect("card is on the dashboard");
    assert_eq!(live.snapshot().marked, std::slice::from_ref(&id));
    assert!(dir.join("state").join("marks.json").is_file());
    assert_eq!(
        live.set_mark("ghost", true),
        Err(pending_runtime::marks::MarkError::UnknownCard)
    );

    // Same source name after a reload: the mark is still there.
    std::fs::write(&path, config("first", "Home")).unwrap();
    live.refresh_now();
    settle().await;
    assert_eq!(live.snapshot().marked, std::slice::from_ref(&id));

    live.set_mark(&id, false).expect("unmark");
    assert!(live.snapshot().marked.is_empty());
}
