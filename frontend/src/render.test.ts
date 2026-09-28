import { describe, expect, test } from "vitest";
import type { DashboardSnapshot, PendingCard } from "./contract.gen";
import { groupKey, renderApp, type View } from "./render";

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
  config_error: null,
  sources: [
    {
      name: "plane",
      status: "ready",
      last_refresh_at: "2026-09-28T09:55:00Z",
      message: null,
    },
    {
      name: "github",
      status: "ready",
      last_refresh_at: "2026-09-28T09:50:00Z",
      message: null,
    },
    {
      name: "todoist",
      status: "ready",
      last_refresh_at: "2026-09-28T09:40:00Z",
      message: null,
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
            { name: "Inbox vazia", cards: [] },
          ],
        },
        {
          source: "github",
          columns: [
            { name: "Revisão", cards: [card("gh-1")] },
            { name: "Meus PRs", cards: [] },
            { name: "Notificações", cards: [] },
          ],
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

const view = (extra: Partial<View> = {}): View => ({
  board: null,
  expanded: new Set(),
  sourcesOpen: false,
  ...extra,
});

const ready = (v: View = view(), data: DashboardSnapshot = snapshot()) =>
  renderApp(
    {
      kind: "ready",
      fetchedAt: new Date("2026-09-28T10:00:00Z"),
      snapshot: data,
    },
    v,
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
   * A página mostra os grupos de todas as áreas; o nome da área aparece ao
   * lado do nome do grupo.
   */
  test("shows every board's groups, each labelled with its board", () => {
    const html = ready();

    expect(html).toMatch(
      /<h2>plane<\/h2>\s*<span class="group-board">Trabalho<\/span>/,
    );
    expect(html).toMatch(
      /<h2>github<\/h2>\s*<span class="group-board">Trabalho<\/span>/,
    );
    expect(html).toMatch(
      /<h2>todoist<\/h2>\s*<span class="group-board">Pessoal<\/span>/,
    );
  });

  /** O filtro do topo é opcional: "Todas" por padrão, ou uma área só. */
  test("the board filter narrows the page to one board", () => {
    expect(ready()).toMatch(/class="filter active" data-board="">\s*Todas/);

    const html = ready(view({ board: "Pessoal" }));

    expect(html).toMatch(
      /class="filter active" data-board="Pessoal">\s*Pessoal/,
    );
    expect(html).toContain("<h2>todoist</h2>");
    expect(html).not.toContain("<h2>plane</h2>");
  });

  /**
   * Colunas vazias somem; um botão no canto do grupo diz quantas são e, ao
   * abrir, mostra essas colunas com a última atualização da fonte.
   */
  test("empty columns hide behind a per-group toggle", () => {
    const collapsed = ready();
    expect(collapsed).not.toContain("Meus PRs");
    expect(collapsed).not.toContain("Inbox vazia");
    expect(collapsed).toMatch(
      new RegExp(
        `data-toggle-empty="${groupKey("Trabalho", "github")}">\\s*2 vazias`,
      ),
    );

    const expanded = ready(
      view({ expanded: new Set([groupKey("Trabalho", "github")]) }),
    );
    expect(expanded).toContain("Meus PRs");
    expect(expanded).toContain("Notificações");
    expect(expanded).toContain("updated");
    expect(expanded).not.toContain("Inbox vazia");
    expect(expanded).toMatch(/ocultar vazias/);
  });

  /** Os cards não trazem selo de gravidade (Info/Warning/Critical). */
  test("cards carry no severity label", () => {
    const html = ready();

    expect(html).not.toMatch(/>\s*(Info|Warning|Critical)\s*</);
    expect(html).toContain('class="card severity-critical"');
  });

  /**
   * Texto vindo das fontes é escapado, porque títulos de issue podem conter
   * HTML; o título vira link só para http(s).
   */
  test("escapes source text and links only http urls", () => {
    const html = ready();
    expect(html).toContain("&lt;script&gt;alert(1)&lt;/script&gt;");
    expect(html).not.toContain("<script>");
    expect(html).toContain(
      'href="https://plane.example/acme/projects/p/issues/1"',
    );

    const unsafe = snapshot();
    unsafe.boards[0].groups[1].columns[0].cards[0].url = "javascript:alert(1)";
    expect(ready(view(), unsafe)).not.toContain('href="javascript');
  });

  /**
   * As fontes não ficam expostas na página: um botão abre um modal com cada
   * fonte, a área dela, o estado, o último fetch e o motivo de falha.
   */
  test("sources live behind a button that opens a modal", () => {
    const closed = ready();
    expect(closed).toContain('data-action="sources"');
    expect(closed).not.toContain('class="sources-modal"');
    expect(closed).not.toContain("⚠");

    const broken = snapshot();
    broken.sources[1] = {
      name: "github",
      status: "failed",
      last_refresh_at: null,
      message: "401 <bad> credentials",
    };
    const open = ready(view({ sourcesOpen: true }), broken);
    expect(open).toMatch(
      /data-action="sources"[^>]*>\s*<span class="attention"[^>]*>⚠/,
    );
    expect(open).toContain('class="sources-modal"');
    expect(open).toMatch(/plane[\s\S]*Trabalho[\s\S]*ready/);
    expect(open).toContain("401 &lt;bad&gt; credentials");
    expect(open).toContain('data-action="close-sources"');
  });

  /** Uma configuração salva com erro também acende o aviso do botão. */
  test("config errors light the warning and show in the modal", () => {
    const broken = snapshot();
    broken.config_error = "invalid config config.toml: <bad>";

    const html = ready(view({ sourcesOpen: true }), broken);

    expect(html).toMatch(
      /data-action="sources"[^>]*>\s*<span class="attention"[^>]*>⚠/,
    );
    expect(html).toContain("invalid config config.toml: &lt;bad&gt;");
  });

  /** Os cards mostram o prazo quando existe, não o horário de atualização. */
  test("cards show due times but not update times", () => {
    const withDue = snapshot();
    withDue.boards[0].groups[1].columns[0].cards[0].due_at =
      "2026-09-29T15:00:00Z";

    const html = ready(view(), withDue);

    expect(html).toContain("due ");
    expect(html).not.toMatch(/class="card[\s\S]*updated/);
  });

  /**
   * Com a API fora do ar depois de já ter mostrado dados, os cards continuam
   * visíveis sob um aviso de que são antigos, com o motivo.
   */
  test("stale state keeps the cards under a warning", () => {
    const html = renderApp(
      {
        kind: "stale",
        fetchedAt: new Date("2026-09-28T10:00:00Z"),
        error: "HTTP 502",
        snapshot: snapshot(),
      },
      view(),
    );

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
