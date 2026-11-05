import { describe, expect, test } from "vitest";

import { STATUS_COLOURS } from "../../../markdown/directives";
import { contrastRatio } from "../../../test/contrast";
import {
  COLOUR_PRESETS,
  SHAPES,
  decodeLabel,
  encodeLabel,
  encodeMarkdownLabel,
  tidyLabel,
} from "./labels";
import { AT_SHAPE_NAMES, CLASSIC_SHAPES } from "./model";

describe("encodeLabel and decodeLabel", () => {
  test.each([
    ["a (b)", '"a (b)"'],
    ["plain", "plain"],
    ['say "hi"', '"say #quot;hi#quot;"'],
    ["one\ntwo", "one<br>two"],
  ])("%j encodes to %j and back", (text, encoded) => {
    expect(encodeLabel(text, false)).toBe(encoded);
    expect(decodeLabel(encoded, encoded.startsWith('"'))).toBe(text);
  });

  test.each([
    ..."[]{}()|<>&;#%".split("").map((c) => `a${c}b`),
    " leading",
    "trailing ",
  ])("%j gets quotes", (text) => {
    const encoded = encodeLabel(text, false);
    expect(encoded.startsWith('"') && encoded.endsWith('"')).toBe(true);
    expect(decodeLabel(encoded, true)).toBe(text);
  });

  test("quotes stay when they were there", () => {
    expect(encodeLabel("plain", true)).toBe('"plain"');
  });

  test.each(["/api", "api/", "-x", "x-"])(
    "%j gets quotes, or it would read as another shape's delimiter",
    (text) => {
      const encoded = encodeLabel(text, false);
      expect(encoded).toBe(`"${text}"`);
      expect(decodeLabel(encoded, true)).toBe(text);
    },
  );

  // What mermaid would mangle on the way to an SVG label (R41).
  test.each([
    ['say "hi"', '"say #quot;hi#quot;"'],
    ["x > y", '"x #gt; y"'],
    ["a<b", '"a#lt;b"'],
    ["a-->b", '"a--#gt;b"'],
    ["> q", '"#gt; q"'],
    ["C:\\new", '"C:#92;new"'],
    ["a\\", '"a#92;"'],
    ["`a`", '"#96;a#96;"'],
    ["a`b", '"a#96;b"'],
    ["&amp;", '"#38;amp;"'],
    ["&#35;", '"#38;#35;35;"'],
    ["#35;", '"#35;35;"'],
    ["#a_b;", '"#35;a_b;"'],
    ["x #_; y", '"x #35;_; y"'],
    ["a:#b;", '"a:#35;b;"'],
    ["<br>", '"#lt;br#gt;"'],
    ["\u00B6\u00DF", '"#182;\u00DF"'],
    ["\uFB02\u00B0", '"#64258;\u00B0"'],
  ])("%j is written as %j and reads back", (text, encoded) => {
    expect(encodeLabel(text, false)).toBe(encoded);
    expect(decodeLabel(encoded, true)).toBe(text);
  });

  test("a plain & and a plain # stay as they are", () => {
    expect(encodeLabel("A & B", false)).toBe('"A & B"');
    expect(encodeLabel("a#b", false)).toBe('"a#b"');
  });

  test("the field shows what is drawn for labels already in a diagram", () => {
    expect(decodeLabel('"#9829; #infin; #lt;"', true)).toBe("\u2665 \u221E <");
    expect(decodeLabel("a &amp; b", false)).toBe("a & b");
    expect(decodeLabel('"#bogus;"', true)).toBe("#bogus;");
  });

  test("a written <br> is text, a <br> in the source is a line break", () => {
    expect(decodeLabel('"#lt;br#gt;"', true)).toBe("<br>");
    expect(decodeLabel("a<br>b", false)).toBe("a\nb");
  });

  test("tidyLabel gives the text mermaid can draw: one space, no outer space", () => {
    expect(tidyLabel("  a \t b  \r\n  c ")).toBe("a b\nc");
    expect(tidyLabel("\n\na\n\nb\n")).toBe("a\n\nb");
    expect(tidyLabel(" \n ")).toBe("");
  });

  test.each(["me@host", "a~~b"])(
    "%j gets quotes, or mermaid's lexer would read an arrow in it",
    (text) => {
      expect(encodeLabel(text, false)).toBe(`"${text}"`);
    },
  );

  test("a # is written as #35; only where it would read as an entity", () => {
    expect(encodeLabel("a#b", false)).toBe('"a#b"');
    expect(encodeLabel("#amp;", false)).toBe('"#35;amp;"');
    expect(decodeLabel('"#35;amp;"', true)).toBe("#amp;");
  });

  test("one decoding pass: #35;quot; is the text #quot;", () => {
    expect(decodeLabel('"#35;quot;"', true)).toBe("#quot;");
    expect(encodeLabel("#quot;", false)).toBe('"#35;quot;"');
  });

  test.each(["a<br>b", "a<br/>b", "a<br />b", "a<BR>b"])(
    "%j decodes to two lines",
    (raw) => {
      expect(decodeLabel(raw, false)).toBe("a\nb");
    },
  );

  test("a line break of any kind is one <br>", () => {
    expect(encodeLabel("a\r\nb\rc", false)).toBe("a<br>b<br>c");
  });

  test("the field shows a markdown label without its wrapper", () => {
    expect(decodeLabel('"`**Engram** file`"', true)).toBe("**Engram** file");
  });

  test("a markdown label keeps its wrapper", () => {
    expect(encodeMarkdownLabel("**New** text")).toBe('"`**New** text`"');
    expect(encodeMarkdownLabel('a "b"\nc')).toBe('"`a #quot;b#quot;<br>c`"');
  });

  test("an unquoted label reads without its outer spaces, as mermaid's does", () => {
    expect(decodeLabel(" x ", false)).toBe("x");
  });
});

