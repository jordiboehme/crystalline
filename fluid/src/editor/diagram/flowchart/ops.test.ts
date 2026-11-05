import { describe, expect, test } from "vitest";

import { applyEdits } from "../edits";
import { COLOUR_PRESETS, SHAPES } from "./labels";
import { parseFlowchart } from "./parse";
import { type FlowOp, checked, compileFlowOp } from "./ops";

const FIXTURES = import.meta.glob<string>(
  "../../fixtures/diagrams/flowchart/*.mmd",
  { query: "?raw", import: "default", eager: true },
);

/** A corpus file as a fence body: without its trailing break. */
function fixture(prefix: string): string {
  const path = Object.keys(FIXTURES).find((p) => p.includes(`/${prefix}-`));
  if (path === undefined) {
    throw new Error(`no fixture ${prefix}`);
  }
  return (FIXTURES[path] ?? "").replace(/\n$/, "");
}

/** The body after `op`; fails on a refusal. */
function run(body: string, op: FlowOp): string {
  const result = compileFlowOp(parseFlowchart(body), op);
  if ("refused" in result) {
    throw new Error(`refused: ${result.refused}`);
  }
  return applyEdits(body, result.edits);
}

function refusal(body: string, op: FlowOp): string | null {
  const result = compileFlowOp(parseFlowchart(body), op);
  return "refused" in result ? result.refused : null;
}

function edits(body: string, op: FlowOp): unknown {
  const result = compileFlowOp(parseFlowchart(body), op);
  return "refused" in result ? result.refused : result.edits;
}

const SKELETON = "flowchart TD\n    A[First step] --> B[Next step]";
const STYLE_BLUE = "style B fill:#1d4ed8,stroke:#1e3a8a,color:#ffffff";

describe("compileFlowOp in general", () => {
  test("a body it does not understand is unsupported", () => {
    const op: FlowOp = { op: "rename", id: "A", text: "x" };
    expect(refusal("flowchart TD\n  A -->", op)).toBe("unsupported");
  });

  test.each<FlowOp>([
    { op: "rename", id: "Z", text: "x" },
    { op: "shape", id: "Z", shape: "diamond" },
    { op: "colour", id: "Z", colour: "red" },
    { op: "colour", id: "Z", colour: null },
  ])("a missing box is gone: %j", (op) => {
    expect(refusal(SKELETON, op)).toBe("gone");
  });

  test.each<FlowOp>([
    { op: "rename", id: "s", text: "x" },
    { op: "shape", id: "s", shape: "diamond" },
    { op: "colour", id: "s", colour: "red" },
  ])("a subgraph id used as an edge end is no box (R4): %j", (op) => {
    const body = "flowchart TD\n  subgraph s\n    A\n  end\n  B --> s";
    expect(refusal(body, op)).toBe("gone");
  });
});

describe("the self-check compares values, not only which field moved", () => {
  const model = parseFlowchart(SKELETON);
  const blue = {
    from: SKELETON.length,
    to: SKELETON.length,
    insert: `\n    ${STYLE_BLUE}`,
  };
  const at = SKELETON.indexOf("Next step");
  const relabel = { from: at, to: at + "Next step".length, insert: "Done" };

  test("the right values pass", () => {
    expect(checked(model, [relabel], { "B label": "Done" })).toEqual({
      edits: [relabel],
    });
    expect(
      checked(model, [blue], {
        "B styles": null,
        "B colours": JSON.stringify(["#1d4ed8", "#1e3a8a", "#ffffff"]),
      }),
    ).toEqual({ edits: [blue] });
  });

  test("a label other than the typed one is refused", () => {
    expect(checked(model, [relabel], { "B label": "Other" })).toEqual({
      refused: "invalid",
    });
  });

  test("a preset that did not land is refused", () => {
    expect(
      checked(model, [blue], {
        "B styles": null,
        "B colours": JSON.stringify(["#b91c1c", "#7f1d1d", "#ffffff"]),
      }),
    ).toEqual({ refused: "invalid" });
  });

  test("a field that moved without being named is refused", () => {
    expect(checked(model, [relabel], {})).toEqual({ refused: "invalid" });
  });
});

