import { describe, expect, test } from "vitest";
import { loadSnapshot } from "./api";

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
});
