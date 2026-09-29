/** Intervals offered in Settings, in minutes; 0 is off. */
export const AUTO_REFRESH_CHOICES = [0, 1, 5, 15, 30] as const;
export const DEFAULT_AUTO_REFRESH_MINUTES = 1;

/**
 * Calls `refresh` every `minutes` while the tab is visible; 0 never calls
 * it. A failed refresh is ignored: the next one tries again. Returns a
 * function that stops it.
 */
export function startAutoRefresh(
  minutes: number,
  refresh: () => Promise<unknown>,
  isHidden: () => boolean,
): () => void {
  if (minutes <= 0) {
    return () => {};
  }
  const timer = setInterval(() => {
    if (!isHidden()) {
      refresh().catch(() => {});
    }
  }, minutes * 60_000);
  return () => clearInterval(timer);
}
