/**
 * The menus and connect mode with a second person in the room: nothing they write
 * may make an item land on another box or arrow, and what still stands stays open.
 * `actions.collab.marks-hidden.test.ts` and `actions.collab.marks-show.test.ts` run
 * this again.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { EditorView } from "@codemirror/view";

import { cellMarksMode } from "../../../test/cellMarksMode";
import {
  bytes,
  cleanupMarkViews,
  markPeers,
  undoOnce,
} from "../../../test/markKit";
import { NOTICES } from "../edits";
import { focusTarget } from "../focus";
import type { TargetKey } from "../intent";
import { diagramIntent } from "../intent";
import { prefetchDiagrams } from "../loader";
import { requestVerdict } from "../verdict";
import { forgetDrawings } from "../widget";
import { parseFlowchart } from "./parse";

vi.mock("mermaid", async (original) => {
  const real = (await original<{ default: object }>()).default;
  return {
    default: {
      ...real,
      render: async (id: string, body: string) => {
        const verdict = await requestVerdict(body, false);
        if (!verdict.ok) {
          throw new Error("no diagram");
        }
        const nodes = [...verdict.domIds.keys()]
          .map((dom) => `<g class="node default" id="${id}-${dom}"></g>`)
          .join("");
        const edges = parseFlowchart(body)
          .edges.map(
            ({ mermaidId }) =>
              `<path d="M0,0L1,1" data-et="edge" data-id="${mermaidId}"></path>`,
          )
          .join("");
        return {
          svg: `<svg id="${id}"><g class="edgePaths">${edges}</g><g class="nodes">${nodes}</g></svg>`,
        };
      },
    },
  };
});

const page = (body: string) =>
  `Above\n\n\`\`\`mermaid\n${body}\n\`\`\`\n\nBelow\n`;
const BODY = "flowchart TD\n    A[First] --> B[Second]\n    A --> C[Third]";
const NODE_A: TargetKey = { kind: "node", id: "A" };
const NODE_B: TargetKey = { kind: "node", id: "B" };
const TWO = "flowchart TD\n    A -->|one| B\n    A -->|two| B";
/** The second of a duplicate pair. */
const SECOND: TargetKey = {
  kind: "edge",
  key: { start: "A", end: "B", occurrence: 1 },
};
const FIRST: TargetKey = {
  kind: "edge",
  key: { start: "A", end: "B", occurrence: 0 },
};

const flush = async () => {
  await new Promise((resolve) => setTimeout(resolve, 0));
};
const settle = async () => {
  await flush();
  await new Promise((resolve) => setTimeout(resolve, 30));
  await flush();
};

const modeOptions = () => {
  const mode = cellMarksMode();
  return mode === "off" ? {} : { mode };
};

async function room(body = BODY) {
  const peers = markPeers(page(body), modeOptions());
  await prefetchDiagrams();
  await vi.waitFor(() => {
    expect(peers.local.dom.querySelector(".cm-diagram svg")).not.toBeNull();
  });
  await flush();
  return peers;
}

const text = (view: EditorView) => bytes(view.state);
const active = () => document.activeElement as HTMLElement;
const menus = () => [...document.querySelectorAll('[role="menu"]')];
const noticeOf = (view: EditorView) =>
  view.dom.querySelector(".cm-diagram-notice")?.textContent ?? "";
const promptOf = (view: EditorView) =>
  view.dom.querySelector('.cm-diagram-overlay [role="status"]');
const intentOf = (view: EditorView) => view.state.field(diagramIntent);

function press(key: string, options: KeyboardEventInit = {}) {
  const event = new KeyboardEvent("keydown", {
    key,
    bubbles: true,
    cancelable: true,
    ...options,
  });
  active().dispatchEvent(event);
  return event;
}

function openOn(view: EditorView, target: TargetKey) {
  focusTarget(view, text(view).indexOf("```"), target);
  press("F10", { shiftKey: true });
  expect(menus()).toHaveLength(1);
}

function row(label: string): HTMLElement {
  const found = [...document.querySelectorAll('[role^="menuitem"]')].find(
    (el) => el.textContent === label,
  );
  if (found === undefined) throw new Error(`no item ${label}`);
  return found as HTMLElement;
}

const choose = (...path: string[]) => {
  for (const label of path) {
    row(label).click();
  }
};