describe("rename", () => {
  test("edits only the label", () => {
    expect(run(SKELETON, { op: "rename", id: "B", text: "Done" })).toBe(
      "flowchart TD\n    A[First step] --> B[Done]",
    );
  });

  test("f22: the later label, the one mermaid shows", () => {
    const body = fixture("f22");
    expect(body).toContain("A[Start]");
    expect(run(body, { op: "rename", id: "A", text: "New" })).toBe(
      body.replace("A[Start]", "A[New]"),
    );
  });

  test("a bare box gets a rectangle after its id", () => {
    expect(
      run("graph LR\n  A-->B", { op: "rename", id: "B", text: "Done" }),
    ).toBe("graph LR\n  A-->B[Done]");
  });

  test("a bare box gets its label after the first ref, before a class", () => {
    expect(
      run("graph LR\n  A:::hot-->B\n  B-->A", {
        op: "rename",
        id: "A",
        text: "Done",
      }),
    ).toBe("graph LR\n  A[Done]:::hot-->B\n  B-->A");
  });

  test("quotes stay as they were", () => {
    expect(
      run('flowchart TD\n  A["Old"]', { op: "rename", id: "A", text: "New" }),
    ).toBe('flowchart TD\n  A["New"]');
  });

  test("text that needs quotes gets them", () => {
    expect(
      run("flowchart TD\n  A[Old]", { op: "rename", id: "A", text: "a (b)" }),
    ).toBe('flowchart TD\n  A["a (b)"]');
  });

  test("a markdown label keeps its wrapper", () => {
    expect(
      run('flowchart TD\n  A["`**Old**`"]', {
        op: "rename",
        id: "A",
        text: "_New_",
      }),
    ).toBe('flowchart TD\n  A["`_New_`"]');
  });

  test("a backtick in a markdown rename stays inside the wrapper", () => {
    expect(
      run('flowchart TD\n  A["`**Old**`"]', {
        op: "rename",
        id: "A",
        text: "a`b",
      }),
    ).toBe('flowchart TD\n  A["`a#96;b`"]');
  });

  test("backticks do not turn a quoted label into a markdown one", () => {
    expect(
      run('flowchart TD\n  A["Old"]', { op: "rename", id: "A", text: "`x`" }),
    ).toBe('flowchart TD\n  A["#96;x#96;"]');
  });

  test("outer space and runs of space, which SVG text cannot draw, are not written", () => {
    expect(run(SKELETON, { op: "rename", id: "B", text: "  a   b " })).toBe(
      "flowchart TD\n    A[First step] --> B[a b]",
    );
    expect(
      edits(SKELETON, { op: "rename", id: "B", text: " Next  step " }),
    ).toEqual([]);
  });

  test.each(["%%{init}%%", "a %%{b: c", "%%{"])(
    "%j is refused, mermaid takes it out of the label as a directive (R42)",
    (text) => {
      expect(refusal(SKELETON, { op: "rename", id: "B", text })).toBe(
        "invalid",
      );
    },
  );

  test("%% and { apart are fine", () => {
    expect(run(SKELETON, { op: "rename", id: "B", text: "%% {" })).toBe(
      'flowchart TD\n    A[First step] --> B["%% {"]',
    );
  });

  test.each([
    ["<script>x</script>", '"#lt;script#gt;x#lt;/script#gt;"'],
    ["<!--c-->", '"#lt;!--c--#gt;"'],
    ["<img src=x onerror=y>", '"#lt;img src=x onerror=y#gt;"'],
  ])(
    "%j is written as entity forms, never as markup (R42)",
    (text, written) => {
      const after = run(SKELETON, { op: "rename", id: "B", text });
      expect(after).toBe(`flowchart TD\n    A[First step] --> B[${written}]`);
      expect(after.split("\n")[1]?.slice(0, 22)).not.toContain("<");
    },
  );

  test("the @{ } form: a plain word stays bare", () => {
    expect(
      run("flowchart TD\n  A@{ shape: diam, label: Old }", {
        op: "rename",
        id: "A",
        text: "New",
      }),
    ).toBe("flowchart TD\n  A@{ shape: diam, label: New }");
  });

  test("the @{ } form keeps quotes", () => {
    expect(
      run('flowchart TD\n  A@{ shape: diam, label: "Old" }', {
        op: "rename",
        id: "A",
        text: "New",
      }),
    ).toBe('flowchart TD\n  A@{ shape: diam, label: "New" }');
  });

  test.each([
    ["true", '"true"'],
    ["NULL", '"NULL"'],
    ["1", '"1"'],
    ["a, b", '"a, b"'],
    // mermaid reads this form as markdown, so the marks are entity forms.
    ["a: b", '"a#58; b"'],
    ["_a_", '"#95;a#95;"'],
    ["http://x.y", '"http#58;//x.y"'],
    ["www.x.y", '"www#46;x.y"'],
  ])("the @{ } form writes %j as %s", (text, written) => {
    expect(
      run("flowchart TD\n  A@{ shape: diam, label: Old }", {
        op: "rename",
        id: "A",
        text,
      }),
    ).toBe(`flowchart TD\n  A@{ shape: diam, label: ${written} }`);
  });

  test("a markdown label keeps its emphasis and loses what would vanish", () => {
    expect(
      run('flowchart TD\n  A["`Old`"]', {
        op: "rename",
        id: "A",
        text: "**b** http://x.y",
      }),
    ).toBe('flowchart TD\n  A["`**b** http#58;//x.y`"]');
  });

  test("the same text is no edit", () => {
    const body = 'flowchart TD\n  A["say #quot;hi#quot;"]\n  B["one<br/>two"]';
    expect(edits(body, { op: "rename", id: "A", text: 'say "hi"' })).toEqual(
      [],
    );
    expect(edits(body, { op: "rename", id: "B", text: "one\ntwo" })).toEqual(
      [],
    );
    expect(
      edits(SKELETON, { op: "rename", id: "A", text: "First step" }),
    ).toEqual([]);
    expect(
      edits("graph LR\n  A-->B", { op: "rename", id: "B", text: "B" }),
    ).toEqual([]);
  });

  test("a line holding direction XX is refused, with no edits (R7)", () => {
    expect(
      compileFlowOp(parseFlowchart(SKELETON), {
        op: "rename",
        id: "B",
        text: "go direction LR",
      }),
    ).toEqual({ refused: "invalid" });
  });

  test("the words alone are fine (R7)", () => {
    expect(run(SKELETON, { op: "rename", id: "B", text: "go direction" })).toBe(
      "flowchart TD\n    A[First step] --> B[go direction]",
    );
  });

  test.each(["", "   ", "\n"])("no text at all is refused: %j", (text) => {
    expect(refusal(SKELETON, { op: "rename", id: "B", text })).toBe("invalid");
  });

  test("a box only a style line names has no ref to edit", () => {
    const body = "flowchart TD\n  A --> B\n  style S fill:#fff";
    expect(refusal(body, { op: "rename", id: "S", text: "x" })).toBe(
      "unsupported",
    );
    expect(refusal(body, { op: "shape", id: "S", shape: "diamond" })).toBe(
      "unsupported",
    );
  });
});

