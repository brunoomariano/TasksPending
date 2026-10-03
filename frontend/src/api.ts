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
    Array.isArray(candidate.boards) &&
    Array.isArray(candidate.sources)
  );
}

export type RefreshResult = { ok: true } | { ok: false; error: string };

/** Asks the API to refresh every source now (rate-limited server-side). */
export async function requestRefresh(
  fetchFn: Fetch = (input, init) => fetch(input, init),
): Promise<RefreshResult> {
  try {
    const response = await fetchFn("/api/v1/refresh", {
      method: "POST",
      headers: { "x-requested-with": "tasks-pending" },
      signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS),
    });
    if (response.ok) {
      return { ok: true };
    }
    const body: unknown = await response.json().catch(() => null);
    const wait =
      typeof body === "object" && body !== null
        ? (body as Record<string, unknown>).retry_after_secs
        : undefined;
    return typeof wait === "number"
      ? { ok: false, error: `wait ${wait}s` }
      : { ok: false, error: `HTTP ${response.status}` };
  } catch (error) {
    return {
      ok: false,
      error: error instanceof Error ? error.message : String(error),
    };
  }
}

/** Marks or unmarks a card as in progress. */
export async function setMark(
  id: string,
  marked: boolean,
  fetchFn: Fetch = (input, init) => fetch(input, init),
): Promise<RefreshResult> {
  try {
    const response = await fetchFn("/api/v1/marks", {
      method: "POST",
      headers: {
        "content-type": "application/json",
        "x-requested-with": "tasks-pending",
      },
      body: JSON.stringify({ id, marked }),
      signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS),
    });
    return response.ok
      ? { ok: true }
      : { ok: false, error: `HTTP ${response.status}` };
  } catch (error) {
    return {
      ok: false,
      error: error instanceof Error ? error.message : String(error),
    };
  }
}