const nodeOf = (view: EditorView, id: string) => {
  const found = view.dom.querySelector(`g.node[id*="flowchart-${id}-"]`);
  if (found === null) throw new Error(`no box ${id}`);
  return found;
};

function peerInsert(peer: EditorView, before: string, insert: string) {
  const at = text(peer).indexOf(before);
  peer.dispatch({ changes: { from: at, insert } });
}

function peerDeleteLine(peer: EditorView, line: string) {
  const at = text(peer).indexOf(line);
  peer.dispatch({ changes: { from: at, to: at + line.length + 1 } });
}

beforeEach(() => {
  forgetDrawings();
});

afterEach(() => {
  cleanupMarkViews();
  document.body.replaceChildren();
});

describe("a box's menu with a peer", () => {
  it("stays open through a peer's %% line, and Delete deletes B and only B", async () => {
    const { local, peer, sync } = await room();
    openOn(local, NODE_B);
    const focused = active();
    peerInsert(peer, "    A --> C", "    %% peer note\n");
    sync();
    await settle();
    expect(text(local)).toContain("%% peer note");
    expect(menus()).toHaveLength(1);
    expect(active()).toBe(focused);
    expect(intentOf(local)?.mode).toBe("menu");
    choose("Delete");
    await vi.waitFor(() => {
      expect(text(local)).not.toContain("Second");
    });
    expect(text(local)).toBe(
      page("flowchart TD\n    A[First]\n    %% peer note\n    A --> C[Third]"),
    );
    undoOnce(local);
    expect(text(local)).toBe(
      page(
        "flowchart TD\n    A[First] --> B[Second]\n    %% peer note\n    A --> C[Third]",
      ),
    );
  });

  it("closes with the gone notice when the peer deletes B, and changes nothing", async () => {
    const { local, peer, sync } = await room();
    openOn(local, NODE_B);
    peerDeleteLine(peer, "    A[First] --> B[Second]");
    sync();
    await settle();
    expect(menus()).toHaveLength(0);
    expect(noticeOf(local)).toBe(NOTICES.gone);
    expect(text(local)).toBe(text(peer));
    expect(active().tagName.toLowerCase()).not.toBe("body");
    expect(local.dom.contains(active())).toBe(true);
    expect(intentOf(local)?.target).toEqual({ kind: "canvas" });
  });

  it("a redraw while it is open leaves the focus in the menu", async () => {
    const { local, peer, sync } = await room();
    openOn(local, NODE_B);
    press("ArrowDown");
    const focused = active();
    const stop = nodeOf(local, "B");
    peerInsert(peer, "    A --> C", "    A --> D\n");
    sync();
    await vi.waitFor(() => {
      expect(nodeOf(local, "B")).not.toBe(stop);
    });
    await settle();
    expect(active()).toBe(focused);
    // The look and the roving tabindex stand on the new drawing's element.
    expect(nodeOf(local, "B").getAttribute("tabindex")).toBe("0");
    expect(nodeOf(local, "B").classList.contains("cm-diagram-focus")).toBe(
      true,
    );
    press("Escape");
    expect(active()).toBe(nodeOf(local, "B"));
    expect(local.dom.contains(active())).toBe(true);
  });

  it("what is checked follows the peer's change", async () => {
    const { local, peer, sync } = await room();
    openOn(local, NODE_B);
    choose("Shape");
    const at = text(peer).indexOf("B[Second]");
    peer.dispatch({
      changes: { from: at, to: at + 9, insert: "B{Second}" },
    });
    sync();
    await settle();
    expect(row("Decision").getAttribute("aria-checked")).toBe("true");
    expect(row("Rectangle").getAttribute("aria-checked")).toBe("false");
    expect(active().textContent).toBe("Rectangle");
  });

  it("a peer's line that reads wrong keeps it open, and an item says stale", async () => {
    const { local, peer, sync } = await room();
    openOn(local, NODE_B);
    peerInsert(peer, "    A --> C", "    A[[[\n");
    sync();
    await settle();
    expect(menus()).toHaveLength(1);
    expect(local.dom.querySelector(".cm-diagram")).not.toBeNull();
    const before = text(local);
    choose("Delete");
    // Nothing holds the fence any more, so it goes to text, and the panel says it.
    await vi.waitFor(() => {
      expect(local.dom.querySelector(".cm-diagram-refusal")?.textContent).toBe(
        NOTICES.stale,
      );
    });
    expect(text(local)).toBe(before);
    expect(active()).toBe(local.contentDOM);
  });
});