describe("shape", () => {
  test("replaces the two delimiters only", () => {
    const body = SKELETON;
    const result = compileFlowOp(parseFlowchart(body), {
      op: "shape",
      id: "A",
      shape: "diamond",
    });
    expect(result).toEqual({
      edits: [
        { from: 18, to: 19, insert: "{" },
        { from: 29, to: 30, insert: "}" },
      ],
    });
    expect(run(body, { op: "shape", id: "A", shape: "diamond" })).toBe(
      "flowchart TD\n    A{First step} --> B[Next step]",
    );
  });

  test.each(SHAPES.map((s) => [s.name, s.open, s.close] as const))(
    "%s writes its delimiters",
    (shape, open, close) => {
      const expected = `flowchart TD\n    A${open}First step${close} --> B[Next step]`;
      expect(run(SKELETON, { op: "shape", id: "A", shape })).toBe(expected);
    },
  );

  test("the shape it has already is no edit", () => {
    expect(edits(SKELETON, { op: "shape", id: "A", shape: "rect" })).toEqual(
      [],
    );
    const at = "flowchart TD\n  A@{ shape: decision, label: x }";
    expect(edits(at, { op: "shape", id: "A", shape: "diamond" })).toEqual([]);
  });

  test("the @{ } form replaces the name with the short name", () => {
    const body = 'flowchart TD\n  A@{ shape: rect, label: "x" }';
    expect(run(body, { op: "shape", id: "A", shape: "trapezoid" })).toBe(
      'flowchart TD\n  A@{ shape: trap-b, label: "x" }',
    );
  });

  test("a shape outside the fourteen can be replaced", () => {
    const body = 'flowchart TD\n  A@{ shape: doc, label: "x" }';
    expect(run(body, { op: "shape", id: "A", shape: "diamond" })).toBe(
      'flowchart TD\n  A@{ shape: diam, label: "x" }',
    );
  });

  test("a bare box gets the shape with its id as the label", () => {
    expect(
      run("graph LR\n  A-->B", { op: "shape", id: "B", shape: "diamond" }),
    ).toBe("graph LR\n  A-->B{B}");
  });

  test("a bare box already draws as a rectangle, so that is no edit", () => {
    expect(
      edits("graph LR\n  A-->B", { op: "shape", id: "B", shape: "rect" }),
    ).toEqual([]);
  });

  test("the later label of f22 is the one that changes", () => {
    const body = fixture("f22");
    expect(run(body, { op: "shape", id: "A", shape: "diamond" })).toBe(
      body.replace("A[Start]", "A{Start}"),
    );
  });

  test("a label that would read as another shape is refused, not misread", () => {
    const body = "flowchart TD\n  A(/x/)";
    expect(refusal(body, { op: "shape", id: "A", shape: "rect" })).toBe(
      "invalid",
    );
  });

  test("a label that would write the ellipse opener is refused (R5)", () => {
    const body = "flowchart TD\n  A[-x]";
    expect(refusal(body, { op: "shape", id: "A", shape: "rounded" })).toBe(
      "invalid",
    );
  });
});

