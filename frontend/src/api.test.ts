import { describe, expect, test } from "vitest";
import { loadSnapshot, requestRefresh } from "./api";

describe("loadSnapshot", () => {
  /**
   * Quando a API responde com erro, o dashboard precisa saber que falhou em vez
   * de receber dados de exemplo que parecem reais.
   */
  test("reports HTTP failures instead of returning sample data", async () => {
    const unavailable = async () => new Response("down", { status: 503 });

    const result = await loadSnapshot(unavailable);

    expect(result).toEqual({ ok: false, error: "HTTP 503" });
  });

  /** Com a API respondendo, o dashboard recebe o snapshot como veio. */
  test("returns the snapshot when the API answers", async () => {
    const snapshot = {
      generated_at: "2026-09-28T10:00:00Z",
      boards: [],
      sources: [],
    };
    const ok = async () => Response.json(snapshot);

    expect(await loadSnapshot(ok)).toEqual({ ok: true, snapshot });
  });

  /** Falha de rede vira erro visível com a mensagem original. */
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
   * Um 200 que não tem forma de snapshot (proxy, versão errada da API) vira
   * erro visível em vez de quebrar a renderização e deixar a página em branco.
   */
  test("reports responses that are not a snapshot", async () => {
    const wrongShape = async () => Response.json({ hello: "world" });

    expect(await loadSnapshot(wrongShape)).toEqual({
      ok: false,
      error: "unexpected snapshot shape",
    });
  });

  /** Uma API que não responde vira erro depois do limite, em vez de travar. */
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
  /** O pedido vai com o cabeçalho que a API exige do dashboard. */
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
