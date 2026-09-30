import { describe, expect, it } from "vitest";

import { crtLines } from "../ui/crt";
import { sectionsOf } from "./sections";

describe("sectionsOf", () => {
  it("returns one section per level-two heading with its first lines", () => {
    const md = [
      "---",
      "type: manifest",
      "## not a heading inside frontmatter",
      "---",
      "# Title",
      "intro",
      "## Scope",
      "",
      "Line one",
      "Line two",
      "### Sub stays inside",
      "## Routing",
      "Go here",
    ].join("\n");
    expect(sectionsOf(md)).toEqual([
      {
        heading: "Scope",
        lines: ["Line one", "Line two", "### Sub stays inside"],
      },
      { heading: "Routing", lines: ["Go here"] },
    ]);
  });

  it("caps the lines of a section at six", () => {
    const body = Array.from({ length: 10 }, (_, i) => `l${i}`).join("\n");
    const [section] = sectionsOf(`## A\n${body}`);
    expect(section?.lines).toHaveLength(6);
  });

  it("ignores headings inside fenced code", () => {
    expect(sectionsOf("## A\n```\n## not\n```\n")).toHaveLength(1);
  });

  it("returns nothing for a body without sections", () => {
    expect(sectionsOf("just text")).toEqual([]);
  });

  it("closes a fence only on the marker that opened it", () => {
    const md = [
      "## A",
      "```",
      "~~~",
      "## inside backticks",
      "~~~",
      "```",
      "## B",
      "~~~",
      "```",
      "## inside tildes",
      "```",
      "~~~",
      "## C",
    ].join("\n");
    expect(sectionsOf(md).map((s) => s.heading)).toEqual(["A", "B", "C"]);
  });

  it("finds the same headings and occurrences as the CRT reader", () => {
    const md = [
      "---",
      "## not in frontmatter",
      "---",
      "## Notes",
      "one",
      "```js",
      "## not in backticks",
      "~~~",
      "## still not",
      "```",
      "## **Bold** heading ##",
      "~~~",
      "```",
      "## not in tildes",
      "~~~",
      "### Sub",
      "## Notes",
      "two",
      "## Trailing #",
    ].join("\n");
    const fromSections = new Map<string, number>();
    const ours = sectionsOf(md).map((s) => {
      const nth = fromSections.get(s.heading) ?? 0;
      fromSections.set(s.heading, nth + 1);
      return [s.heading, nth] as const;
    });
    const { sections } = crtLines(md);
    const theirs = [...sections.entries()]
      .flatMap(([heading, at]) =>
        at.map((line, nth) => ({ heading, nth, line })),
      )
      .sort((a, b) => a.line - b.line)
      .map((e) => [e.heading, e.nth] as const);
    expect(ours).toEqual(theirs);
    expect(ours).toEqual([
      ["Notes", 0],
      ["**Bold** heading", 0],
      ["Notes", 1],
      ["Trailing", 0],
    ]);
  });
});
