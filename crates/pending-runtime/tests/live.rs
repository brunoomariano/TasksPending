//! O painel relê a configuração no refresh e quando o arquivo muda.

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

/// Mudou o arquivo e pediu refresh: a nova configuração entra na hora.
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

/// Refresh com a configuração igual não reconstrói as fontes (mantém o
/// backoff e os limites de taxa delas).
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

/// Um erro de digitação salvo no arquivo não derruba nada: a configuração
/// anterior continua valendo e o erro aparece no painel; corrigido o
/// arquivo, o erro some.
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

/// Sem apertar nada: salvar o arquivo basta, a mudança é percebida em
/// poucos segundos.
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
