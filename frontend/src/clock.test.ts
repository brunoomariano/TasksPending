import { describe, expect, test } from "vitest";
import { clockSvg, clockText } from "./clock";

describe("clock", () => {
  /** The clock shows the local time as 24-hour HH:MM:SS. */
  test("formats local time with seconds", () => {
    expect(clockText(new Date(2026, 8, 28, 7, 5, 3))).toBe("07:05:03");
    expect(clockText(new Date(2026, 8, 28, 23, 59, 59))).toBe("23:59:59");
  });

  /**
   * Digits are drawn with clock-tui's "bricks" font: 6×5 blocks per glyph,
   * each block twice as tall as wide, one block between glyphs.
   */
  test("draws each glyph as blocks", () => {
    const one = clockSvg("1");
    expect(one).toContain('viewBox="0 0 6 10"');
    // "1": top bar of 4, three stems of 2 at x=2, bottom bar of 6.
    expect(one).toContain('<rect x="0" y="0" width="4" height="2"/>');
    expect(one).toContain('<rect x="2" y="2" width="2" height="2"/>');
    expect(one).toContain('<rect x="0" y="8" width="6" height="2"/>');
    expect(one.match(/<rect/g)).toHaveLength(5);

    const time = clockSvg("12:34:56");
    expect(time).toContain('viewBox="0 0 55 10"');
    // The colon, third glyph, starts at x = 2 × 7.
    expect(time).toContain('<rect x="16" y="2" width="2" height="2"/>');
  });

  /** The time is also given as text for screen readers. */
  test("labels the drawing with the time", () => {
    expect(clockSvg("09:30:00")).toContain('aria-label="09:30:00"');
  });
});
