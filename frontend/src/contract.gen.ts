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
config_error: string | null, 
/**
 * Ids of the cards marked as in progress, in the order they were
 * marked. A marked card may be absent from the boards while its source
 * is failing.
 */
marked: Array<string>, 
/**
 * Version of the program that produced the snapshot, e.g. `0.4.0`.
 */
version: string, };

export type Board = { name: string, 
/**
 * One group of columns per source, in configuration order.
 */
groups: Array<Group>, };

export type Group = { 
/**
 * Configured source name; matches a `SourceHealth::name`.
 */
source: string, 
/**
 * The source's icon from the configuration.
 */
icon: Icon | null, columns: Array<Column>, };

export type Column = { name: string, cards: Array<PendingCard>, };

export type PendingCard = { id: string, title: string, body: string, source: string, url: string | null, 
/**
 * When the card is due or starts (deadline, event). Cards with one sort
 * soonest first.
 */
due_at: string | null, severity: CardSeverity, updated_at: string, };

export type CardSeverity = "info" | "warning" | "critical";

export type SourceHealth = { name: string, status: SourceStatus, last_refresh_at: string | null, message: string | null, };

export type Icon = { url: string, 
/**
 * Used instead of `url` on dark themes.
 */
dark_url: string | null, };

export type SourceStatus = "ready" | "refreshing" | "degraded" | "failed";
