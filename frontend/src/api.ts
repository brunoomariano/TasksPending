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
    const body: unknown = await response.json();
    if (!isSnapshot(body)) {
      return { ok: false, error: "unexpected snapshot shape" };
    }
    return { ok: true, snapshot: body };
  } catch (error) {
    return {
      ok: false,
      error: error instanceof Error ? error.message : String(error),
    };
  }
}

function isSnapshot(value: unknown): value is DashboardSnapshot {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const candidate = value as Record<string, unknown>;
  return (
    typeof candidate.generated_at === "string" &&
    Array.isArray(candidate.lanes) &&
    Array.isArray(candidate.sources)
  );
}
