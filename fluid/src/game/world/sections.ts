/**
 * The terminals of a room: one per `## ` section of the body.
 *
 * A terminal shows its heading and the first few lines under it, as written;
 * the full text is for the CRT reader. Frontmatter is skipped and so is
 * fenced code, where a `## ` is content rather than structure.
 *
 * The rules are the reader's own (`ui/crt.ts`), because a terminal names its
 * section by heading and occurrence and the reader must find the same one:
 * a fence opens on a line starting with ```` ``` ```` or `~~~` and closes only
 * on a line starting with the same marker, so a `~~~` inside a backtick fence
 * is content and the reverse too, and a fence left open runs to the end.
 */

import type { Section } from "./types";

/** How many lines a terminal screen shows. */
export const SECTION_LINES = 6;

const FENCE = /^\s*(```|~~~)/;
const SECTION = /^##\s+(.+?)\s*#*\s*$/;

/** The `## ` sections of a markdown body, in document order. */
export function sectionsOf(markdown: string): Section[] {
  const lines = markdown.split(/\r?\n/);
  let i = 0;
  if (lines[0]?.trim() === "---") {
    i = 1;
    while (i < lines.length && lines[i]?.trim() !== "---") i++;
    i++;
  }
  const sections: Section[] = [];
  let current: Section | null = null;
  // The marker of the open fence, or null outside one.
  let fence: string | null = null;
  for (; i < lines.length; i++) {
    const line = lines[i] ?? "";
    // A fence line is never a heading itself; it stays one of the lines.
    let heading: RegExpExecArray | null = null;
    if (fence === null) {
      fence = FENCE.exec(line)?.[1] ?? null;
      if (fence === null) heading = SECTION.exec(line);
    } else if (line.trim().startsWith(fence)) {
      fence = null;
    }
    if (heading !== null && !line.startsWith("###")) {
      current = { heading: heading[1] ?? "", lines: [] };
      sections.push(current);
    } else if (
      current !== null &&
      line.trim() !== "" &&
      current.lines.length < SECTION_LINES
    ) {
      current.lines.push(line.trim());
    }
  }
  return sections;
}
