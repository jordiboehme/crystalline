/**
 * The one module that reads mermaid's own diagram db.
 *
 * A fence is drawn as an editable diagram only when mermaid's parse of it
 * agrees with ours, and an action is checked the same way before it is
 * dispatched. Mermaid's public API answers only "parses" or "does not
 * parse", so this module asks `mermaid.mermaidAPI.getDiagramFromText`, which
 * mermaid marks `@internal`, for the db its parser filled, and copies out the
 * plain data the agreement check compares. Using it is deliberate and kept
 * to this file; `mermaidVersion.test.ts` fails on any mermaid version change,
 * so a bump re-runs the checks in `mermaidDb.test.ts` before anything else
 * relies on the db's shape again.
 *
 * mermaid is imported dynamically, so this module costs nothing until a
 * diagram asks and mermaid stays its own lazy chunk.
 *
 * Two facts of mermaid 12.2.0's flowchart db the callers rely on (pinned in the test):
 *
 * - `domId` is `flowchart-<id>-<n>`, where `n` counts every `addVertex` call
 *   (edge ends and `style` lines included), so it cannot be derived from the
 *   source. The rendered node's id is `<svg id>-<domId>`. Map with this
 *   field, never compute it.
 * - An edge id is the explicit `e1@-->` id when no earlier edge has it, and
 *   otherwise `L_<start>_<end>_<k>` with `k` 0 for the first edge of a pair
 *   and then 2, 3, ... (1 is skipped). The rendered path carries it as
 *   `data-id`. Match it exactly, never by splitting on `_`.
 */

import { mermaidConfig } from "../../theme/mermaid";

/** One vertex, in `db.getVertices()` order. */
export interface FlowVertexView {
  id: string;
  /** The element id mermaid renders, without the `<svg id>-` prefix. */
  domId: string;
  /** The label as mermaid stores it: entities still encoded, `<br>` kept. */
  text: string;
  /** `square`, `round`, ... for the classic forms, the shape name for `@{ shape }`, undefined for a bare id. */
  type: string | undefined;
  classes: string[];
  styles: string[];
  /** `text` or `markdown`, how the label is read. */
  labelType: string | undefined;
  link: string | undefined;
  linkTarget: string | undefined;
  tooltip: string | undefined;
}

/** One edge, in `db.getEdges()` order (which is `linkStyle`'s index order). */
export interface FlowEdgeView {
  id: string;
  start: string;
  end: string;
  text: string;
  type: string | undefined;
  stroke: string | undefined;
  length: number | undefined;
  labelType: string | undefined;
  /** What a numeric `linkStyle` line gave this edge, undefined for none. */
  style: string[] | undefined;
  interpolate: string | undefined;
}

export interface FlowSubgraphView {
  id: string;
  title: string;
  nodes: string[];
  dir: string | undefined;
  labelType: string | undefined;
}

/** One `classDef`. */
export interface FlowClassDefView {
  id: string;
  styles: string[];
  textStyles: string[];
}

export interface FlowDbView {
  direction: string | undefined;
  vertices: FlowVertexView[];
  edges: FlowEdgeView[];
  subgraphs: FlowSubgraphView[];
  classDefs: FlowClassDefView[];
}

/** The part of mermaid's FlowDB this module reads. */
interface RawFlowDb {
  getDirection(): string | undefined;
  getVertices(): Map<
    string,
    {
      id: string;
      domId: string;
      text?: string;
      type?: string;
      classes: string[];
      styles: string[];
      labelType?: string;
      link?: string;
      linkTarget?: string;
    }
  >;
  getEdges(): {
    /** Always set by `addSingleLink`, and what the renderer writes. */
    id: string;
    start: string;
    end: string;
    text: string;
    type?: string;
    stroke?: string;
    length?: number;
    labelType?: string;
    style?: string[];
    interpolate?: string;
  }[];
  getSubGraphs(): {
    id: string;
    title: string;
    nodes: string[];
    dir?: string;
    labelType?: string;
  }[];
  getClasses(): Map<string, { styles: string[]; textStyles: string[] }>;
  getTooltip(id: string): string | undefined;
}

/**
 * What a failed dynamic import says: Chromium, Firefox and WebKit each have
 * their own words, and Vite's preload helper has its own for a CSS file.
 */
const IMPORT_FAILURE =
  /Failed to fetch dynamically imported module|error loading dynamically imported module|Importing a module script failed|Unable to preload CSS/;

/**
 * Whether `error` is a failed dynamic import and not a refusal of the body.
 * mermaid's parser puts `hash` on its errors and quotes the source in them,
 * so an error with a `hash` is a refusal whatever its words.
 */
export function isImportFailure(error: unknown): boolean {
  return (
    error instanceof Error &&
    !("hash" in error) &&
    IMPORT_FAILURE.test(error.message)
  );
}

/** The diagram types whose db is a FlowDB. */
const FLOW_TYPES = new Set(["flowchart-v2", "flowchart", "flowchart-elk"]);

/**
 * Mermaid's flowchart db for `body` (a fence body: frontmatter, directives
 * and comments included, exactly as written).
 *
 * `dark` is the scheme of the view that asks: `initialize` is not queued the
 * way `render` is, so a verdict must set the same configuration a pending
 * render of that view set, or it would switch the theme under it.
 *
 * A body mermaid refuses, or one that is not a flowchart, comes back as
 * `{ error }`. mermaid failing to load says nothing about the body, so it
 * comes back with `unavailable: true` as well, whether that is mermaid's main
 * chunk or the diagram chunk it imports inside `getDiagramFromText`
 * (`isImportFailure`). This never throws.
 */
