/**
 * A click on a drawn flowchart maps back to the box or arrow in the source. Every
 * recorded mermaid 12.2.0 drawing is loaded next to its source and read through
 * the real parser and the real verdict. The expected box comes from the id mermaid
 * wrote into the drawing, and the expected occurrence is counted again here, so
 * neither answer is read out of the code under test.
 */

import { afterEach, beforeAll, describe, expect, test, vi } from "vitest";

import { cleanupMarkViews, markView } from "../../../test/markKit";
import { prefetchDiagrams } from "../loader";
import type { FlowTarget } from "../intent";
import { requestVerdict, type Verdict } from "../verdict";
import { forgetDrawings } from "../widget";
import { decorateFlow, flowElementOf, flowStops, flowTargetOf } from "./hit";
import type { EdgeKey, FlowModel } from "./model";
import { parseFlowchart } from "./parse";

const SOURCES = import.meta.glob<string>(
  "../../fixtures/diagrams/flowchart/*.mmd",
  { query: "?raw", import: "default", eager: true },
);
const DRAWINGS = import.meta.glob<string>("../../fixtures/diagrams/svg/*.svg", {
  query: "?raw",
  import: "default",
  eager: true,
});

function byName(files: Record<string, string>): Map<string, string> {
  return new Map(
    Object.entries(files).map(([path, text]) => [
      path.slice(path.lastIndexOf("/") + 1, path.lastIndexOf(".")),
      text,
    ]),
  );
}

const SOURCE = byName(SOURCES);
const DRAWING = byName(DRAWINGS);
const NAMES = [...SOURCE.keys()].sort((a, b) => a.localeCompare(b));

function need<T>(value: T | null | undefined, what: string): T {
  if (value === null || value === undefined) {
    throw new Error(`missing: ${what}`);
  }
  return value;
}

/** `f16` for `f16-duplicate-edges`. */
const nameOf = (prefix: string) =>
  need(
    NAMES.find((name) => name.startsWith(`${prefix}-`)),
    prefix,
  );

const verdicts = new Map<string, Verdict>();

interface Loaded {
  container: HTMLElement;
  svg: SVGSVGElement;
  svgId: string;
  model: FlowModel;
  verdict: Verdict;
}

const CONTAINERS: HTMLElement[] = [];

/** A recorded drawing in a box as `makeBox` (`../widget`) builds it, with its model. */
function load(name: string): Loaded {
  const container = document.createElement("div");
  container.className = "cm-diagram";
  container.tabIndex = 0;
  container.innerHTML = need(DRAWING.get(name), `the drawing of ${name}`);
  document.body.append(container);
  CONTAINERS.push(container);
  const svg = need(container.querySelector("svg"), `the svg of ${name}`);
  return {
    container,
    svg,
    svgId: svg.id,
    model: parseFlowchart(need(SOURCE.get(name), `the source of ${name}`)),
    verdict: need(verdicts.get(name), `the verdict of ${name}`),
  };
}

const targetOf = (loaded: Loaded, el: Element): FlowTarget =>
  flowTargetOf(el, loaded.svgId, loaded.model, loaded.verdict);

const elementOf = (loaded: Loaded, target: FlowTarget) =>
  flowElementOf(loaded.svg, loaded.svgId, target, loaded.model, loaded.verdict);

/** The source id a box's element id says it draws: `<svg>-flowchart-<id>-<n>`. */
function drawnBoxId(el: Element, svgId: string): string {
  const prefix = `${svgId}-flowchart-`;
  expect(el.id.startsWith(prefix), el.id).toBe(true);
  return el.id.slice(prefix.length).replace(/-\d+$/, "");
}

const edgePaths = (loaded: Loaded) => [
  ...loaded.svg.querySelectorAll<SVGElement>('path[data-et="edge"]'),
];
const edgeLabels = (loaded: Loaded) => [
  ...loaded.svg.querySelectorAll<SVGElement>("g.label[data-id]"),
];

/** Each edge's own key, its occurrence counted again from the order of the edges. */
function expectedKeys(model: FlowModel): Map<string, EdgeKey> {
  const seen = new Map<string, number>();
  const keys = new Map<string, EdgeKey>();
  for (const edge of model.edges) {
    const pair = JSON.stringify([edge.start, edge.end]);
    const occurrence = seen.get(pair) ?? 0;
    seen.set(pair, occurrence + 1);
    keys.set(edge.mermaidId, {
      start: edge.start,
      end: edge.end,
      occurrence,
    });
  }
  return keys;
}

const CANVAS: FlowTarget = { kind: "canvas" };

