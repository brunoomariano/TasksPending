/**
 * A big clock in the style of clock-tui's "bricks" font: each glyph is 6
 * blocks wide and 5 tall, and a block is twice as tall as it is wide (the
 * shape of a terminal cell).
 */

/**
 * Each row alternates the length of an "off" run and an "on" run, starting
 * with "off": `[0, 2, 2, 2]` is `██  ██`.
 */
// prettier-ignore
const GLYPHS: Record<string, number[][]> = {
  "0": [[0, 6], [0, 2, 2, 2], [0, 2, 2, 2], [0, 2, 2, 2], [0, 6]],
  "1": [[0, 4], [2, 2], [2, 2], [2, 2], [0, 6]],
  "2": [[0, 6], [4, 2], [0, 6], [0, 2], [0, 6]],
  "3": [[0, 6], [4, 2], [0, 6], [4, 2], [0, 6]],
  "4": [[0, 2, 2, 2], [0, 2, 2, 2], [0, 6], [4, 2], [4, 2]],
  "5": [[0, 6], [0, 2], [0, 6], [4, 2], [0, 6]],
  "6": [[0, 6], [0, 2], [0, 6], [0, 2, 2, 2], [0, 6]],
  "7": [[0, 6], [4, 2], [4, 2], [4, 2], [4, 2]],
  "8": [[0, 6], [0, 2, 2, 2], [0, 6], [0, 2, 2, 2], [0, 6]],
  "9": [[0, 6], [0, 2, 2, 2], [0, 6], [4, 2], [0, 6]],
  ":": [[], [2, 2], [], [2, 2], []],
};

const GLYPH_WIDTH = 6;
const GLYPH_ROWS = 5;
const BLOCK_HEIGHT = 2;
const SPACING = 1;

/** Local time as 24-hour `HH:MM:SS`. */
export function clockText(now: Date): string {
  return [now.getHours(), now.getMinutes(), now.getSeconds()]
    .map((part) => String(part).padStart(2, "0"))
    .join(":");
}

/** Local date, e.g. "Monday, September 28, 2026" in the browser's locale. */
export function dateText(now: Date): string {
  return now.toLocaleDateString(undefined, {
    weekday: "long",
    year: "numeric",
    month: "long",
    day: "numeric",
  });
}

/** `text` drawn in blocks as an SVG that scales to its box. */
export function clockSvg(text: string): string {
  const glyphs = [...text];
  const rects: string[] = [];
  glyphs.forEach((char, index) => {
    const left = index * (GLYPH_WIDTH + SPACING);
    (GLYPHS[char] ?? []).forEach((row, line) => {
      let x = left;
      row.forEach((length, run) => {
        if (run % 2 === 1) {
          rects.push(
            `<rect x="${x}" y="${line * BLOCK_HEIGHT}" width="${length}" height="${BLOCK_HEIGHT}"/>`,
          );
        }
        x += length;
      });
    });
  });
  const width = glyphs.length * (GLYPH_WIDTH + SPACING) - SPACING;
  const height = GLYPH_ROWS * BLOCK_HEIGHT;
  return `<svg class="clock-digits" viewBox="0 0 ${width} ${height}" role="img" aria-label="${text}" shape-rendering="crispEdges">${rects.join("")}</svg>`;
}

/**
 * Keeps `element` showing the date and a big clock, redrawn on each new
 * second. Returns a function that stops it.
 */
export function startClock(element: HTMLElement): () => void {
  let timer: ReturnType<typeof setTimeout> | undefined;
  let shown = "";
  const tick = () => {
    const now = new Date();
    const text = clockText(now);
    if (text !== shown) {
      shown = text;
      element.innerHTML = `<p class="clock-date">${dateText(now)}</p>${clockSvg(text)}`;
    }
    timer = setTimeout(tick, 1000 - now.getMilliseconds());
  };
  tick();
  return () => clearTimeout(timer);
}
