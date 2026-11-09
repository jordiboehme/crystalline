/**
 * The light paper copies of a dark page's diagrams, drawn one at a time.
 *
 * Every diagram on a dark page draws a second, light copy for the printer.
 * Each copy is a full mermaid render, and a page with a dozen diagrams that
 * starts them all at once paints no frame until the last one is done: about a
 * second and a half on a slow machine, during which a click lands late. So the
 * whole page shares this one queue. It starts one render, waits for it to
 * settle and gives the browser a task of its own before the next, so input and
 * paint get in between two renders.
 *
 * Only `MermaidDiagram` imports this, so it stays in the mermaid chunk.
 */

/** One paper copy waiting for its turn, or drawing. */
type Entry = { work: () => Promise<unknown> };

const waiting: Entry[] = [];
/** The entry whose render is running now, if any. */
let running: Entry | null = null;
/** Cancels the scheduled next step, while one is scheduled. */
let scheduled: (() => void) | null = null;

/**
 * Run `step` when the browser is idle, or after `delay` where it cannot say.
 * Safari and Playwright's WebKit have no `requestIdleCallback`, so a timer is
 * the only way to yield there. Returns the cancel.
 */
function schedule(step: () => void, delay: number): () => void {
  if (typeof window.requestIdleCallback === "function") {
    const handle = window.requestIdleCallback(step, { timeout: 2000 });
    return () => {
      window.cancelIdleCallback(handle);
    };
  }
  const timer = setTimeout(step, delay);
  return () => {
    clearTimeout(timer);
  };
}

/**
 * Schedule the next render, unless one is running or already scheduled.
 *
 * `fromIdle` is a queue that was empty: its first render waits 200 ms where
 * there is no idle callback, so the screen drawings of a page that just opened
 * come first. Between two renders a 0 ms timer is enough, since what matters
 * there is that the next render runs in a task of its own.
 */
function pump(fromIdle: boolean): void {
  if (running !== null || scheduled !== null || waiting.length === 0) {
    return;
  }
  scheduled = schedule(
    () => {
      scheduled = null;
      const entry = waiting.shift();
      if (entry === undefined) {
        return;
      }
      running = entry;
      // Settled either way before the next one starts: a render that fails
      // must not stop the copies behind it. A `work` that throws before it
      // returns its promise is caught here too.
      void Promise.resolve()
        .then(entry.work)
        .catch(() => undefined)
        .finally(() => {
          if (running === entry) {
            running = null;
          }
          pump(false);
        });
    },
    fromIdle ? 200 : 0,
  );
}

/**
 * Queue one paper copy. `work` starts the render and returns its promise.
 * Returns the cancel, which takes a waiting entry out of the queue. A render
 * that already runs cannot be stopped: its caller ignores the result, and the
 * queue still waits for it, so two renders never overlap.
 */
export function queuePaper(work: () => Promise<unknown>): () => void {
  const entry: Entry = { work };
  const idle = running === null && scheduled === null && waiting.length === 0;
  waiting.push(entry);
  pump(idle);
  return () => {
    const at = waiting.indexOf(entry);
    if (at !== -1) {
      waiting.splice(at, 1);
    }
    // Nothing left to draw: the scheduled step would only find the queue
    // empty, so it is not left behind as a stray timer.
    if (waiting.length === 0 && running === null && scheduled !== null) {
      scheduled();
      scheduled = null;
    }
  };
}
