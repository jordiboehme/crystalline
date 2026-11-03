import { EditorSelection, EditorState, StateEffect } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, it } from "vitest";

import { cleanupMarkViews, markView, undoOnce } from "../test/markKit";
import { bytes } from "../test/cellKit";
import { isOwnCopy } from "./clipboardHtml";
import { copyHtmlOf } from "./copyHtml";

afterEach(cleanupMarkViews);

const OWN = '<div data-crystalline-copy="1">';

/** The HTML of the whole of `text`, or of `[from, to)`. */
function html(text: string, from = 0, to = text.length): string {
  const view = markView(text);
  return copyHtmlOf(view.state, [EditorSelection.range(from, to)]);
}

interface Copied {
  data: Map<string, string>;
  setCalls: number;
  prevented: boolean;
}

/** A copy or cut as the browser sends it; what reached the clipboard comes back. */
function send(view: EditorView, type: "copy" | "cut"): Copied {
  const data = new Map<string, string>();
  const copied: Copied = { data, setCalls: 0, prevented: false };
  const event = new Event(type, { bubbles: true, cancelable: true });
  Object.defineProperty(event, "clipboardData", {
    configurable: true,
    value: {
      clearData: () => {
        data.clear();
      },
      setData: (kind: string, value: string) => {
        copied.setCalls += 1;
        data.set(kind, value);
      },
      getData: (kind: string) => data.get(kind) ?? "",
    },
  });
  view.contentDOM.dispatchEvent(event);
  copied.prevented = event.defaultPrevented;
  return copied;
}

function select(view: EditorView, from: number, to: number): void {
  view.dispatch({
    selection: EditorSelection.range(from, to),
    userEvent: "select.pointer",
  });
}