describe("what mermaid's own preprocessing would change", () => {
  test.each(["rect", "circle", "doubleCircle", "cylinder", "rounded"] as const)(
    "a label starting with - after an opener ending in ( is refused: %s (R5)",
    (shape) => {
      const body = "flowchart TD\n  A>-a]";
      const result = compileFlowOp(parseFlowchart(body), {
        op: "shape",
        id: "A",
        shape,
      });
      if (shape === "rect") {
        expect(result).toEqual({
          edits: [
            { from: 16, to: 17, insert: "[" },
            { from: 19, to: 20, insert: "]" },
          ],
        });
      } else {
        expect(result).toEqual({ refused: "invalid" });
      }
    },
  );

  describe("a ; that mermaid would newly drop (R42)", () => {
    const LINES = [
      'A[x] --> B; click A "https://x.y/?a=1;b=2"',
      'A[x] --> B; click A "https://x.y/" "a;b"',
      "A[x] --> B; subgraph s [a;b]\n  end",
    ];
    test.each(LINES)("a rename that makes it so is refused: %s", (line) => {
      const body = `flowchart TD\n  ${line}`;
      for (const text of ["style:#b", "style a:#b", "classDef:#x"]) {
        expect(
          parseFlowchart(body).understood,
          "the body is one the parser reads",
        ).toBe(true);
        expect(refusal(body, { op: "rename", id: "A", text })).toBe("invalid");
      }
    });

    test("other text on the same line is fine", () => {
      const body = `flowchart TD\n  ${LINES[0] ?? ""}`;
      expect(run(body, { op: "rename", id: "A", text: "plain" })).toContain(
        "A[plain]",
      );
    });

    test("a terminator with only space after it may go", () => {
      const body = "flowchart TD\n  A[x] --> B;  ";
      expect(run(body, { op: "rename", id: "A", text: "style:#b" })).toContain(
        'A["style:#b"]',
      );
    });

    test("a ; the body already drops does not stop other ops", () => {
      const body =
        'flowchart <\n  style s1 fill:#fff !important; A["a: #fff; b"] ~~~ C\n  A[y]';
      const model = parseFlowchart(body);
      if (model.understood) {
        expect(
          compileFlowOp(model, { op: "direction", value: "LR" }),
        ).toHaveProperty("edits");
      }
      const bodyTwo = 'flowchart TD\n  A["style: #fff; b"] --> C\n  A[y]';
      expect(
        compileFlowOp(parseFlowchart(bodyTwo), {
          op: "direction",
          value: "LR",
        }),
      ).toHaveProperty("edits");
    });
  });

  test("a label that spells style:#x; would lose its ; so it is refused (R9)", () => {
    expect(
      refusal(SKELETON, { op: "rename", id: "B", text: "style a:#b; c" }),
    ).toBe("invalid");
  });

  test("a # and a ; in a label without the word style are fine", () => {
    expect(run(SKELETON, { op: "rename", id: "B", text: "a:#b; c" })).toContain(
      'B["a:#35;b; c"]',
    );
  });
});

