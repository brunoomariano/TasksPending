import { describe, expect, test } from "vitest";
import type { DashboardSnapshot } from "./contract.gen";
import { nextState, type ViewState } from "./state";

const snapshot = (id: string): DashboardSnapshot => ({
  generated_at: "2026-09-28T10:00:00Z",
  config_error: null,
  sources: [
    { name: "github", status: "ready", last_refresh_at: null, message: null },
  ],
  boards: [
    {
      name: "Work",
      groups: [
        {
          source: "github",
          columns: [
            {
              name: "Review",
              cards: [
                {
                  id,
                  title: id,
                  body: "",
                  source: "github",
                  url: null,
                  severity: "info",
                  due_at: null,
                  updated_at: "2026-09-28T09:00:00Z",
                },
              ],
            },
          ],
        },
      ],
    },
  ],
});
const t1 = new Date("2026-09-28T10:00:00Z");
const t2 = new Date("2026-09-28T10:01:00Z");

describe("nextState", () => {
  /** A primeira resposta boa mostra o snapshot e quando ele chegou. */
  test("a successful load shows the snapshot", () => {
    expect(
      nextState({ kind: "loading" }, { ok: true, snapshot: snapshot("a") }, t1),
    ).toEqual({ kind: "ready", snapshot: snapshot("a"), fetchedAt: t1 });
  });

  /** Sem nenhum dado anterior, a falha mostra a API indisponível. */
  test("a failure before any data is unavailable", () => {
    expect(
      nextState({ kind: "loading" }, { ok: false, error: "HTTP 503" }, t1),
    ).toEqual({ kind: "unavailable", error: "HTTP 503" });
  });

  /**
   * Uma falha depois de um sucesso mantém os últimos cards na tela, marcados
   * como antigos, com o horário da última resposta boa e o motivo.
   */
  test("a failure after data keeps the last snapshot as stale", () => {
    const ready: ViewState = {
      kind: "ready",
      snapshot: snapshot("a"),
      fetchedAt: t1,
    };
    const stale = nextState(ready, { ok: false, error: "HTTP 502" }, t2);
    expect(stale).toEqual({
      kind: "stale",
      snapshot: snapshot("a"),
      fetchedAt: t1,
      error: "HTTP 502",
    });

    expect(nextState(stale, { ok: false, error: "timeout" }, t2)).toEqual({
      kind: "stale",
      snapshot: snapshot("a"),
      fetchedAt: t1,
      error: "timeout",
    });
    expect(nextState(stale, { ok: true, snapshot: snapshot("b") }, t2)).toEqual(
      { kind: "ready", snapshot: snapshot("b"), fetchedAt: t2 },
    );
  });
});
