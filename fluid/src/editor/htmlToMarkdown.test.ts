import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { describe, expect, it } from "vitest";

import { cleanClipboardHtml } from "./clipboardHtml";
import { htmlToMarkdown, prefersPlainText } from "./htmlToMarkdown";

/**
 * The clipboard samples in `fixtures/paste/`, hand-built in the shape those
 * apps put on the clipboard. A real capture replaces a file as it is: every
 * `.html` there is read by its name, with no change here.
 */
const FIXTURES = import.meta.glob<string>("./fixtures/paste/*.html", {
  query: "?raw",
  import: "default",
  eager: true,
});

function readFixture(name: string): string {
  const html = FIXTURES[`./fixtures/paste/${name}`];
  if (html === undefined) {
    throw new Error(`no paste fixture ${name}`);
  }
  return html;
}

describe("htmlToMarkdown", () => {
  it("turns wiki panels, expands and lozenges into the dialect", () => {
    const md = htmlToMarkdown(readFixture("wiki-panels.html"));
    expect(md).toContain("> [!NOTE]\n> Deploys run at night.");
    expect(md).toContain(
      "> [!WARNING] Data loss\n> Never run the migration twice.",
    );
    expect(md).toContain(
      "> [!DETAILS]- Rollback steps\n> 1. Stop the service.\n> 2. Restore the backup.",
    );
    expect(md).toContain("> [!NOTE] Contacts\n> Ask the platform team.");
    expect(md).toContain(":status[In progress]{blue}");
    expect(md).toContain(":status[Done]{green}");
    expect(md).toMatch(/\| Name \| State \|\n\| --- \| --- \|/);
    expect(md).toContain("| Search | :status[Blocked]{red} |");
    expect(md).toContain("- [x] Write the notes\n- [ ] Tag the release");
    expect(md).toContain("[Runbook](https://example.org/runbook)");
    expect(md).toContain("[Diagram](/download/attachments/1/diagram.png)");
    expect(md).not.toMatch(/<[a-z]/i);
  });

  it("reads a docs editor's styled spans and not its bold wrapper", () => {
    const md = htmlToMarkdown(readFixture("docs-editor.html"));
    expect(md).toContain("# Meeting notes");
    expect(md).toContain("We agreed on **two** *small*~~big~~ steps.");
    expect(md).toContain(
      "- Ship the fix\n- [Read the plan](https://example.org/plan)",
    );
    expect(md).toContain("- [x] Send the invite\n- [ ] Book the room");
    // A data URL is no link: the picture's alt text stays.
    expect(md).toContain("Whiteboard photo");
    expect(md).not.toContain("data:");
    expect(md).not.toMatch(/^\*\*/);
  });

  it("turns a word processor's list paragraphs into lists", () => {
    const md = htmlToMarkdown(readFixture("word-processor.html"));
    expect(md).toContain("## Risks");
    expect(md).toContain(
      "The vendor contract ends in **March** and the *new* one is not signed.",
    );
    expect(md).toContain(
      "- Renew the licence\n  - Ask legal first\n- Plan the move",
    );
    expect(md).toContain("1. Call the vendor\n2. Sign the offer");
    expect(md).not.toContain("·");
    expect(md).toContain("| **Risk** | **Owner** |\n| --- | --- |");
    // A pipe in a cell is escaped once; an underscore inside a word is not.
    expect(md).toContain("| a\\|b and snake_case | Ana |");
    expect(md).not.toContain("p.MsoNormal");
  });

  it("never lets markup through", () => {
    const md = htmlToMarkdown(
      '<p onclick="x()">Hi<script>alert(1)</script><style>p{}</style></p>',
    );
    expect(md.trim()).toBe("Hi");
  });

  it.each([
    ["iframe", "<iframe>framed</iframe>"],
    ["object", "<object>embedded</object>"],
    ["noscript", "<noscript><p>fallback</p></noscript>"],
    ["template", "<template><p>later</p></template>"],
  ])("drops a %s with its text", (_name, html) => {
    expect(htmlToMarkdown(`<p>Keep</p>${html}`).trim()).toBe("Keep");
  });

  it("removes active elements and every event attribute before converting", () => {
    const doc = cleanClipboardHtml(
      '<link rel="stylesheet" href="x.css"><meta http-equiv="refresh" content="0">' +
        '<p onclick="x()" onmouseover="y()" title="t">Hi</p>' +
        '<embed src="x.swf"><img src="a.png" onerror="z()" alt="a">' +
        "<script>1</script><style>p{}</style><iframe></iframe><object></object>",
    );
    for (const tag of [
      "link",
      "meta",
      "embed",
      "script",
      "style",
      "iframe",
      "object",
    ]) {
      expect(doc.querySelector(tag), tag).toBeNull();
    }
    const attributes = Array.from(doc.querySelectorAll("*")).flatMap((el) =>
      Array.from(el.attributes, (a) => a.name),
    );
    expect(attributes.filter((name) => name.startsWith("on"))).toEqual([]);
    expect(doc.querySelector("p")?.getAttribute("title")).toBe("t");
  });

  it.each([
    "javascript:alert(1)",
    " JaVa\tScRiPt:alert(1)",
    "data:text/html,<b>x</b>",
    "vbscript:x",
    "file:///etc/passwd",
    "&amp;#106;avascript:alert(1)",
    "javascript&amp;colon;alert(1)",
    "java&amp;Tab;script:alert(1)",
  ])("keeps the text of a link to %j and drops the link", (href) => {
    const md = htmlToMarkdown(`<p><a href="${href}">Click</a></p>`);
    expect(md.trim()).toBe("Click");
  });

  it.each([
    ["https://x.y/a", "[a](https://x.y/a)"],
    ["http://x.y/", "[a](http://x.y/)"],
    ["mailto:a@x.y", "[a](mailto:a@x.y)"],
    ["/docs/page", "[a](/docs/page)"],
    ["../page#part", "[a](../page#part)"],
    ["#part", "[a](#part)"],
  ])("keeps a link to %j", (href, expected) => {
    expect(htmlToMarkdown(`<p><a href="${href}">a</a></p>`).trim()).toBe(
      expected,
    );
  });

  it("drops an image whose source is a script, however it is encoded", () => {
    expect(
      htmlToMarkdown(
        '<p><img src="&amp;#106;avascript:alert(1)" alt="i1"></p>',
      ).trim(),
    ).toBe("i1");
  });

  it.each([
    ["https://x.y/?a=1&amp;b=2", "https://x.y/?a=1&b=2"],
    ["https://x.y/?a=1&amp;copy=2", "https://x.y/?a=1&copy=2"],
    ["https://x.y/?x=&amp;#106;", "https://x.y/?x=\\&#106;"],
    ["https://x.y/?x=&amp;amp;", "https://x.y/?x=\\&amp;"],
    ["https://x.y/a\\b", "https://x.y/a\\\\b"],
  ])(
    "writes the link %j so its Markdown destination reads back as the DOM href",
    (href, destination) => {
      expect(htmlToMarkdown(`<p><a href="${href}">a</a></p>`).trim()).toBe(
        `[a](${destination})`,
      );
    },
  );

  it.each([
    ["&lt;script&gt;alert(1)&lt;/script&gt;", "\\<script>alert(1)\\</script>"],
    [
      '&lt;a href="javascript:alert(1)"&gt;x&lt;/a&gt;',
      '\\<a href="javascript:alert(1)">x\\</a>',
    ],
    ["&lt;!-- r:deadbeef --&gt;", "\\<!-- r:deadbeef -->"],
    ["&lt;?php", "\\<?php"],
    ["a &lt; b and 1&lt;2", "a < b and 1<2"],
    ["snake_case and __init__", "snake_case and \\_\\_init\\_\\_"],
    ["_word_", "\\_word\\_"],
    ["a &amp;copy; b &amp; c", "a \\&copy; b & c"],
    ["~~gone~~ and $$x$$", "\\~\\~gone\\~\\~ and \\$\\$x\\$\\$"],
    ["*a* `b` [c]", "\\*a\\* \\`b\\` \\[c\\]"],
    ["# not a heading", "\\# not a heading"],
  ])("writes the visible text %j as text", (html, expected) => {
    expect(htmlToMarkdown(`<p>${html}</p>`).trim()).toBe(expected);
  });

  it("escapes a status label and a callout title like any text", () => {
    expect(
      htmlToMarkdown(
        '<p><span data-macro-name="status">a\\ *b* &lt;i&gt;</span></p>',
      ).trim(),
    ).toBe(":status[a\\\\ \\*b\\* \\<i>]{grey}");
    expect(
      htmlToMarkdown(
        '<div data-node-type="expand" data-title="T ] [!X] *y*"><p>Body</p></div>',
      ),
    ).toBe("> [!DETAILS]- T \\] \\[!X\\] \\*y\\*\n> Body");
  });

  it("turns an image into a link and drops a data URL image to its alt text", () => {
    expect(
      htmlToMarkdown('<p><img src="https://x.y/a.png" alt="Chart"></p>').trim(),
    ).toBe("[Chart](https://x.y/a.png)");
    expect(
      htmlToMarkdown(
        '<p><img src="data:image/png;base64,AAAA" alt="Chart"></p>',
      ).trim(),
    ).toBe("Chart");
  });

  it("keeps headings, lists, task lists, links and code", () => {
    const md = htmlToMarkdown(
      "<h2>Steps</h2><ul><li><input type=checkbox checked> done</li></ul><p><a href='https://x.y'>x</a></p><pre><code>cargo test</code></pre>",
    );
    expect(md).toContain("## Steps");
    expect(md).toContain("- [x] done");
    expect(md).toContain("[x](https://x.y)");
    expect(md).toContain("```\ncargo test\n```");
  });

  it("indents a nested list and a second paragraph under the marker", () => {
    const md = htmlToMarkdown(
      "<ol><li><p>One</p><p>More</p></li><li>Two<ul><li>Inner</li></ul></li></ol>",
    );
    expect(md).toBe("1. One\n\n   More\n\n2. Two\n   - Inner");
  });

  it("leaves an unknown element as its text", () => {
    expect(
      htmlToMarkdown("<custom-widget><p>Inside</p></custom-widget>").trim(),
    ).toBe("Inside");
  });

  it("never nests a callout in a callout", () => {
    const md = htmlToMarkdown(
      '<div data-macro-name="note"><p>Outer</p><div data-macro-name="tip"><p>Inner</p></div></div>',
    );
    expect(md).toBe("> [!NOTE]\n> Outer\n>\n> Inner");
  });

  it.each([
    ["success", "green"],
    ["error", "red"],
    ["current", "yellow"],
    ["complete", "blue"],
    ["new", "purple"],
    ["moved", "yellow"],
    ["purple", "purple"],
    ["shiny", "grey"],
    ["", "grey"],
  ])("gives a lozenge marked %j the colour %s", (modifier, colour) => {
    const md = htmlToMarkdown(
      `<p><span class="lozenge ${modifier === "" ? "" : `lozenge-${modifier}`}">Ok</span></p>`,
    );
    expect(md.trim()).toBe(`:status[Ok]{${colour}}`);
  });

  it("keeps a lozenge's brackets from closing the label", () => {
    const md = htmlToMarkdown(
      '<p><span data-macro-name="status" class="lozenge">[x] done</span></p>',
    );
    expect(md.trim()).toBe(":status[x done]{grey}");
  });
});

