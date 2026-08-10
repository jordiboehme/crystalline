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
import { useEffect, useId, useState } from "react";

import { useTheme } from "../theme/context";

export default function MermaidDiagram({ source }: { source: string }) {
  const { resolved } = useTheme();
  // `useId` is stable across renders and unique per instance, which is what
  // mermaid wants for the element it names its definitions after. Its colons
  // are not valid in a CSS identifier, so they come out.
  const id = `mermaid-${useId().replace(/[^a-zA-Z0-9_-]/g, "")}`;
  const [svg, setSvg] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    // `strict` is mermaid's own sanitizing mode: the diagram is drawn from
    // text somebody wrote into the knowledge base, and labels in it are text.
    mermaid.initialize({
      startOnLoad: false,
      securityLevel: "strict",
      // A failed render must leave NOTHING behind. Mermaid's default is to
      // append its own error graphic to `document.body`, outside React's
      // tree, where nothing here can ever take it down again: the bombs
      // stack up at the bottom of the page until a reload. The fallback
      // below - the source the author wrote - is this component's answer to
      // a diagram that will not parse.
      suppressErrorRendering: true,
      theme: resolved === "dark" ? "dark" : "default",
    });
    mermaid
      .render(id, source)
      .then((result) => {
        if (live) {
          setSvg(result.svg);
        }
      })
      .catch(() => {
        if (live) {
          setSvg(null);
        }
      });
    return () => {
      live = false;
    };
  }, [id, source, resolved]);

  if (svg === null) {
    return (
      <pre className="overflow-x-auto font-mono text-xs text-slate-500 dark:text-slate-400">
        {source}
      </pre>
    );
  }
  // The markup is mermaid's own output, produced by its sanitizing mode from
  // the source above; nothing from the document reaches here unparsed.
  return <div dangerouslySetInnerHTML={{ __html: svg }} />;
}
