//! Onde a configuração é procurada, como é lida e que fontes ela liga.

use std::path::{Path, PathBuf};

use pending_runtime::config::{ConfigLocation, LoadError, Origin, load_plan, locate};

fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
    move |key| {
        vars.iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| (*v).to_owned())
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

/// O caminho vem, nesta ordem: flag da linha de comando, variável
/// `TASKS_PENDING_CONFIG`, `$XDG_CONFIG_HOME/tasks-pending/config.toml` e
/// `~/.config/tasks-pending/config.toml`. Só os dois primeiros são explícitos.
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

/// Na primeira execução, sem arquivo no local padrão, o app sobe com a fonte
/// de exemplo e informa onde procurou.
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

/// Um caminho pedido explicitamente que não existe é erro, citando o caminho,
/// em vez de subir silenciosamente com dados de exemplo.
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

/// Erros de sintaxe e de regra apontam o arquivo, para o usuário saber o que
/// corrigir.
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

/// A configuração liga as fontes habilitadas, cada uma na lane e no
/// intervalo configurados, com o timeout global.
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
    assert_eq!(plan.specs[0].lane, "Work");
    assert_eq!(plan.specs[0].interval.as_secs(), 45);
}

/// O exemplo versionado no repositório é uma configuração válida; se o
/// formato mudar sem atualizar o exemplo, o gate falha.
#[test]
fn committed_example_config_is_valid() {
    let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config.example.toml");

    let (plan, _) = load_plan(Some(example), &env(&[])).expect("example config loads");

    assert!(!plan.specs.is_empty());
}
