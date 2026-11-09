/**
 * What a part of a drawn sequence diagram is in the source, and back. Elements are
 * told apart by what mermaid writes into them, never by their text: a participant
 * by the `data-id` of its box and its life line, a message by the record number in
 * its line's `data-id` (`i<record>`). A message's text carries no id: it is the
 * text before its line, so it maps to the first message line after it. A bottom
 * box carries none either, and stands below the top box of the same participant, so
 * it maps by where its label stands. Notes, blocks and whatever else does not map
 * are the canvas, so no key or click can reach a part of the source it does not
 * mean.
 */

import type { TargetKey } from "../intent";
import type { Verdict } from "../verdict";
import type { SeqMessage, SeqModel, SeqTarget } from "./model";
import { textOf } from "./ops";

type MessageTarget = SeqTarget & { kind: "message" };

const CANVAS: TargetKey = { kind: "canvas" };
const SVG_NS = "http://www.w3.org/2000/svg";

/** The target of a message: its ends, its text and its occurrence among equals. */
export function messageTarget(
  model: SeqModel,
  message: SeqMessage,
): MessageTarget {
  return {
    kind: "message",
    from: message.from,
    to: message.to,
    text: textOf(model, message),
    occurrence: message.occurrence,
  };
}

/** The message `target` names, or undefined when the source has none such. */
export function seqMessageOf(
  model: SeqModel,
  target: MessageTarget,
): SeqMessage | undefined {
  return model.messages.find(
    (m) =>
      m.from === target.from &&
      m.to === target.to &&
      m.occurrence === target.occurrence &&
      textOf(model, m) === target.text,
  );
}

/** Canvas, then participants left to right, then messages in source order. */
export function seqStops(model: SeqModel): TargetKey[] {
  return [
    CANVAS,
    ...model.order.map((name): TargetKey => ({ kind: "participant", name })),
    ...model.messages.map((message) => messageTarget(model, message)),
  ];
}

/** The message drawn by a line whose `data-id` is `i<record>`. */
function messageOfLine(model: SeqModel, el: Element): SeqMessage | undefined {
  const id = el.getAttribute("data-id") ?? "";
  const record = /^i(\d+)$/.exec(id)?.[1];
  return record === undefined
    ? undefined
    : model.messages.find((m) => m.record === Number(record));
}

/** The line a message's text belongs to: the first message line after it. */
function lineAfterText(text: Element): Element | null {
  for (
    let el = text.nextElementSibling;
    el !== null;
    el = el.nextElementSibling
  ) {
    if (el.getAttribute("data-et") === "message") {
      return el;
    }
  }
  return null;
}

/** Where the label of a participant's box stands across: its text's `x`. */
function labelX(group: Element | null): string | null {
  return group?.querySelector("text")?.getAttribute("x") ?? null;
}

/**
 * The participant a bottom box stands under: the one whose top box has its label at
 * the same place across. The box's label is in the element marked `actor-bottom`
 * or beside it.
 */
function bottomBoxName(el: Element, svg: Element): string | null {
  let bottom = el.closest(".actor-bottom");
  if (bottom === null) {
    bottom = el.closest("g")?.querySelector(":scope > .actor-bottom") ?? null;
  }
  if (bottom === null || !svg.contains(bottom)) {
    return null;
  }
  const x =
    labelX(bottom) ??
    (bottom.parentElement === null ? null : labelX(bottom.parentElement));
  if (x === null) {
    return null;
  }
  for (const top of svg.querySelectorAll('[data-et="participant"]')) {
    if (labelX(top) === x) {
      return top.getAttribute("data-id");
    }
  }
  return null;
}

/** What `el` stands for in the source: the canvas when nothing maps. */
export function seqTargetOf(
  el: Element,
  _svgId: string,
  model: SeqModel,
  _verdict: Verdict,
): TargetKey {
  const svg = el.closest("svg");
  if (svg === null) {
    return CANVAS;
  }
  const participant = el.closest(
    '[data-et="participant"], [data-et="life-line"]',
  );
  if (participant !== null) {
    const name = participant.getAttribute("data-id") ?? "";
    return model.participants.has(name)
      ? { kind: "participant", name }
      : CANVAS;
  }
  if (el.closest('[data-et="note"], [data-et="control-structure"]') !== null) {
    return CANVAS;
  }
  const hit = el.closest("[data-hit-for]");
  const line =
    hit === null
      ? el.closest('[data-et="message"]')
      : [...svg.querySelectorAll('[data-et="message"]')].find(
          (part) =>
            part.getAttribute("data-id") === hit.getAttribute("data-hit-for"),
        );
  const text = line ? null : el.closest(".messageText");
  const drawn = line ?? (text === null ? null : lineAfterText(text));
  if (drawn !== null && drawn !== undefined) {
    const message = messageOfLine(model, drawn);
    return message === undefined ? CANVAS : messageTarget(model, message);
  }
  const bottom = bottomBoxName(el, svg);
  return bottom !== null && model.participants.has(bottom)
    ? { kind: "participant", name: bottom }
    : CANVAS;
}

