import { describe, expect, test } from "vitest";
import { renderApp } from "./render";

describe("renderApp", () => {
  /**
   * Com a API fora do ar, a tela diz que ela está indisponível e por quê, e não
   * mostra nenhum card, para ninguém agir sobre pendências inventadas.
   */
  test("shows the API error and no cards when unavailable", async () => {
    const html = renderApp({ ok: false, error: "HTTP 503" });

    expect(html).toContain("API unavailable");
    expect(html).toContain("HTTP 503");
    expect(html).not.toContain('class="card');
  });

  /**
   * Com a API respondendo, a tela mostra lanes e cards; texto vindo das fontes
   * é escapado, porque títulos de issue podem conter HTML.
   */
  test("renders lanes and cards with escaped source text", async () => {
    const html = renderApp({
      ok: true,
      snapshot: {
        generated_at: "2026-09-28T10:00:00Z",
        sources: [],
        lanes: [
          {
            name: "Work",
            sections: [
              {
                name: "Review",
                cards: [
                  {
                    id: "github:pr:1",
                    title: "<script>alert(1)</script>",
                    body: "body",
                    source: "github",
                    url: "https://github.com/o/r/pull/1",
                    severity: "critical",
                    updated_at: "2026-09-28T09:00:00Z",
                  },
                ],
              },
            ],
          },
        ],
      },
    });

    expect(html).toContain("Work");
    expect(html).toContain('class="card severity-critical"');
    expect(html).toContain("&lt;script&gt;alert(1)&lt;/script&gt;");
    expect(html).not.toContain("<script>");
    expect(html).toContain('href="https://github.com/o/r/pull/1"');
  });
});
