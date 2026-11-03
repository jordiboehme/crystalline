import { describe, expect, it } from "vitest";

import { stripHtmlComments, withoutHtmlComments } from "./htmlComments";

describe("HTML comments", () => {
  it("takes out whole comments, on one line or many, and the empty forms", () => {
    expect(withoutHtmlComments("a <!-- b --> c")).toBe("a  c");
    expect(withoutHtmlComments("<!--\nb\n-->")).toBe("");
    expect(withoutHtmlComments("<!--> x <!---> y")).toBe(" x  y");
    expect(withoutHtmlComments("<!-- r:7f3a9c2e -->")).toBe("");
  });

  it("leaves a comment that never closes and every other tag", () => {
    expect(withoutHtmlComments("a <!-- b")).toBe("a <!-- b");
    expect(withoutHtmlComments("<div>x</div>")).toBe("<div>x</div>");
  });

  it("takes out a comment that taking out another one made", () => {
    expect(withoutHtmlComments("<!-<!-- x -->- y -->")).toBe("");
    expect(withoutHtmlComments("a <!-<!-<!-- x -->- y -->- z --> b")).toBe(
      "a  b",
    );
    expect(stripHtmlComments("<!-<!-- x -->- y")).toBe("");
  });

  it("cleans a snippet the window cut inside a comment, and keeps an arrow", () => {
    expect(stripHtmlComments("night <!-- sources: the har")).toBe("night ");
    expect(stripHtmlComments("A --> B <!-- x --> C")).toBe("A --> B  C");
    expect(stripHtmlComments("<!-- x A --> B")).toBe(" B");
  });
});
