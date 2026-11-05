import { describe, expect, test } from "vitest";

import { defined } from "../../../test/assert";
import type { Span } from "../edits";
import type { FlowModel, NodeRef } from "./model";
import { MAX_TEXT_SIZE, edgeByKey, parseFlowchart } from "./parse";

const FLOWCHART = import.meta.glob<string>(
  "../../fixtures/diagrams/flowchart/*.mmd",
  { query: "?raw", import: "default", eager: true },
);
const FALLBACK = import.meta.glob<string>(
  "../../fixtures/diagrams/fallback/*.mmd",
  { query: "?raw", import: "default", eager: true },
);

function nameOf(path: string): string {
  const file = path.slice(path.lastIndexOf("/") + 1);
  return file.slice(0, file.lastIndexOf("."));
}

const corpus = new Map(
  Object.entries({ ...FLOWCHART, ...FALLBACK }).map(([path, text]) => [
    nameOf(path),
    text,
  ]),
);

/** A fixture by its prefix (`f16`), as a fence body (no trailing break). */
function fixture(prefix: string): string {
  const name = defined(
    [...corpus.keys()].find((n) => n.startsWith(`${prefix}-`)),
    `fixture ${prefix}`,
  );
  return defined(corpus.get(name)).replace(/\n$/, "");
}

function read(model: FlowModel, span: Span | null | undefined): string {
  if (!span) {
    throw new Error("expected a span");
  }
  return model.body.slice(span.from, span.to);
}

/** `value`, which the test knows is there (null or undefined throws). */
function present<T>(value: T | null | undefined, what = "value"): T {
  if (value === null || value === undefined) {
    throw new Error(`expected ${what}`);
  }
  return value;
}

function node(model: FlowModel, id: string) {
  return defined(model.nodes.get(id), `node ${id}`);
}

