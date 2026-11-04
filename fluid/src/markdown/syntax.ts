/**
 * The Markdown the reading page parses, in one place.
 *
 * `markdownSyntax` is every plugin of the page's pipeline that changes how
 * text is parsed (GFM's tables, task lists and footnotes, the status label
 * syntax, math), with the options it takes. `markdown/pipeline.ts` runs it first and
 * then the page's rendering-only plugins; the editor's footnote read
 * (`editor/footnoteSyntax.ts`) runs exactly this list, so a footnote is found
 * by the parser that draws it. A plugin that adds syntax goes here; a test
 * (`syntax.test.ts`) fails if one is added to the page's pipeline alone.
 */

import type { PluggableList } from "unified";

import remarkGfm from "remark-gfm";

import { remarkStatusSyntax } from "./directives";
import { remarkMath, remarkMathOptions } from "./math";

export const markdownSyntax: PluggableList = [
  remarkGfm,
  remarkStatusSyntax,
  [remarkMath, remarkMathOptions],
];
