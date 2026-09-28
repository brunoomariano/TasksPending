import type {
  Column,
  DashboardSnapshot,
  Group,
  Icon,
  PendingCard,
  SourceHealth,
} from "./contract.gen";
import type { ViewState } from "./state";

/** Stroke icons (Lucide shapes), inline so the page needs no icon font. */
const ICON_PATHS = {
  refresh: '<path d="M21 12a9 9 0 1 1-2.64-6.36L21 8"/><path d="M21 3v5h-5"/>',
  sources: '<path d="M22 12h-4l-3 9L9 3l-3 9H2"/>',
  warning:
    '<path d="m21.73 18-8-14a2 2 0 0 0-3.46 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3"/><path d="M12 9v4"/><path d="M12 17h.01"/>',
  close: '<path d="M18 6 6 18"/><path d="m6 6 12 12"/>',
  eye: '<path d="M2.06 12.35a1 1 0 0 1 0-.7 10.75 10.75 0 0 1 19.88 0 1 1 0 0 1 0 .7 10.75 10.75 0 0 1-19.88 0"/><circle cx="12" cy="12" r="3"/>',
  eyeOff:
    '<path d="M10.73 5.08A10.43 10.43 0 0 1 12 5c7 0 10 7 10 7a13.16 13.16 0 0 1-1.67 2.68"/><path d="M6.61 6.61A13.53 13.53 0 0 0 2 12s3 7 10 7a9.74 9.74 0 0 0 5.39-1.61"/><path d="m2 2 20 20"/>',
  inbox:
    '<path d="M22 12h-6l-2 3h-4l-2-3H2"/><path d="M5.45 5.11 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11"/>',
  clock: '<circle cx="12" cy="12" r="10"/><path d="M12 6v6l4 2"/>',
  ok: '<circle cx="12" cy="12" r="10"/><path d="m9 12 2 2 4-4"/>',
  failed:
    '<circle cx="12" cy="12" r="10"/><path d="m15 9-6 6"/><path d="m9 9 6 6"/>',
  layers:
    '<path d="m12.83 2.18 8.58 3.9a1 1 0 0 1 0 1.83l-8.58 3.9a2 2 0 0 1-1.66 0L2.6 7.9a1 1 0 0 1 0-1.83l8.58-3.9a2 2 0 0 1 1.66 0Z"/><path d="m2 12 8.58 3.91a2 2 0 0 0 1.66 0L21 12"/><path d="m2 17 8.58 3.91a2 2 0 0 0 1.66 0L21 17"/>',
} as const;

type IconName = keyof typeof ICON_PATHS;

function icon(name: IconName): string {
  return `<svg class="icon icon-${name}" viewBox="0 0 24 24" aria-hidden="true">${ICON_PATHS[name]}</svg>`;
}

const STATUS_ICON: Record<SourceHealth["status"], IconName> = {
  ready: "ok",
  refreshing: "refresh",
  degraded: "warning",
  failed: "failed",
};

/** Choices of the person looking at the page. */
export interface View {
  /** Only this board's groups; `null` shows every board. */
  board: string | null;
  /** Groups (see `groupKey`) showing their empty columns. */
  expanded: ReadonlySet<string>;
  /** Columns (see `columnKey`) showing every card, not just the first ones. */
  openColumns: ReadonlySet<string>;
  sourcesOpen: boolean;
}

export const DEFAULT_VIEW: View = {
  board: null,
  expanded: new Set(),
  openColumns: new Set(),
  sourcesOpen: false,
};

/** Cards a column shows before "show more". */
export const COLUMN_LIMIT = 5;

/** Identifies a group across refreshes. */
export function groupKey(board: string, source: string): string {
  return `${board}/${source}`;
}

/** Identifies a column across refreshes. */
export function columnKey(
  board: string,
  source: string,
  column: string,
): string {
  return `${board}/${source}/${column}`;
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
  const icons = new Map(
    snapshot.boards.flatMap((board) =>
      board.groups.map((group) => [group.source, group.icon] as const),
    ),
  );
  const pending = new Set(
    boards.flatMap((board) =>
      board.groups.flatMap((group) =>
        group.columns.flatMap((column) => column.cards.map((card) => card.id)),
      ),
    ),
  ).size;
  const warning = needsAttention(snapshot)
    ? `<span class="attention" aria-label="attention needed">${icon("warning")}</span>`
    : icon("sources");
  return `
    <header class="topbar">
      <div>
        <h1>TasksPending</h1>
        <p class="meta">
          <span>${pending} pending</span>
          <span class="dot" aria-hidden="true">·</span>
          ${icon("clock")} ${escapeHtml(localTime(snapshot.generated_at))}
        </p>
      </div>
      <div class="actions">
        <button type="button" class="button sources-button" data-action="sources" aria-label="Sources">${warning}<span class="label">Sources</span></button>
        <button type="button" class="button primary refresh" data-action="refresh" aria-label="Refresh">${icon("refresh")}<span class="label">Refresh</span></button>
      </div>
    </header>
    ${banner}
    <nav class="filters">
      ${renderFilter("All", "", view.board === null)}
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
    ${view.sourcesOpen ? renderSourcesModal(snapshot, icons) : ""}
  `;
}

