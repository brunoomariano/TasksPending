import "./styles.css";

type CardSeverity = "info" | "warning" | "critical";

interface PendingCard {
  id: string;
  title: string;
  body: string;
  source: string;
  severity: CardSeverity;
  updated_at: string;
}

interface Section {
  name: string;
  cards: PendingCard[];
}

interface Lane {
  name: string;
  sections: Section[];
}

interface SourceHealth {
  name: string;
  status: string;
  last_refresh_at: string | null;
  message: string | null;
}

interface DashboardSnapshot {
  generated_at: string;
  lanes: Lane[];
  sources: SourceHealth[];
}

const fallbackSnapshot: DashboardSnapshot = {
  generated_at: new Date().toISOString(),
  sources: [
    {
      name: "sample",
      status: "ready",
      last_refresh_at: new Date().toISOString(),
      message: null,
    },
  ],
  lanes: [
    {
      name: "Work",
      sections: [
        {
          name: "Review",
          cards: [
            {
              id: "sample:review:1",
              title: "Design the first real source contract",
              body: "Define refresh, cache, and error semantics before binding to GitHub.",
              source: "sample",
              severity: "info",
              updated_at: new Date().toISOString(),
            },
          ],
        },
      ],
    },
  ],
};

async function loadSnapshot(): Promise<DashboardSnapshot> {
  try {
    const response = await fetch("/api/v1/snapshot");
    if (!response.ok) {
      throw new Error(`HTTP ${response.status}`);
    }
    return (await response.json()) as DashboardSnapshot;
  } catch {
    return fallbackSnapshot;
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

function render(snapshot: DashboardSnapshot): void {
  const app = document.querySelector<HTMLElement>("#app");
  if (!app) {
    return;
  }

  app.innerHTML = `
    <header class="topbar">
      <div>
        <h1>TasksPending</h1>
        <p>${snapshot.lanes.length} lanes · ${snapshot.sources.length} sources · ${snapshot.generated_at}</p>
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
  return `
    <article class="card severity-${card.severity}">
      <div class="card-title">
        <strong>${escapeHtml(card.title)}</strong>
        <span>${severityLabel(card.severity)}</span>
      </div>
      <p>${escapeHtml(card.body)}</p>
      <footer>${escapeHtml(card.source)} · ${escapeHtml(card.updated_at)}</footer>
    </article>
  `;
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

loadSnapshot().then(render);