describe("parseFlowchart spans", () => {
  test("a ref, its label and the gap around an arrow", () => {
    const body = "flowchart TD\n    A[Start]  -->  B(Next)";
    const model = parseFlowchart(body);
    expect(model.understood, model.reasons.join("; ")).toBe(true);
    const chain = defined(model.chains[0]);
    const link = defined(chain.links[0]);
    expect(read(model, link.arrow.span)).toBe("-->");
    expect(read(model, link.gap)).toBe("  -->  ");
    const a = defined(chain.groups[0]?.refs[0]);
    expect(read(model, a.idSpan)).toBe("A");
    expect(read(model, a.shape?.label)).toBe("Start");
    expect(read(model, a.shape?.open)).toBe("[");
    expect(read(model, a.shape?.close)).toBe("]");
    expect(read(model, a.span)).toBe("A[Start]");
    const b = defined(chain.groups[1]?.refs[0]);
    expect(b.shape?.name).toBe("rounded");
    expect(read(model, b.shape?.label)).toBe("Next");
    expect(read(model, chain.span)).toBe("A[Start]  -->  B(Next)");
    expect(model.direction?.value).toBe("TD");
    expect(read(model, model.direction?.span)).toBe("TD");
  });

  test("f22: the later shaped ref defines the label", () => {
    const model = parseFlowchart(fixture("f22"));
    expect(model.understood).toBe(true);
    const a = node(model, "A");
    expect(a.refs).toHaveLength(2);
    expect(a.defining).toBe(a.refs[1]);
    expect(read(model, a.defining?.shape?.label)).toBe("Start");
    expect(model.nodeOrder).toEqual(["A", "B", "C"]);
  });

  test("f16: occurrences 0, 1, 2 and mermaid ids _0, _2, _3", () => {
    const model = parseFlowchart(fixture("f16"));
    expect(model.edges.map((e) => [e.occurrence, e.mermaidId])).toEqual([
      [0, "L_A_B_0"],
      [1, "L_A_B_2"],
      [2, "L_A_B_3"],
    ]);
    expect(
      edgeByKey(model, { start: "A", end: "B", occurrence: 1 })?.mermaidId,
    ).toBe("L_A_B_2");
    expect(edgeByKey(model, { start: "A", end: "B", occurrence: 3 })).toBe(
      null,
    );
    expect(edgeByKey(model, { start: "B", end: "A", occurrence: 0 })).toBe(
      null,
    );
  });

  test("f09: pipe, text and quoted labels, and an explicit id", () => {
    const model = parseFlowchart(fixture("f09"));
    expect(model.understood).toBe(true);
    const arrows = model.chains.map((c) => defined(c.links[0]).arrow);
    const [pipe, text, quoted, id] = arrows.map((a) => defined(a));
    if (!pipe || !text || !quoted || !id) {
      throw new Error("four arrows");
    }
    expect(pipe.form).toBe("pipe");
    expect(read(model, pipe.label)).toBe("pipe label");
    expect(read(model, pipe.span)).toBe("-->");
    expect(pipe.opener).toBe(null);

    expect(text.form).toBe("text");
    expect(read(model, text.label)).toBe("text label");
    expect(read(model, text.opener)).toBe("--");
    expect(read(model, text.closer)).toBe("-->");
    expect(read(model, text.span)).toBe("-- text label -->");

    expect(quoted.form).toBe("text");
    expect(quoted.labelQuoted).toBe(true);
    expect(read(model, quoted.label)).toBe('"quoted label"');

    expect(read(model, id.idSpan)).toBe("e1");
    expect(read(model, id.span)).toBe("-->");
    expect(model.edges.map((e) => e.mermaidId)).toEqual([
      "L_A_B_0",
      "L_B_C_0",
      "L_C_D_0",
      "e1",
    ]);
  });

  test("f10: linkStyle indices span the digits only", () => {
    const model = parseFlowchart(fixture("f10"));
    expect(model.understood).toBe(true);
    const styles = model.directives.filter((d) => d.kind === "linkStyle");
    expect(
      styles.map((d) => d.indices.map((i) => [i.n, read(model, i.span)])),
    ).toEqual([
      [[0, "0"]],
      [
        [1, "1"],
        [3, "3"],
      ],
      [],
    ]);
    expect(
      styles.map((d) => d.props.map((p) => [p.key, read(model, p.value)])),
    ).toEqual([
      [
        ["stroke", "#d33"],
        ["stroke-width", "2px"],
      ],
      [["stroke", "#36c"]],
      [["stroke", "#888"]],
    ]);
  });

  test("f13: subgraphs and their nodes as the db gives them", () => {
    const model = parseFlowchart(fixture("f13"));
    expect(model.understood, model.reasons.join("; ")).toBe(true);
    expect(
      Object.fromEntries([...model.nodes].map(([id, n]) => [id, n.subgraph])),
    ).toEqual({ X: "inner", Y: "inner", Z: "outer", Q: null });
    expect(
      model.subgraphs.map((s) => [s.id, s.parent, [...s.members].sort()]),
    ).toEqual([
      ["inner", "outer", ["X", "Y"]],
      ["outer", null, ["Z", "inner"]],
    ]);
    const direction = defined(
      model.directives.find((d) => d.kind === "direction"),
    );
    expect(read(model, direction.props[0]?.value)).toBe("LR");
  });

  test("f05: & groups run start outer, end inner", () => {
    const model = parseFlowchart(fixture("f05"));
    expect(model.edges.map((e) => `${e.start}-${e.end}`)).toEqual([
      "A-C",
      "A-D",
      "B-C",
      "B-D",
    ]);
    const group = defined(model.chains[0]?.groups[0]);
    expect(read(model, group.span)).toBe("A & B");
  });

  test("an explicit id on an & group lands on the last start and first end", () => {
    const model = parseFlowchart("flowchart TD\n  X & Y e2@--> P & Q");
    expect(model.edges.map((e) => [e.mermaidId, e.start, e.end])).toEqual([
      ["L_X_P_0", "X", "P"],
      ["L_X_Q_0", "X", "Q"],
      ["e2", "Y", "P"],
      ["L_Y_Q_0", "Y", "Q"],
    ]);
  });

  test("a taken explicit id falls back to the pair counter", () => {
    const model = parseFlowchart("flowchart LR\n  A e1@--> B\n  C e1@--> D");
    expect(model.edges.map((e) => e.mermaidId)).toEqual(["e1", "L_C_D_0"]);
  });

  test("f06: the fourteen shapes by name", () => {
    const model = parseFlowchart(fixture("f06"));
    expect(model.understood).toBe(true);
    expect(
      model.nodeOrder.map((id) => node(model, id).defining?.shape?.name),
    ).toEqual([
      "rect",
      "rounded",
      "stadium",
      "subroutine",
      "cylinder",
      "circle",
      "flag",
      "diamond",
      "hexagon",
      "leanRight",
      "leanLeft",
      "trapezoid",
      "trapezoidAlt",
      "doubleCircle",
    ]);
  });

  test("f07: @{ } shapes carry the shape and label value spans", () => {
    const model = parseFlowchart(fixture("f07"));
    expect(model.understood).toBe(true);
    const a = present(node(model, "A").defining?.shape);
    expect(a.form).toBe("at");
    expect(a.name).toBe("diamond");
    expect(read(model, a.atShape)).toBe("diam");
    expect(read(model, a.label)).toBe('"x"');
    expect(a.quoted).toBe(true);
    expect(read(model, a.open)).toBe("@{");
    expect(read(model, a.close)).toBe("}");
  });

  test("an @{ } shape outside the fourteen has name null", () => {
    const model = parseFlowchart(
      'flowchart TB\n  cfg@{ shape: doc, label: "store settings" }',
    );
    expect(model.understood).toBe(true);
    expect(node(model, "cfg").defining?.shape?.name).toBe(null);
  });

  test("f21: markdown labels", () => {
    const model = parseFlowchart(fixture("f21"));
    const shape = present(node(model, "A").defining?.shape);
    expect(shape.markdown).toBe(true);
    expect(shape.quoted).toBe(true);
    expect(read(model, shape.label)).toBe('"`**Engram** file`"');
  });

  test("f11: style, classDef, class and :::", () => {
    const model = parseFlowchart(fixture("f11"));
    expect(model.understood).toBe(true);
    const c = defined(node(model, "C").refs[0]);
    expect(read(model, c.classSpan)).toBe("done");
    const kinds = model.directives.map((d) => d.kind);
    expect(kinds).toEqual(["classDef", "classDef", "class", "style"]);
    const klass = defined(model.directives[2]);
    expect(klass.targets.map((t) => [t.id, read(model, t.span)])).toEqual([
      ["A", "A"],
      ["B", "B"],
    ]);
    expect(read(model, klass.className?.span)).toBe("hot");
    const style = defined(model.directives[3]);
    expect(style.targets.map((t) => t.id)).toEqual(["C"]);
    expect(
      style.props.map((p) => [read(model, p.key_), read(model, p.entry)]),
    ).toEqual([["stroke-width", "stroke-width:3px"]]);
  });

  test("f15: arrow kinds", () => {
    const model = parseFlowchart(fixture("f15"));
    expect(model.understood).toBe(true);
    expect(
      model.chains.map((c) => {
        const a = defined(c.links[0]).arrow;
        return [read(model, a.span), a.line, a.start, a.end, a.length];
      }),
    ).toEqual([
      ["--->", "solid", "none", "arrow", 2],
      ["-.->", "dotted", "none", "arrow", 1],
      ["-..->", "dotted", "none", "arrow", 2],
      ["==>", "thick", "none", "arrow", 1],
      ["===", "thick", "none", "none", 1],
      ["~~~", "invisible", "none", "none", 1],
      ["--o", "solid", "none", "circle", 1],
      ["--x", "solid", "none", "cross", 1],
      ["<-->", "solid", "arrow", "arrow", 1],
      ["<-.->", "dotted", "arrow", "arrow", 1],
      ["<==>", "thick", "arrow", "arrow", 1],
    ]);
  });

  test("statements after the header and after an accDescr block are read", () => {
    const header = parseFlowchart("graph TD; A-->B");
    expect(header.understood).toBe(true);
    expect(header.edges.map((e) => e.mermaidId)).toEqual(["L_A_B_0"]);
    const acc = parseFlowchart(
      "flowchart TD\n  accDescr {\n x\n } A-->B; C-->D",
    );
    expect(acc.understood, acc.reasons.join("; ")).toBe(true);
    expect(acc.edges.map((e) => `${e.start}-${e.end}`)).toEqual(["A-B", "C-D"]);
  });

  test("a style line makes a node, a class or click line does not", () => {
    const model = parseFlowchart(
      'flowchart TD\n  A --> B\n  style S fill:#fff\n  class K hot\n  click L href "x"',
    );
    expect(model.understood).toBe(true);
    expect(model.nodeOrder).toEqual(["A", "B", "S"]);
    expect(node(model, "S").refs).toEqual([]);
  });

  test("an edge end that is a subgraph id is a cluster, never a node (R4)", () => {
    const model = parseFlowchart(
      "flowchart TD\n  A --> grp\n  subgraph grp [Group]\n    B\n  end",
    );
    expect(model.understood).toBe(true);
    expect(model.nodeOrder).toEqual(["A", "B"]);
    expect(model.edges.map((e) => `${e.start}-${e.end}`)).toEqual(["A-grp"]);
  });

  test("subgraphs without an id are numbered in the order they close", () => {
    const model = parseFlowchart(
      'flowchart TD\n  subgraph "Spaced title"\n    A\n  end\n  subgraph s2\n    B\n  end\n  subgraph two words\n    C\n  end',
    );
    expect(model.understood).toBe(true);
    expect(model.subgraphs.map((s) => s.id)).toEqual([
      "subGraph0",
      "s2",
      "subGraph2",
    ]);
  });
});

