import type { DashboardSnapshot } from "./contract.gen";

export type SnapshotResult =
  { ok: true; snapshot: DashboardSnapshot } | { ok: false; error: string };

type Fetch = (input: string) => Promise<Response>;

export async function loadSnapshot(
  fetchFn: Fetch = (input) => fetch(input),
): Promise<SnapshotResult> {
  try {
    const response = await fetchFn("/api/v1/snapshot");
    if (!response.ok) {
      return { ok: false, error: `HTTP ${response.status}` };
    }
    return { ok: true, snapshot: (await response.json()) as DashboardSnapshot };
  } catch (error) {
    return {
      ok: false,
      error: error instanceof Error ? error.message : String(error),
    };
  }
}
