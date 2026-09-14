/**
 * One mermaid diagram, drawn from the fence that declared it.
 *
 * Its own module because of its own chunk: mermaid is the heaviest thing this
 * app can load, and a document without a diagram must not pay for it. Nothing
 * else imports it, so the bundler gives it a chunk of its own that arrives when
 * `Markdown` meets a mermaid fence and never otherwise.
 *
 * A diagram that will not parse is not an error screen. The source is what the
 * author wrote, so a broken one falls back to showing it, the way an unknown
 * language falls back to plain code.
 */

import mermaid from "mermaid";
import { useEffect, useId, useRef, useState } from "react";

import { useTheme } from "../theme/context";
import { mermaidConfig } from "../theme/mermaid";
import DiagramOverlay from "./DiagramOverlay";
import DiagramToolbar from "./DiagramToolbar";
import { unclampDiagram, unclampWideDiagram } from "./wideDiagram";

export default function MermaidDiagram({ source }: { source: string }) {
  const { resolved } = useTheme();
  // `useId` is stable across renders and unique per instance, which is what
  // mermaid wants for the element it names its definitions after. Its colons
  // are not valid in a CSS identifier, so they come out.
  const id = `mermaid-${useId().replace(/[^a-zA-Z0-9_-]/g, "")}`;
  const [drawn, setDrawn] = useState<{ svg: string; wide: boolean } | null>(
    null,
  );
  // Both asked for, never inferred, and neither of them remembered: the width
  // is this reader's decision about this diagram on this visit, and a diagram
  // that came back wide because somebody once widened it would be a preference
  // nobody set.
  const [fullWidth, setFullWidth] = useState(false);
  const [fullWindow, setFullWindow] = useState(false);
  const fullWindowRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    let live = true;
    // The shared configuration: sanitizing mode, the app's own palette for
    // this scheme and `suppressErrorRendering`, which is what keeps a failed
    // render from leaving mermaid's error graphic on `document.body`, outside
    // React's tree, where nothing here could ever take it down again. The
    // fallback below - the source the author wrote - is this component's
    // answer to a diagram that will not parse.
    mermaid.initialize(mermaidConfig(resolved === "dark"));
    mermaid
      .render(id, source)
      .then((result) => {
        if (live) {
          setDrawn(unclampWideDiagram(result.svg));
        }
      })
      .catch(() => {
        if (live) {
          setDrawn(null);
        }
      });
    return () => {
      live = false;
    };
  }, [id, source, resolved]);

  if (drawn === null) {
    return (
      <pre className="overflow-x-auto font-mono text-xs text-slate-500 dark:text-slate-400">
        {source}
      </pre>
    );
  }
  // The markup is mermaid's own output, produced by its sanitizing mode from
  // the source above; nothing from the document reaches here unparsed.
  //
  // Full width forces the path the measurement below chooses on its own past
  // the threshold: the same unclamp, the same scroll region, asked for by the
  // reader rather than decided by the drawing's size.
  const wide = fullWidth || drawn.wide;
  const markup = fullWidth ? unclampDiagram(drawn.svg) : drawn.svg;
  return (
    // `group` and `relative` are the toolbar's: it floats in this corner and
    // shows itself when the pointer or the keyboard is anywhere in here.
    <div className="group relative">
      {wide ? (
        // Past the threshold the diagram keeps its own width and this
        // container scrolls, because scaling it to fit leaves labels too small
        // to read. Why this and not a click-to-expand lightbox: a focusable
        // scroll region is the WAI pattern for exactly this, it needs no focus
        // trap, no Escape contract and no new dependency, and the reader never
        // leaves the document. Seeing a huge diagram whole is the thing it
        // does not give, and that is what the full-window overlay is for -
        // opened deliberately, never in the reader's way.
        //
        // `max-w-full` is deliberately absent: it is a clamp of the same kind
        // mermaid's inline `max-width` was, so leaving it on would undo the
        // unclamp and nothing would change on screen. `shrink-0` is here for
        // the same reason one step further on: a flex item shrinks to its line
        // by default, which quietly scales the diagram back down to the column
        // width and leaves the container with nothing to scroll.
        //
        // The rest of the class list is the touch contract. A horizontal
        // scroller nested in a scrolling page is the common case on a phone,
        // where nearly every diagram is wider than the viewport:
        // `overscroll-x-contain` keeps a fling from walking the page or firing
        // the browser's back gesture, the `touch-pan-*` trio keeps both axes
        // (and pinch zoom) pannable so a swipe that starts over the diagram can
        // still scroll the page, and the mask fades the scrollable edge so a
        // diagram wider than its container reads as scrollable rather than as
        // cut off.
        <div
          role="region"
          aria-label="Diagram, scrollable sideways"
          tabIndex={0}
          className="flex touch-pan-x touch-pan-y touch-pinch-zoom justify-start overflow-x-auto overscroll-x-contain rounded [mask-image:linear-gradient(to_right,black_calc(100%_-_1.5rem),transparent)] focus-visible:ring-2 focus-visible:ring-accent-600 focus-visible:outline-none dark:focus-visible:ring-accent-400 [&_svg]:h-auto [&_svg]:shrink-0"
          dangerouslySetInnerHTML={{ __html: markup }}
        />
      ) : (
        // A diagram is usually narrower than the column it sits in, so it is
        // centered, and mermaid's own width attribute is left to hug its
        // height.
        <div
          className="flex justify-center [&_svg]:h-auto [&_svg]:max-w-full"
          dangerouslySetInnerHTML={{ __html: markup }}
        />
      )}
      {/*
        After the diagram in source order, not before it: the toolbar's own
        icons are `<svg>` elements too, and a reader - or a test - that asks
        this container for its diagram should find the drawing rather than a
        glyph.
      */}
      <DiagramToolbar
        fullWidth={fullWidth}
        onToggleFullWidth={() => {
          setFullWidth((on) => !on);
        }}
        onOpenFullWindow={() => {
          setFullWindow(true);
        }}
        fullWindowRef={fullWindowRef}
      />
      {fullWindow && (
        // The diagram as mermaid drew it, not as this column shows it: the
        // overlay sizes the root itself, and a width this page forced on it
        // would be one scale factor too many.
        <DiagramOverlay
          svg={drawn.svg}
          returnFocusTo={fullWindowRef}
          onClose={() => {
            setFullWindow(false);
          }}
        />
      )}
    </div>
  );
}
