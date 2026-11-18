/**
 * The quiet "Copy" at the top right of a code block.
 *
 * A text button, not an icon, drawn only while the block is hovered or the
 * button has focus, and never on paper. The confirmation arrives in a polite
 * live region that is in the document from the start and empties itself
 * (`useSaid`), so a screen reader hears it and the next copy is announced
 * again.
 */

import { copyText, useSaid } from "./useSaid";
import { FOCUS_RING } from "./primitives";

export default function CopyCode({ text }: { text: string }) {
  const [said, say] = useSaid();
  return (
    <div
      data-anchor-skip=""
      className="absolute top-1.5 right-2 flex items-center gap-2 text-caption select-none print:hidden"
    >
      <span role="status" className="text-ink-3">
        {said}
      </span>
      <button
        type="button"
        aria-label="Copy code"
        onClick={() => {
          copyText(text, say);
        }}
        className={`rounded px-1 text-ink-3 opacity-0 group-hover:opacity-100 [@media(hover:none)]:opacity-100 hover:text-ink focus-visible:opacity-100 ${FOCUS_RING}`}
      >
        Copy
      </button>
    </div>
  );
}
