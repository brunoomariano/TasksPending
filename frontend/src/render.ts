import type { SnapshotResult } from "./api";
import type {
  CardSeverity,
  DashboardSnapshot,
  Lane,
  PendingCard,
  Section,
} from "./contract.gen";

export function renderApp(result: SnapshotResult): string {
  return result.ok
    ? renderSnapshot(result.snapshot)
    : renderUnavailable(result.error);
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

function renderSnapshot(snapshot: DashboardSnapshot): string {
  return `
    <header class="topbar">
      <div>
        <h1>TasksPending</h1>
        <p>${snapshot.lanes.length} lanes · ${snapshot.sources.length} sources · ${escapeHtml(snapshot.generated_at)}</p>
      </div>
    </header>
    <section class="lanes">
      ${snapshot.lanes.map(renderLane).join("")}
    </section>
  `;
}

function renderLane(lane: Lane): string {
  return `
    <article class="lane">
      <h2>${escapeHtml(lane.name)}</h2>
      ${lane.sections.map(renderSection).join("")}
    </article>
  `;
}

function renderSection(section: Section): string {
  return `
    <section class="section">
      <h3>${escapeHtml(section.name)}</h3>
      <div class="cards">
        ${section.cards.map(renderCard).join("")}
      </div>
    </section>
  `;
}

function renderCard(card: PendingCard): string {
  const title =
    card.url && isHttpUrl(card.url)
      ? `<a href="${escapeHtml(card.url)}" target="_blank" rel="noreferrer">${escapeHtml(card.title)}</a>`
      : escapeHtml(card.title);

  return `
    <article class="card severity-${card.severity}">
      <div class="card-title">
        <strong>${title}</strong>
        <span>${severityLabel(card.severity)}</span>
      </div>
      <p>${escapeHtml(card.body)}</p>
      <footer>${escapeHtml(card.source)} · ${escapeHtml(card.updated_at)}</footer>
    </article>
  `;
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