describe("an arrow's menu among duplicates (R94)", () => {
  it("a duplicate added above: Delete takes my arrow, not the new one", async () => {
    const { local, peer, sync } = await room(TWO);
    openOn(local, SECOND);
    peerInsert(peer, "    A -->|one|", "    A -->|peers| B\n");
    sync();
    await settle();
    expect(menus()).toHaveLength(1);
    choose("Delete");
    await vi.waitFor(() => {
      expect(text(local)).not.toContain("two");
    });
    expect(text(local)).toBe(
      page("flowchart TD\n    A -->|peers| B\n    A -->|one| B"),
    );
  });

  it("the first of the pair deleted: Line lands on mine, which is now the first", async () => {
    const { local, peer, sync } = await room(
      "flowchart TD\n    A -->|one| B\n    A -->|two| B\n    A -->|three| B",
    );
    openOn(local, SECOND);
    peerDeleteLine(peer, "    A -->|one| B");
    sync();
    await settle();
    choose("Line", "Dotted");
    await vi.waitFor(() => {
      expect(text(local)).toContain("-.->");
    });
    expect(text(local)).toBe(
      page("flowchart TD\n    A -.->|two| B\n    A -->|three| B"),
    );
  });

  it("my arrow deleted, its twin stands: stale, and the twin is untouched", async () => {
    const { local, peer, sync } = await room(TWO);
    openOn(local, SECOND);
    peerDeleteLine(peer, "    A -->|two| B");
    sync();
    await settle();
    const before = text(local);
    choose("Delete");
    await vi.waitFor(() => {
      expect(noticeOf(local)).toBe(NOTICES.stale);
    });
    expect(text(local)).toBe(before);
    expect(text(local)).toContain("|one|");
    // The ring does not stand on the arrow the next Delete would take.
    expect(intentOf(local)?.target).toEqual({ kind: "canvas" });
  });

  it("a sole arrow gets a twin, then goes: stale, in one sync or in two", async () => {
    for (const oneSync of [true, false]) {
      const { local, peer, sync } = await room(
        "flowchart TD\n    A -->|mine| B",
      );
      openOn(local, FIRST);
      peerInsert(peer, "    A -->|mine|", "    A -->|peers| B\n");
      if (!oneSync) {
        sync();
        await settle();
      }
      peerDeleteLine(peer, "    A -->|mine| B");
      sync();
      await settle();
      const before = text(local);
      choose("Reverse");
      await vi.waitFor(() => {
        expect(noticeOf(local)).toBe(NOTICES.stale);
      });
      expect(text(local)).toBe(before);
      expect(text(local)).toContain("A -->|peers| B");
      cleanupMarkViews();
    }
  });

  it("a duplicate landing during the verify does not move the item", async () => {
    const { local, peer, sync } = await room(TWO);
    openOn(local, SECOND);
    choose("Delete");
    peerInsert(peer, "    A -->|one|", "    A -->|peers| B\n");
    sync();
    await vi.waitFor(() => {
      expect(text(local)).not.toContain("two");
    });
    expect(text(local)).toBe(
      page("flowchart TD\n    A -->|peers| B\n    A -->|one| B"),
    );
  });

  it("the Delete key on a stop holds its arrow through the verify too", async () => {
    const { local, peer, sync } = await room(TWO);
    focusTarget(local, text(local).indexOf("```"), SECOND);
    press("Delete");
    peerInsert(peer, "    A -->|one|", "    A -->|peers| B\n");
    sync();
    await vi.waitFor(() => {
      expect(text(local)).not.toContain("two");
    });
    expect(text(local)).toBe(
      page("flowchart TD\n    A -->|peers| B\n    A -->|one| B"),
    );
  });

  it("Edit label opens on my arrow after a duplicate went above", async () => {
    const { local, peer, sync } = await room(TWO);
    openOn(local, SECOND);
    peerDeleteLine(peer, "    A -->|one| B");
    sync();
    await settle();
    choose("Edit label");
    const field = local.dom.querySelector<HTMLTextAreaElement>(
      ".cm-diagram-overlay textarea",
    );
    expect(field?.value).toBe("two");
  });

  it("the stop it returns to is my arrow, by its place", async () => {
    const { local, peer, sync } = await room(TWO);
    openOn(local, SECOND);
    peerInsert(peer, "    A -->|one|", "    A -->|peers| B\n");
    sync();
    await settle();
    press("Escape");
    expect(intentOf(local)?.target).toEqual({
      kind: "edge",
      key: { start: "A", end: "B", occurrence: 2 },
    });
  });

  it("an Escape right after the peer's line returns by place, too", async () => {
    const { local, peer, sync } = await room(TWO);
    openOn(local, SECOND);
    peerInsert(peer, "    A -->|one|", "    A -->|peers| B\n");
    sync();
    // No tick between: nothing has looked at the peer's line yet.
    press("Escape");
    expect(intentOf(local)?.target).toEqual({
      kind: "edge",
      key: { start: "A", end: "B", occurrence: 2 },
    });
  });

  it("the second field-held case: two edits in one sync, on the rename field", async () => {
    const { local, peer, sync } = await room("flowchart TD\n    A -->|mine| B");
    focusTarget(local, text(local).indexOf("```"), FIRST);
    press("Enter");
    const field = local.dom.querySelector<HTMLTextAreaElement>(
      ".cm-diagram-overlay textarea",
    );
    if (field === null) throw new Error("no field");
    field.value = "renamed";
    field.dispatchEvent(new Event("input", { bubbles: true }));
    peerInsert(peer, "    A -->|mine|", "    A -->|peers| B\n");
    peerDeleteLine(peer, "    A -->|mine| B");
    sync();
    await settle();
    press("Enter");
    await settle();
    expect(text(local)).toContain("A -->|peers| B");
    expect(text(local)).not.toContain("renamed");
  });
});

