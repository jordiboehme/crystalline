import { describe, expect, test } from "vitest";

import { drawnLabels, entityText } from "./mermaidText";

/** An SVG as mermaid's render hands it over, one label with `text`. */
function svgWith(text: string): string {
  const div = document.createElement("div");
  const span = document.createElement("span");
  span.textContent = text;
  return `<svg id="m" viewBox="0 0 10 10"><style>.a>b{fill:red}</style><g class="label"><text y="-10.1"><tspan class="row">${span.innerHTML}</tspan></text></g></svg>${div.innerHTML}`;
}

function shown(svg: string): string {
  const host = document.createElement("div");
  host.innerHTML = svg;
  return host.querySelector("tspan")?.textContent ?? "";
}

describe("drawnLabels", () => {
  test.each([
    ["&quot;hi&quot;", '"hi"'],
    ["&#35;", "#"],
    ["a:&#35;b;", "a:#b;"],
    ["C:&#92;new", "C:\\new"],
    ["&#38;amp;", "&amp;"],
    ["&#35;9829;", "#9829;"],
    ["&#9829;", "\u2665"],
    ["&infin;", "\u221E"],
    ["&lt;b&gt;", "<b>"],
  ])("%s draws as %s", (text, wanted) => {
    expect(shown(drawnLabels(svgWith(text)))).toBe(wanted);
  });

  test("one pass: what a decode gives is not decoded again", () => {
    expect(shown(drawnLabels(svgWith("&#38;quot;")))).toBe("&quot;");
  });

  test("a name it does not know stays as it is", () => {
    expect(shown(drawnLabels(svgWith("AT&T; &bogus;")))).toBe("AT&T; &bogus;");
  });

  test("an svg with nothing to decode comes back the same string", () => {
    const svg = svgWith("plain & simple");
    expect(drawnLabels(svg)).toBe(svg);
    expect(drawnLabels("<svg data-diagram></svg>")).toBe(
      "<svg data-diagram></svg>",
    );
  });

  test.each([
    "&lt;script&gt;x&lt;/script&gt;",
    "&lt;img src=x onerror=y&gt;",
    "&lt;!--c--&gt;",
    "&#60;svg onload=y&#62;",
  ])("%s stays text, never markup", (text) => {
    const host = document.createElement("div");
    host.innerHTML = drawnLabels(svgWith(text));
    expect(host.querySelectorAll("script, img, svg svg").length).toBe(0);
    expect(host.querySelector("tspan")?.children.length).toBe(0);
    expect(host.querySelector("tspan")?.textContent).toMatch(/^</);
  });

  test("only drawn text changes: no style, attribute or foreign object", () => {
    const svg =
      '<svg aria-label="&amp;quot;"><style>.a::after{content:"&amp;quot;"}</style>' +
      "<foreignObject><div>&amp;quot;</div></foreignObject>" +
      "<title>&amp;quot;</title><text>&amp;quot;</text></svg>";
    const host = document.createElement("div");
    host.innerHTML = drawnLabels(svg);
    expect(host.querySelector("text")?.textContent).toBe('"');
    expect(host.querySelector("foreignObject div")?.textContent).toBe("&quot;");
    expect(host.querySelector("title")?.textContent).toBe("&quot;");
    expect(host.querySelector("svg")?.getAttribute("aria-label")).toBe(
      "&quot;",
    );
    expect(host.querySelector("style")?.textContent).toContain("&quot;");
  });

  test("the viewBox keeps its case", () => {
    expect(drawnLabels(svgWith("&quot;"))).toContain('viewBox="0 0 10 10"');
  });

  test("text that is not an svg comes back as it was", () => {
    expect(drawnLabels("&amp;quot;")).toBe("&amp;quot;");
  });
});

describe("entityText", () => {
  test.each([
    ["#9829", "\u2665"],
    ["quot", '"'],
    ["amp", "&"],
    ["lt", "<"],
    ["gt", ">"],
    ["bogus", null],
    ["#55555555555", null],
    ["#55357", null],
    ["#0", null],
  ])("%s is %j", (name, wanted) => {
    expect(entityText(name)).toBe(wanted);
  });
});