describe("colour", () => {
  test("no style line: a new line at the end of the body", () => {
    expect(run(SKELETON, { op: "colour", id: "B", colour: "blue" })).toBe(
      `${SKELETON}\n    ${STYLE_BLUE}`,
    );
  });

  test("the new line goes before a trailing line break, not after", () => {
    expect(
      run(`${SKELETON}\n`, { op: "colour", id: "B", colour: "blue" }),
    ).toBe(`${SKELETON}\n    ${STYLE_BLUE}\n`);
  });

  test("the new line goes after the last style, classDef or class line", () => {
    const body =
      "flowchart TD\n  A --> B\n  classDef hot fill:#f00\n  class A hot\n  %% note\n  C --> D";
    expect(run(body, { op: "colour", id: "B", colour: "blue" })).toBe(
      "flowchart TD\n  A --> B\n  classDef hot fill:#f00\n  class A hot\n" +
        `  ${STYLE_BLUE}\n  %% note\n  C --> D`,
    );
  });

  test("the new line never goes above the box's first ref", () => {
    const body = "flowchart TD\n  classDef hot fill:#f00\n  A --> B";
    expect(run(body, { op: "colour", id: "B", colour: "blue" })).toBe(
      `${body}\n  ${STYLE_BLUE}`,
    );
  });

  test("with no statement line the indent falls back to four spaces", () => {
    const body = "graph TD; A --> B";
    expect(run(body, { op: "colour", id: "B", colour: "blue" })).toBe(
      `${body}\n    ${STYLE_BLUE}`,
    );
  });

  test("a style line is edited: values replaced, the rest appended", () => {
    expect(
      run("flowchart TD\n  A --> B\n  style B fill:#fff,stroke-width:4px", {
        op: "colour",
        id: "B",
        colour: "blue",
      }),
    ).toBe(
      "flowchart TD\n  A --> B\n  style B fill:#1d4ed8,stroke-width:4px,stroke:#1e3a8a,color:#ffffff",
    );
  });

  test("the last style line is the one edited", () => {
    const body =
      "flowchart TD\n  A --> B\n  style B fill:#fff\n  style B stroke:#000";
    expect(run(body, { op: "colour", id: "B", colour: "blue" })).toBe(
      "flowchart TD\n  A --> B\n  style B fill:#fff\n" +
        "  style B stroke:#1e3a8a,fill:#1d4ed8,color:#ffffff",
    );
  });

  test("r41: the values go before the trailing ; mermaid drops (R9)", () => {
    const body = fixture("r41");
    expect(body).toContain("style hook fill:#eef6ff;");
    expect(run(body, { op: "colour", id: "hook", colour: "blue" })).toBe(
      body.replace(
        "style hook fill:#eef6ff;",
        "style hook fill:#1d4ed8,stroke:#1e3a8a,color:#ffffff;",
      ),
    );
  });

  test("no ; is ever written (R35)", () => {
    for (const name of Object.keys(
      COLOUR_PRESETS,
    ) as (keyof typeof COLOUR_PRESETS)[]) {
      const result = compileFlowOp(parseFlowchart(SKELETON), {
        op: "colour",
        id: "B",
        colour: name,
      });
      expect(
        "edits" in result && result.edits.some((e) => e.insert.includes(";")),
      ).toBe(false);
    }
  });

  test("a colour on the new line is one the self-check confirmed", () => {
    for (const colour of ["grey", "red"] as const) {
      const after = run(SKELETON, { op: "colour", id: "B", colour });
      expect(after).toContain(`fill:${COLOUR_PRESETS[colour].fill}`);
    }
  });

  test("a # that makes mermaid drop a ; between two statements is refused (R9)", () => {
    const body = "flowchart TD\n  A --> B\n  style B stroke-width:4px; C --> D";
    expect(refusal(body, { op: "colour", id: "B", colour: "blue" })).toBe(
      "invalid",
    );
  });

  test("the colour it has already is no edit", () => {
    const body = `${SKELETON}\n    ${STYLE_BLUE}`;
    expect(edits(body, { op: "colour", id: "B", colour: "blue" })).toEqual([]);
  });

  test("a colour key in any casing is the same key", () => {
    expect(
      run("flowchart TD\n  A\n  style A FILL:#fff", {
        op: "colour",
        id: "A",
        colour: "blue",
      }),
    ).toBe(
      "flowchart TD\n  A\n  style A FILL:#1d4ed8,stroke:#1e3a8a,color:#ffffff",
    );
  });

  describe("no colour", () => {
    test("deletes a line that holds only the three keys", () => {
      const body = `${SKELETON}\n    ${STYLE_BLUE}\n    %% after`;
      expect(run(body, { op: "colour", id: "B", colour: null })).toBe(
        `${SKELETON}\n    %% after`,
      );
    });

    test("deletes the last line with the line break before it", () => {
      const body = `${SKELETON}\n    ${STYLE_BLUE}`;
      expect(run(body, { op: "colour", id: "B", colour: null })).toBe(SKELETON);
    });

    test("r41: the ; goes with the line", () => {
      const body = fixture("r41");
      expect(run(body, { op: "colour", id: "hook", colour: null })).toBe(
        body.replace("\n    style hook fill:#eef6ff;", ""),
      );
    });

    test.each([
      ["fill:#fff,stroke-width:4px", "stroke-width:4px"],
      ["stroke-width:4px,fill:#fff", "stroke-width:4px"],
      ["fill:#fff,stroke-width:4px,color:#000", "stroke-width:4px"],
      [
        "stroke-dasharray:3,fill:#fff,stroke:#000,stroke-width:4px",
        "stroke-dasharray:3,stroke-width:4px",
      ],
      ["stroke-width:4px,fill:#fff,stroke:#000", "stroke-width:4px"],
      ["fill:#fff,stroke:#000,stroke-width:4px", "stroke-width:4px"],
      [
        "fill:#fff,stroke-width:4px,stroke:#000,opacity:1,color:#000",
        "stroke-width:4px,opacity:1",
      ],
    ])("%s keeps %s", (props, kept) => {
      const body = `flowchart TD\n  A\n  style A ${props}`;
      expect(run(body, { op: "colour", id: "A", colour: null })).toBe(
        `flowchart TD\n  A\n  style A ${kept}`,
      );
    });

    test("clears every style line of the box, or the colour stays", () => {
      const body =
        "flowchart TD\n  A\n  style A fill:#fff\n  B\n  style A stroke:#000,opacity:1\n  style A color:#000";
      expect(run(body, { op: "colour", id: "A", colour: null })).toBe(
        "flowchart TD\n  A\n  B\n  style A opacity:1",
      );
    });

    test("two style lines at the end leave no stray line break", () => {
      const body =
        "flowchart TD\n  A\n  style A fill:#fff\n  style A stroke:#000";
      expect(run(body, { op: "colour", id: "A", colour: null })).toBe(
        "flowchart TD\n  A",
      );
    });

    test("a box with no style line is no edit", () => {
      expect(edits(SKELETON, { op: "colour", id: "B", colour: null })).toEqual(
        [],
      );
    });

    test("a line with no colour key on it is no edit", () => {
      const body = "flowchart TD\n  A\n  style A stroke-width:4px";
      expect(edits(body, { op: "colour", id: "A", colour: null })).toEqual([]);
    });

    test("a statement that shares the line is never deleted", () => {
      const body = "flowchart TD\n  A\n  style A fill:red; B --> C";
      expect(refusal(body, { op: "colour", id: "A", colour: null })).toBe(
        "unsupported",
      );
    });

    test("an accDescr block's } is never deleted (R33)", () => {
      const body = "flowchart TD\n  A\n  accDescr { x } style A fill:red";
      expect(refusal(body, { op: "colour", id: "A", colour: null })).toBe(
        "unsupported",
      );
    });

    test("a box only a style line names would vanish, so it is refused", () => {
      const body = "flowchart TD\n  A --> B\n  style S fill:#fff";
      expect(refusal(body, { op: "colour", id: "S", colour: null })).toBe(
        "invalid",
      );
    });
  });
});

