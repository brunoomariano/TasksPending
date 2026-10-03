import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import {
  loadWeather,
  renderWeather,
  startWeather,
  WEATHER_INTERVAL_MS,
  weatherIcon,
  type WeatherReport,
} from "./weather";

const report: WeatherReport = {
  place: "Recife",
  temperature: "29°C",
  wind: "←15km/h",
  condition: "partly-cloudy-day",
};

describe("loadWeather", () => {
  /** With weather available, the widget gets the report the API sent. */
  test("returns the report when weather is available", async () => {
    const requested: string[] = [];
    const ok = async (input: string) => {
      requested.push(input);
      return Response.json({ available: true, ...report });
    };

    expect(await loadWeather(ok)).toEqual(report);
    expect(requested).toEqual(["/api/v1/weather"]);
  });

  /** Off Omarchy or offline the API says so, and there is nothing to show. */
  test("returns nothing when weather is unavailable", async () => {
    const unavailable = async () => Response.json({ available: false });

    expect(await loadWeather(unavailable)).toBeNull();
  });

  /**
   * Weather is optional: an older daemon without the route, a network
   * failure or a body in another shape all mean no widget, never an error.
   */
  test("returns nothing on any failure", async () => {
    const missing = async () => new Response("not found", { status: 404 });
    const offline = async (): Promise<Response> => {
      throw new TypeError("Failed to fetch");
    };
    const notJson = async () => new Response("<html>");
    const body = (value: unknown) => async () => Response.json(value);

    expect(await loadWeather(missing)).toBeNull();
    expect(await loadWeather(offline)).toBeNull();
    expect(await loadWeather(notJson)).toBeNull();
    expect(await loadWeather(body(null))).toBeNull();
    expect(await loadWeather(body({ available: true }))).toBeNull();
    expect(
      await loadWeather(body({ available: true, ...report, temperature: 29 })),
    ).toBeNull();
    expect(
      await loadWeather(body({ available: true, ...report, place: "" })),
    ).toBeNull();
  });
});

describe("renderWeather", () => {
  /** The widget shows what Omarchy shows: place, temperature and wind. */
  test("shows the temperature, place and wind", () => {
    const html = renderWeather(report);

    expect(html).toContain('<span class="weather-temperature">29°C</span>');
    expect(html).toContain('<span class="weather-place">Recife</span>');
    expect(html).toContain('<span class="weather-wind">Wind ←15km/h</span>');
    expect(html).toContain("<svg");
  });

  /** Text from the weather service can't inject markup into the page. */
  test("escapes the texts", () => {
    const html = renderWeather({
      ...report,
      place: '<img src=x onerror="alert(1)">',
      wind: "<b>9</b>",
    });

    expect(html).not.toContain("<img");
    expect(html).not.toContain("<b>");
    expect(html).toContain("&lt;img src=x onerror=&quot;alert(1)&quot;&gt;");
  });
});

describe("weatherIcon", () => {
  /** Every condition the API names has its own drawing and a spoken name. */
  test("draws a distinct icon for each known condition", () => {
    const conditions = [
      "clear-day",
      "clear-night",
      "partly-cloudy-day",
      "partly-cloudy-night",
      "cloudy",
      "fog",
      "rain",
      "sleet",
      "snow",
      "thunder",
    ];

    const icons = conditions.map(weatherIcon);

    expect(new Set(icons).size).toBe(conditions.length);
    for (const svg of icons) {
      expect(svg).toMatch(/^<svg class="icon weather-icon" /);
      expect(svg).toMatch(/role="img" aria-label="[A-Z][a-z ]+"/);
    }
    expect(weatherIcon("clear-night")).toContain('aria-label="Clear night"');
  });

  /**
   * An unknown condition, or a keyword from a newer API, falls back to a
   * plain cloud that says nothing about the weather.
   */
  test("falls back to a generic cloud", () => {
    const generic = weatherIcon("unknown");

    expect(generic).toContain('aria-hidden="true"');
    expect(generic).not.toContain("aria-label");
    expect(weatherIcon("volcanic-ash")).toBe(generic);
    expect(weatherIcon("constructor")).toBe(generic);
    expect(weatherIcon("cloudy")).not.toBe(generic);
  });
});

describe("startWeather", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  /**
   * The widget keeps itself current: it loads at once and then every ten
   * minutes, on its own timer, until stopped.
   */
  test("loads at once and then every interval", async () => {
    const load = vi.fn(async () => report);
    const shown: (WeatherReport | null)[] = [];
    const weather = startWeather({
      load,
      show: (value) => shown.push(value),
      isHidden: () => false,
    });

    await vi.advanceTimersByTimeAsync(0);
    expect(load).toHaveBeenCalledTimes(1);
    expect(shown).toEqual([report]);

    await vi.advanceTimersByTimeAsync(WEATHER_INTERVAL_MS - 1);
    expect(load).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(load).toHaveBeenCalledTimes(2);

    weather.stop();
    await vi.advanceTimersByTimeAsync(3 * WEATHER_INTERVAL_MS);
    expect(load).toHaveBeenCalledTimes(2);
    expect(WEATHER_INTERVAL_MS).toBe(600_000);
  });

  /**
   * A hidden tab doesn't ask for weather; when it becomes visible again the
   * page asks at once instead of waiting for the next interval.
   */
  test("skips hidden tabs and reloads on request", async () => {
    let hidden = true;
    const load = vi.fn(async () => report);
    const weather = startWeather({
      load,
      show: () => {},
      isHidden: () => hidden,
    });

    await vi.advanceTimersByTimeAsync(2 * WEATHER_INTERVAL_MS);
    expect(load).not.toHaveBeenCalled();

    hidden = false;
    weather.refresh();
    await vi.advanceTimersByTimeAsync(0);
    expect(load).toHaveBeenCalledTimes(1);
  });

  /**
   * When weather goes away (offline, API down) the widget is removed, and
   * it comes back by itself on a later load.
   */
  test("shows nothing while unavailable and recovers", async () => {
    const answers: (() => Promise<WeatherReport | null>)[] = [
      async () => report,
      async () => null,
      async () => {
        throw new Error("boom");
      },
      async () => report,
    ];
    const shown: (WeatherReport | null)[] = [];
    startWeather({
      load: () => answers.shift()!(),
      show: (value) => shown.push(value),
      isHidden: () => false,
    });

    await vi.advanceTimersByTimeAsync(3 * WEATHER_INTERVAL_MS);

    expect(shown).toEqual([report, null, null, report]);
  });

  /** A slow answer that arrives after stopping is not shown. */
  test("ignores an answer that arrives after stopping", async () => {
    let answer: (value: WeatherReport | null) => void = () => {};
    const show = vi.fn();
    const weather = startWeather({
      load: () => new Promise((resolve) => (answer = resolve)),
      show,
      isHidden: () => false,
    });

    weather.stop();
    answer(report);
    await vi.advanceTimersByTimeAsync(0);

    expect(show).not.toHaveBeenCalled();
  });
});