beforeAll(async () => {
  for (const name of NAMES) {
    verdicts.set(
      name,
      await requestVerdict(need(SOURCE.get(name), name), false),
    );
  }
}, 120_000);

afterEach(() => {
  for (const container of CONTAINERS.splice(0)) {
    container.remove();
  }
});

describe("the recordings", () => {
  test("every source has a drawing, and only the fallback bodies have none", () => {
    expect(NAMES).toHaveLength(68);
    expect(NAMES.filter((name) => !DRAWING.has(name))).toEqual([]);
    expect(
      [...DRAWING.keys()]
        .filter((name) => !SOURCE.has(name))
        .sort((a, b) => a.localeCompare(b)),
    ).toEqual([
      "x01-multiline-at-shape",
      "x02-multiline-markdown-label",
      "x03-unknown-statement",
    ]);
  });

  test.each(NAMES)("%s has a drawable verdict", (name) => {
    const verdict = need(verdicts.get(name), name);
    expect(verdict).toMatchObject({ ok: true, kind: "flowchart" });
  });
});

describe("a box", () => {
  test.each(NAMES)(
    "%s: each g.node maps to its model node and back",
    (name) => {
      const loaded = load(name);
      const nodes = [...loaded.svg.querySelectorAll<SVGElement>("g.node")];
      const mapped: string[] = [];
      for (const el of nodes) {
        const id = drawnBoxId(el, loaded.svgId);
        expect(loaded.model.nodes.has(id), id).toBe(true);
        const target: FlowTarget = { kind: "node", id };
        expect(targetOf(loaded, el)).toEqual(target);
        expect(elementOf(loaded, target)).toBe(el);
        mapped.push(id);
      }
      // One element per box: none left out, none drawn twice.
      const boxes = [...loaded.model.nodes.keys()].filter(
        (id) => !loaded.model.subgraphs.some((s) => s.id === id),
      );
      expect(mapped.toSorted()).toEqual(boxes.toSorted());
    },
  );

  test.each(NAMES)("%s: anything inside a g.node maps to its box", (name) => {
    const loaded = load(name);
    for (const el of loaded.svg.querySelectorAll("g.node")) {
      const id = drawnBoxId(el, loaded.svgId);
      for (const inner of el.querySelectorAll("*")) {
        expect(targetOf(loaded, inner)).toEqual({ kind: "node", id });
      }
    }
  });

  test("f17's ids with - and _ map to exactly those boxes", () => {
    const loaded = load(nameOf("f17"));
    const at = (id: string) =>
      targetOf(loaded, need(loaded.svg.querySelector(`[id="${id}"]`), id));
    const prefix = `${loaded.svgId}-flowchart`;
    expect(at(`${prefix}-step-1-0`)).toEqual({ kind: "node", id: "step-1" });
    expect(at(`${prefix}-a_b-1`)).toEqual({ kind: "node", id: "a_b" });
    expect(at(`${prefix}-X1-3`)).toEqual({ kind: "node", id: "X1" });
  });

  test("a g.node with another drawing's id, or with a refused verdict, is the canvas", () => {
    const loaded = load(nameOf("f17"));
    const el = need(loaded.svg.querySelector("g.node"), "a g.node");
    expect(flowTargetOf(el, "other", loaded.model, loaded.verdict)).toEqual(
      CANVAS,
    );
    expect(
      flowTargetOf(el, loaded.svgId, loaded.model, {
        ok: false,
        reason: "no",
      }),
    ).toEqual(CANVAS);
  });

  test("a box the drawing does not hold has no element", () => {
    const loaded = load(nameOf("f17"));
    expect(elementOf(loaded, { kind: "node", id: "nowhere" })).toBeNull();
  });
});

