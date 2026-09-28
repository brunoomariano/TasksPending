import { describe, expect, test } from "vitest";
import { loadSnapshot, requestRefresh } from "./api";

describe("loadSnapshot", () => {
  /**
   * When the API answers with an error, the dashboard must know it failed
   * instead of getting sample data that looks real.
   */
  test("reports HTTP failures instead of returning sample data", async () => {
    const unavailable = async () => new Response("down", { status: 503 });

    const result = await loadSnapshot(unavailable);

    expect(result).toEqual({ ok: false, error: "HTTP 503" });
  });

  /** With the API answering, the dashboard gets the snapshot as sent. */
  test("returns the snapshot when the API answers", async () => {
    const snapshot = {
      generated_at: "2026-09-28T10:00:00Z",
      config_error: null,
      boards: [],
      sources: [],
    };
    const ok = async () => Response.json(snapshot);

    expect(await loadSnapshot(ok)).toEqual({ ok: true, snapshot });
  });

  /** A network failure becomes a visible error with the original message. */
  test("reports network failures", async () => {
    const offline = async (): Promise<Response> => {
      throw new TypeError("Failed to fetch");
    };

    expect(await loadSnapshot(offline)).toEqual({
      ok: false,
      error: "Failed to fetch",
    });
  });

  /**
   * A 200 that is not shaped like a snapshot (a proxy, the wrong API version)
   * becomes a visible error instead of breaking rendering and blanking the page.
   */
  test("reports responses that are not a snapshot", async () => {
    const wrongShape = async () => Response.json({ hello: "world" });

    expect(await loadSnapshot(wrongShape)).toEqual({
      ok: false,
      error: "unexpected snapshot shape",
    });
  });

  /** An API that never answers becomes an error after the limit, not a hang. */
  test("gives up on requests that hang", async () => {
    const hanging = (_input: string, init?: RequestInit) =>
      new Promise<Response>((_, reject) => {
        init?.signal?.addEventListener("abort", () =>
          reject(init.signal?.reason),
        );
      });

    const result = await loadSnapshot(hanging, 20);

    expect(result).toEqual({ ok: false, error: "request timed out" });
  });
});

describe("requestRefresh", () => {
  /** The request carries the header the API requires from the dashboard. */
  test("asks the API to refresh every source", async () => {
    let sent: RequestInit | undefined;
    const accepted = async (_input: string, init?: RequestInit) => {
      sent = init;
      return new Response(JSON.stringify({ accepted: true }), { status: 202 });
    };

    expect(await requestRefresh(accepted)).toEqual({ ok: true });
    expect(sent?.method).toBe("POST");
    expect(new Headers(sent?.headers).get("x-requested-with")).toBe(
      "tasks-pending",
    );
  });

  /** Pedido repetido cedo demais diz quanto falta esperar. */
  test("reports the cooldown", async () => {
    const tooSoon = async () =>
      new Response(JSON.stringify({ retry_after_secs: 7 }), { status: 429 });

    expect(await requestRefresh(tooSoon)).toEqual({
      ok: false,
      error: "wait 7s",
    });
  });
});
