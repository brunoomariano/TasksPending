import { describe, expect, test } from "vitest";
import { renderApp } from "./render";

describe("renderApp", () => {
  /**
   * Com a API fora do ar, a tela diz que ela está indisponível e por quê, e não
   * mostra nenhum card, para ninguém agir sobre pendências inventadas.
   */
  test("shows the API error and no cards when unavailable", async () => {
    const html = renderApp({ kind: "unavailable", error: "HTTP 503" });

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
      kind: "ready",
      fetchedAt: new Date("2026-09-28T10:00:00Z"),
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
                    due_at: null,
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

  /**
   * Um link com esquema diferente de http(s) nunca vira `href`: o título
   * aparece como texto, sem link clicável.
   */
  test("does not link card urls that are not http", () => {
    const html = renderApp({
      kind: "ready",
      fetchedAt: new Date("2026-09-28T10:00:00Z"),
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
                    id: "x",
                    title: "Suspicious",
                    body: "",
                    source: "test",
                    url: "javascript:alert(1)",
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
    });

    expect(html).toContain("Suspicious");
    expect(html).not.toContain("href=");
  });

  /**
   * A saúde de cada fonte aparece na tela com o motivo das falhas, para o
   * usuário saber que parte das pendências pode estar faltando.
   */
  test("shows source health with failure reasons", () => {
    const html = renderApp({
      kind: "ready",
      fetchedAt: new Date("2026-09-28T10:00:00Z"),
      snapshot: {
        generated_at: "2026-09-28T10:00:00Z",
        lanes: [],
        sources: [
          {
            name: "github",
            status: "ready",
            last_refresh_at: null,
            message: null,
          },
          {
            name: "jira",
            status: "failed",
            last_refresh_at: null,
            message: "401 <bad> credentials",
          },
        ],
      },
    });

    expect(html).toContain('class="source source-ready"');
    expect(html).toContain('class="source source-failed"');
    expect(html).toContain("401 &lt;bad&gt; credentials");
  });

  /**
   * Com a API fora do ar depois de já ter mostrado dados, os cards continuam
   * visíveis sob um aviso de que são antigos, com o motivo.
   */
  test("stale state keeps the cards under a warning", () => {
    const html = renderApp({
      kind: "stale",
      fetchedAt: new Date("2026-09-28T10:00:00Z"),
      error: "HTTP 502",
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
                    id: "a",
                    title: "Still here",
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
    });

    expect(html).toContain("Still here");
    expect(html).toContain('class="stale"');
    expect(html).toContain("HTTP 502");
  });

  /** Antes da primeira resposta, a tela diz que está carregando. */
  test("loading state", () => {
    expect(renderApp({ kind: "loading" })).toContain("Loading");
  });

  /** Com dados na tela, há um botão para atualizar todas as fontes agora. */
  test("offers a refresh button", () => {
    const html = renderApp({
      kind: "ready",
      fetchedAt: new Date("2026-09-28T10:00:00Z"),
      snapshot: {
        generated_at: "2026-09-28T10:00:00Z",
        lanes: [],
        sources: [],
      },
    });

    expect(html).toContain('data-action="refresh"');
  });
});
