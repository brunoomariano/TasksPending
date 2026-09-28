// Generated from crates/pending-core by `make contract`. Do not edit.

export type DashboardSnapshot = { generated_at: string, lanes: Array<Lane>, sources: Array<SourceHealth>, };

export type Lane = { name: string, sections: Array<Section>, };

export type Section = { name: string, cards: Array<PendingCard>, };

export type PendingCard = { id: string, title: string, body: string, source: string, url: string | null, severity: CardSeverity, updated_at: string, };

export type CardSeverity = "info" | "warning" | "critical";

export type SourceHealth = { name: string, status: SourceStatus, last_refresh_at: string | null, message: string | null, };

export type SourceStatus = "ready" | "refreshing" | "degraded" | "failed";