describe("the shape table", () => {
  test("the menu labels, in the menu's order", () => {
    expect(SHAPES.map((s) => [s.label, s.open, s.close])).toEqual([
      ["Rectangle", "[", "]"],
      ["Rounded", "(", ")"],
      ["Stadium", "([", "])"],
      ["Subroutine", "[[", "]]"],
      ["Database", "[(", ")]"],
      ["Circle", "((", "))"],
      ["Double circle", "(((", ")))"],
      ["Flag", ">", "]"],
      ["Decision", "{", "}"],
      ["Hexagon", "{{", "}}"],
      ["Slanted right", "[/", "/]"],
      ["Slanted left", "[\\", "\\]"],
      ["Trapezoid", "[/", "\\]"],
      ["Upside-down trapezoid", "[\\", "/]"],
    ]);
  });

  test("the delimiters are the model's table, not a copy", () => {
    expect(SHAPES.map((s) => [s.name, s.open, s.close])).toEqual(
      CLASSIC_SHAPES.map((s) => [s.name, s.open, s.close]),
    );
  });

  test("each at name is the research note's short name for that shape", () => {
    expect(SHAPES.map((s) => s.at)).toEqual([
      "rect",
      "rounded",
      "stadium",
      "fr-rect",
      "cyl",
      "circle",
      "dbl-circ",
      "odd",
      "diam",
      "hex",
      "lean-r",
      "lean-l",
      "trap-b",
      "trap-t",
    ]);
    for (const shape of SHAPES) {
      expect(AT_SHAPE_NAMES[shape.at]).toBe(shape.name);
    }
  });

  test("the ellipse and the icon are never offered (R5)", () => {
    expect(SHAPES).toHaveLength(14);
    expect(SHAPES.some((s) => s.open === "(-" || s.at === "icon")).toBe(false);
  });
});

describe("the colour presets", () => {
  test("there is one for every status colour", () => {
    expect(Object.keys(COLOUR_PRESETS).sort()).toEqual(
      [...STATUS_COLOURS].sort(),
    );
  });

  test("the exact values", () => {
    expect(COLOUR_PRESETS).toEqual({
      grey: {
        label: "Grey",
        fill: "#475569",
        stroke: "#1e293b",
        color: "#ffffff",
      },
      blue: {
        label: "Blue",
        fill: "#1d4ed8",
        stroke: "#1e3a8a",
        color: "#ffffff",
      },
      green: {
        label: "Green",
        fill: "#15803d",
        stroke: "#14532d",
        color: "#ffffff",
      },
      yellow: {
        label: "Yellow",
        fill: "#a16207",
        stroke: "#713f12",
        color: "#ffffff",
      },
      red: {
        label: "Red",
        fill: "#b91c1c",
        stroke: "#7f1d1d",
        color: "#ffffff",
      },
      purple: {
        label: "Purple",
        fill: "#7e22ce",
        stroke: "#581c87",
        color: "#ffffff",
      },
    });
  });

  test.each(STATUS_COLOURS)("the text on %s is readable", (name) => {
    const preset = COLOUR_PRESETS[name];
    expect(contrastRatio(preset.color, preset.fill)).toBeGreaterThanOrEqual(
      4.5,
    );
  });
});
