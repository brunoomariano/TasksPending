//! TypeScript contract consumed by the frontend, derived from the core model.

use ts_rs::{Config, TS};

use crate::model::{
    CardSeverity, DashboardSnapshot, Lane, PendingCard, Section, SourceHealth, SourceStatus,
};

/// Contract file location, relative to the `pending-core` crate.
pub const CONTRACT_PATH: &str = "../../frontend/src/contract.gen.ts";

pub fn typescript_contract() -> String {
    let cfg = Config::new();
    let decls = [
        DashboardSnapshot::decl(&cfg),
        Lane::decl(&cfg),
        Section::decl(&cfg),
        PendingCard::decl(&cfg),
        CardSeverity::decl(&cfg),
        SourceHealth::decl(&cfg),
        SourceStatus::decl(&cfg),
    ];

    let mut out =
        String::from("// Generated from crates/pending-core by `make contract`. Do not edit.\n\n");
    for decl in decls {
        out.push_str("export ");
        out.push_str(&decl);
        out.push_str("\n\n");
    }
    out.truncate(out.trim_end().len());
    out.push('\n');
    out
}
