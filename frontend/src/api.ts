import type { DashboardSnapshot } from "./contract.gen";

export type SnapshotResult =
  { ok: true; snapshot: DashboardSnapshot } | { ok: false; error: string };

type Fetch = (input: string, init?: RequestInit) => Promise<Response>;

const REQUEST_TIMEOUT_MS = 10_000;

export async function loadSnapshot(
  fetchFn: Fetch = (input, init) => fetch(input, init),
  timeoutMs = REQUEST_TIMEOUT_MS,
): Promise<SnapshotResult> {
  try {
    const response = await fetchFn("/api/v1/snapshot", {
      signal: AbortSignal.timeout(timeoutMs),
    });
    if (!response.ok) {
      return { ok: false, error: `HTTP ${response.status}` };
    }
    const body: unknown = await response.json();
    if (!isSnapshot(body)) {
      return { ok: false, error: "unexpected snapshot shape" };
    }
    return { ok: true, snapshot: body };
  } catch (error) {
    if (error instanceof DOMException && error.name === "TimeoutError") {
      return { ok: false, error: "request timed out" };
    }
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