/**
 * The cells of the first body row of `markdown`'s table, as a GFM reader draws
 * them: text, with `code` around a code span. `null` when no table is drawn.
 */
function readBackCells(markdown: string): string[] | null {
  const html = renderToStaticMarkup(
    createElement(ReactMarkdown, { remarkPlugins: [remarkGfm] }, markdown),
  );
  const doc = new DOMParser().parseFromString(html, "text/html");
  const row = doc.querySelector("tbody tr");
  if (row === null) {
    return null;
  }
  return Array.from(row.querySelectorAll("td"), (td) =>
    Array.from(td.childNodes, (node) =>
      node.nodeName === "CODE"
        ? `code(${node.textContent})`
        : (node.textContent ?? ""),
    ).join(""),
  );
}

describe("htmlToMarkdown table cells", () => {
  function table(cell: string): string {
    return `<table><tr><th>h</th><th>i</th></tr><tr><td>${cell}</td><td>z</td></tr></table>`;
  }

  it.each([
    ["ends in a backslash", "a\\", "a\\"],
    ["ends in a backslash before a pipe", "a\\|", "a\\|"],
    ["holds a backslash and a pipe", "a\\|b", "a\\|b"],
    ["holds two backslashes", "a\\\\b", "a\\\\b"],
    ["holds a pipe", "x | y", "x | y"],
    ["holds a pipe in code", "<code>a|b</code>", "code(a|b)"],
    [
      "holds code that ends in a backslash",
      "<code>a\\</code>|x",
      "code(a\\)|x",
    ],
    [
      "holds two backslashes and a pipe in code",
      "<code>a\\\\|b</code>",
      "code(a\\\\|b)",
    ],
  ])("reads back as pasted when it %s", (_name, cell, shown) => {
    const md = htmlToMarkdown(table(cell));
    expect(readBackCells(md)).toEqual([shown, "z"]);
  });

  it("keeps the row whole when code holds a backslash before a pipe", () => {
    // GFM cannot keep this one exactly: in a cell, `\|` in code reads as a
    // pipe and `\\` as two backslashes. The pipe stays escaped and the
    // backslash stays, doubled, rather than the row breaking apart.
    const md = htmlToMarkdown(table("<code>a\\|b</code>"));
    expect(md).toContain("| `a\\\\\\|b` | z   |");
    expect(readBackCells(md)).toEqual(["code(a\\\\|b)", "z"]);
  });
});

describe("prefersPlainText", () => {
  it("prefers the plain text of a code editor's preformatted copy", () => {
    expect(
      prefersPlainText(
        '<meta charset="utf-8"><div style="color: #d4d4d4;font-family: Menlo;white-space: pre;"><div><span style="color: #569cd6;">const</span>  x = 1;</div></div>',
      ),
    ).toBe(true);
  });

  it("converts everything else", () => {
    expect(prefersPlainText(readFixture("docs-editor.html"))).toBe(false);
    expect(prefersPlainText(readFixture("wiki-panels.html"))).toBe(false);
    expect(prefersPlainText("<p>Hi</p>")).toBe(false);
  });
});
