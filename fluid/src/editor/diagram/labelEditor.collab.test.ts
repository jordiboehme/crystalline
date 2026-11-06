/**
 * The rename field with a second person in the room: what they write reaches the
 * field's fence while it is open, and a composition must not notice.
 * `labelEditor.collab.marks-hidden.test.ts` and
 * `labelEditor.collab.marks-show.test.ts` run this file again.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { EditorView } from "@codemirror/view";

import { cellMarksMode } from "../../test/cellMarksMode";
import {
  bytes,
  cleanupMarkViews,
  markPeers,
  markView,
  roomDelete,
  undoOnce,
} from "../../test/markKit";
import { NOTICES } from "./edits";
import { parseFlowchart } from "./flowchart/parse";
import { diagramIntent, intentStale } from "./intent";
import { prefetchDiagrams } from "./loader";
import { requestVerdict } from "./verdict";
import { forgetDrawings, hostOf } from "./widget";

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

const BODY =
  "flowchart TD\n    A[First step] --> B[Next step]\n    A --> C[Third]";
const DOC = `Above\n\n\`\`\`mermaid\n${BODY}\n\`\`\`\n\nBelow\n`;
const FENCE_AT = DOC.indexOf("```");
const LINE_B = DOC.indexOf("    A[First");

const flush = async () => {
  await new Promise((resolve) => setTimeout(resolve, 0));
};

const modeOptions = () => {
  const mode = cellMarksMode();
  return mode === "off" ? {} : { mode };
};

async function room() {
  const peers = markPeers(DOC, modeOptions());
  await prefetchDiagrams();
  await vi.waitFor(() => {
    expect(
      peers.local.dom.querySelector(".cm-diagram svg g.node"),
    ).not.toBeNull();
  });
  await flush();
  return peers;
}

const nodeOf = (view: EditorView, id: string) => {
  const found = view.dom.querySelector(`g.node[id*="flowchart-${id}-"]`);
  if (found === null) throw new Error(`no box ${id}`);
  return found;
};

function fieldOf(view: EditorView): HTMLTextAreaElement {
  const found = view.dom.querySelector<HTMLTextAreaElement>(
    ".cm-diagram-overlay textarea",
  );
  if (found === null) throw new Error("no field");
  return found;
}

const noField = (view: EditorView) =>
  view.dom.querySelector(".cm-diagram-overlay textarea") === null;

function type(field: HTMLTextAreaElement, value: string): void {
  field.value = value;
  field.dispatchEvent(new Event("input", { bubbles: true }));
}

function press(el: Element | null, key: string): void {
  el?.dispatchEvent(
    new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }),
  );
}

function openOn(view: EditorView, id: string, draft: string) {
  nodeOf(view, id).dispatchEvent(new MouseEvent("click", { bubbles: true }));
  const field = fieldOf(view);
  type(field, draft);
  return field;
}

function insertAt(peer: EditorView, at: number, insert: string) {
  peer.dispatch({ changes: { from: at, insert } });
}

const notice = (view: EditorView) =>
  view.dom.querySelector(".cm-diagram-notice")?.textContent;

const peerRenamesAgain = (peer: EditorView, was: string, label: string) => {
  const at = bytes(peer.state).indexOf(was);
  peer.dispatch({ changes: { from: at, to: at + was.length, insert: label } });
};

const peerRenamesB = (peer: EditorView, label: string) => {
  const at = bytes(peer.state).indexOf("Next step");
  peer.dispatch({ changes: { from: at, to: at + 9, insert: label } });
};

beforeEach(() => {
  forgetDrawings();
});

afterEach(() => {
  cleanupMarkViews();
  document.body.replaceChildren();
});

describe("the field with a peer in the room", () => {
  it("stays the same element through a peer's line in the body, and Enter writes the draft", async () => {
    const { local, peer, sync } = await room();
    const field = openOn(local, "B", "Don");
    insertAt(peer, LINE_B, "    %% peer note\n");
    sync();
    await vi.waitFor(() => {
      expect(bytes(local.state)).toContain("%% peer note");
    });
    await flush();
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(fieldOf(local)).toBe(field);
    expect(field.value).toBe("Don");
    expect(document.activeElement).toBe(field);
    press(field, "Enter");
    await vi.waitFor(() => {
      expect(noField(local)).toBe(true);
    });
    const expected = DOC.replace("Next step", "Don").replace(
      "    A[First",
      "    %% peer note\n    A[First",
    );
    expect(bytes(local.state)).toBe(expected);
    sync();
    expect(bytes(peer.state)).toBe(expected);
  });

  it("does not cut a composition with a peer's edit or a new drawing", async () => {
    const { local, peer, sync } = await room();
    const field = openOn(local, "B", "");
    field.dispatchEvent(new CompositionEvent("compositionstart"));
    type(field, "ㄱ");
    insertAt(peer, LINE_B, "    %% peer note\n");
    sync();
    await flush();
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(fieldOf(local)).toBe(field);
    expect(field.isConnected).toBe(true);
    expect(document.activeElement).toBe(field);
    expect(field.value).toBe("ㄱ");
  });

  it("keeps the field through a peer's line at the fence's first mark", async () => {
    const { local, peer, sync } = await room();
    const field = openOn(local, "B", "Don");
    field.setSelectionRange(1, 2);
    insertAt(peer, FENCE_AT, "peer line\n\n");
    sync();
    await vi.waitFor(() => {
      expect(bytes(local.state)).toContain("peer line");
    });
    await flush();
    await new Promise((resolve) => setTimeout(resolve, 20));
    const now = fieldOf(local);
    expect(now.value).toBe("Don");
    expect(document.activeElement).toBe(now);
    expect([now.selectionStart, now.selectionEnd]).toEqual([1, 2]);
    expect(local.state.field(diagramIntent)?.mode).toBe("rename");
    const anchor = local.state.field(diagramIntent)?.anchor ?? -1;
    expect(hostOf(local, anchor)?.container).toBe(
      local.dom.querySelector(".cm-diagram"),
    );
    press(now, "Enter");
    await vi.waitFor(() => {
      expect(noField(local)).toBe(true);
    });
    await vi.waitFor(() => {
      expect(document.activeElement?.getAttribute("aria-label")).toBe(
        "Box: Don",
      );
    });
    expect(bytes(local.state)).toBe(
      DOC.replace("Next step", "Don").replace(
        "```mermaid",
        "peer line\n\n```mermaid",
      ),
    );
  });

  it("closes with the gone notice and no change when a peer deletes the box", async () => {
    const { local, peer, sync } = await room();
    openOn(local, "B", "Don");
    const start = bytes(peer.state).indexOf(" --> B[Next step]");
    peer.dispatch({
      changes: { from: start, to: start + " --> B[Next step]".length },
    });
    sync();
    await vi.waitFor(() => {
      expect(noField(local)).toBe(true);
    });
    expect(notice(local)).toBe(NOTICES.gone);
    const expected = DOC.replace(" --> B[Next step]", "");
    expect(bytes(local.state)).toBe(expected);
    expect(local.state.field(diagramIntent)?.mode).toBe("focus");
  });

  it("does not call a box gone while the parse is behind, and the answer comes from the text", async () => {
    const view = markView(DOC, { ...modeOptions(), room: true });
    await prefetchDiagrams();
    await vi.waitFor(() => {
      expect(view.dom.querySelector(".cm-diagram svg g.node")).not.toBeNull();
    });
    await flush();
    const field = openOn(view, "B", "Don");
    let now = Date.now();
    const clock = vi.spyOn(Date, "now").mockImplementation(() => {
      now += 1_000;
      return now;
    });
    try {
      const text = bytes(view.state);
      const at = text.indexOf(" --> B[Next step]");
      roomDelete(view, at, " --> B[Next step]".length);
    } finally {
      clock.mockRestore();
    }
    expect(intentStale(view.state)).toBe(true);
    await flush();
    expect(fieldOf(view)).toBe(field);
    expect(field.value).toBe("Don");
    press(field, "Enter");
    await vi.waitFor(() => {
      expect(noField(view)).toBe(true);
    });
    expect(notice(view)).toBe(NOTICES.gone);
    expect(bytes(view.state)).not.toContain("Don");
  });

  it("writes a typed old name back over a peer's rename (M1)", async () => {
    const { local, peer, sync } = await room();
    const field = openOn(local, "B", "Zed");
    type(field, "Next step");
    peerRenamesB(peer, "Peer name");
    sync();
    await flush();
    press(field, "Enter");
    await vi.waitFor(() => {
      expect(noField(local)).toBe(true);
    });
    expect(bytes(local.state)).toContain("B[Next step]");
    expect(bytes(local.state)).not.toContain("Peer name");
  });

  it("counts the old name typed back as a draft after a peer's rename, so the page asks (N9)", async () => {
    const { local, peer, sync } = await room();
    const field = openOn(local, "B", "Zed");
    type(field, "Next step");
    peerRenamesB(peer, "Peer name");
    sync();
    await flush();
    const event = new Event("beforeunload", { cancelable: true });
    window.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
  });

  it("follows the fence through a line above it, then a broken line stays held", async () => {
    const { local, peer, sync } = await room();
    const field = openOn(local, "B", "Don");
    insertAt(peer, 0, "New top\n\n");
    sync();
    await flush();
    const at = bytes(peer.state).indexOf("    A[First");
    insertAt(peer, at, "    C -->\n");
    sync();
    await flush();
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(fieldOf(local)).toBe(field);
    expect(local.dom.querySelector(".cm-diagram")).not.toBeNull();
  });

  it("keeps the selection on all of a peer's new name, and not on part of it", async () => {
    const { local, peer, sync } = await room();
    nodeOf(local, "B").dispatchEvent(
      new MouseEvent("click", { bubbles: true }),
    );
    const field = fieldOf(local);
    peerRenamesB(peer, "Peer name");
    sync();
    await vi.waitFor(() => {
      expect(field.value).toBe("Peer name");
    });
    expect([field.selectionStart, field.selectionEnd]).toEqual([0, 9]);
    field.setSelectionRange(1, 1);
    peerRenamesAgain(peer, "Peer name", "Other");
    sync();
    await vi.waitFor(() => {
      expect(field.value).toBe("Other");
    });
    expect(field.selectionStart).not.toBe(0);
  });

  it("puts the focus on the canvas when the box goes under it, not anywhere else", async () => {
    const { local, peer, sync } = await room();
    openOn(local, "B", "Don");
    const other = document.createElement("input");
    document.body.append(other);
    const start = bytes(peer.state).indexOf(" --> B[Next step]");
    peer.dispatch({
      changes: { from: start, to: start + " --> B[Next step]".length },
    });
    sync();
    await vi.waitFor(() => {
      expect(noField(local)).toBe(true);
    });
    expect(document.activeElement).toBe(local.dom.querySelector(".cm-diagram"));
  });

  it("leaves the focus where it went when the box goes after a blur's refusal", async () => {
    const { local, peer, sync } = await room();
    const field = openOn(local, "B", "a %%{ b");
    const other = document.createElement("input");
    document.body.append(other);
    other.focus();
    await vi.waitFor(() => {
      expect(notice(local)).toContain(NOTICES.invalid);
    });
    expect(fieldOf(local)).toBe(field);
    const start = bytes(peer.state).indexOf(" --> B[Next step]");
    peer.dispatch({
      changes: { from: start, to: start + " --> B[Next step]".length },
    });
    sync();
    await vi.waitFor(() => {
      expect(noField(local)).toBe(true);
    });
    expect(document.activeElement).toBe(other);
  });

  it("draws the new body when one edit takes in the first mark and the body", async () => {
    const { local, peer, sync } = await room();
    const field = openOn(local, "B", "Don");
    const box = local.dom.querySelector(".cm-diagram");
    peer.dispatch({
      changes: [
        { from: FENCE_AT, insert: "peer line\n\n" },
        { from: LINE_B, insert: "    D[Fourth]\n" },
      ],
    });
    sync();
    await vi.waitFor(() => {
      expect(
        local.dom.querySelector('g.node[id*="flowchart-D-"]'),
      ).not.toBeNull();
    });
    expect(local.dom.querySelector(".cm-diagram")).toBe(box);
    expect(fieldOf(local)).toBe(field);
    expect(document.activeElement).toBe(field);
  });

  it("waits with the close until a composition has ended", async () => {
    const { local, peer, sync } = await room();
    const field = openOn(local, "B", "");
    field.dispatchEvent(new CompositionEvent("compositionstart"));
    type(field, "ㄱ");
    const start = bytes(peer.state).indexOf(" --> B[Next step]");
    peer.dispatch({
      changes: { from: start, to: start + " --> B[Next step]".length },
    });
    sync();
    await flush();
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(fieldOf(local)).toBe(field);
    expect(field.value).toBe("ㄱ");
    field.dispatchEvent(new CompositionEvent("compositionend"));
    await vi.waitFor(() => {
      expect(noField(local)).toBe(true);
    });
    expect(notice(local)).toBe(NOTICES.gone);
  });

  it("says gone in the panel when a peer deletes the whole fence", async () => {
    const { local, peer, sync } = await room();
    openOn(local, "B", "Don");
    const at = bytes(peer.state).indexOf("```");
    peer.dispatch({
      changes: { from: at, to: bytes(peer.state).lastIndexOf("```") + 3 },
    });
    sync();
    await vi.waitFor(() => {
      expect(noField(local)).toBe(true);
    });
    await vi.waitFor(() => {
      expect(local.dom.querySelector(".cm-diagram-refusal")?.textContent).toBe(
        NOTICES.gone,
      );
    });
    expect(bytes(local.state)).toBe("Above\n\n\n\nBelow\n");
  });

  it("writes over a peer's rename only when the person typed, and shows it when they did not", async () => {
    const { local, peer, sync } = await room();
    nodeOf(local, "B").dispatchEvent(
      new MouseEvent("click", { bubbles: true }),
    );
    const field = fieldOf(local);
    expect(field.value).toBe("Next step");
    peerRenamesB(peer, "Peer name");
    sync();
    await vi.waitFor(() => {
      expect(field.value).toBe("Peer name");
    });
    press(field, "Enter");
    await vi.waitFor(() => {
      expect(noField(local)).toBe(true);
    });
    expect(bytes(local.state)).toContain("B[Peer name]");
  });

  it("keeps a typed draft over a peer's rename of the same box, last writer wins", async () => {
    const { local, peer, sync } = await room();
    const field = openOn(local, "B", "Mine");
    peerRenamesB(peer, "Peer name");
    sync();
    await flush();
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(field.value).toBe("Mine");
    press(field, "Enter");
    await vi.waitFor(() => {
      expect(noField(local)).toBe(true);
    });
    sync();
    expect(bytes(local.state)).toContain("B[Mine]");
    expect(bytes(peer.state)).toContain("B[Mine]");
    undoOnce(local);
    expect(bytes(local.state)).toContain("B[Peer name]");
  });

  it("stays open through a peer's half-typed line that mermaid refuses, and writes once it reads again", async () => {
    const { local, peer, sync } = await room();
    const field = openOn(local, "B", "Don");
    insertAt(peer, LINE_B, "    C -->\n");
    sync();
    await flush();
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(fieldOf(local)).toBe(field);
    expect(field.value).toBe("Don");
    expect(document.activeElement).toBe(field);
    expect(local.dom.querySelector(".cm-diagram")).not.toBeNull();
    press(field, "Enter");
    await vi.waitFor(() => {
      expect(notice(local)).toContain(NOTICES.stale);
    });
    expect(fieldOf(local)).toBe(field);
    expect(bytes(local.state)).not.toContain("Don");
    const end = bytes(peer.state).indexOf("    C -->\n") + 9;
    insertAt(peer, end, " B");
    sync();
    await vi.waitFor(() => {
      expect(bytes(local.state)).toContain("C --> B");
    });
    await flush();
    await new Promise((resolve) => setTimeout(resolve, 50));
    press(field, "Enter");
    await vi.waitFor(() => {
      expect(noField(local)).toBe(true);
    });
    expect(bytes(local.state)).toContain("B[Don]");
  });

  it("gives a refused fence back to text once the field has closed", async () => {
    const { local, peer, sync } = await room();
    const field = openOn(local, "B", "Don");
    insertAt(peer, LINE_B, "    C -->\n");
    sync();
    await flush();
    await new Promise((resolve) => setTimeout(resolve, 50));
    press(field, "Escape");
    await vi.waitFor(() => {
      expect(local.dom.querySelector(".cm-diagram")).toBeNull();
    });
    expect(noField(local)).toBe(true);
  });
});

describe("an arrow held through a peer's duplicates (C1, R94)", () => {
  const body = (...lines: string[]) =>
    `Top\n\n\`\`\`mermaid\nflowchart TD\n${lines.map((l) => `    ${l}`).join("\n")}\n\`\`\`\n\nEnd\n`;

  async function roomOf(...lines: string[]) {
    const doc = body(...lines);
    const peers = markPeers(doc, modeOptions());
    await prefetchDiagrams();
    await vi.waitFor(() => {
      expect(
        peers.local.dom.querySelectorAll('.cm-diagram svg path[data-et="edge"]')
          .length,
      ).toBeGreaterThan(0);
    });
    await flush();
    return { ...peers, doc };
  }

  function openArrow(view: EditorView, at: number) {
    view.dom
      .querySelectorAll('path[data-et="edge"]')
      [at]?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    return fieldOf(view);
  }

  const atLine = (peer: EditorView, line: string) =>
    bytes(peer.state).indexOf(line);
  const delLine = (peer: EditorView, line: string) => {
    const at = atLine(peer, line);
    peer.dispatch({ changes: { from: at, to: at + line.length } });
  };

  async function enter(view: EditorView, field: HTMLTextAreaElement) {
    press(field, "Enter");
    await vi.waitFor(() => {
      expect(noField(view)).toBe(true);
    });
  }

  it("a duplicate added above: the label lands on the person's arrow", async () => {
    const { local, peer, sync } = await roomOf("A -->|mine| B");
    const field = openArrow(local, 0);
    type(field, "renamed");
    insertAt(peer, atLine(peer, "    A -->|mine|"), "    A -->|peers| B\n");
    sync();
    await flush();
    await enter(local, field);
    expect(bytes(local.state)).toBe(body("A -->|peers| B", "A -->|renamed| B"));
  });

  it("the same with the peer's line landing during the verify", async () => {
    const { local, peer, sync } = await roomOf("A -->|mine| B");
    const field = openArrow(local, 0);
    type(field, "renamed");
    press(field, "Enter");
    insertAt(peer, atLine(peer, "    A -->|mine|"), "    A -->|peers| B\n");
    sync();
    await vi.waitFor(() => {
      expect(noField(local)).toBe(true);
    });
    expect(bytes(local.state)).toBe(body("A -->|peers| B", "A -->|renamed| B"));
  });

  it("an earlier duplicate deleted: the third is not overwritten", async () => {
    const { local, peer, sync } = await roomOf(
      "A -->|one| B",
      "A -->|two| B",
      "A -->|three| B",
    );
    const field = openArrow(local, 1);
    expect(field.value).toBe("two");
    type(field, "renamed");
    delLine(peer, "    A -->|one| B\n");
    sync();
    await flush();
    await enter(local, field);
    expect(bytes(local.state)).toBe(body("A -->|renamed| B", "A -->|three| B"));
  });

  it("an earlier duplicate deleted: no false gone, the draft stays", async () => {
    const { local, peer, sync } = await roomOf("A -->|one| B", "A -->|two| B");
    const field = openArrow(local, 1);
    type(field, "renamed");
    delLine(peer, "    A -->|one| B\n");
    sync();
    await flush();
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(fieldOf(local)).toBe(field);
    expect(field.value).toBe("renamed");
    await enter(local, field);
    expect(bytes(local.state)).toBe(body("A -->|renamed| B"));
  });

  it("a field nobody typed in keeps showing its own arrow's label", async () => {
    const { local, peer, sync } = await roomOf("A -->|mine| B");
    const field = openArrow(local, 0);
    insertAt(peer, atLine(peer, "    A -->|mine|"), "    A -->|peers| B\n");
    sync();
    await flush();
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(field.value).toBe("mine");
  });

  it("a peer's new label on that very arrow shows in a field nobody typed in", async () => {
    const { local, peer, sync } = await roomOf("A -->|mine| B");
    const field = openArrow(local, 0);
    const at = atLine(peer, "mine");
    peer.dispatch({ changes: { from: at, to: at + 4, insert: "theirs" } });
    sync();
    await vi.waitFor(() => {
      expect(field.value).toBe("theirs");
    });
  });

  it("answers stale, with no bytes, when its arrow was rewritten among duplicates", async () => {
    const { local, peer, sync } = await roomOf("A -->|one| B", "A -->|two| B");
    const field = openArrow(local, 1);
    type(field, "renamed");
    const line = "    A -->|two| B";
    const at = atLine(peer, line);
    peer.dispatch({
      changes: { from: at, to: at + line.length, insert: "    A -.->|re| B" },
    });
    sync();
    await flush();
    const before = bytes(local.state);
    press(field, "Enter");
    await vi.waitFor(() => {
      expect(notice(local)).toContain(NOTICES.arrowChanged);
    });
    expect(notice(local)).not.toContain("Fix it");
    expect(bytes(local.state)).toBe(before);
    expect(fieldOf(local)).toBe(field);
  });

  it.each([
    ["two", 1, "A -->|one| B"],
    ["one", 0, "A -->|two| B"],
  ])(
    "never writes over the other arrow when a peer deletes %s, its own",
    async (mine, at, left) => {
      const { local, peer, sync } = await roomOf(
        "A -->|one| B",
        "A -->|two| B",
      );
      const field = openArrow(local, at);
      expect(field.value).toBe(mine);
      type(field, "renamed");
      delLine(
        peer,
        `    A -->|${mine}| B\n`.replace(/\n$/, "") + (at === 1 ? "" : "\n"),
      );
      sync();
      await flush();
      press(field, "Enter");
      await vi.waitFor(() => {
        expect(notice(local)).toContain(NOTICES.arrowChanged);
      });
      expect(bytes(local.state)).toContain(left);
      expect(bytes(local.state)).not.toContain("renamed");
      expect(fieldOf(local)).toBe(field);
    },
  );

  it("the same with the delete landing during the verify", async () => {
    const { local, peer, sync } = await roomOf("A -->|one| B", "A -->|two| B");
    const field = openArrow(local, 1);
    type(field, "renamed");
    press(field, "Enter");
    delLine(peer, "\n    A -->|two| B");
    sync();
    await vi.waitFor(() => {
      expect(notice(local)).toContain(NOTICES.arrowChanged);
    });
    expect(bytes(local.state)).toContain("A -->|one| B");
    expect(bytes(local.state)).not.toContain("renamed");
  });

  it("takes the one arrow left of that pair when its own was rewritten", async () => {
    const { local, peer, sync } = await roomOf("A -->|one| B");
    const field = openArrow(local, 0);
    type(field, "renamed");
    const line = "    A -->|one| B";
    const at = atLine(peer, line);
    peer.dispatch({
      changes: { from: at, to: at + line.length, insert: "    A -.->|re| B" },
    });
    sync();
    await flush();
    await enter(local, field);
    expect(bytes(local.state)).toContain("A -.->|renamed| B");
  });

  it("closes as gone when a peer deletes the arrow, with its boxes left", async () => {
    const { local, peer, sync } = await roomOf("A --> B", "A --> C");
    openArrow(local, 0);
    type(fieldOf(local), "renamed");
    delLine(peer, "    A --> B\n");
    sync();
    await vi.waitFor(() => {
      expect(noField(local)).toBe(true);
    });
    expect(notice(local)).toBe(NOTICES.gone);
    expect(bytes(local.state)).not.toContain("renamed");
  });
});
