/**
 * Deleting boxes and arrows. This is the riskiest kind of op: a wrong one silently
 * loses what a person wrote. So a removal only ever takes the refs and the links it
 * means, and the bytes of what remains stay as written.
 *
 * A ref is *needed* when removing it would take something away: it is the last ref
 * of its box, or it holds the box's shape or class. A ref with an old label that a
 * later ref overrides counts too, so a person's text is not lost for a drawing that
 * does not show it. A subgraph id's ref is not a box's (R4) and is not needed.
 *
 * Every removal has a form that removes as much as the rules allow, and a second
 * that only splits the statement and keeps every ref. The second serves when the
 * self-check refuses the first, because a ref is what holds a box in its subgraph
 * (the first subgraph to name a box keeps it) and what lets an earlier `class` or
 * `click` line reach it. Both go through `checked`, so an id that would move to
 * another arrow or a line that would read as a direction still refuses.
 */

import type { OpResult, Span, TextEdit } from "../edits";
import {
  type Wanted,
  arrowsAfter,
  checked,
  refused,
  sameBoxes,
} from "./checked";
import {
  type GoneStatements,
  goneStatementRanges,
  markGone,
  shiftedIndices,
} from "./linkStyle";
import type {
  Chain,
  DirectiveLine,
  EdgeKey,
  FlowModel,
  FlowNode,
  Group,
  NodeRef,
} from "./model";
import { edgeByKey } from "./parse";

/** How many refs each box has left while a removal decides what to drop. */
type RefsLeft = Map<string, number>;

function refsLeft(model: FlowModel): RefsLeft {
  return new Map(
    [...model.nodes.values()].map((node) => [node.id, node.refs.length]),
  );
}

/** Whether `ref` must stay, given the refs its box has left. */
function isNeeded(model: FlowModel, ref: NodeRef, left: RefsLeft): boolean {
  if (!model.nodes.has(ref.id)) {
    return false;
  }
  return (
    (left.get(ref.id) ?? 0) <= 1 || ref.shape !== null || ref.classSpan !== null
  );
}

/** Whether `ref` can go, and if so counts it as gone. */
function dropped(model: FlowModel, ref: NodeRef, left: RefsLeft): boolean {
  if (isNeeded(model, ref, left)) {
    return false;
  }
  if (left.has(ref.id)) {
    left.set(ref.id, (left.get(ref.id) ?? 0) - 1);
  }
  return true;
}

function indentOf(model: FlowModel, chain: Chain): string {
  return model.scan.lines[chain.line]?.indent ?? "";
}

/** Deleting these spans, the linkStyle digits and every statement of `gone`. */
function assembled(
  model: FlowModel,
  edits: readonly TextEdit[],
  gone: GoneStatements,
  removed: ReadonlySet<number>,
): TextEdit[] {
  const out = [...edits];
  if (removed.size > 0) {
    out.push(...shiftedIndices(model, Math.min(...removed), -1, removed, gone));
  }
  out.push(
    ...goneStatementRanges(model, gone).map((span: Span) => ({
      ...span,
      insert: "",
    })),
  );
  return out;
}

/** The first of the tries whose result the self-check accepts, else the last refusal. */
function firstAccepted(tries: readonly (() => OpResult)[]): OpResult {
  let result: OpResult = refused("invalid");
  for (const attempt of tries) {
    result = attempt();
    if ("edits" in result) {
      return result;
    }
  }
  return result;
}

/** The form of a removal: what to take out of the chain for that one link. */
type LinkRemoval = { statement: true } | { span: Span; insert: string };

