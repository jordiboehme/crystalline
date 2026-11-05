/**
 * Test helpers for a drag in jsdom, which has no `DataTransfer` and no
 * `DragEvent`: a transfer that keeps what was set on it the way a browser's
 * does, and drag events carrying it and a point on the screen.
 *
 * `getData("Text")` reads `text/plain`, as in a browser, so CodeMirror's own
 * text drop finds the reference a picture drag carries: a test that lets it
 * through would see a copy land beside the move.
 */

/** A browser's own name for a type: `Text` is `text/plain`. */
function normal(type: string): string {
  const lower = type.toLowerCase();
  return lower === "text" ? "text/plain" : lower;
}

export class FakeTransfer {
  readonly data = new Map<string, string>();
  readonly files: File[];
  /** Whether `types` names "Files": a dragover lists the type, not the files. */
  readonly namesFiles: boolean;
  effectAllowed = "uninitialized";
  dropEffect = "none";
  dragImage: Element | null = null;

  constructor(options: { files?: File[]; namesFiles?: boolean } = {}) {
    this.files = options.files ?? [];
    this.namesFiles = options.namesFiles ?? this.files.length > 0;
  }

  get types(): string[] {
    return [...(this.namesFiles ? ["Files"] : []), ...this.data.keys()];
  }

  setData(type: string, value: string): void {
    this.data.set(normal(type), value);
  }

  getData(type: string): string {
    return this.data.get(normal(type)) ?? "";
  }

  clearData(): void {
    this.data.clear();
  }

  setDragImage(image: Element): void {
    this.dragImage = image;
  }
}

/** Where a drag event points, in client coordinates. */
export interface Point {
  x: number;
  y: number;
}

/**
 * Dispatches a drag event of `type` on `target`, carrying `transfer` and,
 * when given, a point; answers the event so a test can read
 * `defaultPrevented`.
 */
export function fireDrag(
  target: EventTarget,
  type: string,
  transfer: FakeTransfer | null,
  at?: Point,
  relatedTarget: EventTarget | null = null,
): MouseEvent {
  const event = new MouseEvent(type, {
    bubbles: true,
    cancelable: true,
    relatedTarget,
    ...(at ? { clientX: at.x, clientY: at.y } : {}),
  });
  Object.defineProperty(event, "dataTransfer", { value: transfer });
  target.dispatchEvent(event);
  return event;
}
