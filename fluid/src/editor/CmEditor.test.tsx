import type { EditorView } from "@codemirror/view";
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import CmEditor from "./CmEditor";
import { baseExtensions, lineSeparatorFor } from "./setup";

describe("the editor binding", () => {
  it("mounts a view, reports edits and tears down", () => {
    let view: EditorView | null = null;
    const changed: string[] = [];
    const { unmount } = render(
      <CmEditor
        initialDoc={"hello\n"}
        extensions={baseExtensions(false)}
        ariaLabel="Engram source"
        onReady={(v) => {
          view = v;
        }}
        onDocChanged={(doc) => changed.push(doc)}
      />,
    );
    // The label names the box you actually type in, not the wrapper around
    // it: CodeMirror's content element is the one with the textbox role.
    const box = screen.getByLabelText("Engram source");
    expect(box).toHaveAttribute("contenteditable", "true");
    expect(box).toHaveAttribute("role", "textbox");
    // The narrowing here is the assertion: `view` is assigned from the
    // callback, which TypeScript cannot see across the render call.
    const ready = view as EditorView | null;
    expect(ready).not.toBeNull();
    expect(ready!.contentDOM).toBe(box);

    ready!.dispatch({ changes: { from: 0, insert: "# " } });
    expect(changed.at(-1)).toBe("# hello\n");

    unmount();
    // A destroyed view detaches itself; a leaked one would still be in a
    // document nobody is rendering.
    expect(ready!.dom.isConnected).toBe(false);
  });

  it("reports a CRLF document's edits with its own line endings", () => {
    const doc = "alpha\r\nbeta\r\n";
    const changed: string[] = [];
    let view: EditorView | null = null;
    render(
      <CmEditor
        initialDoc={doc}
        extensions={[...lineSeparatorFor(doc), ...baseExtensions(false)]}
        ariaLabel="Windows source"
        onReady={(v) => {
          view = v;
        }}
        onDocChanged={(next) => changed.push(next)}
      />,
    );
    const ready = view as EditorView | null;
    ready!.dispatch({ changes: { from: 0, insert: "# " } });
    expect(changed.at(-1)).toBe("# alpha\r\nbeta\r\n");
  });
});
