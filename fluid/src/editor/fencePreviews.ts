/**
 * Live previews below the blocks that earn them: a mermaid fence renders its
 * diagram, a table renders a readable grid. Both are widgets appended AFTER
 * the source they preview - the syntax stays on screen and editable, which
 * is the model's known trade-off, stated in the spec.
 *
 * Mermaid arrives by dynamic import so the editor chunk does not carry the
 * renderer; the module is the same one MarkdownBody lazy-loads, so the
 * bundler serves one copy. Rendering uses mermaid's strict sanitizing mode,
 * exactly as MermaidDiagram does, and a diagram that will not parse simply
 * shows nothing - its source is right above it.
 *
 * A `StateField` rather than a `ViewPlugin`: CodeMirror refuses block
 * decorations from a plugin's dynamic source outright ("Block decorations
 * may not be specified via plugins"), since a block widget changes line
 * layout and the editor has to know about it before it decides how a line
 * even draws. The field reads the whole document's syntax tree rather than
 * `view.visibleRanges` because a state field has no view to ask; the same
 * node-offset positions come out either way.
 */

import { syntaxTree } from "@codemirror/language";
import type { EditorState, Extension, Range } from "@codemirror/state";
import { StateField } from "@codemirror/state";
import type { DecorationSet } from "@codemirror/view";
import { Decoration, EditorView, WidgetType } from "@codemirror/view";

let mermaidSequence = 0;

class MermaidPreviewWidget extends WidgetType {
  // Assigned in the body rather than declared as constructor parameters: the
  // build erases types, it does not run a TypeScript transform, and
  // parameter properties are syntax that would need one.
  readonly source: string;
  readonly dark: boolean;

  constructor(source: string, dark: boolean) {
    super();
    this.source = source;
    this.dark = dark;
  }

  override eq(other: MermaidPreviewWidget): boolean {
    return other.source === this.source && other.dark === this.dark;
  }

  override toDOM(): HTMLElement {
    const box = document.createElement("div");
    box.className = "cm-mermaid-preview";
    const { source, dark } = this;
    mermaidSequence += 1;
    const id = `cm-mermaid-${String(mermaidSequence)}`;
    void import("mermaid")
      .then(async ({ default: mermaid }) => {
        mermaid.initialize({
          startOnLoad: false,
          securityLevel: "strict",
          // The live preview renders on every keystroke, so most of what it
          // asks mermaid to draw is a half-typed diagram that fails. Without
          // this, each of those failures appends mermaid's error graphic to
          // `document.body` - outside the editor, outside CodeMirror's own
          // teardown - and they accumulate under the page for the whole
          // session. A broken diagram previews as nothing; the source is
          // right above it.
          suppressErrorRendering: true,
          theme: dark ? "dark" : "default",
        });
        const rendered = await mermaid.render(id, source);
        if (box.isConnected || box.childElementCount === 0) {
          box.innerHTML = rendered.svg;
        }
      })
      .catch(() => {
        // A broken diagram previews as nothing: the source is right above.
      });
    return box;
  }

  override ignoreEvent(): boolean {
    return true;
  }
}

/** A pipe table, parsed just far enough to draw: cells are text, never HTML. */
class TablePreviewWidget extends WidgetType {
  readonly source: string;

  constructor(source: string) {
    super();
    this.source = source;
  }

  override eq(other: TablePreviewWidget): boolean {
    return other.source === this.source;
  }

  override toDOM(): HTMLElement {
    const box = document.createElement("div");
    box.className = "cm-table-preview";
    const rows = this.source
      .split("\n")
      .map((line) => line.trim())
      .filter((line) => line.startsWith("|"))
      .map((line) =>
        line
          .replace(/^\||\|$/g, "")
          .split("|")
          .map((cell) => cell.trim()),
      );
    const table = document.createElement("table");
    rows.forEach((cells, index) => {
      if (index === 1 && cells.every((cell) => /^:?-+:?$/.test(cell))) {
        return; // the alignment row draws nothing
      }
      const row = document.createElement("tr");
      for (const cell of cells) {
        const element = document.createElement(index === 0 ? "th" : "td");
        element.textContent = cell;
        row.appendChild(element);
      }
      table.appendChild(row);
    });
    box.appendChild(table);
    return box;
  }

  override ignoreEvent(): boolean {
    return true;
  }
}

function buildPreviews(state: EditorState, dark: boolean): DecorationSet {
  const widgets: Range<Decoration>[] = [];
  const doc = state.doc;
  syntaxTree(state).iterate({
    enter: (node) => {
      if (node.name === "FencedCode") {
        const text = doc.sliceString(node.from, node.to);
        const fence = /^```(\w+)?[^\n]*\n([\s\S]*?)\n?```\s*$/.exec(text);
        if (fence && fence[1] === "mermaid") {
          widgets.push(
            Decoration.widget({
              widget: new MermaidPreviewWidget(fence[2] ?? "", dark),
              block: true,
              side: 1,
            }).range(node.to),
          );
        }
      }
      if (node.name === "Table") {
        widgets.push(
          Decoration.widget({
            widget: new TablePreviewWidget(doc.sliceString(node.from, node.to)),
            block: true,
            side: 1,
          }).range(node.to),
        );
      }
    },
  });
  return Decoration.set(widgets.sort((a, b) => a.from - b.from));
}

const previewTheme = EditorView.baseTheme({
  ".cm-mermaid-preview": { padding: "0.5rem 0.75rem" },
  ".cm-table-preview": { padding: "0.25rem 0.75rem" },
  ".cm-table-preview table": { borderCollapse: "collapse" },
  ".cm-table-preview th, .cm-table-preview td": {
    border: "1px solid var(--color-slate-300)",
    padding: "0.15rem 0.5rem",
    textAlign: "left",
  },
});

/** Mermaid and table previews. `dark` picks the mermaid theme. */
export function fencePreviews(dark: boolean): Extension {
  const field = StateField.define<DecorationSet>({
    create: (state) => buildPreviews(state, dark),
    update: (value, tr) =>
      tr.docChanged ? buildPreviews(tr.state, dark) : value,
    provide: (f) => EditorView.decorations.from(f),
  });
  return [field, previewTheme];
}
