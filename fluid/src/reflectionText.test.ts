/**
 * The section text a quote is looked up in has to be byte for byte the text
 * the engine's `resolve_anchor` searches (core `split_sections`: the heading
 * line and the body, the id line left out). The fixture is the core plan's
 * own `PAGE`, so the two sides are pinned to one example.
 */
import { describe, expect, it } from "vitest";

import {
  firstTitle,
  TOP_SECTION,
  ageText,
  isIdLine,
  locateQuote,
  occurrenceAt,
  pageOffset,
  sectionTexts,
  stripIdLines,
  roomText,
  stripProvenance,
} from "./reflectionText";

const PAGE =
  "# D\n\n## Night deploys\n<!-- r:7f3a9c2e -->\n\nDeploys run at night, after the batch.\nDeploys run at night.\n\n## Other\n<!-- r:0b1c2d3e -->\n\nOther text.\n";
const SECTIONS = [
  { id: "7f3a9c2e", heading: "Night deploys", line: 3 },
  { id: "0b1c2d3e", heading: "Other", line: 9 },
];

describe("sectionTexts", () => {
  it("cuts the page the way the engine does, id lines left out", () => {
    const texts = sectionTexts(PAGE, SECTIONS);
    expect(texts.map((s) => s.id)).toEqual([
      TOP_SECTION,
      "7f3a9c2e",
      "0b1c2d3e",
    ]);
    expect(texts[0]?.text).toBe("# D\n");
    expect(texts[1]?.text).toBe(
      "## Night deploys\n\nDeploys run at night, after the batch.\nDeploys run at night.\n",
    );
    expect(texts[2]?.text).toBe("## Other\n\nOther text.\n");
    expect(texts.some((s) => s.text.includes("<!-- r:"))).toBe(false);
  });

  it("counts the frontmatter in the lines and leaves it out of the top", () => {
    const withFront = `---\ntitle: D\nreflection:\n  7f3a9c2e:\n    reflected: ab\n---\n${PAGE}`;
    const texts = sectionTexts(withFront, [
      { id: "7f3a9c2e", heading: "Night deploys", line: 9 },
      { id: "0b1c2d3e", heading: "Other", line: 15 },
    ]);
    expect(texts[0]?.text).toBe("# D\n");
    expect(texts[1]?.text.startsWith("## Night deploys\n\nDeploys")).toBe(true);
  });

  it("finds the headings when the lines were counted on a longer frontmatter", () => {
    // Defensive: the server counts `line` on the markdown it answers (core
    // Task 12), but a line that does not hold its heading is searched for
    // rather than trusted.
    const stripped = `---\ntitle: D\n---\n${PAGE}`;
    const texts = sectionTexts(stripped, [
      { id: "7f3a9c2e", heading: "Night deploys", line: 9 },
      { id: "0b1c2d3e", heading: "Other", line: 15 },
    ]);
    expect(texts).toEqual(sectionTexts(PAGE, SECTIONS));
  });

  it("leaves out a heading that is nowhere on the page", () => {
    expect(
      sectionTexts(PAGE, [
        ...SECTIONS,
        { id: "deadbeef", heading: "Gone", line: 40 },
      ]).map((s) => s.id),
    ).toEqual([TOP_SECTION, "7f3a9c2e", "0b1c2d3e"]);
  });

  it("reads a CRLF file in LF space, like the engine", () => {
    const texts = sectionTexts(PAGE.replace(/\n/g, "\r\n"), SECTIONS);
    expect(texts[2]?.text).toBe("## Other\n\nOther text.\n");
  });

  it("keeps a heading with no id as its own section, addressed by heading", () => {
    const texts = sectionTexts("# D\n\n## New\n\nTyped by hand.\n", [
      { id: null, heading: "New", line: 3 },
    ]);
    expect(texts[1]).toEqual({
      id: null,
      heading: "New",
      text: "## New\n\nTyped by hand.\n",
    });
  });
});