export async function flowDbOf(
  body: string,
  dark: boolean,
): Promise<FlowDbView | { error: unknown; unavailable?: true }> {
  let mermaid: (typeof import("mermaid"))["default"];
  try {
    ({ default: mermaid } = await import("mermaid"));
  } catch (error) {
    return { error, unavailable: true };
  }
  try {
    mermaid.initialize(mermaidConfig(dark));
    const diagram = await mermaid.mermaidAPI.getDiagramFromText(body);
    const type = diagram.type;
    if (!FLOW_TYPES.has(type)) {
      return { error: new Error(`not a flowchart: ${type}`) };
    }
    const db = diagram.db as unknown as RawFlowDb;
    return {
      direction: db.getDirection(),
      vertices: [...db.getVertices().values()].map((v) => ({
        id: v.id,
        domId: v.domId,
        text: v.text ?? v.id,
        type: v.type,
        classes: [...v.classes],
        styles: [...v.styles],
        labelType: v.labelType,
        link: v.link,
        linkTarget: v.linkTarget,
        tooltip: db.getTooltip(v.id),
      })),
      edges: db.getEdges().map((e) => ({
        id: e.id,
        start: e.start,
        end: e.end,
        text: e.text,
        type: e.type,
        stroke: e.stroke,
        length: e.length,
        labelType: e.labelType,
        style: e.style && [...e.style],
        interpolate: e.interpolate,
      })),
      subgraphs: db.getSubGraphs().map((s) => ({
        id: s.id,
        title: s.title,
        nodes: [...s.nodes],
        dir: s.dir,
        labelType: s.labelType,
      })),
      classDefs: [...db.getClasses()].map(([id, c]) => ({
        id,
        styles: [...c.styles],
        textStyles: [...c.textStyles],
      })),
    };
  } catch (error) {
    return isImportFailure(error) ? { error, unavailable: true } : { error };
  }
}

/** One participant, in `db.getActors()` order, which is left to right. */
export interface SequenceActorView {
  name: string;
  /** The label as mermaid stores it: entities still encoded. */
  description: string;
  type: string;
}

/** One entry of mermaid's message list. Its index is the `i<n>` the drawing carries. */
export interface SequenceRecordView {
  id: string;
  from: string | undefined;
  to: string | undefined;
  /** The text; empty on an `autonumber` record, whose db message is its numbers. */
  message: string;
  /** mermaid's `LINETYPE`. */
  type: number | undefined;
  /** An `autonumber` record's numbers. */
  autonumber?: { start?: number; step?: number; visible: boolean };
  /** A note's `PLACEMENT`. */
  placement?: number;
}

export interface SequenceDbView {
  actors: SequenceActorView[];
  records: SequenceRecordView[];
  /** Each box's members. Its colour and title depend on the DOM's CSS support, so they are left out. */
  boxes: { actors: string[] }[];
  /** A created participant and the index of the record that creates it. */
  created: [string, number][];
  /** A destroyed participant and the index of the record that destroys it. */
  destroyed: [string, number][];
}

/** The part of mermaid's SequenceDB this module reads. */
interface RawSequenceDb {
  getActors(): Map<string, { name: string; description: string; type: string }>;
  getMessages(): {
    id: string;
    from?: string;
    to?: string;
    message: string | { start?: number; step?: number; visible: boolean };
    type?: number;
    placement?: number;
  }[];
  getBoxes(): { actorKeys: string[] }[];
  getCreatedActors(): Map<string, number>;
  getDestroyedActors(): Map<string, number>;
}

/**
 * Mermaid's sequence db for `body`, a new `SequenceDB` per diagram. The rest
 * is as for `flowDbOf`: `dark` is the scheme of the view that asks, a refusal or
 * another diagram type is `{ error }`, mermaid or its diagram chunk failing to
 * load is `{ error, unavailable: true }`, and this never throws.
 */
export async function sequenceDbOf(
  body: string,
  dark: boolean,
): Promise<SequenceDbView | { error: unknown; unavailable?: true }> {
  let mermaid: (typeof import("mermaid"))["default"];
  try {
    ({ default: mermaid } = await import("mermaid"));
  } catch (error) {
    return { error, unavailable: true };
  }
  try {
    mermaid.initialize(mermaidConfig(dark));
    const diagram = await mermaid.mermaidAPI.getDiagramFromText(body);
    if (diagram.type !== "sequence") {
      return { error: new Error(`not a sequence diagram: ${diagram.type}`) };
    }
    const db = diagram.db as unknown as RawSequenceDb;
    return {
      actors: [...db.getActors().values()].map((a) => ({
        name: a.name,
        description: a.description,
        type: a.type,
      })),
      records: db.getMessages().map((m) => ({
        id: m.id,
        from: m.from,
        to: m.to,
        message: typeof m.message === "string" ? m.message : "",
        type: m.type,
        ...(typeof m.message === "string"
          ? {}
          : { autonumber: definedOnly(m.message) }),
        ...(m.placement === undefined ? {} : { placement: m.placement }),
      })),
      boxes: db.getBoxes().map((b) => ({ actors: [...b.actorKeys] })),
      created: [...db.getCreatedActors()],
      destroyed: [...db.getDestroyedActors()],
    };
  } catch (error) {
    return isImportFailure(error) ? { error, unavailable: true } : { error };
  }
}

function definedOnly(numbers: {
  start?: number;
  step?: number;
  visible: boolean;
}): { start?: number; step?: number; visible: boolean } {
  return {
    ...(numbers.start === undefined ? {} : { start: numbers.start }),
    ...(numbers.step === undefined ? {} : { step: numbers.step }),
    visible: numbers.visible,
  };
}