describe("an arrow", () => {
  test.each(NAMES)("%s: each edge path maps to its edge and back", (name) => {
    const loaded = load(name);
    const keys = expectedKeys(loaded.model);
    const paths = edgePaths(loaded);
    expect(paths.map((el) => el.getAttribute("data-id")).toSorted()).toEqual(
      [...keys.keys()].toSorted(),
    );
    for (const el of paths) {
      const key = need(keys.get(el.getAttribute("data-id") ?? ""), el.id);
      const target: FlowTarget = { kind: "edge", key };
      expect(targetOf(loaded, el)).toEqual(target);
      expect(elementOf(loaded, target)).toBe(el);
    }
  });

  test.each(NAMES)(
    "%s: each edge label, and anything inside it, maps to its edge",
    (name) => {
      const loaded = load(name);
      const keys = expectedKeys(loaded.model);
      const labels = edgeLabels(loaded);
      expect(labels.map((el) => el.getAttribute("data-id")).toSorted()).toEqual(
        [...keys.keys()].toSorted(),
      );
      for (const el of labels) {
        const key = need(keys.get(el.getAttribute("data-id") ?? ""), "a label");
        expect(targetOf(loaded, el)).toEqual({ kind: "edge", key });
        for (const inner of el.querySelectorAll("*")) {
          expect(targetOf(loaded, inner)).toEqual({ kind: "edge", key });
        }
      }
    },
  );

  test("f16's three A to B arrows are occurrences 0, 1 and 2, in order", () => {
    const loaded = load(nameOf("f16"));
    const key = (occurrence: number): FlowTarget => ({
      kind: "edge",
      key: { start: "A", end: "B", occurrence },
    });
    const paths = edgePaths(loaded);
    expect(paths.map((el) => el.getAttribute("data-id"))).toEqual([
      "L_A_B_0",
      "L_A_B_2",
      "L_A_B_3",
    ]);
    expect(paths.map((el) => targetOf(loaded, el))).toEqual([
      key(0),
      key(1),
      key(2),
    ]);
    expect(edgeLabels(loaded).map((el) => targetOf(loaded, el))).toEqual([
      key(0),
      key(1),
      key(2),
    ]);
    expect([0, 1, 2].map((n) => elementOf(loaded, key(n)))).toEqual(paths);
    expect(elementOf(loaded, key(3))).toBeNull();
  });

  test("f17's ids with - and _ map to exactly those arrows", () => {
    const loaded = load(nameOf("f17"));
    const first: FlowTarget = {
      kind: "edge",
      key: { start: "step-1", end: "a_b", occurrence: 0 },
    };
    const second: FlowTarget = {
      kind: "edge",
      key: { start: "a_b", end: "X1", occurrence: 0 },
    };
    const paths = edgePaths(loaded);
    expect(paths.map((el) => el.getAttribute("data-id"))).toEqual([
      "L_step-1_a_b_0",
      "L_a_b_X1_0",
    ]);
    expect(paths.map((el) => targetOf(loaded, el))).toEqual([first, second]);
    expect(edgeLabels(loaded).map((el) => targetOf(loaded, el))).toEqual([
      first,
      second,
    ]);
    expect(elementOf(loaded, first)).toBe(paths[0]);
    expect(elementOf(loaded, second)).toBe(paths[1]);
  });

  test("f09's explicit id e1 maps to D to E", () => {
    const loaded = load(nameOf("f09"));
    const path = need(
      loaded.svg.querySelector('path[data-id="e1"]'),
      "the path of e1",
    );
    const label = need(
      loaded.svg.querySelector('g.label[data-id="e1"]'),
      "the label of e1",
    );
    const target: FlowTarget = {
      kind: "edge",
      key: { start: "D", end: "E", occurrence: 0 },
    };
    expect(targetOf(loaded, path)).toEqual(target);
    expect(targetOf(loaded, label)).toEqual(target);
  });

  test("a part with an id no arrow has is the canvas", () => {
    const loaded = load(nameOf("f17"));
    const path = need(edgePaths(loaded)[0], "a path");
    path.setAttribute("data-id", "L_nowhere_0");
    expect(targetOf(loaded, path)).toEqual(CANVAS);
  });
});

describe("the canvas", () => {
  const WITH_CLUSTERS = NAMES.filter((name) =>
    need(SOURCE.get(name), name).includes("subgraph"),
  );

  test("the corpus holds drawings with clusters", () => {
    expect(WITH_CLUSTERS.length).toBeGreaterThanOrEqual(6);
    expect(WITH_CLUSTERS).toContain(nameOf("f13"));
  });

  test.each(WITH_CLUSTERS)(
    "%s: a cluster and everything in it is the canvas",
    (name) => {
      const loaded = load(name);
      const clusters = [...loaded.svg.querySelectorAll("g.cluster")];
      expect(clusters.map((el) => el.id).toSorted()).toEqual(
        loaded.model.subgraphs.map((s) => `${loaded.svgId}-${s.id}`).toSorted(),
      );
      for (const el of clusters) {
        expect(targetOf(loaded, el)).toEqual(CANVAS);
        for (const inner of el.querySelectorAll("*")) {
          expect(targetOf(loaded, inner)).toEqual(CANVAS);
        }
      }
    },
  );

  test("f13's two clusters are the canvas by name", () => {
    const loaded = load(nameOf("f13"));
    for (const id of ["outer", "inner"]) {
      const el = need(
        loaded.svg.querySelector(`g.cluster[id="${loaded.svgId}-${id}"]`),
        id,
      );
      expect(targetOf(loaded, el)).toEqual(CANVAS);
    }
  });

  test("the svg itself is the canvas, and the canvas has no element", () => {
    const loaded = load(nameOf("f01"));
    expect(targetOf(loaded, loaded.svg)).toEqual(CANVAS);
    expect(elementOf(loaded, CANVAS)).toBeNull();
  });
});