/** The element of a participant or message on screen; null for the canvas or none. */
export function seqElementOf(
  svg: SVGSVGElement,
  _svgId: string,
  target: TargetKey,
  model: SeqModel,
  _verdict: Verdict,
): SVGElement | null {
  if (target.kind === "participant") {
    for (const el of svg.querySelectorAll<SVGElement>(
      '[data-et="participant"]',
    )) {
      if (el.getAttribute("data-id") === target.name) {
        return el;
      }
    }
    return null;
  }
  if (target.kind === "message") {
    const message = seqMessageOf(model, target);
    if (message !== undefined) {
      const id = `i${String(message.record)}`;
      for (const el of svg.querySelectorAll<SVGElement>(
        '[data-et="message"]',
      )) {
        if (el.getAttribute("data-id") === id) {
          return el;
        }
      }
    }
  }
  return null;
}

/** The texts drawn for a message, in the order they stand before its line. */
export function seqMessageTexts(line: Element): Element[] {
  const texts: Element[] = [];
  for (
    let el = line.previousElementSibling;
    el !== null && el.classList.contains("messageText");
    el = el.previousElementSibling
  ) {
    texts.unshift(el);
  }
  return texts;
}

function participantLabel(model: SeqModel, name: string): string {
  return model.participants.get(name)?.label || name;
}

/**
 * What the rename field shows for `target`, which is what is drawn: a participant's
 * label, a message's text (empty with none). Null when the source has no such
 * participant or message, so a target a peer took away can be told.
 */
export function seqLabelOf(model: SeqModel, target: TargetKey): string | null {
  if (target.kind === "participant") {
    return model.participants.has(target.name)
      ? participantLabel(model, target.name)
      : null;
  }
  if (target.kind === "message") {
    const message = seqMessageOf(model, target);
    return message === undefined ? null : textOf(model, message);
  }
  return null;
}

function messageName(model: SeqModel, message: SeqMessage): string {
  const ends = `Message from ${participantLabel(model, message.from)} to ${participantLabel(model, message.to)}`;
  const text = textOf(model, message);
  return text === "" ? ends : `${ends}: ${text}`;
}

/** Where a message's arrow token starts in the body, or null with no such message. */
export function seqMessageSpanOf(
  model: SeqModel,
  target: MessageTarget,
): number | null {
  return seqMessageOf(model, target)?.arrow.span.from ?? null;
}

/** The stop whose source holds `offset`, a position in the body; the canvas otherwise. */
export function seqStopAt(model: SeqModel, offset: number): TargetKey {
  for (const message of model.messages) {
    const { fromSpan, toSpan, span } = message;
    if (fromSpan.from <= offset && offset <= fromSpan.to) {
      return { kind: "participant", name: message.from };
    }
    if (toSpan.from <= offset && offset <= toSpan.to) {
      return { kind: "participant", name: message.to };
    }
    if (span.from <= offset && offset <= span.to) {
      return messageTarget(model, message);
    }
  }
  for (const statement of model.statements) {
    for (const actor of statement.actors) {
      if (actor.span.from <= offset && offset <= actor.span.to) {
        return model.participants.has(actor.name)
          ? { kind: "participant", name: actor.name }
          : CANVAS;
      }
    }
  }
  for (const [name, participant] of model.participants) {
    for (const declaration of participant.declarations) {
      const { span } = declaration;
      if (span.from <= offset && offset <= span.to) {
        return { kind: "participant", name };
      }
    }
  }
  return CANVAS;
}

/**
 * Makes the drawing's participants and messages stops: out of the tab order until
 * the focus code gives one `tabindex="0"`, named for a screen reader, and each
 * message with a wide transparent copy of its line beside it, so a thin line can be
 * hit. The copy goes after the line, so the walk from a text to its line still
 * meets the line first.
 */
export function decorateSequence(
  svg: SVGSVGElement,
  _svgId: string,
  model: SeqModel,
  _verdict: Verdict,
): void {
  for (const el of svg.querySelectorAll<SVGElement>(
    '[data-et="participant"]',
  )) {
    const name = el.getAttribute("data-id") ?? "";
    if (!model.participants.has(name)) {
      continue;
    }
    el.setAttribute("tabindex", "-1");
    el.setAttribute("role", "button");
    el.setAttribute("aria-haspopup", "menu");
    el.setAttribute(
      "aria-label",
      `Participant: ${participantLabel(model, name)}`,
    );
  }
  for (const el of svg.querySelectorAll<SVGElement>('[data-et="message"]')) {
    const message = messageOfLine(model, el);
    const id = el.getAttribute("data-id");
    if (message === undefined || id === null) {
      continue;
    }
    el.setAttribute("tabindex", "-1");
    el.setAttribute("role", "button");
    el.setAttribute("aria-haspopup", "menu");
    el.setAttribute("aria-label", messageName(model, message));
    const hit = document.createElementNS(SVG_NS, el.localName);
    for (const attr of ["d", "x1", "y1", "x2", "y2"]) {
      const value = el.getAttribute(attr);
      if (value !== null) {
        hit.setAttribute(attr, value);
      }
    }
    hit.setAttribute("data-hit-for", id);
    hit.setAttribute("aria-hidden", "true");
    hit.style.fill = "none";
    hit.style.stroke = "transparent";
    hit.style.strokeWidth = "14px";
    hit.style.pointerEvents = "stroke";
    el.after(hit);
  }
}
