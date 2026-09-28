//! O contrato TypeScript do frontend é derivado dos tipos Rust do core.

use pending_core::contract::{CONTRACT_PATH, typescript_contract};

/// O frontend consome `/api/v1/snapshot` pelos tipos em `contract.gen.ts`. Se o
/// modelo Rust mudar e o arquivo não for regenerado, o gate falha em vez de o
/// frontend ler campos que não existem mais.
///
/// Regenerar com `make contract`.
#[test]
fn committed_typescript_contract_matches_rust_model() {
    // Resolved at runtime: a test binary reused from another worktree through a
    // shared target dir must still read and write this checkout's file.
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("run through cargo test");
    let path = std::path::Path::new(&manifest_dir).join(CONTRACT_PATH);
    let expected = typescript_contract();

    if std::env::var_os("UPDATE_CONTRACT").is_some() {
        std::fs::write(&path, &expected).expect("write contract");
        return;
    }

    let committed = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "{} is missing ({error}); run `make contract`",
            path.display()
        )
    });
    assert!(
        committed == expected,
        "{} is stale; run `make contract`",
        path.display()
    );
}
