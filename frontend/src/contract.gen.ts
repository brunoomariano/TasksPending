// Generated from crates/pending-core by `make contract`. Do not edit.

export type DashboardSnapshot = { generated_at: string, 
/**
 * Areas of work (Work, Personal, …), shown as tabs.
 */
boards: Array<Board>, sources: Array<SourceHealth>, 
/**
 * Why the config file on disk was rejected; the previous config keeps
 * running meanwhile.
 */
config_error: string | null, };

export type Board = { name: string, 
/**
 * One group of columns per source, in configuration order.
 */
groups: Array<Group>, };

export type Group = { 
/**
 * Configured source name; matches a `SourceHealth::name`.
 */
source: string, columns: Array<Column>, };

export type Column = { name: string, cards: Array<PendingCard>, };

export type PendingCard = { id: string, title: string, body: string, source: string, url: string | null, 
/**
 * When the card is due or starts (deadline, event). Cards with one sort
 * soonest first.
 */
due_at: string | null, severity: CardSeverity, updated_at: string, };

export type CardSeverity = "info" | "warning" | "critical";

export type SourceHealth = { name: string, status: SourceStatus, last_refresh_at: string | null, message: string | null, };

export type SourceStatus = "ready" | "refreshing" | "degraded" | "failed";