describe("copyHtmlOf", () => {
  it("writes headings, paragraphs, strong and links with no whitespace between blocks", () => {
    expect(html("# T\n\n**b** and [l](https://x)")).toBe(
      '<h1>T</h1><p><strong>b</strong> and <a href="https://x">l</a></p>',
    );
  });

  it("writes emphasis, strikethrough and code", () => {
    expect(html("*a* ~~b~~ `c<d`")).toBe(
      "<p><em>a</em> <del>b</del> <code>c&lt;d</code></p>",
    );
  });

  it("refuses an address that can run script", () => {
    expect(html("[l](javascript:alert(1))")).toBe("<p>l</p>");
    expect(html("[l]( JavaScript:alert(1))")).toBe("<p>l</p>");
    // Not even a link to the parser: the words stay as escaped text.
    expect(html("[l](java\tscript:alert(1))")).not.toContain("<a");
    expect(html("[l](&#106;avascript:alert(1))")).toBe("<p>l</p>");
    expect(html("[l](data:text/html,x)")).toBe("<p>l</p>");
    expect(html("[l](relative.md)")).toBe("<p>l</p>");
  });

  it("allows http, https and mailto, folded and trimmed", () => {
    expect(html("[a](HTTP://x.org)")).toBe(
      '<p><a href="HTTP://x.org">a</a></p>',
    );
    expect(html("[a](mailto:a@b.de)")).toBe(
      '<p><a href="mailto:a@b.de">a</a></p>',
    );
  });

  it("escapes quotes and angles in an address and script in link text", () => {
    expect(html("[a](https://x/\"y'z<w)")).toBe(
      '<p><a href="https://x/&quot;y&#39;z&lt;w">a</a></p>',
    );
    expect(html("[<script>x</script>](https://x)")).toBe(
      '<p><a href="https://x">&lt;script&gt;x&lt;/script&gt;</a></p>',
    );
    expect(html("[a](https://x?a=1&b=2)")).toBe(
      '<p><a href="https://x?a=1&amp;b=2">a</a></p>',
    );
  });

  it("writes raw HTML in the source as escaped text", () => {
    expect(html("<b>x</b>")).toBe("<p>&lt;b&gt;x&lt;/b&gt;</p>");
    expect(html("a <i>b</i> c")).toBe("<p>a &lt;i&gt;b&lt;/i&gt; c</p>");
    expect(html('<div onclick="x">\nraw\n</div>')).toBe(
      "<p>&lt;div onclick=&quot;x&quot;&gt;\nraw\n&lt;/div&gt;</p>",
    );
  });

  it("never writes a style or event attribute", () => {
    const out = html(
      '[a](https://x "t") ![i](https://x/i.png) <img src=x onerror=alert(1)>\n\n| a |\n|---|\n| b |',
    );
    expect(out).not.toMatch(/<img/);
    expect(out).not.toMatch(/<[a-z][^>]*\s(?:style|on\w+)=/i);
  });

  it("writes task items with their box", () => {
    expect(html("- [x] a\n- [~] b")).toBe("<ul><li>☑ a</li><li>☒ b</li></ul>");
    expect(html("- [ ] a")).toBe("<ul><li>☐ a</li></ul>");
  });

  it("writes ordered lists with their start", () => {
    expect(html("1. a\n2. b")).toBe("<ol><li>a</li><li>b</li></ol>");
    expect(html("3. a\n4. b")).toBe('<ol start="3"><li>a</li><li>b</li></ol>');
  });

  it("writes nested lists", () => {
    expect(html("- a\n  - b")).toBe("<ul><li>a<ul><li>b</li></ul></li></ul>");
  });

  it("writes a table", () => {
    expect(html("| a | b |\n|---|---|\n| 1 | \\| 2 |")).toBe(
      "<table><thead><tr><th>a</th><th>b</th></tr></thead><tbody><tr><td>1</td><td>| 2</td></tr></tbody></table>",
    );
  });

  it("writes a callout as a blockquote with the kind in strong", () => {
    expect(html("> [!NOTE]\n> body")).toBe(
      "<blockquote><p><strong>Note</strong></p><p>body</p></blockquote>",
    );
    expect(html("> plain")).toBe("<blockquote><p>plain</p></blockquote>");
  });

  it("writes a code block, a rule and math", () => {
    expect(html("```js\na < b\n\nc\n```")).toBe(
      "<pre><code>a &lt; b\n\nc</code></pre>",
    );
    expect(html("---\n\na")).toBe("<hr><p>a</p>");
    expect(html("x $$a^2$$ y")).toBe("<p>x <code>a^2</code> y</p>");
    expect(html("$$\nE=mc^2\n$$")).toBe("<pre><code>E=mc^2</code></pre>");
  });

  it("writes a status label as its words and an image as its alt text", () => {
    expect(html("s :status[In review]{blue} e")).toBe("<p>s In review e</p>");
    expect(html("![the alt](a.png)")).toBe("<p>the alt</p>");
  });

  it("writes a wikilink as its words", () => {
    expect(html("see [[Some Page|the page]] and [[Other]]")).toBe(
      "<p>see the page and Other</p>",
    );
  });

  it("writes an escaped character without its backslash", () => {
    expect(html("a \\*b\\* \\[c\\]")).toBe("<p>a *b* [c]</p>");
  });

  it("writes a footnote reference as sup with the number the page shows", () => {
    expect(html("b[^z] a[^y]\n\n[^y]: Y\n[^z]: Z")).toContain(
      "<p>b<sup>1</sup> a<sup>2</sup></p>",
    );
  });

  it("writes the label when the footnote model has not read the document yet", () => {
    const filler = "x\n".repeat(200000);
    const view = markView(`a[^1]\n\n${filler}\n[^1]: n`);
    const out = copyHtmlOf(view.state, [EditorSelection.range(0, 5)]);
    expect(out).toBe("<p>a[^1]</p>");
  });

  it("drops comments and Reflection id lines", () => {
    const out = html("## Night\n<!-- r:7f3a9c2e -->\n\nText.\n");
    expect(out).toBe("<h2>Night</h2><p>Text.</p>");
    expect(html("a <!-- x --> b\n\n<!-- c -->\n\nd")).toBe(
      "<p>a  b</p><p>d</p>",
    );
  });

  it("writes a hard break", () => {
    expect(html("a  \nb")).toBe("<p>a<br>b</p>");
  });

  it("keeps the formatting a selected slice has in the document", () => {
    expect(html("ab **cd** ef", 1, 6)).toBe("<p>b <strong>c</strong></p>");
    expect(html("[link text](https://x)", 3, 7)).toBe(
      '<p><a href="https://x">nk t</a></p>',
    );
  });

  it("copies several ranges", () => {
    const view = markView("# A\n\nb");
    expect(
      copyHtmlOf(view.state, [
        EditorSelection.range(0, 3),
        EditorSelection.range(5, 6),
      ]),
    ).toBe("<h1>A</h1><p>b</p>");
  });
});