describe("direction", () => {
  test("replaces only the direction", () => {
    expect(run(SKELETON, { op: "direction", value: "LR" })).toBe(
      "flowchart LR\n    A[First step] --> B[Next step]",
    );
  });

  test("f18: inserts after the keyword", () => {
    const body = fixture("f18");
    expect(run(body, { op: "direction", value: "LR" })).toBe(
      body.replace("flowchart", "flowchart LR"),
    );
  });

  test("inserts before a header's ;", () => {
    expect(run("graph;\n  A", { op: "direction", value: "LR" })).toBe(
      "graph LR;\n  A",
    );
  });

  test("an indented header", () => {
    expect(run("  graph\n  A", { op: "direction", value: "LR" })).toBe(
      "  graph LR\n  A",
    );
  });

  test.each([
    ["TB", "TD"],
    ["TD", "TD"],
    ["v", "TD"],
    ["LR", "LR"],
    [">", "LR"],
  ] as const)(
    "%s already reads as %s, so that is no edit",
    (written, value) => {
      const body = `flowchart ${written}\n  A`;
      expect(edits(body, { op: "direction", value })).toEqual([]);
    },
  );

  test("no direction already reads as top down, so TD is no edit", () => {
    expect(edits("flowchart\n  A", { op: "direction", value: "TD" })).toEqual(
      [],
    );
  });

  test.each([
    ["TB", "LR"],
    ["v", "LR"],
    ["LR", "TD"],
    [">", "TD"],
    ["BT", "TD"],
    ["RL", "LR"],
    ["<", "LR"],
    ["^", "TD"],
  ] as const)("%s to %s replaces the span", (written, value) => {
    expect(run(`flowchart ${written}\n  A`, { op: "direction", value })).toBe(
      `flowchart ${value}\n  A`,
    );
  });

  test("a subgraph's own direction is not the diagram's", () => {
    const body = "flowchart TD\n  subgraph s\n    direction RL\n    A\n  end";
    expect(run(body, { op: "direction", value: "LR" })).toBe(
      "flowchart LR\n  subgraph s\n    direction RL\n    A\n  end",
    );
  });
});
