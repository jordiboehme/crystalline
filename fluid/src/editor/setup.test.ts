import { EditorState } from "@codemirror/state";
import { describe, expect, it } from "vitest";

import { baseExtensions, docText, lineSeparatorFor } from "./setup";

/** Documents chosen to break reserializers: none of this may change. */
const FIXTURES = [
  "---\ntitle: Alpha\npermalink: alpha\ntags:\n  - eng\nstatus: stable\n---\n\n# Alpha\n\nBody with *emphasis*, **strong**, `code` and a [[Wiki Link]].\n",
  "---\ntitle: Edge\n---\n\n- [decision] we chose X #tag (context)\n- relates_to [[Target]]\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n```mermaid\ngraph TD; A-->B;\n```\n\nEscaped \\* star, trailing spaces  \ntabs\there, no final newline",
  "---\r\ntitle: Windows\r\n---\r\n\r\nA CRLF document.\r\n",
  "no frontmatter at all\n\n1) odd list marker\n   wrapped   oddly\n",
  // A file somebody edited on two machines: CRLF endings with one LF line
  // left in the middle, and no final newline to close it.
  "---\r\ntitle: Mixed\r\n---\r\n\r\nCRLF here.\nLF there.\r\nno final newline",
  // An LF document carrying a bare carriage return, which the default split
  // would read as a line ending and hand back as "\n".
  "carriage\rreturn inline\n\nsecond paragraph\n",
  // The degenerate ends of the range: nothing at all, and nothing but breaks.
  "",
  "\n\n\r\n",
];

/** The buffer as an editor surface builds it. */
function open(doc: string): EditorState {
  return EditorState.create({
    doc,
    extensions: [...lineSeparatorFor(doc), ...baseExtensions(false)],
  });
}

describe("the buffer is the file", () => {
  it("reads back byte-identical, whatever came in", () => {
    for (const doc of FIXTURES) {
      expect(docText(open(doc))).toBe(doc);
    }
  });

  it("adopts the document's own line ending for the text it inserts", () => {
    expect(open("---\r\ntitle: Windows\r\n---\r\n").lineBreak).toBe("\r\n");
    expect(open("# Unix\n").lineBreak).toBe("\n");
    // No ending to read is not a reason to invent CRLF.
    expect(open("single line").lineBreak).toBe("\n");
  });

  it("keeps the endings when the document is edited", () => {
    const doc = "---\r\ntitle: Windows\r\n---\r\n\r\nBody.\r\n";
    const state = open(doc);
    // Positions address the buffer, whose lines are joined internally with a
    // single character: `state.doc.length` is not `doc.length` for a CRLF
    // file, and anything computing an offset has to ask the document.
    const edited = state.update({
      changes: { from: state.doc.length, insert: state.lineBreak + "More." },
    }).state;
    expect(docText(edited)).toBe(doc + "\r\nMore.");
  });

  it("does not trust doc.toString, which is what makes this test necessary", () => {
    // The tripwire on the one mistake that would quietly undo all of the
    // above: `Text` joins with "\n" and knows nothing of the separator.
    const doc = "---\r\ntitle: Windows\r\n---\r\n";
    expect(open(doc).doc.toString()).not.toBe(doc);
  });
});