describe("the copy and cut handlers", () => {
  it("copies Markdown and HTML, without the id line", () => {
    const view = markView("## Night\n<!-- r:7f3a9c2e -->\n\nText.\n", {
      adapter: "reflection",
    });
    view.focus();
    select(view, 0, view.state.doc.length);
    const copied = send(view, "copy");
    expect(copied.data.get("text/plain")).toBe("## Night\n\nText.\n");
    expect(copied.data.get("text/html")).toBe(
      `${OWN}<h2>Night</h2><p>Text.</p></div>`,
    );
    const joined = [...copied.data.values()].join("");
    expect(joined).not.toContain("r:");
    expect(joined).not.toContain("<!--");
    expect(copied.prevented).toBe(true);
  });

  it("copies a slice with the marks the selection holds", () => {
    const view = markView("ab **cd** ef");
    view.focus();
    select(view, 1, 6);
    const copied = send(view, "copy");
    expect(copied.data.get("text/plain")).toBe("b **c");
    expect(copied.data.get("text/html")).toBe(
      `${OWN}<p>b <strong>c</strong></p></div>`,
    );
  });

  it.each([
    { name: "solo", room: false },
    { name: "in a room", room: true },
  ])("cuts with one undo step ($name)", ({ room }) => {
    const view = markView("ab **cd** ef", { mode: "hidden", room });
    view.focus();
    select(view, 1, 6);
    const copied = send(view, "cut");
    expect(copied.data.get("text/plain")).toBe("b **c");
    expect(copied.data.get("text/html")).toBe(
      `${OWN}<p>b <strong>c</strong></p></div>`,
    );
    expect(bytes(view.state)).toBe("a**d** ef");
    undoOnce(view);
    expect(bytes(view.state)).toBe("ab **cd** ef");
  });

  it.each([
    { name: "read-only", config: EditorState.readOnly.of(true) },
    { name: "not editable", config: EditorView.editable.of(false) },
  ])(
    "copies on a $name view and refuses the deletion of a cut",
    ({ config }) => {
      const view = markView("ab **cd** ef");
      view.dispatch({ effects: StateEffect.appendConfig.of(config) });
      view.focus();
      select(view, 1, 6);
      const copied = send(view, "cut");
      expect(copied.data.get("text/plain")).toBe("b **c");
      expect(copied.data.get("text/html")).toBe(
        `${OWN}<p>b <strong>c</strong></p></div>`,
      );
      expect(bytes(view.state)).toBe("ab **cd** ef");
    },
  );

  it("leaves an empty selection to CodeMirror", () => {
    const view = markView("ab **cd** ef");
    view.focus();
    const copied = send(view, "copy");
    expect(copied.setCalls).toBe(1);
    expect(copied.data.has("text/html")).toBe(false);
    expect(copied.data.get("text/plain")).toBe("ab **cd** ef");
  });

  it("gives inside one cell of a drawn table the cell's words, not a table (F4)", () => {
    const text = "| a | b |\n|---|---|\n| one | two |\n";
    const view = markView(text);
    view.focus();
    const from = text.indexOf("one");
    select(view, from, from + 3);
    const copied = send(view, "copy");
    expect(copied.data.get("text/plain")).toBe("one");
    expect(copied.data.get("text/html")).toBe(`${OWN}one</div>`);
  });
});

