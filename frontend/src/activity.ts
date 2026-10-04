/**
 * Events that count as the user looking at the page: things done on
 * purpose. The pointer merely crossing the window (or the window taking
 * focus because the pointer is over it) is not one: it would start a new
 * sitting, and clear what changed, without anyone having looked.
 */
export const ACTIVITY_EVENTS = [
  "pointerdown",
  "keydown",
  // Reading by scrolling presses nothing.
  "wheel",
  "scroll",
  "touchstart",
] as const;

/** Activity is reported at most this often. */
const REPORT_EVERY_MS = 60_000;

interface ActivityTarget {
  addEventListener: (
    type: string,
    listener: () => void,
    options?: { passive: boolean },
  ) => void;
  removeEventListener: (type: string, listener: () => void) => void;
}

/**
 * Calls `report` when the user is active on the page: at once on the first
 * activity, then at most once a minute while it goes on. Returns a function
 * that stops it.
 */
export function startActivity(
  report: () => void,
  options: { target: ActivityTarget; now: () => number },
): () => void {
  let last = Number.NEGATIVE_INFINITY;
  const onActivity = () => {
    const now = options.now();
    if (now - last >= REPORT_EVERY_MS) {
      last = now;
      report();
    }
  };
  for (const type of ACTIVITY_EVENTS) {
    // Passive: the listener never blocks scrolling.
    options.target.addEventListener(type, onActivity, { passive: true });
  }
  return () => {
    for (const type of ACTIVITY_EVENTS) {
      options.target.removeEventListener(type, onActivity);
    }
  };
}
