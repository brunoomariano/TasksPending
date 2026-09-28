import type {
  Column,
  DashboardSnapshot,
  Group,
  PendingCard,
  SourceHealth,
} from "./contract.gen";
import type { ViewState } from "./state";

/** Choices of the person looking at the page. */
export interface View {
  /** Only this board's groups; `null` shows every board. */
  board: string | null;
  /** Groups (see `groupKey`) showing their empty columns. */
  expanded: ReadonlySet<string>;
  sourcesOpen: boolean;
}

const DEFAULT_VIEW: View = {
  board: null,
  expanded: new Set(),
  sourcesOpen: false,
};

/** Identifies a group across refreshes. */
export function groupKey(board: string, source: string): string {
  return `${board}/${source}`;
}

export function renderApp(state: ViewState, view: View = DEFAULT_VIEW): string {
  switch (state.kind) {
    case "loading":
      return renderMessage("Loading…");
    case "unavailable":
      return renderUnavailable(state.error);
    case "ready":
      return renderSnapshot(state.snapshot, view, "");
    case "stale":
      return renderSnapshot(
        state.snapshot,
        view,
        `<section class="stale" role="alert">
          API unavailable (${escapeHtml(state.error)}); showing data from
          ${escapeHtml(state.fetchedAt.toLocaleTimeString())}.
        </section>`,
      );
  }
}

function renderMessage(message: string): string {
  return `
    <header class="topbar">
      <div>
        <h1>TasksPending</h1>
        <p>${escapeHtml(message)}</p>
      </div>
    </header>
  `;
}

function renderUnavailable(error: string): string {
  return `
    <header class="topbar">
      <div>
        <h1>TasksPending</h1>
      </div>
    </header>
    <section class="unavailable" role="alert">
      <h2>API unavailable</h2>
      <p>${escapeHtml(error)}</p>
    </section>
  `;
}

function needsAttention(snapshot: DashboardSnapshot): boolean {
  return (
    snapshot.config_error !== null ||
    snapshot.sources.some(
      (s) => s.status === "degraded" || s.status === "failed",
    )
  );
}

function renderSnapshot(
  snapshot: DashboardSnapshot,
  view: View,
  banner: string,
): string {
  const boards = snapshot.boards.filter(
    (board) => view.board === null || board.name === view.board,
  );
  const warning = needsAttention(snapshot)
    ? '<span class="attention" aria-label="attention needed">⚠</span>'
    : "";
  return `
    <header class="topbar">
      <div>
        <h1>TasksPending</h1>
        <p>${escapeHtml(localTime(snapshot.generated_at))}</p>
      </div>
      <div class="actions">
        <button type="button" class="sources-button" data-action="sources">${warning} Sources</button>
        <button type="button" class="refresh" data-action="refresh">Refresh</button>
      </div>
    </header>
    ${banner}
    <nav class="filters">
      ${renderFilter("Todas", "", view.board === null)}
      ${snapshot.boards
        .map((board) =>
          renderFilter(board.name, board.name, view.board === board.name),
        )
        .join("")}
    </nav>
    <section class="board">
      ${boards
        .flatMap((board) =>
          board.groups.map((group) =>
            renderGroup(board.name, group, snapshot, view),
          ),
        )
        .join("")}
    </section>
    ${view.sourcesOpen ? renderSourcesModal(snapshot) : ""}
  `;
}

function renderFilter(label: string, value: string, active: boolean): string {
  return `
    <button type="button" class="filter${active ? " active" : ""}" data-board="${escapeHtml(value)}">
      ${escapeHtml(label)}
    </button>
  `;
}