describe("parseFlowchart refusals", () => {
  test.each([
    ["x01", /shape data that does not close on its line/],
    ["x02", /markdown label that does not close on its line/],
    ["x03", /shape data key animate/],
  ])("%s is not understood, for its own reason", (prefix, reason) => {
    const model = parseFlowchart(fixture(prefix));
    expect(model.understood).toBe(false);
    expect(model.reasons.join("; ")).toMatch(reason);
  });

  test.each([
    ["the ellipse (R5)", "flowchart LR\n  A(-x-)"],
    [
      "a direction inside a label (R7)",
      'flowchart TD\n  A["go direction LR now"] --> B',
    ],
    [
      "a direction at the top level (R7)",
      "flowchart TD\n  direction LR\n  A --> B",
    ],
    ["a bare %% line (R8)", "flowchart TD\n%%\n  A --> B"],
    ["a trailing %% (R8)", "flowchart TD\n  A --> B %% note"],
    ["a reserved word as an id", "flowchart TD\n  A --> end"],
    ["end-point as an id", "flowchart TD\n  A --> end-point"],
    ["an unclosed subgraph", "flowchart TD\n  subgraph s\n    A"],
    ["a stray end", "flowchart TD\n  A\n  end"],
    ["flowchart-elk (R30)", "flowchart-elk LR\n  A --> B"],
    ["an unknown statement", "flowchart TD\n  A --> B\n  A B"],
    ["a dangling link", "flowchart TD\n  A -->"],
    ["a multi-line label", 'flowchart TD\n  A["one\n  two"] --> B'],
    [
      "linkStyle past the last edge (R9)",
      "flowchart TD\n  A --> B\n  linkStyle 1 stroke:#f00",
    ],
    ["a head only at the start", "flowchart TD\n  A <--- B"],
    [
      "@{ } with an unknown key",
      'flowchart TD\n  A@{ shape: rect, label: "x", icon: "y" }',
    ],
    [
      "a subgraph id with a shape",
      "flowchart TD\n  subgraph g\n    A\n  end\n  g[Label] --> A",
    ],
    ["not a flowchart", "sequenceDiagram\n  A->>B: hi"],
  ])("%s", (_name, body) => {
    const model = parseFlowchart(body);
    expect(model.understood).toBe(false);
    expect(model.reasons.length).toBeGreaterThan(0);
  });

  test("never throws, on any prefix of a corpus file", () => {
    for (const text of corpus.values()) {
      for (let i = 0; i <= text.length; i += 7) {
        expect(() => parseFlowchart(text.slice(0, i))).not.toThrow();
      }
    }
  });
});

