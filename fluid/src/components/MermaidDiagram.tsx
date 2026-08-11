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
import { mermaidConfig } from "../theme/mermaid";

export default function MermaidDiagram({ source }: { source: string }) {
  const { resolved } = useTheme();
  // `useId` is stable across renders and unique per instance, which is what
  // mermaid wants for the element it names its definitions after. Its colons
  // are not valid in a CSS identifier, so they come out.
  const id = `mermaid-${useId().replace(/[^a-zA-Z0-9_-]/g, "")}`;
  const [svg, setSvg] = useState<string | null>(null);

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
  // the source above; nothing from the document reaches here unparsed. A
  // diagram is usually narrower than the column it sits in, so it is centered,
  // and mermaid's own width attribute is left to hug its height.
  return (
    <div
      className="flex justify-center [&_svg]:h-auto [&_svg]:max-w-full"
      dangerouslySetInnerHTML={{ __html: svg }}
    />
  );
}
