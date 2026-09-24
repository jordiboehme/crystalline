/**
 * The terminals of a room: one per `## ` section of the body.
 *
 * A terminal shows its heading and the first few lines under it, as written;
 * the full text is for the CRT reader of milestone 2. Frontmatter is skipped
 * and so is fenced code, where a `## ` is content rather than structure.
 */

import type { Section } from "./types";

/** How many lines a terminal screen shows. */
export const SECTION_LINES = 6;

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
  let fenced = false;
  for (; i < lines.length; i++) {
    const line = lines[i] ?? "";
    if (/^\s*(```|~~~)/.test(line)) fenced = !fenced;
    const heading = fenced ? null : /^##\s+(.+?)\s*#*\s*$/.exec(line);
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
