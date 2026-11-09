/**
 * A click on a drawn sequence diagram maps back to the participant or message in
 * the source. Every recorded mermaid 12.2.0 drawing of a sequence is loaded next to
 * its source and read through the real parser. The expected participant comes from
 * the `data-id` mermaid wrote, the expected message from `data-from`, `data-to`
 * and an occurrence counted again here, and a message's texts are found walking
 * back from its line, the other way round from the code under test.
 */

import { afterEach, describe, expect, test } from "vitest";

import type { TargetKey } from "../intent";
import type { Verdict } from "../verdict";
import {
  decorateSequence,
  seqElementOf,
  seqLabelOf,
  seqStops,
  seqTargetOf,
} from "./hit";
import type { SeqModel } from "./model";
import { decodeSeqText, parseSequence } from "./parse";

const SOURCES = import.meta.glob<string>(
  "../../fixtures/diagrams/sequence/*.mmd",
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
const VERDICT: Verdict = { ok: true, kind: "sequence", domIds: new Map() };
const CANVAS: TargetKey = { kind: "canvas" };

function need<T>(value: T | null | undefined, what: string): T {
  if (value === null || value === undefined) {
    throw new Error(`missing: ${what}`);
  }
  return value;
}

interface Loaded {
  svg: SVGSVGElement;
  model: SeqModel;
}

function load(name: string): Loaded {
  const container = document.createElement("div");
  container.className = "cm-diagram";
  container.innerHTML = need(DRAWING.get(name), `the drawing of ${name}`);
  document.body.append(container);
  return {
    svg: need(container.querySelector("svg"), `the svg of ${name}`),
    model: parseSequence(need(SOURCE.get(name), `the source of ${name}`)),
  };
}

afterEach(() => {
  document.body.replaceChildren();
});

const targetOf = (loaded: Loaded, el: Element) =>
  seqTargetOf(el, loaded.svg.id, loaded.model, VERDICT);

/** Each drawn message line's target, its occurrence counted again in line order. */
function expectedMessages(loaded: Loaded): Map<Element, TargetKey> {
  const { model, svg } = loaded;
  const seen = new Map<string, number>();
  const out = new Map<Element, TargetKey>();
  const lines = [...svg.querySelectorAll('[data-et="message"]')];
  for (const line of lines) {
    const record = Number(line.getAttribute("data-id")?.slice(1));
    const own = model.records[record];
    const text =
      own?.text === null || own === undefined
        ? ""
        : decodeSeqText(model.body.slice(own.text.from, own.text.to));
    const from = need(line.getAttribute("data-from"), "data-from");
    const to = need(line.getAttribute("data-to"), "data-to");
    const key = JSON.stringify([from, to, text]);
    const occurrence = seen.get(key) ?? 0;
    seen.set(key, occurrence + 1);
    out.set(line, { kind: "message", from, to, text, occurrence });
  }
  return out;
}

/** The texts before a line, walking back. */
function textsOf(line: Element): Element[] {
  const texts: Element[] = [];
  for (
    let el = line.previousElementSibling;
    el?.classList.contains("messageText") === true;
    el = el.previousElementSibling
  ) {
    texts.push(el);
  }
  return texts;
}

describe("every recorded sequence drawing maps", () => {
  test("the sweep has every source drawn", () => {
    expect(NAMES.length).toBeGreaterThanOrEqual(33);
    for (const name of NAMES) {
      expect(DRAWING.has(name), name).toBe(true);
    }
  });

  test.each(NAMES)("%s: participant boxes and life lines", (name) => {
    const loaded = load(name);
    const parts = [
      ...loaded.svg.querySelectorAll(
        '[data-et="participant"], [data-et="life-line"]',
      ),
    ];
    expect(parts.length).toBe(2 * loaded.model.participants.size);
    for (const part of parts) {
      const wanted = {
        kind: "participant",
        name: part.getAttribute("data-id"),
      };
      expect(targetOf(loaded, part)).toEqual(wanted);
      for (const inner of part.querySelectorAll("*")) {
        expect(targetOf(loaded, inner)).toEqual(wanted);
      }
    }
  });

  test.each(NAMES)("%s: bottom boxes, by where they stand", (name) => {
    const loaded = load(name);
    const bottoms = [...loaded.svg.querySelectorAll(".actor-bottom")];
    const tops = new Map(
      [...loaded.svg.querySelectorAll('[data-et="participant"]')].map((top) => [
        top.querySelector("text")?.textContent ?? "",
        top.getAttribute("data-id"),
      ]),
    );
    for (const bottom of bottoms) {
      // A link menu's hidden panel is no box: what it holds is the canvas.
      if (bottom.closest(".actorPopupMenu") !== null) {
        expect(targetOf(loaded, bottom)).toEqual(CANVAS);
        continue;
      }
      // Its label is in it or beside it: the same words as its top box's label.
      const label =
        bottom.querySelector("text")?.textContent ??
        bottom.parentElement?.querySelector("text")?.textContent ??
        "";
      const name = need(tops.get(label), `a top box reading ${label}`);
      expect(targetOf(loaded, bottom)).toEqual({ kind: "participant", name });
    }
  });

  test.each(NAMES)("%s: message lines and their texts", (name) => {
    const loaded = load(name);
    const wanted = expectedMessages(loaded);
    expect(wanted.size).toBe(loaded.model.messages.length);
    for (const [line, target] of wanted) {
      expect(targetOf(loaded, line)).toEqual(target);
      for (const text of textsOf(line)) {
        expect(targetOf(loaded, text)).toEqual(target);
      }
      expect(
        seqElementOf(loaded.svg, loaded.svg.id, target, loaded.model, VERDICT),
      ).toBe(line);
    }
  });

  test.each(NAMES)("%s: notes and blocks are the canvas", (name) => {
    const loaded = load(name);
    for (const part of loaded.svg.querySelectorAll(
      '[data-et="note"], [data-et="control-structure"]',
    )) {
      expect(targetOf(loaded, part)).toEqual(CANVAS);
      for (const inner of part.querySelectorAll("*")) {
        expect(targetOf(loaded, inner)).toEqual(CANVAS);
      }
    }
    expect(targetOf(loaded, loaded.svg)).toEqual(CANVAS);
  });

  test.each(NAMES)("%s: every stop but the canvas has its element", (name) => {
    const loaded = load(name);
    for (const stop of seqStops(loaded.model)) {
      const el = seqElementOf(
        loaded.svg,
        loaded.svg.id,
        stop,
        loaded.model,
        VERDICT,
      );
      if (stop.kind === "canvas") {
        expect(el).toBeNull();
      } else {
        expect(el, JSON.stringify(stop)).not.toBeNull();
        expect(targetOf(loaded, need(el, "element"))).toEqual(stop);
      }
    }
  });

  test.each(NAMES)(
    "%s: decorated, each stop is named and a hit line maps",
    (name) => {
      const loaded = load(name);
      decorateSequence(loaded.svg, loaded.svg.id, loaded.model, VERDICT);
      for (const stop of seqStops(loaded.model)) {
        if (stop.kind === "canvas") {
          continue;
        }
        const el = need(
          seqElementOf(loaded.svg, loaded.svg.id, stop, loaded.model, VERDICT),
          "element",
        );
        expect(el.getAttribute("role")).toBe("button");
        expect(el.getAttribute("tabindex")).toBe("-1");
        const label = seqLabelOf(loaded.model, stop) ?? "";
        if (stop.kind === "participant") {
          expect(el.getAttribute("aria-label")).toBe(`Participant: ${label}`);
        } else if (stop.kind === "message") {
          const named = (n: string) =>
            loaded.model.participants.get(n)?.label ?? n;
          const ends = `Message from ${named(stop.from)} to ${named(stop.to)}`;
          expect(el.getAttribute("aria-label")).toBe(
            label === "" ? ends : `${ends}: ${label}`,
          );
          const hit = need(el.nextElementSibling, "the hit line");
          expect(hit.getAttribute("data-hit-for")).toBe(
            el.getAttribute("data-id"),
          );
          expect(targetOf(loaded, hit)).toEqual(stop);
        }
      }
      // The texts still find their line past the copies.
      for (const [line, target] of expectedMessages(loaded)) {
        for (const text of textsOf(line)) {
          expect(targetOf(loaded, text)).toEqual(target);
        }
      }
    },
  );
});

describe("the starter", () => {
  test("stops: the canvas, caller, service, Request, Reply", () => {
    const { model } = load("s01-starter");
    expect(seqStops(model)).toEqual([
      CANVAS,
      { kind: "participant", name: "caller" },
      { kind: "participant", name: "service" },
      {
        kind: "message",
        from: "caller",
        to: "service",
        text: "Request",
        occurrence: 0,
      },
      {
        kind: "message",
        from: "service",
        to: "caller",
        text: "Reply",
        occurrence: 0,
      },
    ]);
  });

  test("labels read what is drawn", () => {
    const { model } = load("s01-starter");
    expect(seqLabelOf(model, { kind: "participant", name: "caller" })).toBe(
      "Caller",
    );
    expect(
      seqLabelOf(model, {
        kind: "message",
        from: "caller",
        to: "service",
        text: "Request",
        occurrence: 0,
      }),
    ).toBe("Request");
    expect(seqLabelOf(model, { kind: "participant", name: "nobody" })).toBe(
      null,
    );
  });
});