describe("parseFlowchart on large bodies", () => {
  /** The best of three runs, in ms. */
  function fastest(body: string): number {
    let best = Infinity;
    for (let i = 0; i < 3; i++) {
      const from = performance.now();
      parseFlowchart(body);
      best = Math.min(best, performance.now() - from);
    }
    return best;
  }

  test("a body at mermaid's text limit is read, one past it is refused", () => {
    const head = "flowchart TD\n  A\n  %% ";
    const at = head + "x".repeat(MAX_TEXT_SIZE - head.length);
    expect(parseFlowchart(at).understood).toBe(true);
    const past = parseFlowchart(`${at}x`);
    expect(past.understood).toBe(false);
    expect(past.reasons.join(" ")).toMatch(/too long/);
  });

  test("a body that would overflow the scanner's regex comes back refused", () => {
    const body = `%%{init: "${"x".repeat(16_000_000)}`;
    expect(parseFlowchart(body).understood).toBe(false);
  });

  test("a huge & group stops at mermaid's edge limit", () => {
    const names = (c: string) =>
      Array.from({ length: 1500 }, (_, i) => `${c}${String(i)}`).join(" & ");
    const model = parseFlowchart(
      `flowchart TD\n  ${names("a")} --> ${names("b")}`,
    );
    expect(model.understood).toBe(false);
    expect(model.edges.length).toBeLessThanOrEqual(500);
  });

  test.each([
    [
      "a chain without spaces",
      () =>
        `flowchart TD\n  ${Array.from({ length: 6000 }, (_, i) => `N${String(i)}`).join("-->")}`,
    ],
    [
      "spaces inside a text label",
      () => `flowchart TD\n  A -- a${" ".repeat(49000)}b --> B`,
    ],
    [
      "spaces inside shape data",
      () => `flowchart TD\n  A@{ shape: rect, label: x${" ".repeat(49000)}y }`,
    ],
  ])("%s is read in linear time", (_name, make) => {
    const body = make();
    expect(body.length).toBeLessThan(MAX_TEXT_SIZE);
    expect(fastest(body)).toBeLessThan(250);
  });
});

