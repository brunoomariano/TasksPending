import type { SnapshotResult } from "./api";
import type { DashboardSnapshot } from "./contract.gen";

/** What the dashboard shows. */
export type ViewState =
  | { kind: "loading" }
  | { kind: "ready"; snapshot: DashboardSnapshot; fetchedAt: Date }
  | { kind: "unavailable"; error: string }
  /** The API failed after an earlier success; `snapshot` is from `fetchedAt`. */
  | {
      kind: "stale";
      snapshot: DashboardSnapshot;
      fetchedAt: Date;
      error: string;
    };

export function nextState(
  previous: ViewState,
  result: SnapshotResult,
  now: Date,
): ViewState {
  if (result.ok) {
    return { kind: "ready", snapshot: result.snapshot, fetchedAt: now };
  }
  if (previous.kind === "ready" || previous.kind === "stale") {
    return {
      kind: "stale",
      snapshot: previous.snapshot,
      fetchedAt: previous.fetchedAt,
      error: result.error,
    };
  }
  return { kind: "unavailable", error: result.error };
}