describe("sectionTexts against the engine", () => {
  it("keeps the trailing empty element like split_sections: a last section ends in a line break", () => {
    const texts = sectionTexts(PAGE, SECTIONS);
    expect(texts[2]?.text.endsWith("Other text.\n")).toBe(true);
    // A page with no ## at all: the whole body is the top, ending in "\n".
    expect(sectionTexts("# D\n\nJust text.\n", [])).toEqual([
      { id: TOP_SECTION, heading: "", text: "# D\n\nJust text.\n" },
    ]);
    // No final line break: nothing to keep.
    expect(
      sectionTexts("# D\n\n## A\n\nEnd.", [
        { id: null, heading: "A", line: 3 },
      ])[1]?.text,
    ).toBe("## A\n\nEnd.");
  });

  it("reads a heading the way parse_heading does", () => {
    const page = "# D\n\n   ## Indented ##\n\nText.\n\n##\n\nEmpty one.\n";
    const texts = sectionTexts(page, [
      { id: null, heading: "Indented", line: 3 },
      { id: null, heading: "", line: 7 },
    ]);
    expect(texts.map((s) => s.heading)).toEqual(["", "Indented", ""]);
    expect(texts[1]?.text).toBe("   ## Indented ##\n\nText.\n");
    expect(texts[2]?.text).toBe("##\n\nEmpty one.\n");
  });

  it("never drops a section whose line is a level-two heading, whatever its text says", () => {
    const texts = sectionTexts("# D\n\n## Renamed\n\nBody.\n", [
      { id: "7f3a9c2e", heading: "Old name", line: 3 },
    ]);
    expect(texts.map((s) => s.id)).toEqual([TOP_SECTION, "7f3a9c2e"]);
  });
});

describe("quotes", () => {
  const night = sectionTexts(PAGE, SECTIONS)[1]?.text ?? "";

  it("numbers an occurrence the way resolve_anchor counts it", () => {
    const second = night.lastIndexOf("Deploys run at night");
    expect(
      occurrenceAt(night, "Deploys run at night", night.indexOf("Deploys")),
    ).toBe(1);
    expect(occurrenceAt(night, "Deploys run at night", second)).toBe(2);
    expect(locateQuote(night, "Deploys run at night", 2)).toEqual({
      start: second,
      end: second + "Deploys run at night".length,
    });
  });

  it("finds nothing when the passage is gone", () => {
    expect(locateQuote(night, "Deploys run in the morning", 1)).toBeNull();
    expect(locateQuote(night, "Deploys run at night", 3)).toBeNull();
  });

  it("counts without overlap, as the engine does", () => {
    // "aaa" in "aaaa": the engine finds one match at 0 and resumes at 3.
    expect(occurrenceAt("aaaa", "aaa", 0)).toBe(1);
    expect(locateQuote("aaaa", "aaa", 2)).toBeNull();
    // A selection that starts inside the first match is filed under it.
    expect(occurrenceAt("aaaa", "aaa", 1)).toBe(1);
  });
});

describe("display helpers", () => {
  it("never shows an id line", () => {
    expect(isIdLine("<!-- r:7f3a9c2e -->")).toBe(true);
    expect(isIdLine("  <!-- r:0b1c -->  ")).toBe(true);
    expect(isIdLine("<!-- a comment -->")).toBe(false);
    expect(isIdLine("- <!-- r:7f3a9c2e -->")).toBe(false);
    // A pasted id-like line arrives escaped, as visible text: never an id.
    expect(isIdLine("\\<!-- r:7f3a9c2e -->")).toBe(false);
    expect(stripIdLines("## A\n<!-- r:7f3a9c2e -->\nText.")).toBe(
      "## A\nText.",
    );
    expect(
      stripIdLines("@@ -1 +1 @@\n-## A\n-<!-- r:7f3a9c2e -->\n+## B\n", true),
    ).toBe("@@ -1 +1 @@\n-## A\n+## B\n");
  });

  it("takes the provenance block out of the frontmatter and nothing else", () => {
    const page =
      "---\ntitle: D\nreflection:\n  7f3a9c2e:\n    sources:\n      - engram: runbooks/deploy\n        checksum: ab\nstatus: current\n---\n\n# D\n";
    expect(stripProvenance(page)).toBe(
      "---\ntitle: D\nstatus: current\n---\n\n# D\n",
    );
    expect(stripProvenance("# No frontmatter\n")).toBe("# No frontmatter\n");
  });

  it("keeps the closing fence of a frontmatter with CRLF line ends", () => {
    // A `reflection:` block last in the frontmatter must not swallow the
    // closing `---\r`.
    const page =
      "---\r\ntitle: D\r\nreflection:\r\n  7f3a9c2e:\r\n    reflected: ab\r\n---\r\n\r\n# D\r\n";
    expect(stripProvenance(page)).toBe("---\r\ntitle: D\r\n---\r\n\r\n# D\r\n");
  });

  it("leaves the page settings out of the room text, as the engine's room does", () => {
    // The room shows neither the block nor freshness, follows and status
    // (`without_page_keys` over `without_provenance`), so a conflict compares
    // the room's own text and an editor never sees them.
    const page =
      "---\ntitle: D\nfreshness: 1d\nfollows: arch/overview.md\nreflection:\n  7f3a9c2e:\n    reflected: ab\nstatus: current\ntags: [a]\n---\n\n# D\n";
    expect(roomText(page)).toBe("---\ntitle: D\ntags: [a]\n---\n\n# D\n");
    // The download keeps them: only the engine's own block goes there.
    expect(stripProvenance(page)).toContain("freshness: 1d");
    // A frontmatter that held only those keys goes with them.
    expect(
      roomText(
        "---\nfreshness: 6h\nreflection:\n  a:\n    reflected: ab\n---\n\n# D\n",
      ),
    ).toBe("# D\n");
    expect(roomText("# No frontmatter\n")).toBe("# No frontmatter\n");
  });

  it("drops the fences with the block when it was the only key, as the engine does", () => {
    // Byte for byte the room text core Task 18 serves (`without_provenance`):
    // no empty fences, no leading blank line.
    const page =
      "---\nreflection:\n  7f3a9c2e:\n    reflected: ab\n---\n\n# D\n";
    expect(stripProvenance(page)).toBe("# D\n");
  });

  it("says an age in whole units", () => {
    expect(ageText(30)).toBe("just now");
    expect(ageText(60)).toBe("1 minute");
    expect(ageText(7200)).toBe("2 hours");
    expect(ageText(86400 * 3 + 5)).toBe("3 days");
  });
});