function renderGroup(
  board: string,
  group: Group,
  snapshot: DashboardSnapshot,
  view: View,
): string {
  const key = groupKey(board, group.source);
  const expanded = view.expanded.has(key);
  const empty = group.columns.filter((column) => column.cards.length === 0);
  const shown = group.columns.filter(
    (column) => column.cards.length > 0 || expanded,
  );
  const health = snapshot.sources.find((s) => s.name === group.source);
  const toggle = empty.length
    ? `<button type="button" class="empty-toggle" data-toggle-empty="${escapeHtml(key)}">
        ${expanded ? "ocultar vazias" : `${empty.length} vazia${empty.length > 1 ? "s" : ""}`}
      </button>`
    : "";
  const columns = shown.length
    ? shown.map((column) => renderColumn(column, health)).join("")
    : '<p class="empty">Nada pendente.</p>';

  return `
    <section class="group">
      <header class="group-header">
        <h2>${escapeHtml(group.source)}</h2>
        <span class="group-board">${escapeHtml(board)}</span>
        ${toggle}
      </header>
      <div class="columns">${columns}</div>
    </section>
  `;
}

function renderColumn(
  column: Column,
  health: SourceHealth | undefined,
): string {
  const cards = column.cards.length
    ? column.cards.map(renderCard).join("")
    : `<p class="empty">— ${
        health?.last_refresh_at
          ? `updated ${escapeHtml(localTime(health.last_refresh_at))}`
          : "not refreshed yet"
      }</p>`;
  return `
    <section class="column">
      <h3>${escapeHtml(column.name)} <span>${column.cards.length}</span></h3>
      <div class="cards">${cards}</div>
    </section>
  `;
}

function renderCard(card: PendingCard): string {
  const title =
    card.url && isHttpUrl(card.url)
      ? `<a href="${escapeHtml(card.url)}" target="_blank" rel="noreferrer">${escapeHtml(card.title)}</a>`
      : escapeHtml(card.title);
  // The last fetch time is at the top; only a due time is per card.
  const due = card.due_at
    ? `<footer>due ${escapeHtml(localTime(card.due_at))}</footer>`
    : "";

  return `
    <article class="card severity-${card.severity}">
      <strong class="card-title">${title}</strong>
      <p>${escapeHtml(card.body)}</p>
      ${due}
    </article>
  `;
}

function renderSourcesModal(snapshot: DashboardSnapshot): string {
  const boardOf = (source: string) =>
    snapshot.boards
      .filter((board) => board.groups.some((group) => group.source === source))
      .map((board) => board.name)
      .join(", ");
  const rows = snapshot.sources
    .map(
      (source) => `
        <li class="source-row source-${source.status}">
          <div>
            <strong>${escapeHtml(source.name)}</strong>
            <span class="group-board">${escapeHtml(boardOf(source.name))}</span>
          </div>
          <div class="source-status">${source.status}</div>
          <div class="source-time">${
            source.last_refresh_at
              ? `last fetch ${escapeHtml(localTime(source.last_refresh_at))}`
              : "not refreshed yet"
          }</div>
          ${source.message ? `<p>${escapeHtml(source.message)}</p>` : ""}
        </li>
      `,
    )
    .join("");
  const configError = snapshot.config_error
    ? `<p class="stale" role="alert">Config not reloaded (the previous one is still running): ${escapeHtml(snapshot.config_error)}</p>`
    : "";

  return `
    <div class="modal-backdrop" data-action="close-sources">
      <section class="sources-modal" role="dialog" aria-modal="true" aria-label="Sources">
        <header>
          <h2>Sources</h2>
          <button type="button" class="close" data-action="close-sources" aria-label="Close">×</button>
        </header>
        ${configError}
        <ul>${rows}</ul>
      </section>
    </div>
  `;
}

/** An ISO timestamp in the browser's time zone and locale. */
function localTime(iso: string): string {
  const date = new Date(iso);
  return Number.isNaN(date.getTime())
    ? iso
    : date.toLocaleString(undefined, {
        dateStyle: "short",
        timeStyle: "short",
      });
}

function isHttpUrl(value: string): boolean {
  try {
    const { protocol } = new URL(value);
    return protocol === "http:" || protocol === "https:";
  } catch {
    return false;
  }
}

function escapeHtml(value: string): string {
  return value.replace(/[&<>"']/g, (char) => {
    const entities: Record<string, string> = {
      "&": "&amp;",
      "<": "&lt;",
      ">": "&gt;",
      '"': "&quot;",
      "'": "&#039;",
    };
    return entities[char] ?? char;
  });
}
