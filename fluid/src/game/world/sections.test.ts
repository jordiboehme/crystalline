import { describe, expect, it } from "vitest";

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
});