function ruledRemoval(
  model: FlowModel,
  chain: Chain,
  link: number,
  left: NodeRef,
  right: NodeRef,
  cut: LinkRemoval,
): LinkRemoval {
  const count = refsLeft(model);
  const links = chain.links.length;
  // A later ref goes first, so a box keeps its first mention.
  if (links === 1) {
    const rightGoes = dropped(model, right, count);
    const leftGoes = dropped(model, left, count);
    if (rightGoes && leftGoes) {
      return { statement: true };
    }
    if (leftGoes) {
      return {
        span: { from: left.span.from, to: right.span.from },
        insert: "",
      };
    }
    return rightGoes
      ? { span: { from: left.span.to, to: right.span.to }, insert: "" }
      : cut;
  }
  if (link === 0 && dropped(model, left, count)) {
    return { span: { from: left.span.from, to: right.span.from }, insert: "" };
  }
  if (link === links - 1 && dropped(model, right, count)) {
    return { span: { from: left.span.to, to: right.span.to }, insert: "" };
  }
  return cut;
}

export function deleteEdge(model: FlowModel, key: EdgeKey): OpResult {
  const edge = edgeByKey(model, key);
  const chain = model.chains[edge?.chain ?? -1];
  if (edge === null || chain === undefined) {
    return refused("gone");
  }
  const link = chain.links[edge.link];
  const leftGroup = chain.groups[edge.link];
  const rightGroup = chain.groups[edge.link + 1];
  const left = leftGroup?.refs[0];
  const right = rightGroup?.refs[0];
  if (
    link === undefined ||
    left === undefined ||
    right === undefined ||
    leftGroup?.refs.length !== 1 ||
    rightGroup?.refs.length !== 1
  ) {
    return refused("group");
  }
  const removed = new Set([edge.index]);
  const wanted: Record<string, Wanted> = {
    boxes: sameBoxes(model),
    ...arrowsAfter(model, removed),
  };
  const cut: LinkRemoval = {
    span: link.gap,
    insert: `\n${indentOf(model, chain)}`,
  };
  const attempt = (removal: LinkRemoval) => (): OpResult => {
    const gone: GoneStatements = new Map();
    const edits: TextEdit[] = [];
    if ("statement" in removal) {
      markGone(gone, chain.line, chain.span.from);
    } else {
      edits.push({ ...removal.span, insert: removal.insert });
    }
    return checked(model, assembled(model, edits, gone, removed), wanted);
  };
  return firstAccepted([
    attempt(ruledRemoval(model, chain, edge.link, left, right, cut)),
    attempt(cut),
  ]);
}

/** A run of neighbouring groups of a chain that keep at least one ref. */
interface Part {
  groups: Group[];
  /** The refs that stay, in order. */
  refs: NodeRef[];
}

/** The chain's parts once the refs of `id` are out. */
function partsWithout(chain: Chain, id: string): Part[] {
  const parts: Part[] = [];
  let open: Part | null = null;
  for (const group of chain.groups) {
    const refs = group.refs.filter((ref) => ref.id !== id);
    if (refs.length === 0) {
      open = null;
      continue;
    }
    if (open === null) {
      open = { groups: [], refs: [] };
      parts.push(open);
    }
    open.groups.push(group);
    open.refs.push(...refs);
  }
  return parts;
}

/**
 * Taking `id` out of the groups inside a part: the ref and one adjacent `&` with
 * its whitespace, on the side that keeps the rest byte-identical. A ref at the
 * part's outer edge goes with the span around the part instead.
 */
function refDeletionsInPart(part: Part, id: string): Span[] {
  const out: Span[] = [];
  for (const [g, group] of part.groups.entries()) {
    const i = group.refs.findIndex((ref) => ref.id === id);
    const ref = group.refs[i];
    if (ref === undefined) {
      continue;
    }
    const next = group.refs[i + 1];
    const previous = group.refs[i - 1];
    if (previous && (next || g < part.groups.length - 1)) {
      out.push({ from: previous.span.to, to: ref.span.to });
    } else if (next && !previous && g > 0) {
      out.push({ from: ref.span.from, to: next.span.from });
    }
  }
  return out;
}