describe("span invariants over the corpus", () => {
  const understood = [...corpus.entries()].filter(([name]) =>
    /^[fr]\d/.test(name),
  );

  test.each(understood)("%s", (_name, text) => {
    const model = parseFlowchart(text);
    expect(model.understood, model.reasons.join("; ")).toBe(true);
    const body = model.body;
    const slice = (s: Span) => body.slice(s.from, s.to);
    const inside = (inner: Span, outer: Span) =>
      inner.from >= outer.from && inner.to <= outer.to;
    const checkRef = (ref: NodeRef) => {
      expect(slice(ref.idSpan)).toBe(ref.id);
      expect(ref.span.from).toBe(ref.idSpan.from);
      const shape = ref.shape;
      if (shape) {
        expect(inside(shape.open, ref.span)).toBe(true);
        expect(inside(shape.close, ref.span)).toBe(true);
        expect(inside(shape.label, ref.span)).toBe(true);
        if (shape.form === "delimiters") {
          expect(shape.open.from).toBe(ref.idSpan.to);
          expect(shape.open.to).toBe(shape.label.from);
          expect(shape.label.to).toBe(shape.close.from);
        }
      }
      if (ref.classSpan) {
        expect(body.slice(ref.classSpan.from - 3, ref.classSpan.from)).toBe(
          ":::",
        );
        expect(ref.classSpan.to).toBe(ref.span.to);
      }
    };
    for (const chain of model.chains) {
      const first = defined(chain.groups[0]);
      const last = defined(chain.groups[chain.groups.length - 1]);
      expect(first.span.from).toBe(chain.span.from);
      expect(last.span.to).toBe(chain.span.to);
      expect(chain.links).toHaveLength(chain.groups.length - 1);
      for (const group of chain.groups) {
        expect(group.span.from).toBe(group.refs[0]?.span.from);
        expect(group.span.to).toBe(group.refs[group.refs.length - 1]?.span.to);
        group.refs.forEach(checkRef);
      }
      chain.links.forEach((link, i) => {
        const left = defined(chain.groups[i]);
        const right = defined(chain.groups[i + 1]);
        expect(link.gap.from).toBe(left.span.to);
        expect(link.gap.to).toBe(right.span.from);
        const arrow = link.arrow;
        expect(inside(arrow.span, link.gap)).toBe(true);
        expect(slice(arrow.span).trim()).toBe(slice(arrow.span));
        for (const part of [
          arrow.label,
          arrow.opener,
          arrow.closer,
          arrow.idSpan,
        ]) {
          if (part) {
            expect(inside(part, link.gap)).toBe(true);
          }
        }
        for (const index of link.edges) {
          const edge = defined(model.edges[index]);
          expect(edge.index).toBe(index);
          expect(left.refs.map((r) => r.id)).toContain(edge.start);
          expect(right.refs.map((r) => r.id)).toContain(edge.end);
        }
      });
    }
    for (const directive of model.directives) {
      for (const target of directive.targets) {
        expect(slice(target.span)).toBe(target.id);
      }
      for (const index of directive.indices) {
        expect(slice(index.span)).toBe(String(index.n));
      }
      for (const prop of directive.props) {
        expect(slice(prop.key_)).toBe(prop.key);
        expect(inside(prop.key_, prop.entry)).toBe(true);
        expect(inside(prop.value, prop.entry)).toBe(true);
      }
    }
    for (const [id, n] of model.nodes) {
      expect(n.id).toBe(id);
      for (const ref of n.refs) {
        expect(ref.id).toBe(id);
      }
    }
  });
});
