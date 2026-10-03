/**
 * A small weather widget beside the clock. It is optional context: the API
 * reports weather only where Omarchy's weather commands work, and anywhere
 * else (or on any error) the widget is simply not on the page.
 */

/** What `GET /api/v1/weather` sends when weather is available. */
export type WeatherReport = {
  place: string;
  /** As Omarchy formats it, e.g. `29°C`. */
  temperature: string;
  /** As Omarchy formats it, e.g. `←15km/h`. */
  wind: string;
  /** A keyword of {@link CONDITIONS}, or `unknown`. */
  condition: string;
};

type Fetch = (input: string, init?: RequestInit) => Promise<Response>;

const REQUEST_TIMEOUT_MS = 10_000;
/** The API caches a report for this long, so asking sooner changes nothing. */
export const WEATHER_INTERVAL_MS = 10 * 60_000;

/** The current weather, or `null` when there is none to show. */
export async function loadWeather(
  fetchFn: Fetch = (input, init) => fetch(input, init),
  timeoutMs = REQUEST_TIMEOUT_MS,
): Promise<WeatherReport | null> {
  try {
    const response = await fetchFn("/api/v1/weather", {
      signal: AbortSignal.timeout(timeoutMs),
    });
    if (!response.ok) {
      return null;
    }
    const body: unknown = await response.json();
    if (typeof body !== "object" || body === null) {
      return null;
    }
    const { available, place, temperature, wind, condition } = body as Record<
      string,
      unknown
    >;
    const text = (value: unknown): value is string =>
      typeof value === "string" && value !== "";
    if (
      available !== true ||
      !text(place) ||
      !text(temperature) ||
      !text(wind) ||
      !text(condition)
    ) {
      return null;
    }
    return { place, temperature, wind, condition };
  } catch {
    return null;
  }
}

const CLOUD_OVER =
  '<path d="M4 14.899A7 7 0 1 1 15.71 8h1.79a4.5 4.5 0 0 1 2.5 8.242"/>';

/** Stroke icons (Lucide shapes) and a spoken name per condition keyword. */
const CONDITIONS = new Map<string, { label: string; paths: string }>([
  [
    "clear-day",
    {
      label: "Clear",
      paths:
        '<circle cx="12" cy="12" r="4"/><path d="M12 2v2"/><path d="M12 20v2"/><path d="m4.93 4.93 1.41 1.41"/><path d="m17.66 17.66 1.41 1.41"/><path d="M2 12h2"/><path d="M20 12h2"/><path d="m6.34 17.66-1.41 1.41"/><path d="m19.07 4.93-1.41 1.41"/>',
    },
  ],
  [
    "clear-night",
    {
      label: "Clear night",
      paths: '<path d="M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z"/>',
    },
  ],
  [
    "partly-cloudy-day",
    {
      label: "Partly cloudy",
      paths:
        '<path d="M12 2v2"/><path d="m4.93 4.93 1.41 1.41"/><path d="M20 12h2"/><path d="m19.07 4.93-1.41 1.41"/><path d="M15.947 12.65a4 4 0 0 0-5.925-4.128"/><path d="M13 22H7a5 5 0 1 1 4.9-6H13a3 3 0 0 1 0 6Z"/>',
    },
  ],
  [
    "partly-cloudy-night",
    {
      label: "Partly cloudy night",
      paths:
        '<path d="M10.188 8.5A6 6 0 0 1 16 4a1 1 0 0 0 6 6 6 6 0 0 1-3 5.197"/><path d="M13 16a3 3 0 1 1 0 6H7a5 5 0 1 1 4.9-6Z"/>',
    },
  ],
  [
    "cloudy",
    {
      label: "Cloudy",
      paths:
        '<path d="M17.5 21H9a7 7 0 1 1 6.71-9h1.79a4.5 4.5 0 1 1 0 9Z"/><path d="M22 10a3 3 0 0 0-3-3h-2.207a5.502 5.502 0 0 0-10.702.5"/>',
    },
  ],
  [
    "fog",
    {
      label: "Fog",
      paths: `${CLOUD_OVER}<path d="M16 17H7"/><path d="M17 21H9"/>`,
    },
  ],
  [
    "rain",
    {
      label: "Rain",
      paths: `${CLOUD_OVER}<path d="M16 14v6"/><path d="M8 14v6"/><path d="M12 16v6"/>`,
    },
  ],
  [
    "sleet",
    {
      label: "Sleet",
      paths: `${CLOUD_OVER}<path d="M16 14v2"/><path d="M8 14v2"/><path d="M16 20h.01"/><path d="M8 20h.01"/><path d="M12 16v2"/><path d="M12 22h.01"/>`,
    },
  ],
  [
    "snow",
    {
      label: "Snow",
      paths: `${CLOUD_OVER}<path d="M8 15h.01"/><path d="M8 19h.01"/><path d="M12 17h.01"/><path d="M12 21h.01"/><path d="M16 15h.01"/><path d="M16 19h.01"/>`,
    },
  ],
  [
    "thunder",
    {
      label: "Thunderstorm",
      paths:
        '<path d="M6 16.326A7 7 0 1 1 15.71 8h1.79a4.5 4.5 0 0 1 .5 8.973"/><path d="m13 12-3 5h4l-3 5"/>',
    },
  ],
]);

