import type {
  Board,
  CardSeverity,
  Column,
  DashboardSnapshot,
  Group,
  PendingCard,
  SourceHealth,
} from "./contract.gen";
import type { ViewState } from "./state";

/** `board` is the index of the selected tab. */
export function renderApp(state: ViewState, board = 0): string {
  switch (state.kind) {
    case "loading":
      return renderMessage("Loading…");
    case "unavailable":
      return renderUnavailable(state.error);
    case "ready":
      return renderSnapshot(state.snapshot, board, "");
    case "stale":
      return renderSnapshot(
        state.snapshot,
        board,
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

function renderSnapshot(
  snapshot: DashboardSnapshot,
  board: number,
  banner: string,
): string {
  const selected = snapshot.boards[Math.min(board, snapshot.boards.length - 1)];
  return `
    <header class="topbar">
      <div>
        <h1>TasksPending</h1>
        <p>${snapshot.sources.length} sources · ${escapeHtml(localTime(snapshot.generated_at))}</p>
      </div>
      <button type="button" class="refresh" data-action="refresh">Refresh</button>
    </header>
    ${banner}
    ${
      snapshot.config_error
        ? `<section class="stale" role="alert">Config not reloaded (the previous one is still running): ${escapeHtml(snapshot.config_error)}</section>`
        : ""
    }
    <nav class="tabs">
      ${snapshot.boards.map((b, index) => renderTab(b, index, b === selected)).join("")}
    </nav>
    <ul class="sources">
      ${snapshot.sources.map(renderSource).join("")}
    </ul>
    ${selected ? renderBoard(selected) : '<p class="empty">No sources configured.</p>'}
  `;
}

function renderTab(board: Board, index: number, active: boolean): string {
  const count = board.groups
    .flatMap((group) => group.columns)
    .reduce((total, column) => total + column.cards.length, 0);
  return `
    <button type="button" class="tab${active ? " active" : ""}" data-board="${index}">
      ${index + 1} ${escapeHtml(board.name)} <span>${count}</span>
    </button>
  `;
}

function renderSource(source: SourceHealth): string {
  const message = source.message ? `: ${escapeHtml(source.message)}` : "";
  return `
    <li class="source source-${source.status}">
      <strong>${escapeHtml(source.name)}</strong> ${source.status}${message}
    </li>
  `;
}

function renderBoard(board: Board): string {
  return `
    <section class="board">
      ${board.groups.map(renderGroup).join("")}
    </section>
  `;
}

function renderGroup(group: Group): string {
  return `
    <section class="group">
      <h2>${escapeHtml(group.source)}</h2>
      <div class="columns">
        ${group.columns.map(renderColumn).join("")}
      </div>
    </section>
  `;
}

function renderColumn(column: Column): string {
  const cards = column.cards.length
    ? column.cards.map(renderCard).join("")
    : '<p class="empty">—</p>';
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
      <div class="card-title">
        <strong>${title}</strong>
        <span>${severityLabel(card.severity)}</span>
      </div>
      <p>${escapeHtml(card.body)}</p>
      ${due}
    </article>
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

function severityLabel(severity: CardSeverity): string {
  switch (severity) {
    case "critical":
      return "Critical";
    case "warning":
      return "Warning";
    case "info":
      return "Info";
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
