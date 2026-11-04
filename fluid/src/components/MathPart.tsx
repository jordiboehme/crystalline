/**
 * One formula, set by KaTeX.
 *
 * Its own module because of its own chunk: KaTeX and its stylesheet load when
 * a document actually holds math and never otherwise. It draws into an
 * element with `katex.render`, so no HTML string is built or injected. A
 * formula KaTeX cannot read stays on screen as the TeX that was written
 * (`throwOnError: false` paints it in the error colour), and so does one
 * the setter throws on for any other reason.
 *
 * With `report` (the formula popover's Preview) a formula KaTeX cannot read
 * shows KaTeX's message instead, so the person sees what to fix. Nothing
 * here ever throws.
 */

import "katex/dist/katex.min.css";

import katex from "katex";
import { useEffect, useRef } from "react";

export default function MathPart({
  tex,
  display,
  report = false,
}: {
  tex: string;
  display: boolean;
  report?: boolean;
}) {
  const ref = useRef<HTMLSpanElement>(null);
  useEffect(() => {
    const element = ref.current;
    if (element === null) {
      return;
    }
    try {
      katex.render(tex, element, {
        displayMode: display,
        throwOnError: report,
        strict: "ignore",
        trust: false,
      });
    } catch (error) {
      element.textContent =
        report && error instanceof Error ? error.message : tex;
    }
  }, [tex, display, report]);
  return <span ref={ref} />;
}