/** A paste as the browser sends it, with the flavours `copied` holds. */
function pasteInto(view: EditorView, copied: Copied): void {
  const event = new Event("paste", { bubbles: true, cancelable: true });
  Object.defineProperty(event, "clipboardData", {
    value: {
      getData: (kind: string) => copied.data.get(kind) ?? "",
      types: [...copied.data.keys()],
      files: [],
      items: [],
    },
  });
  view.contentDOM.dispatchEvent(event);
}

describe("round trip inside Fluid", () => {
  const DOC =
    "See [[Other Page]] and ![diagram](att/x.png) :status[Done]{green} a[^1]\n\n> [!WARNING]\n> careful\n\n```rust\nlet x = 1;\n```\n\n[^1]: note\n";

  it("pastes a copy back byte for byte", () => {
    const from = markView(DOC);
    from.focus();
    select(from, 0, DOC.length);
    const copied = send(from, "copy");
    expect(copied.data.get("text/html")?.startsWith(OWN)).toBe(true);
    const to = markView("");
    to.focus();
    pasteInto(to, copied);
    expect(bytes(to.state)).toBe(DOC);
  });

  it("pastes a cut back byte for byte", () => {
    const from = markView(DOC);
    from.focus();
    select(from, 0, DOC.length);
    const copied = send(from, "cut");
    expect(bytes(from.state)).toBe("");
    const to = markView("");
    to.focus();
    pasteInto(to, copied);
    expect(bytes(to.state)).toBe(DOC);
  });
});

describe("what CodeMirror remembers of a line copy", () => {
  it("does not make a paste after our copy a line paste", () => {
    const view = markView("foo\nbar foo baz");
    view.focus();
    view.dispatch({ selection: { anchor: 1 } });
    const line = send(view, "copy");
    expect(line.data.get("text/plain")).toBe("foo");
    select(view, 8, 11);
    const word = send(view, "copy");
    expect(word.data.get("text/plain")).toBe("foo");
    view.dispatch({ selection: { anchor: 5 }, userEvent: "select.pointer" });
    pasteInto(view, word);
    expect(bytes(view.state)).toBe("foo\nbfooar foo baz");
  });
});

describe("copyHtmlOf, fix round 1", () => {
  it.each([
    ["    a\n    b\n\n    c", "<pre><code>a\nb\n\nc</code></pre>"],
    [
      "> ```\n> a\n> b\n> ```",
      "<blockquote><pre><code>a\nb</code></pre></blockquote>",
    ],
    [
      "- x\n\n  ```\n  a\n  b\n  ```",
      "<ul><li>x<pre><code>a\nb</code></pre></li></ul>",
    ],
  ])("keeps the lines of a code block in a container (%#)", (doc, out) => {
    expect(html(doc)).toBe(out);
  });

  it("writes reference links as links and leaves the definitions out", () => {
    expect(
      html(
        '[**a**][r] [b] [c][]\n\n[r]: https://x "t"\n[b]: <https://y>\n[c]: javascript:x',
      ),
    ).toBe(
      '<p><a href="https://x"><strong>a</strong></a> <a href="https://y">b</a> c</p>',
    );
    expect(html("[r]\n\n[r]: https://x")).toBe(
      '<p><a href="https://x">r</a></p>',
    );
    expect(html("a[^1]\n\n[^1]: note")).toBe("<p>a<sup>1</sup></p>");
  });

  it("keeps the inner formatting of bracket text with no address", () => {
    expect(html("[**b** c]")).toBe("<p>[<strong>b</strong> c]</p>");
  });

  it("reads the not-applicable box the way the reader does", () => {
    expect(html("- [~]\tb")).toBe("<ul><li>\u2612 b</li></ul>");
    expect(html("- a\n\n  [~] b")).toBe("<ul><li>a<p>[~] b</p></li></ul>");
  });

  it("decodes every named entity the page decodes", () => {
    expect(html("a&nbsp;b &copy; &amp; &bogus;")).toBe(
      "<p>a\u00a0b \u00a9 &amp; &amp;bogus;</p>",
    );
  });

  it("copies only the selected part of a display formula", () => {
    const doc = "$$\nE=mc^2\n$$";
    expect(html(doc, 4, 7)).toBe("<pre><code>=mc</code></pre>");
    expect(html(doc)).toBe("<pre><code>E=mc^2</code></pre>");
  });

  it("resolves a backslash escape in an address", () => {
    expect(html("[l](https://x/\\_y)")).toBe(
      '<p><a href="https://x/_y">l</a></p>',
    );
    expect(html("[l](\\javascript:x)")).toBe("<p>l</p>");
  });
});

