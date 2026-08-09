import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { FindingsPanel, jumpToLine } from "./FindingsPanel";

describe("the findings panel", () => {
  it("lists findings with rule, severity and a jump for the ones with a line", async () => {
    const onJump = vi.fn();
    render(
      <FindingsPanel
        pending={false}
        onJump={onJump}
        report={{
          errors: 1,
          findings: [
            {
              rule: "E001",
              severity: "error",
              message: "frontmatter will not parse",
              line: 2,
              fix: null,
            },
            {
              rule: "T005",
              severity: "warning",
              message: "superseded without successor",
              line: null,
              fix: "add - superseded_by [[Target]]",
            },
          ],
        }}
      />,
    );
    expect(screen.getByText("E001")).toBeInTheDocument();
    expect(screen.getByText(/frontmatter will not parse/)).toBeInTheDocument();
    expect(screen.getByText(/add - superseded_by/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Go to line 2" }));
    expect(onJump).toHaveBeenCalledWith(2);
  });

  it("teaches when there is nothing to fix", () => {
    render(
      <FindingsPanel
        pending={false}
        onJump={() => undefined}
        report={{ errors: 0, findings: [] }}
      />,
    );
    expect(screen.getByText(/nothing to fix/i)).toBeInTheDocument();
  });
});

describe("jumpToLine", () => {
  it("moves the selection to the line's start", () => {
    const view = new EditorView({
      state: EditorState.create({ doc: "one\ntwo\nthree\n" }),
      parent: document.body,
    });
    jumpToLine(view, 2);
    expect(view.state.selection.main.head).toBe(4);
    view.destroy();
  });
});
