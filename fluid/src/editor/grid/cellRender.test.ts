/**
 * A static grid cell: its Markdown drawn inline as text and elements, never
 * as an HTML string. Each content kind the cell can hold is drawn the way the
 * main view's own layers draw it.
 */

import type { MockInstance } from "vitest";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";

import { renderCellContent } from "./cellRender";

let errors: MockInstance;
beforeEach(() => {
  errors = vi.spyOn(console, "error");
});
afterEach(() => {
  expect(errors).not.toHaveBeenCalled();
  errors.mockRestore();
});

function draw(text: string, options?: Parameters<typeof renderCellContent>[2]) {
  const target = document.createElement("td");
  document.body.appendChild(target);
  renderCellContent(text, target, options);
  return target;
}

describe("renderCellContent", () => {
  test("plain text is a text node", () => {
    const cell = draw("Berth 4");
    expect(cell.textContent).toBe("Berth 4");
    expect(cell.children.length).toBe(0);
  });

  test("an empty cell draws nothing", () => {
    expect(draw("").childNodes.length).toBe(0);
  });

  test("bold, italic and strikethrough draw as their elements", () => {
    const cell = draw("**big** and *small* and ~~gone~~");
    expect(cell.querySelector("strong")?.textContent).toBe("big");
    expect(cell.querySelector("em")?.textContent).toBe("small");
    expect(cell.querySelector("s")?.textContent).toBe("gone");
    expect(cell.textContent).toBe("big and small and gone");
  });

  test("nested emphasis nests the elements", () => {
    const cell = draw("**very *much* so**");
    expect(cell.querySelector("strong em")?.textContent).toBe("much");
    expect(cell.textContent).toBe("very much so");
  });

  test("inline code draws as code, its content left as written", () => {
    const cell = draw("run `a **b**` now");
    const code = cell.querySelector("code");
    expect(code?.textContent).toBe("a **b**");
    expect(code?.querySelector("strong")).toBeNull();
    expect(cell.textContent).toBe("run a **b** now");
  });

  test("a link is an anchor with rel noopener and its words", () => {
    const cell = draw("see [the docs](https://example.com/a) here");
    const anchor = cell.querySelector("a");
    expect(anchor?.getAttribute("href")).toBe("https://example.com/a");
    expect(anchor?.getAttribute("rel")).toContain("noopener");
    // Not a Tab stop of its own: the grid's roving tabindex is the only one.
    expect(anchor?.tabIndex).toBe(-1);
    expect(anchor?.textContent).toBe("the docs");
    expect(cell.textContent).toBe("see the docs here");
  });

  test("a link's own emphasis is drawn inside the anchor", () => {
    const cell = draw("[**bold** link](https://example.com)");
    expect(cell.querySelector("a strong")?.textContent).toBe("bold");
  });

  test("a plain click on a link does not navigate, a Mod-click does", () => {
    const anchor = draw("[x](https://example.com)").querySelector("a");
    if (!anchor) throw new Error("no anchor");
    const plain = new MouseEvent("click", { bubbles: true, cancelable: true });
    anchor.dispatchEvent(plain);
    expect(plain.defaultPrevented).toBe(true);
    const mod = new MouseEvent("click", {
      bubbles: true,
      cancelable: true,
      metaKey: true,
      ctrlKey: true,
    });
    // jsdom would try to navigate on the allowed click; stop it at the body.
    const stop = (event: Event) => {
      if (!event.defaultPrevented) event.preventDefault();
    };
    let preventedBefore: boolean | null = null;
    const spy = (event: Event) => {
      preventedBefore = event.defaultPrevented;
      stop(event);
    };
    document.body.addEventListener("click", spy);
    anchor.dispatchEvent(mod);
    document.body.removeEventListener("click", spy);
    expect(preventedBefore).toBe(false);
  });

  test("a link to anything but a web address keeps its words and no href", () => {
    const cell = draw("[run](javascript:alert(1))");
    expect(cell.querySelector("a[href]")).toBeNull();
    expect(cell.textContent).toBe("run");
  });

  test("a bracket with no address is the text that was written", () => {
    const cell = draw("[not a link]");
    expect(cell.querySelector("a")).toBeNull();
    expect(cell.textContent).toBe("[not a link]");
  });

  test("a wikilink draws as the reference chip", () => {
    const cell = draw("go to [[Harbor Map]] now", { resolve: () => null });
    const chip = cell.querySelector(".cm-wikilink");
    expect(chip?.className).toBe("cm-wikilink cm-wikilink-pending");
    expect(chip?.textContent).toBe("Harbor Map");
    expect(chip?.getAttribute("title")).toBe("[[Harbor Map]]");
    expect(cell.textContent).toBe("go to Harbor Map now");
  });

  test("without a resolver a wikilink is its plain label text (R12)", () => {
    const cell = draw("go to [[Harbor Map]] or [[Tide Tables\\|the tides]]");
    expect(cell.querySelector(".cm-wikilink")).toBeNull();
    expect(cell.children.length).toBe(0);
    expect(cell.textContent).toBe("go to Harbor Map or the tides");
  });

  test("a wikilink with an alias shows the alias, resolved by the resolver", () => {
    // In a table cell the alias pipe is escaped.
    const cell = draw("[[Harbor Map\\|the map]]", {
      resolve: (inner) =>
        inner === "Harbor Map|the map"
          ? { kind: "resolved", href: "/x", label: "Harbor Map" }
          : null,
    });
    const chip = cell.querySelector(".cm-wikilink");
    expect(chip?.className).toBe("cm-wikilink cm-wikilink-resolved");
    expect(chip?.textContent).toBe("the map");
  });

  test("a wikilink inside code stays code", () => {
    const cell = draw("`[[Harbor Map]]`");
    expect(cell.querySelector(".cm-wikilink")).toBeNull();
    expect(cell.querySelector("code")?.textContent).toBe("[[Harbor Map]]");
  });

  test("a status label is the pill the table preview draws", () => {
    const cell = draw(":status[In review]{blue} now");
    const pill = cell.querySelector(".status-label");
    expect(pill?.tagName).toBe("SPAN");
    expect(pill?.className).toBe("status-label");
    expect(pill?.getAttribute("data-status")).toBe("blue");
    expect(pill?.textContent).toBe("In review");
    expect(cell.textContent).toBe("In review now");
  });

  test("a status label's words are its plain words", () => {
    const pill = draw(":status[**Done**]").querySelector(".status-label");
    expect(pill?.textContent).toBe("Done");
    expect(pill?.getAttribute("data-status")).toBe("grey");
  });

  test("inline math shows its TeX until KaTeX arrives, then the formula", async () => {
    const cell = draw("area $$x^2$$ here");
    const math = cell.querySelector(".cm-math-inline");
    expect(math?.textContent).toBe("x^2");
    expect(cell.textContent).toBe("area x^2 here");
    await vi.waitFor(() => {
      expect(math?.querySelector(".katex")).not.toBeNull();
    });
  });

  test("code between dollar signs is inline math too", () => {
    const cell = draw("$`a+b`$");
    const math = cell.querySelector(".cm-math-inline");
    expect(math?.textContent).toBe("a+b");
    expect(cell.querySelector(".cm-math-inline + *")).toBeNull();
    expect(cell.textContent).toBe("a+b");
  });

  test("math redrawn before KaTeX arrives is not overwritten by the late render", async () => {
    const cell = draw("$$y$$");
    cell.replaceChildren();
    renderCellContent("plain", cell);
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(cell.textContent).toBe("plain");
  });

  test("an escaped pipe draws as a pipe", () => {
    expect(draw("a \\| b").textContent).toBe("a | b");
  });

  test("an escaped backslash draws as one backslash", () => {
    expect(draw("C:\\\\Temp").textContent).toBe("C:\\Temp");
    expect(draw("x\\\\\\|y").textContent).toBe("x\\|y");
  });

  test("a br tag is a line break", () => {
    const cell = draw("one<br>two<br/>three<BR />four");
    expect(cell.querySelectorAll("br").length).toBe(3);
    expect(cell.textContent).toBe("onetwothreefour");
  });

  test("a script tag is text", () => {
    const cell = draw("<script>alert(1)</script>");
    expect(cell.querySelector("script")).toBeNull();
    expect(cell.textContent).toBe("<script>alert(1)</script>");
  });

  test("any other HTML is text", () => {
    const cell = draw('<img src=x onerror="alert(1)"> <b>no</b>');
    expect(cell.querySelector("img")).toBeNull();
    expect(cell.querySelector("b")).toBeNull();
    expect(cell.textContent).toBe('<img src=x onerror="alert(1)"> <b>no</b>');
  });

  test("an attachment image is drawn small, with its alt text", () => {
    const cell = draw("see ![x](assets/a.png#right,w=50%) here");
    const image = cell.querySelector("img");
    expect(image?.getAttribute("alt")).toBe("x");
    expect(image?.getAttribute("src")).toBe("assets/a.png");
    expect(image?.getAttribute("draggable")).toBe("false");
    expect(image?.style.maxHeight).toBe("6em");
    // No float in a cell: the fragment's placement is for the page.
    expect(image?.style.float).toBe("");
    expect(image?.classList.contains("cm-picture")).toBe(false);
    expect(cell.textContent).toBe("see  here");
  });

  test("an image's alt text is drawn without its escapes", () => {
    const cell = draw("![a \\[b\\] c](assets/a.png)");
    expect(cell.querySelector("img")?.getAttribute("alt")).toBe("a [b] c");
  });

  test("an image reads its address from the page's resolver", () => {
    const cell = draw("![x](assets/a.png)", {
      assets: {
        resolve: (target) => target,
        url: (path) => `/files/${path}`,
        refused: () => null,
      },
    });
    expect(cell.querySelector("img")?.getAttribute("src")).toBe(
      "/files/assets/a.png",
    );
  });

  test("an external image is drawn from its address", () => {
    const cell = draw("![alt](https://example.com/a.png)");
    expect(cell.querySelector("img")?.getAttribute("src")).toBe(
      "https://example.com/a.png",
    );
  });

  test("a refused or unavailable image is its alt text, and fetches nothing", () => {
    const refused = draw("![no](assets/a.png)", {
      assets: {
        resolve: (target) => target,
        url: (path) => path,
        refused: () => "Not here.",
      },
    });
    expect(refused.querySelector("img")).toBeNull();
    expect(refused.textContent).toBe("no");
    const unavailable = draw("![deck](assets/deck.pdf)");
    expect(unavailable.querySelector("img")).toBeNull();
    expect(unavailable.textContent).toBe("deck");
  });

  test("drawing again replaces what was there", () => {
    const cell = draw("**a**");
    renderCellContent("b", cell);
    expect(cell.querySelector("strong")).toBeNull();
    expect(cell.textContent).toBe("b");
  });

  test("block syntax in a cell is text, as in a table", () => {
    expect(draw("# not a heading").textContent).toBe("# not a heading");
    expect(draw("- not a list").textContent).toBe("- not a list");
    expect(draw("> not a quote").textContent).toBe("> not a quote");
  });
});