describe("fix round 2", () => {
  it.each([
    ["frontmatter alone", "---\ntitle: x\n---\n"],
    ["a lone definition", "[r]: https://x\n"],
    ["a lone footnote definition", "[^1]: note\n"],
    ["a comment", "<!-- c -->\n"],
  ])("sets no HTML when the body would be empty (%s)", (_name, doc) => {
    const view = markView(doc);
    view.focus();
    select(view, 0, doc.length);
    const copied = send(view, "copy");
    expect(copied.data.get("text/plain")).toBe(doc);
    expect(copied.data.has("text/html")).toBe(false);
  });

  it("leaves out footnote definitions of any length", () => {
    expect(html("a[^1]\n\n[^1]: several words of **note**")).toBe(
      "<p>a<sup>1</sup></p>",
    );
    expect(html("a[^1]\n\n[^1]: first line\ncontinued here")).toBe(
      "<p>a<sup>1</sup></p>",
    );
  });

  it.each([
    ["the bare root", '<div data-crystalline-copy="1"><p>x</p></div>'],
    [
      "a browser wrapper",
      "<meta charset='utf-8'><html><body><!--StartFragment--><div data-crystalline-copy=\"1\"><p>x</p></div>",
    ],
    [
      "a head and a doctype",
      '<!DOCTYPE html><html><head><title>t</title></head><body><div data-crystalline-copy="1">x</div></body></html>',
    ],
    ["single quotes", "<div data-crystalline-copy='1'>x</div>"],
    [
      "extra attributes on the root",
      '<div class="a" data-crystalline-copy="1" style="x:y">x</div>',
    ],
  ])("knows its own copy: %s", (_name, markup) => {
    expect(isOwnCopy(markup)).toBe(true);
  });

  it.each([
    ["no marker", "<div><p>x</p></div>"],
    ["a marker deeper in", '<p>x</p><div data-crystalline-copy="1">y</div>'],
    ["another value", '<div data-crystalline-copy="2">x</div>'],
    ["plain text", "x"],
  ])("does not take %s for its own copy", (_name, markup) => {
    expect(isOwnCopy(markup)).toBe(false);
  });

  it("copies a read-only cut without deleting and leaves no line-wise memory", () => {
    const view = markView("foo\nbar foo baz");
    view.dispatch({
      effects: StateEffect.appendConfig.of(EditorView.editable.of(false)),
    });
    view.focus();
    view.dispatch({ selection: { anchor: 1 } });
    send(view, "copy");
    select(view, 8, 11);
    const cut = send(view, "cut");
    expect(cut.data.get("text/html")).toBe(`${OWN}<p>foo</p></div>`);
    expect(bytes(view.state)).toBe("foo\nbar foo baz");
    view.dispatch({ selection: { anchor: 5 }, userEvent: "select.pointer" });
    pasteInto(view, cut);
    expect(bytes(view.state)).toBe("foo\nbfooar foo baz");
  });
});
