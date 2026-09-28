import { describe, expect, test } from "vitest";
import type { DashboardSnapshot, PendingCard } from "./contract.gen";
import { renderApp } from "./render";

const card = (id: string, extra: Partial<PendingCard> = {}): PendingCard => ({
  id,
  title: id,
  body: "",
  source: "test",
  url: null,
  due_at: null,
  severity: "info",
  updated_at: "2026-09-28T09:00:00Z",
  ...extra,
});

const snapshot = (): DashboardSnapshot => ({
  generated_at: "2026-09-28T10:00:00Z",
  sources: [
    { name: "plane", status: "ready", last_refresh_at: null, message: null },
    {
      name: "calendar",
      status: "failed",
      last_refresh_at: null,
      message: "401 <bad> credentials",
    },
  ],
  boards: [
    {
      name: "Trabalho",
      groups: [
        {
          source: "plane",
          columns: [
            {
              name: "Minhas",
              cards: [
                card("API-1", {
                  title: "<script>alert(1)</script>",
                  url: "https://plane.example/acme/projects/p/issues/1",
                  severity: "critical",
                }),
              ],
            },
            { name: "Inbox sem responsável", cards: [] },
          ],
        },
        {
          source: "github",
          columns: [{ name: "Review", cards: [card("gh-1")] }],
        },
      ],
    },
    {
      name: "Pessoal",
      groups: [
        {
          source: "todoist",
          columns: [{ name: "Hoje", cards: [card("todo-1")] }],
        },
      ],
    },
  ],
});

const ready = (board = 0) =>
  renderApp(
    {
      kind: "ready",
      fetchedAt: new Date("2026-09-28T10:00:00Z"),
      snapshot: snapshot(),
    },
    board,
  );

describe("renderApp", () => {
  /**
   * Com a API fora do ar, a tela diz que ela está indisponível e por quê, e não
   * mostra nenhum card, para ninguém agir sobre pendências inventadas.
   */
  test("shows the API error and no cards when unavailable", () => {
    const html = renderApp({ kind: "unavailable", error: "HTTP 503" });

    expect(html).toContain("API unavailable");
    expect(html).toContain("HTTP 503");
    expect(html).not.toContain('class="card');
  });

  /**
   * As áreas viram abas com a contagem de cards; a área escolhida mostra as
   * ferramentas como grupos de colunas, inclusive colunas vazias.
   */
  test("renders boards as tabs and sources as groups of columns", () => {
    const html = ready();

    expect(html).toMatch(
      /class="tab active" data-board="0">\s*1 Trabalho <span>2<\/span>/,
    );
    expect(html).toMatch(/data-board="1">\s*2 Pessoal <span>1<\/span>/);
    expect(html).toContain("<h2>plane</h2>");
    expect(html).toContain("<h2>github</h2>");
    expect(html).toContain("Minhas <span>1</span>");
    expect(html).toContain("Inbox sem responsável <span>0</span>");
    expect(html).not.toContain("todo-1");
    expect(html).toContain("alert(1)");
  });

  /** Escolher outra aba mostra as colunas daquela área. */
  test("renders the selected board", () => {
    const html = ready(1);

    expect(html).toMatch(/class="tab active" data-board="1"/);
    expect(html).toContain("<h2>todoist</h2>");
    expect(html).toContain("todo-1");
    expect(html).not.toContain("alert(1)");
  });

  /**
   * Texto vindo das fontes é escapado, porque títulos de issue podem conter
   * HTML; o título vira link só para http(s).
   */
  test("escapes source text and links only http urls", () => {
    const html = ready();

    expect(html).toContain('class="card severity-critical"');
    expect(html).toContain("&lt;script&gt;alert(1)&lt;/script&gt;");
    expect(html).not.toContain("<script>");
    expect(html).toContain(
      'href="https://plane.example/acme/projects/p/issues/1"',
    );

    const unsafe = snapshot();
    unsafe.boards[0].groups[1].columns[0].cards[0].url = "javascript:alert(1)";
    const risky = renderApp({
      kind: "ready",
      fetchedAt: new Date(),
      snapshot: unsafe,
    });
    expect(risky).not.toContain('href="javascript');
  });

  /** A saúde de cada fonte aparece com o motivo das falhas, escapado. */
  test("shows source health with failure reasons", () => {
    const html = ready();

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
      snapshot: snapshot(),
    });

    expect(html).toContain("alert(1)");
    expect(html).toContain('class="stale"');
    expect(html).toContain("HTTP 502");
  });

  /** Antes da primeira resposta, a tela diz que está carregando. */
  test("loading state", () => {
    expect(renderApp({ kind: "loading" })).toContain("Loading");
  });

  /** Com dados na tela, há um botão para atualizar todas as fontes agora. */
  test("offers a refresh button", () => {
    expect(ready()).toContain('data-action="refresh"');
  });
});
