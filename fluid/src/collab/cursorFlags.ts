/**
 * Name flags on the other people's carets: each remote caret carries its
 * owner's name in their own color, always, rather than only while the pointer
 * hovers it (the binding's default). The flag is the binding's own element,
 * restyled; its background is the caret's color, and its text takes the ink
 * that reads on that color (`inkOn`), set once the binding has drawn it.
 *
 * The flag is small enough to sit in the space between two lines of text at
 * the reading line height (12px type on a 12px line), so it rests on the
 * caret's top without covering the line above.
 *
 * `Prec.high`, because the binding's base theme sets the same properties and
 * the view mounts style modules in reverse order: at default precedence this
 * would lose to the rule it overrides.
 */

import type { Extension } from "@codemirror/state";
import { Prec } from "@codemirror/state";
import type { ViewUpdate } from "@codemirror/view";
import { EditorView, ViewPlugin } from "@codemirror/view";

import { inkOn } from "./colors";

const look = Prec.high(
  EditorView.theme({
    ".cm-ySelectionCaret > .cm-ySelectionInfo": {
      opacity: "1",
      top: "-12px",
      left: "-1px",
      height: "12px",
      padding: "0 4px",
      borderRadius: "3px 3px 3px 0",
      fontFamily: "var(--font-sans, ui-sans-serif, system-ui, sans-serif)",
      fontSize: "12px",
      fontWeight: "600",
      lineHeight: "12px",
      // Until the ink below is set: near-black reads on the whole palette.
      color: "#0b0d13",
      pointerEvents: "none",
      // A layer of its own. The flag hangs above its line, outside the
      // line's box, and Safari repaints only that box when a caret moves or
      // goes: the old flag stayed on the screen, half covered by the next
      // line, until something else redrew the spot. Composited, the flag is
      // drawn and taken away as a whole.
      willChange: "transform",
      transform: "translateZ(0)",
    },
    ".cm-ySelectionCaret > .cm-ySelectionCaretDot": {
      display: "none",
    },
  }),
);

/** Sets each flag's text to the ink its own background color needs. */
const ink = ViewPlugin.fromClass(
  class {
    readonly view: EditorView;

    constructor(view: EditorView) {
      this.view = view;
      this.paint();
    }

    update(update: ViewUpdate) {
      if (update.docChanged || update.transactions.length > 0) {
        this.paint();
      }
    }

    paint() {
      // After the binding's widgets are in the DOM.
      this.view.requestMeasure({
        read: () => null,
        write: () => {
          for (const caret of this.view.dom.querySelectorAll<HTMLElement>(
            ".cm-ySelectionCaret",
          )) {
            const flag = caret.querySelector<HTMLElement>(".cm-ySelectionInfo");
            const color = caret.style.backgroundColor;
            if (flag !== null && color !== "") {
              flag.style.color = inkOn(color);
            }
          }
        },
      });
    }
  },
);

export const cursorFlags: Extension = [look, ink];