/** What is left of `chain` without `id`, as edits or as a statement that goes. */
function chainWithout(
  model: FlowModel,
  chain: Chain,
  id: string,
  count: RefsLeft | null,
  edits: TextEdit[],
  gone: GoneStatements,
): void {
  const parts = partsWithout(chain, id);
  const kept: Part[] = [];
  // From the back, so a box keeps its first mention.
  for (const part of [...parts].reverse()) {
    const only = part.refs[0];
    const single = part.refs.length === 1 && only !== undefined;
    if (!(single && count !== null && dropped(model, only, count))) {
      kept.unshift(part);
    }
  }
  const first = kept[0]?.refs[0];
  const last = kept.at(-1)?.refs.at(-1);
  if (first === undefined || last === undefined) {
    markGone(gone, chain.line, chain.span.from);
    return;
  }
  const remove = ({ from, to }: Span) => {
    if (from < to) {
      edits.push({ from, to, insert: "" });
    }
  };
  remove({ from: chain.span.from, to: first.span.from });
  remove({ from: last.span.to, to: chain.span.to });
  const indent = indentOf(model, chain);
  for (const [k, part] of kept.entries()) {
    const end = part.refs.at(-1);
    const next = kept[k + 1]?.refs[0];
    if (end !== undefined && next !== undefined) {
      edits.push({
        from: end.span.to,
        to: next.span.from,
        insert: `\n${indent}`,
      });
    }
    refDeletionsInPart(part, id).forEach(remove);
  }
}

/** `id` out of the targets of a `class` or `click` line that names others too. */
function targetDeletions(directive: DirectiveLine, id: string): Span[] {
  const targets = directive.targets;
  const out: Span[] = [];
  for (let i = 0; i < targets.length; i++) {
    const first = targets[i];
    if (first === undefined || first.id !== id) {
      continue;
    }
    let end = i;
    while (targets[end + 1]?.id === id) {
      end++;
    }
    const next = targets[end + 1];
    const previous = targets[i - 1];
    const last = targets[end];
    if (next) {
      out.push({ from: first.span.from, to: next.span.from });
    } else if (previous && last) {
      out.push({ from: previous.span.to, to: last.span.to });
    }
    i = end;
  }
  return out;
}

/** What the box's `style`, `class` and `click` lines lose. `classDef` lines stay. */
function directiveChanges(
  model: FlowModel,
  id: string,
  edits: TextEdit[],
  gone: GoneStatements,
): void {
  for (const directive of model.directives) {
    if (
      directive.kind !== "style" &&
      directive.kind !== "class" &&
      directive.kind !== "click"
    ) {
      continue;
    }
    if (directive.targets.every((target) => target.id === id)) {
      markGone(gone, directive.line, directive.span.from);
    } else {
      edits.push(
        ...targetDeletions(directive, id).map((span) => ({
          ...span,
          insert: "",
        })),
      );
    }
  }
}

export function deleteNode(model: FlowModel, node: FlowNode): OpResult {
  const id = node.id;
  const removed = new Set(
    model.edges
      .filter((edge) => edge.start === id || edge.end === id)
      .map((edge) => edge.index),
  );
  const wanted: Record<string, Wanted> = {
    boxes: sameBoxes(model, id),
    subgraphs: JSON.stringify(
      model.subgraphs.map((s) => [
        s.id,
        s.parent,
        s.members.filter((member) => member !== id).sort(),
      ]),
    ),
    ...arrowsAfter(model, removed),
  };
  const chains = model.chains.filter((chain) =>
    chain.groups.some((group) => group.refs.some((ref) => ref.id === id)),
  );
  const attempt = (dropParts: boolean) => (): OpResult => {
    const count = dropParts ? refsLeft(model) : null;
    const edits: TextEdit[] = [];
    const gone: GoneStatements = new Map();
    // From the back, so a box keeps its first mention.
    for (const chain of [...chains].reverse()) {
      chainWithout(model, chain, id, count, edits, gone);
    }
    directiveChanges(model, id, edits, gone);
    return checked(model, assembled(model, edits, gone, removed), wanted, id);
  };
  return firstAccepted([attempt(true), attempt(false)]);
}
