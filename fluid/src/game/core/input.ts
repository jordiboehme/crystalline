/**
 * Keyboard and mouse, read the way a first-person game reads them.
 *
 * Keys are tracked by `KeyboardEvent.code`, the physical position, so WASD
 * sits under the same fingers on a German, French or US layout. The loop asks
 * two different questions: `held` for movement, which lasts as long as the
 * key is down, and `pressed` for commands like switching the look, which is
 * true once per physical press and ignores the operating system's
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
 */

/** The live input state of one canvas. */
export interface Input {
  /** Whether a key is down right now. */
  held(code: string): boolean;
  /** Whether a key was pressed since the last call for it. Consumes the press. */
  pressed(code: string): boolean;
  /** The mouse movement since the last call, in pixels, while locked. */
  takeLook(): { dx: number; dy: number };
  /** Whether the pointer is locked to the target. */
  readonly locked: boolean;
  /**
   * Forgets every held key, every unconsumed press and the mouse movement
   * gathered so far. The session calls it when an overlay that read the
   * keys itself (the CRT reader) closes, so a key pressed for the overlay
   * is not replayed as a command or a step once the game has the keys back.
   */
  clear(): void;
  /**
   * Forgets every unconsumed press and leaves held keys and the mouse as
   * they are. The session calls it when it enters a room, so a command key
   * pressed for the room left behind (an E hit while the next room was
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
const RELOCK_DELAY_MS = 1100;

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
  let dx = 0;
  let dy = 0;
  let lockEndedAt = -Infinity;

  const isLocked = () => doc.pointerLockElement === target;

  const onKeyDown = (e: KeyboardEvent) => {
    if (!e.repeat) edges.add(e.code);
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
    takeLook() {
      const look = { dx, dy };
      dx = 0;
      dy = 0;
      return look;
    },
    get locked() {
      return isLocked();
    },
    clear() {
      down.clear();
      edges.clear();
      dx = 0;
      dy = 0;
    },
    dropPresses() {
      edges.clear();
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
    },
  };
}