describe("an arrow once chosen is that arrow or nothing (R94)", () => {
  const SOLE = "flowchart TD\n    A -->|mine| B\n    C --> D";
  const AFTER = "flowchart TD\n    C --> D\n    A -->|peers| B";

  /** One peer transaction, as a merged update after a reconnect arrives. */
  function swapInOne(peer: EditorView) {
    const body = text(peer);
    const line = "    A -->|mine| B\n";
    const at = body.indexOf(line);
    const end = body.indexOf("    C --> D\n") + "    C --> D\n".length;
    peer.dispatch({
      changes: [
        { from: at, to: at + line.length },
        { from: end, insert: "    A -->|peers| B\n" },
      ],
    });
  }

  function swapInTwo(peer: EditorView) {
    peerInsert(peer, "    C --> D", "    A -->|peers| B\n");
    peerDeleteLine(peer, "    A -->|mine| B");
  }

  const ITEMS: [string, string[]][] = [
    ["Delete", ["Delete"]],
    ["Line", ["Line", "Dotted"]],
    ["Arrowhead", ["Arrowhead", "None"]],
    ["Reverse", ["Reverse"]],
  ];

  it.each(ITEMS)(
    "input A: %s, with both edits during its verify, leaves the peer's arrow",
    async (_name, path) => {
      const { local, peer, sync } = await room(SOLE);
      openOn(local, FIRST);
      choose(...path);
      swapInTwo(peer);
      sync();
      await settle();
      expect(text(local)).toContain("A -->|peers| B");
      expect(text(local)).not.toContain("|mine|");
      expect(noticeOf(local)).toBe(NOTICES.stale);
    },
  );

  it.each([
    ["two dispatches in one tick", swapInTwo],
    ["one transaction", swapInOne],
  ])("input B: the Delete key, with %s during its verify", async (_n, swap) => {
    const { local, peer, sync } = await room(SOLE);
    focusTarget(local, text(local).indexOf("```"), FIRST);
    press("Delete");
    swap(peer);
    sync();
    await settle();
    expect(text(local)).toContain("|peers|");
    expect(noticeOf(local)).toBe(NOTICES.stale);
  });

  it("input B, a microtask apart", async () => {
    const { local, peer, sync } = await room(SOLE);
    focusTarget(local, text(local).indexOf("```"), FIRST);
    press("Delete");
    peerInsert(peer, "    C --> D", "    A -->|peers| B\n");
    sync();
    await Promise.resolve();
    peerDeleteLine(peer, "    A -->|mine| B");
    sync();
    await settle();
    expect(text(local)).toContain("|peers|");
  });

  it.each(ITEMS)(
    "input C: %s after one transaction took my arrow and added a twin",
    async (_name, path) => {
      const { local, peer, sync } = await room(SOLE);
      openOn(local, FIRST);
      swapInOne(peer);
      sync();
      await settle();
      expect(text(local)).toBe(page(AFTER));
      choose(...path);
      await vi.waitFor(() => {
        expect(noticeOf(local)).toBe(NOTICES.stale);
      });
      expect(text(local)).toBe(page(AFTER));
    },
  );

  it("input C: Edit label opens no field on the peer's arrow", async () => {
    const { local, peer, sync } = await room(SOLE);
    openOn(local, FIRST);
    swapInOne(peer);
    sync();
    await settle();
    choose("Edit label");
    expect(local.dom.querySelector(".cm-diagram-overlay textarea")).toBeNull();
    expect(noticeOf(local)).toBe(NOTICES.stale);
  });

  const typeLabel = (view: EditorView, label: string) => {
    focusTarget(view, text(view).indexOf("```"), FIRST);
    press("Enter");
    const field = view.dom.querySelector<HTMLTextAreaElement>(
      ".cm-diagram-overlay textarea",
    );
    if (field === null) throw new Error("no field");
    field.value = label;
    field.dispatchEvent(new Event("input", { bubbles: true }));
    return field;
  };

  it("input D: the field, one transaction, then Enter", async () => {
    const { local, peer, sync } = await room(SOLE);
    const field = typeLabel(local, "renamed");
    swapInOne(peer);
    sync();
    await settle();
    press("Enter");
    await settle();
    expect(text(local)).toBe(page(AFTER));
    expect(noticeOf(local)).toContain(NOTICES.arrowChanged);
    expect(field.isConnected).toBe(true);
  });

  it("input D, during the field's own verify", async () => {
    const { local, peer, sync } = await room(SOLE);
    typeLabel(local, "renamed");
    press("Enter");
    swapInOne(peer);
    sync();
    await settle();
    expect(text(local)).toBe(page(AFTER));
  });

  it("a whole token rewritten by a peer is that arrow no more: the field says so", async () => {
    const { local, peer, sync } = await room(SOLE);
    typeLabel(local, "renamed");
    const at = text(peer).indexOf("-->|mine|");
    peer.dispatch({ changes: { from: at, to: at + 3, insert: "-.->" } });
    sync();
    await settle();
    press("Enter");
    await settle();
    expect(text(local)).not.toContain("renamed");
    expect(noticeOf(local)).toContain(NOTICES.arrowChanged);
  });

  it("a peer's edit that leaves the token alone keeps the arrow, label and all", async () => {
    const { local, peer, sync } = await room(SOLE);
    openOn(local, FIRST);
    const at = text(peer).indexOf("mine");
    peer.dispatch({ changes: { from: at, to: at + 4, insert: "theirs" } });
    peerInsert(peer, "    C --> D", "    %% note\n");
    sync();
    await settle();
    choose("Line", "Dotted");
    await vi.waitFor(() => {
      expect(text(local)).toContain("A -.->|theirs| B");
    });
  });

  describe("a stop, with nothing open", () => {
    const heldOnStop = async () => {
      const peers = await room(SOLE);
      focusTarget(peers.local, text(peers.local).indexOf("```"), FIRST);
      await flush();
      return peers;
    };

    it("after one transaction: the ring leaves, and Delete cannot take the peer's arrow", async () => {
      const { local, peer, sync } = await heldOnStop();
      swapInOne(peer);
      sync();
      await settle();
      expect(intentOf(local)?.target).toEqual({ kind: "node", id: "A" });
      expect(noticeOf(local)).toBe(NOTICES.stale);
      expect(text(local)).toBe(page(AFTER));
    });

    it("follows its arrow when a twin lands above it", async () => {
      const { local, peer, sync } = await room(TWO);
      focusTarget(local, text(local).indexOf("```"), SECOND);
      await flush();
      peerInsert(peer, "    A -->|one|", "    A -->|peers| B\n");
      sync();
      await settle();
      expect(intentOf(local)?.target).toEqual({
        kind: "edge",
        key: { start: "A", end: "B", occurrence: 2 },
      });
      press("Delete");
      await vi.waitFor(() => {
        expect(text(local)).not.toContain("two");
      });
      expect(text(local)).toContain("|peers|");
      expect(text(local)).toContain("|one|");
    });

    it("stays on its arrow through its own Line and its own label", async () => {
      const { local } = await heldOnStop();
      openOn(local, FIRST);
      choose("Line", "Dotted");
      await vi.waitFor(() => {
        expect(text(local)).toContain("A -.->|mine| B");
      });
      await settle();
      expect(intentOf(local)?.target).toEqual(FIRST);
      expect(noticeOf(local)).toBe("");
      press("Delete");
      await vi.waitFor(() => {
        expect(text(local)).not.toContain("|mine|");
      });
    });
  });
});