describe("decorateFlow", () => {
  test.each(NAMES)(
    "%s: exactly one element has tabindex 0, and it is the box",
    (name) => {
      const loaded = load(name);
      decorateFlow(loaded.svg, loaded.svgId, loaded.model, loaded.verdict);
      expect([...document.body.querySelectorAll('[tabindex="0"]')]).toEqual([
        loaded.container,
      ]);
      // Every stop is out of the tab order, and nothing else was given a tabindex.
      const stops = [
        ...loaded.svg.querySelectorAll('g.node, path[data-et="edge"]'),
      ];
      expect([...loaded.svg.querySelectorAll("[tabindex]")]).toEqual(stops);
      for (const stop of stops) {
        expect(stop.getAttribute("tabindex")).toBe("-1");
      }
      expect(stops).toHaveLength(flowStops(loaded.model).length - 1);
    },
  );

  test.each(NAMES)("%s: the wide hit path maps to its arrow", (name) => {
    const loaded = load(name);
    decorateFlow(loaded.svg, loaded.svgId, loaded.model, loaded.verdict);
    const keys = expectedKeys(loaded.model);
    const hits = [...loaded.svg.querySelectorAll("[data-hit-for]")];
    expect(
      hits.map((el) => el.getAttribute("data-hit-for")).toSorted(),
    ).toEqual([...keys.keys()].toSorted());
    for (const hit of hits) {
      const edgeId = hit.getAttribute("data-hit-for") ?? "";
      expect(targetOf(loaded, hit)).toEqual({
        kind: "edge",
        key: need(keys.get(edgeId), edgeId),
      });
      expect(hit.previousElementSibling?.getAttribute("data-id")).toBe(edgeId);
    }
  });

  test("f16's three arrows stay told apart after it", () => {
    const loaded = load(nameOf("f16"));
    decorateFlow(loaded.svg, loaded.svgId, loaded.model, loaded.verdict);
    const hits = [...loaded.svg.querySelectorAll("[data-hit-for]")];
    expect(hits.map((el) => targetOf(loaded, el))).toEqual(
      [0, 1, 2].map((occurrence) => ({
        kind: "edge",
        key: { start: "A", end: "B", occurrence },
      })),
    );
  });
});

vi.mock("mermaid", async (original) => {
  const real = (await original<{ default: object }>()).default;
  return {
    default: {
      ...real,
      /** The recorded drawing of a body, under the id the widget asks for. */
      render: (id: string, body: string) => {
        const name = need(
          NAMES.find((n) => SOURCE.get(n)?.trim() === body.trim()),
          "a recording of the body",
        );
        return Promise.resolve({
          svg: need(DRAWING.get(name), name).replaceAll(`rec-${name}`, id),
        });
      },
    },
  };
});

describe("on the real surface", () => {
  afterEach(() => {
    cleanupMarkViews();
    forgetDrawings();
  });

  const press = (key: string) => {
    document.activeElement?.dispatchEvent(
      new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }),
    );
  };

  test.each(["f13", "f16", "f17"])(
    "%s has one tabindex 0 when drawn and at each stop the keys reach",
    { timeout: 60_000 },
    async (prefix) => {
      const name = nameOf(prefix);
      const source = need(SOURCE.get(name), name);
      const model = parseFlowchart(source);
      const body = source.trimEnd();
      const view = markView(`Top\n\n\`\`\`mermaid\n${body}\n\`\`\`\n\nEnd\n`);
      await prefetchDiagrams();
      const box = await vi.waitFor(() =>
        need(
          view.dom.querySelector<HTMLElement>(".cm-diagram:has(svg)"),
          "the drawn box",
        ),
      );
      await vi.waitFor(() => {
        expect(box.querySelectorAll('[role="button"]').length).toBeGreaterThan(
          0,
        );
      });
      const zeros = () => [
        ...(box.matches('[tabindex="0"]') ? [box] : []),
        ...box.querySelectorAll('[tabindex="0"]'),
      ];
      expect(zeros()).toEqual([box]);
      box.focus();
      const stops = flowStops(model).length;
      for (let walked = 1; walked < stops; walked++) {
        press("ArrowRight");
        const at = zeros();
        expect(at).toHaveLength(1);
        expect(at[0]).toBe(document.activeElement);
        expect(at[0]).not.toBe(box);
      }
    },
  );
});