/** The configured source icon, or a generic one. */
function sourceIcon(source: Icon | null | undefined): string {
  if (!source || !isHttpUrl(source.url)) {
    return icon("layers");
  }
  const dark =
    source.dark_url && isHttpUrl(source.dark_url)
      ? `<source srcset="${escapeHtml(source.dark_url)}" media="(prefers-color-scheme: dark)">`
      : "";
  return `<picture class="source-icon">${dark}<img src="${escapeHtml(source.url)}" alt="" loading="lazy" referrerpolicy="no-referrer"></picture>`;
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
    ? `<button type="button" class="empty-toggle" data-toggle-empty="${escapeHtml(key)}" title="${expanded ? "Hide" : "Show"} empty columns">
        ${expanded ? `${icon("eyeOff")} hide empty` : `${icon("eye")} ${empty.length} empty`}
      </button>`
    : "";
  const columns = shown.length
    ? shown
        .map((column) => {
          const key = columnKey(board, group.source, column.name);
          return renderColumn(column, key, view.openColumns.has(key), health);
        })
        .join("")
    : `<p class="empty">${icon("inbox")} Nothing pending.</p>`;

  return `
    <section class="group">
      <header class="group-header">
        <h2>${sourceIcon(group.icon)}${escapeHtml(group.source)}</h2>
        <span class="group-board">${escapeHtml(board)}</span>
        ${toggle}
      </header>
      <div class="columns">${columns}</div>
    </section>
  `;
}

function renderColumn(
  column: Column,
  key: string,
  open: boolean,
  health: SourceHealth | undefined,
): string {
  const hidden = open ? 0 : Math.max(0, column.cards.length - COLUMN_LIMIT);
  const more =
    hidden > 0 || (open && column.cards.length > COLUMN_LIMIT)
      ? `<button type="button" class="link-button" data-more-column="${escapeHtml(key)}">${
          open ? "Show less" : `Show ${hidden} more`
        }</button>`
      : "";
  const cards = column.cards.length
    ? column.cards
        .slice(0, open ? undefined : COLUMN_LIMIT)
        .map(renderCard)
        .join("") + more
    : `<p class="empty">${icon("inbox")} ${
        health?.last_refresh_at
          ? `updated ${escapeHtml(localTime(health.last_refresh_at))}`
          : "not refreshed yet"
      }</p>`;
  return `
    <section class="column">
      <h3>${escapeHtml(column.name)} <span class="count">${column.cards.length}</span></h3>
      <div class="cards">${cards}</div>
    </section>
  `;
}

function cardTitle(card: PendingCard): string {
  return card.url && isHttpUrl(card.url)
    ? `<a href="${escapeHtml(card.url)}" target="_blank" rel="noreferrer">${escapeHtml(card.title)}</a>`
    : escapeHtml(card.title);
}

function renderCard(card: PendingCard): string {
  const title = cardTitle(card);
  // No date line: sources write the due time or event time into the body,
  // and the last fetch time is at the top. No severity marker either.

  return `
    <article class="card">
      <strong class="card-title">${title}</strong>
      <p title="${escapeHtml(card.body)}">${escapeHtml(card.body)}</p>
    </article>
  `;
}

function renderSourcesModal(
  snapshot: DashboardSnapshot,
  icons: Map<string, Icon | null>,
): string {
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
            ${sourceIcon(icons.get(source.name))}
            <strong>${escapeHtml(source.name)}</strong>
            <span class="group-board">${escapeHtml(boardOf(source.name))}</span>
          </div>
          <div class="source-status">${icon(STATUS_ICON[source.status])}${source.status}</div>
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
          <button type="button" class="close" data-action="close-sources" aria-label="Close">${icon("close")}</button>
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