describe("firstTitle", () => {
  it("reads an ATX title, a setext title, and skips code fences", () => {
    expect(firstTitle("![x](a.png)\n\n# Deploying  ##\n")).toBe("Deploying");
    expect(firstTitle("Deploying\n===\n\nText.\n")).toBe("Deploying");
    expect(firstTitle("```\n# not a title\n```\n\n# Real\n")).toBe("Real");
    expect(firstTitle("~~~\n# never closed\n")).toBeNull();
    expect(firstTitle("## Only a section\n")).toBeNull();
  });
});

describe("pageOffset", () => {
  const PAGE = [
    "---",
    "title: Checks",
    "---",
    "",
    "# Checks",
    "",
    "Intro.",
    "",
    "## One",
    "<!-- r:aaaa1111 -->",
    "",
    "- [ ] check",
    "",
    "## Two",
    "<!-- r:bbbb2222 -->",
    "",
    "- [ ] check",
    "",
  ].join("\n");
  const SECTIONS = [
    { id: "aaaa1111", heading: "One", line: 9 },
    { id: "bbbb2222", heading: "Two", line: 14 },
  ];

  it("maps an offset in a section's text back to the page, id lines and frontmatter included", () => {
    const [top, one, two] = sectionTexts(PAGE, SECTIONS);
    if (top === undefined || one === undefined || two === undefined) {
      throw new Error("three sections expected");
    }
    expect(pageOffset(PAGE, SECTIONS, 2, two.text.indexOf("[ ]"))).toBe(
      PAGE.lastIndexOf("[ ]"),
    );
    expect(pageOffset(PAGE, SECTIONS, 1, one.text.indexOf("[ ]"))).toBe(
      PAGE.indexOf("[ ]"),
    );
    expect(pageOffset(PAGE, SECTIONS, 0, top.text.indexOf("Intro"))).toBe(
      PAGE.indexOf("Intro"),
    );
  });

  it("maps through CRLF line endings", () => {
    const crlf = PAGE.replace(/\n/g, "\r\n");
    const two = sectionTexts(crlf, SECTIONS)[2];
    if (two === undefined) {
      throw new Error("section Two expected");
    }
    expect(pageOffset(crlf, SECTIONS, 2, two.text.indexOf("[ ]"))).toBe(
      crlf.lastIndexOf("[ ]"),
    );
  });

  it("answers null for an offset outside the section", () => {
    const two = sectionTexts(PAGE, SECTIONS)[2];
    if (two === undefined) {
      throw new Error("section Two expected");
    }
    expect(pageOffset(PAGE, SECTIONS, 2, -1)).toBeNull();
    expect(pageOffset(PAGE, SECTIONS, 2, two.text.length + 1)).toBeNull();
    expect(pageOffset(PAGE, SECTIONS, 3, 0)).toBeNull();
  });
});