/** Shown for `unknown` and for keywords this page doesn't know yet. */
const GENERIC_CLOUD =
  '<path d="M17.5 19H9a7 7 0 1 1 6.71-9h1.79a4.5 4.5 0 1 1 0 9Z"/>';

/** The icon for a condition keyword; a plain cloud when it isn't known. */
export function weatherIcon(condition: string): string {
  const known = CONDITIONS.get(condition);
  const name = known
    ? `role="img" aria-label="${known.label}"`
    : 'aria-hidden="true"';
  return `<svg class="icon weather-icon" viewBox="0 0 24 24" ${name}>${known?.paths ?? GENERIC_CLOUD}</svg>`;
}

/** The inside of the widget: icon and temperature, then place and wind. */
export function renderWeather(report: WeatherReport): string {
  return `<p class="weather-now">${weatherIcon(report.condition)}<span class="weather-temperature">${escapeHtml(report.temperature)}</span></p><p class="weather-details"><span class="weather-place">${escapeHtml(report.place)}</span><span class="weather-wind">Wind ${escapeHtml(report.wind)}</span></p>`;
}

function escapeHtml(value: string): string {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

export type WeatherOptions = {
  load: () => Promise<WeatherReport | null>;
  /** Called after each load; `null` means there is no weather to show. */
  show: (report: WeatherReport | null) => void;
  isHidden: () => boolean;
  intervalMs?: number;
};

/**
 * Loads the weather now and then every `intervalMs` while the tab is
 * visible, independent of the dashboard polling. A failed load shows
 * nothing; the next one tries again. `refresh` loads right away.
 */
export function startWeather(options: WeatherOptions): {
  refresh: () => void;
  stop: () => void;
} {
  let stopped = false;
  const refresh = () => {
    if (stopped || options.isHidden()) {
      return;
    }
    options
      .load()
      .catch(() => null)
      .then((report) => {
        if (!stopped) {
          options.show(report);
        }
      });
  };
  refresh();
  const timer = setInterval(refresh, options.intervalMs ?? WEATHER_INTERVAL_MS);
  return {
    refresh,
    stop: () => {
      stopped = true;
      clearInterval(timer);
    },
  };
}

/**
 * Keeps a weather widget as the first item of `hero` (the side of the clock
 * opposite the controls) while there is weather to show, and no element at
 * all while there isn't. Returns a function that stops it.
 */
export function mountWeather(hero: HTMLElement): () => void {
  let widget: HTMLElement | null = null;
  let shown = "";
  const weather = startWeather({
    load: () => loadWeather(),
    show: (report) => {
      if (!report) {
        widget?.remove();
        widget = null;
        shown = "";
        return;
      }
      if (!widget) {
        widget = document.createElement("aside");
        widget.className = "weather";
        widget.setAttribute("aria-label", "Weather");
        hero.prepend(widget);
      }
      const html = renderWeather(report);
      if (html !== shown) {
        shown = html;
        widget.innerHTML = html;
      }
    },
    isHidden: () => document.hidden,
  });
  const onVisible = () => weather.refresh();
  document.addEventListener("visibilitychange", onVisible);
  return () => {
    weather.stop();
    document.removeEventListener("visibilitychange", onVisible);
    widget?.remove();
  };
}
