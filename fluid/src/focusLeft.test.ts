import { describe, expect, it } from "vitest";

import { focusLeft } from "./focusLeft";

function tree() {
  const page = document.createElement("main");
  const group = document.createElement("div");
  const inside = document.createElement("button");
  const outside = document.createElement("button");
  group.append(inside);
  page.append(group, outside);
  return { page, group, inside, outside };
}

describe("focusLeft", () => {
  it("is not leaving when there is no destination", () => {
    const { group } = tree();
    expect(focusLeft(group, null)).toBe(false);
  });

  it("is not leaving for a control inside the group", () => {
    const { group, inside } = tree();
    expect(focusLeft(group, inside)).toBe(false);
  });

  it("is not leaving for an ancestor of the group, where Safari puts a click", () => {
    const { group, page } = tree();
    expect(focusLeft(group, page)).toBe(false);
  });

  it("is leaving for a sibling outside the group", () => {
    const { group, outside } = tree();
    expect(focusLeft(group, outside)).toBe(true);
  });
});
