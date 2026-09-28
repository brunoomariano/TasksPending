//! Onde a configuração é procurada, como é lida e que fontes ela liga.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

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
    assert_eq!(plan.specs[0].board, "Work");
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

/// Variáveis vazias ou com caminho relativo não contam: a busca segue para a
/// próxima opção, como manda a especificação XDG.
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

/// Uma fonte `github` na configuração vira uma fonte agendada com o nome e a
/// lane configurados; o token vem do ambiente, nunca do arquivo.
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

/// Sem token no ambiente, o `gh` é consultado uma vez só, mesmo com várias
/// fontes GitHub, para não multiplicar a espera na subida.
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

/// O cache fica em `$XDG_STATE_HOME/tasks-pending/cache.json`, ou em
/// `~/.local/state/...`; caminhos relativos são ignorados.
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

/// Uma fonte `plane` na configuração vira uma fonte agendada; as credenciais
/// vêm do ambiente, nunca do arquivo.
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

/// Uma fonte `ical` na configuração vira uma fonte agendada; a URL secreta vem
/// do ambiente, nunca do arquivo.
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

/// Cada fonte declara as colunas dela na configuração, e a área (aba) onde
/// aparece; `lane` continua aceito como nome antigo de `board`.
#[test]
fn sources_declare_board_and_columns() {
    let path = scratch("columns").join("config.toml");
    std::fs::write(
        &path,
        r#"
        [[sources]]
        name = "plane"
        kind = "plane"
        board = "Trabalho"

          [[sources.columns]]
          name = "Minhas"

          [[sources.columns]]
          name = "Inbox sem responsável"
          assignee = "none"
          state = ["Inbox"]

        [[sources]]
        name = "todo"
        kind = "todoist"
        lane = "Pessoal"

          [[sources.columns]]
          name = "Hoje"
          filter = "today | overdue"

        [[sources]]
        name = "gh"
        kind = "github"
        board = "Contribuições"

          [[sources.columns]]
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
                "Trabalho".to_owned(),
                vec!["Minhas".to_owned(), "Inbox sem responsável".to_owned()]
            ),
            (
                "todo".to_owned(),
                "Pessoal".to_owned(),
                vec!["Hoje".to_owned()]
            ),
            (
                "gh".to_owned(),
                "Contribuições".to_owned(),
                vec!["OSS reviews".to_owned()]
            ),
        ]
    );
}

/// Um filtro com chave errada ou valor inválido é recusado citando a fonte e
/// a coluna, em vez de virar uma coluna que nunca mostra nada.
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
                "[[sources]]\nname = \"plane\"\nkind = \"plane\"\n[[sources.columns]]\nname = \"Minhas\"\n{body}\n"
            ),
        )
        .unwrap();

        let error = load_plan_with(Some(path), &env(&[]), &|| None).expect_err(name);
        let message = error.to_string();
        assert!(
            message.contains("plane") && message.contains("Minhas"),
            "{message}"
        );
    }
}
