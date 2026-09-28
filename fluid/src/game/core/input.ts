/**
 * Keyboard and mouse, read the way a first-person game reads them.
 *
 * Keys are tracked by `KeyboardEvent.code`, the physical position, so WASD
 * sits under the same fingers on a German, French or US layout. The loop asks
 * two different questions: `held` for movement, which lasts as long as the
 * key is down, and `pressed` for commands like inverting the mouse, which
 * is true once per physical press and ignores the operating system's
 * auto-repeat.
 *
 * Every held key is dropped when the window loses focus, the tab is hidden or
 * pointer lock ends. The browser never sends the keyup for a key released
 * while it was looking elsewhere, and without this the player would keep
 * walking into a wall after an alt-tab.
 *
 * Pointer lock is feature-detected rather than assumed. Newer browsers return
 * a Promise from `requestPointerLock`, older Safari returns nothing;
 * `unadjustedMovement` (raw mouse input) exists only in Chromium and is
 * retried without when refused; and Chrome refuses a new lock for about a
 * second after the last one ended, so a request inside that window is
 * dropped instead of raising an error the player cannot act on.
 *
 * `typed` keeps a separate ordered log of the same fresh presses `pressed`
 * tracks per code, for the level cheat's word: it needs the order two
 * presses came in, which a per-code set cannot give. It leaves out
 * auto-repeat, same as `pressed`, but also leaves out a press held with
 * Ctrl, Cmd or Alt, which `pressed` still answers true for.
 *
 * A keydown another listener cancelled before this one heard it (a
 * `preventDefault` in the capture phase, the way the connecting screen
 * takes the key that skips it) is not taken at all: it is neither held
 * nor pressed nor typed. The keyup is always taken.
 */

/** The live input state of one canvas. */
export interface Input {
  /** Whether a key is down right now. */
  held(code: string): boolean;
  /** Whether a key was pressed since the last call for it. Consumes the press. */
  pressed(code: string): boolean;
  /**
   * The codes of the keys pressed since the last call, oldest first, and
   * consumes them: one per physical press (no auto-repeat), none pressed
   * with Ctrl, Cmd or Alt, and at most `TYPED_CAP`, the oldest dropped
   * first. The session reads it once per tick for the level cheat's word,
   * which needs the order `pressed` cannot give. A code enters the log on
   * such a press and leaves it at the next call to `typed`, which drains
   * the whole log at once, or earlier, when `clear` or `dropPresses` empties
   * it; unlike the edges `pressed` reads, a code here is not tied to
   * whether its own press was read yet.
   */
  typed(): string[];
  /** The mouse movement since the last call, in pixels, while locked. */
  takeLook(): { dx: number; dy: number };
  /** Whether the pointer is locked to the target. */
  readonly locked: boolean;
  /**
   * When the lock last ended, on `performance.now()`'s clock, or
   * `-Infinity` before it ever did. `requestLock` does nothing until
   * `RELOCK_DELAY_MS` after it, so the pause screen waits that long too.
   */
  readonly lockEndedAt: number;
  /**
   * Forgets every held key, every unconsumed press and the mouse movement
   * gathered so far. The session calls it when an overlay that reads the
   * keys itself (the CRT reader or the level select) opens or closes, so a
   * key pressed on one side of the switch is not replayed as a command or a
   * step on the other.
   */
  clear(): void;
  /**
   * Forgets every unconsumed press and leaves held keys and the mouse as
   * they are. The session calls it when it enters a room, so a command key
   * pressed for the room left behind (a use hit while the next room was
   * loading) is not replayed in the new one, while a W held through the
   * door keeps walking.
   */
  dropPresses(): void;
  /** Asks for pointer lock. Call from a user gesture (a click). */
  requestLock(): void;
  /** Removes every listener and releases the lock. */
  dispose(): void;
}

/** How long Chrome refuses a new lock after one ended. */
export const RELOCK_DELAY_MS = 1100;

/**
 * How many typed codes wait at most between two reads of `typed`. A tick
 * reads them 35 times a second, so only a stalled loop ever fills it.
 */
export const TYPED_CAP = 16;

/** The shape of `Element.requestPointerLock` across browsers: newer ones return a Promise, older Safari returns nothing. */
type LockRequest = (options?: {
  unadjustedMovement?: boolean;
}) => Promise<void> | undefined;

/** Input for `target`, listening on its window and document. */
export function createInput(
  target: HTMLElement,
  doc: Document = document,
): Input {
  const win = doc.defaultView ?? window;
  const down = new Set<string>();
  const edges = new Set<string>();
  const log: string[] = [];
  let dx = 0;
  let dy = 0;
  let lockEndedAt = -Infinity;

  const isLocked = () => doc.pointerLockElement === target;

  const onKeyDown = (e: KeyboardEvent) => {
    // A key something before this listener cancelled was taken there (the
    // connecting screen's skip, M4 C26 and C9): no command, no step.
    if (e.defaultPrevented) return;
    if (!e.repeat) {
      edges.add(e.code);
      if (!e.ctrlKey && !e.metaKey && !e.altKey) {
        log.push(e.code);
        if (log.length > TYPED_CAP) log.shift();
      }
    }
    down.add(e.code);
  };
  const onKeyUp = (e: KeyboardEvent) => {
    down.delete(e.code);
  };
  const clear = () => {
    down.clear();
  };
  const onLockChange = () => {
    clear();
    if (!isLocked()) lockEndedAt = performance.now();
  };
  const onMouseMove = (e: MouseEvent) => {
    if (!isLocked()) return;
    dx += e.movementX;
    dy += e.movementY;
  };

  win.addEventListener("keydown", onKeyDown);
  win.addEventListener("keyup", onKeyUp);
  win.addEventListener("blur", clear);
  doc.addEventListener("visibilitychange", clear);
  doc.addEventListener("pointerlockchange", onLockChange);
  doc.addEventListener("mousemove", onMouseMove);

  return {
    held: (code) => down.has(code),
    pressed(code) {
      const was = edges.has(code);
      edges.delete(code);
      return was;
    },
    typed() {
      return log.splice(0);
    },
    takeLook() {
      const look = { dx, dy };
      dx = 0;
      dy = 0;
      return look;
    },
    get locked() {
      return isLocked();
    },
    get lockEndedAt() {
      return lockEndedAt;
    },
    clear() {
      down.clear();
      edges.clear();
      log.length = 0;
      dx = 0;
      dy = 0;
    },
    dropPresses() {
      edges.clear();
      log.length = 0;
    },
    requestLock() {
      // `requestPointerLock` is feature-detected: jsdom and browsers without
      // pointer lock support have no such method on the element.
      const request = (
        target as { requestPointerLock?: LockRequest }
      ).requestPointerLock?.bind(target);
      if (request === undefined || isLocked()) return;
      if (performance.now() - lockEndedAt < RELOCK_DELAY_MS) return;
      const raw = request({ unadjustedMovement: true });
      raw?.catch(() => {
        // Raw input refused (not Chromium, or the OS says no): take the
        // ordinary lock instead. A second refusal is left alone; the next
        // click tries again.
        request()?.catch(() => {});
      });
    },
    dispose() {
      win.removeEventListener("keydown", onKeyDown);
      win.removeEventListener("keyup", onKeyUp);
      win.removeEventListener("blur", clear);
      doc.removeEventListener("visibilitychange", clear);
      doc.removeEventListener("pointerlockchange", onLockChange);
      doc.removeEventListener("mousemove", onMouseMove);
      if (isLocked()) doc.exitPointerLock();
      down.clear();
      edges.clear();
      log.length = 0;
    },
  };
}
