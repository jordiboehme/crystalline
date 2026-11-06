/**
 * A render with real mermaid, in a place where its drawing cannot finish: jsdom
 * has no text measurement, so its `draw` throws, and a body that does not parse
 * throws earlier. Neither leaves a temporary element in the document. The verdict
 * is stubbed, so the fence is drawn whatever mermaid then says.
 */

import { afterEach, describe, expect, it, vi } from "vitest";

import { cleanupMarkViews, markView } from "../../test/markKit";
import { prefetchDiagrams } from "./loader";
import { forgetDrawings, hostOf } from "./widget";

vi.mock("./verdict", () => ({
  verdictFor: () => ({ ok: true, kind: "flowchart", domIds: new Map() }),
  requestVerdict: () => new Promise(() => {}),
}));

afterEach(() => {
  cleanupMarkViews();
  forgetDrawings();
});

const leftovers = () =>
  document.querySelectorAll('[id^="dcm-diagram-"], [id^="cm-diagram-"]');

describe("a real render that fails", () => {
  it.each([
    ["in the drawing", "flowchart TD\n    A --> B"],
    ["in the parse", "flowchart TD\n    A -->"],
  ])(
    "leaves no temporary element when it stops %s, and the box stays",
    { timeout: 60_000 },
    async (_where, body) => {
      const view = markView(`Top\n\n\`\`\`mermaid\n${body}\n\`\`\`\n\nEnd`);
      await prefetchDiagrams();
      await vi.waitFor(() => {
        expect(view.dom.querySelector(".cm-diagram")).not.toBeNull();
      });
      // Mermaid made its temporary element, and it is gone again.
      await new Promise((resolve) => setTimeout(resolve, 1_500));
      expect(leftovers()).toHaveLength(0);
      expect(document.querySelectorAll(".cm-diagram svg")).toHaveLength(0);
      expect(hostOf(view, 5)?.renderedBody()).toBe("");
      expect(view.dom.querySelector(".cm-diagram")).not.toBeNull();
    },
  );
});