describe("connect mode with a peer", () => {
  function startOn(view: EditorView, target: TargetKey = NODE_A) {
    openOn(view, target);
    choose("Connect to...");
  }

  it("keeps the mode through a peer's line, and Enter on B appends A --> B", async () => {
    const { local, peer, sync } = await room();
    startOn(local);
    press("ArrowRight");
    expect(active()).toBe(nodeOf(local, "B"));
    peerInsert(peer, "    A --> C", "    %% peer note\n");
    sync();
    await vi.waitFor(() => {
      expect(text(local)).toContain("%% peer note");
    });
    await settle();
    expect(intentOf(local)?.mode).toBe("connect");
    expect(promptOf(local)).not.toBeNull();
    expect(active()).toBe(nodeOf(local, "B"));
    press("Enter");
    await vi.waitFor(() => {
      expect(text(local)).toContain("A --> B\n");
    });
    expect(text(local)).toContain("    %% peer note\n");
    expect(text(local).match(/A --> B/g)).toHaveLength(1);
    expect(promptOf(local)).toBeNull();
  });

  it("a redraw puts the focus on the new element, never on the body", async () => {
    const { local, peer, sync } = await room();
    startOn(local);
    const old = nodeOf(local, "A");
    peerInsert(peer, "    A --> C", "    A --> D\n");
    sync();
    await vi.waitFor(() => {
      expect(nodeOf(local, "A")).not.toBe(old);
    });
    await settle();
    expect(active()).toBe(nodeOf(local, "A"));
    expect(intentOf(local)?.mode).toBe("connect");
    expect(promptOf(local)).not.toBeNull();
  });

  it("says stale while the drawing is behind the text, and writes nothing", async () => {
    const { local, peer, sync } = await room();
    startOn(local);
    press("ArrowRight");
    peerInsert(peer, "    A --> C", "    A[[[\n");
    sync();
    await settle();
    const before = text(local);
    press("Enter");
    await vi.waitFor(() => {
      expect(noticeOf(local)).toBe(NOTICES.stale);
    });
    expect(text(local)).toBe(before);
    expect(intentOf(local)?.mode).toBe("connect");
    press("Escape");
    await flush();
    expect(promptOf(local)).toBeNull();
    expect(text(local)).toBe(before);
  });

  it("the peer deleting the box it began from ends it with the notice", async () => {
    const { local, peer, sync } = await room(
      "flowchart TD\n    A --> B\n    C --> D",
    );
    startOn(local, { kind: "node", id: "A" });
    peerDeleteLine(peer, "    A --> B");
    sync();
    await settle();
    expect(promptOf(local)).toBeNull();
    expect(noticeOf(local)).toBe(NOTICES.gone);
    expect(intentOf(local)?.mode).toBe("focus");
    expect(local.dom.contains(active())).toBe(true);
  });

  it("the peer deleting the box it stands on goes back to where it began", async () => {
    const { local, peer, sync } = await room(
      "flowchart TD\n    A --> C\n    B --> D",
    );
    startOn(local);
    press("End");
    expect(intentOf(local)?.target).toEqual({ kind: "node", id: "D" });
    peerDeleteLine(peer, "    B --> D");
    sync();
    await settle();
    expect(intentOf(local)?.mode).toBe("connect");
    expect(intentOf(local)?.target).toEqual(NODE_A);
    expect(active()).toBe(nodeOf(local, "A"));
  });

  it("a peer's box deleted during the check says gone and writes nothing", async () => {
    const { local, peer, sync } = await room(
      "flowchart TD\n    A --> C\n    B --> D",
    );
    startOn(local);
    press("End");
    press("Enter");
    peerDeleteLine(peer, "    B --> D");
    sync();
    await settle();
    expect(text(local)).not.toContain("A --> D");
    expect(promptOf(local)).toBeNull();
    expect(noticeOf(local)).toBe(NOTICES.gone);
    expect(local.dom.contains(active())).toBe(true);
  });
});
