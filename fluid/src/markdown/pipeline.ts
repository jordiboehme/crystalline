/**
 * The reading page's whole remark pipeline: the shared syntax
 * (`syntax.ts`), then the plugins that only transform the tree.
 */

import type { PluggableList } from "unified";

import { remarkCallouts } from "./callouts";
import { remarkCrystallineDirectives } from "./directives";
import { remarkGithubMath } from "./math";
import { markdownSyntax } from "./syntax";
import { remarkTaskState } from "./taskState";

/**
 * The page's whole remark pipeline for a document read with brace text
 * `shown`: the shared syntax, then the plugins that only transform the tree.
 * 1. The status labels' transform names them; it reads `shown` for the brace
 *    text, so it runs on the tree the syntax just built.
 * 2. GitHub's inline math reads the inline code nodes the parser built and
 *    checks the `$` beside them in the source.
 * 3. The callouts read the quotes and strip the head paragraph's first line.
 * 4. The task state (`[~]`) is last: a callout's first line may hold the
 *    words a box would be read from, so the head is cut first.
 */
export function remarkPipeline(
  shown: Parameters<typeof remarkCrystallineDirectives>[0],
): PluggableList {
  return [
    ...markdownSyntax,
    remarkCrystallineDirectives(shown),
    remarkGithubMath,
    remarkCallouts,
    remarkTaskState,
  ];
}
