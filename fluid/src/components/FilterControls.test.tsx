/**
 * The tag facet, held to the one thing that breaks on a real archive.
 *
 * A domain that has been taught for a year is written in a few hundred tags,
 * and a rail that drew a chip for every one of them buried the results under
 * its own vocabulary. So the rail is capped, and what the cap hides is reached
 * by narrowing rather than by expanding: the tests here are the cap, the
 * promise that a chosen tag is never the one hidden, and the filter that gets
 * at the rest.
 */

import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";

import type { TagCount } from "../api/vocabulary";
import { MAX_VISIBLE_TAGS, TagChips } from "./FilterControls";

/** A vocabulary of `count` tags, commonest first, the way the API sends it. */
function vocabulary(count: number): TagCount[] {
  return Array.from({ length: count }, (_, index) => ({
    name: `tag-${String(index)}`,
    engrams: count - index,
  }));
}

/** The chips on screen, in the order they are drawn. */
function chipNames(): string[] {
  const list = screen.getByRole("list", { name: "Tags" });
  return within(list)
    .queryAllByRole("button")
    .map((chip) => chip.textContent ?? "");
}

/** The list element itself, for the classes that bound its growth. */
function tagList(): HTMLElement {
  return screen.getByRole("list", { name: "Tags" });
}

/** The rail with somebody holding the selection, the way a screen does. */
function Harness({
  tags,
  initial = [],
  onChange,
}: {
  tags: TagCount[];
  initial?: string[];
  onChange?: (next: string[]) => void;
}) {
  const [chosen, setChosen] = useState(initial);
  return (
    <TagChips
      tags={tags}
      chosen={chosen}
      onChange={(next) => {
        setChosen(next);
        onChange?.(next);
      }}
    />
  );
}

describe("the tag facet", () => {
  it("draws a small vocabulary whole, with nothing to filter or count", () => {
    render(<Harness tags={vocabulary(MAX_VISIBLE_TAGS)} />);

    expect(chipNames()).toHaveLength(MAX_VISIBLE_TAGS);
    expect(screen.queryByLabelText("Filter tags")).toBeNull();
    expect(screen.queryByRole("button", { name: /more$/ })).toBeNull();
  });

  it("caps a few-hundred-tag rail and says what the cap is hiding", () => {
    render(<Harness tags={vocabulary(300)} />);

    expect(chipNames()).toHaveLength(MAX_VISIBLE_TAGS);
    expect(chipNames()[0]).toContain("#tag-0");
    expect(screen.getByRole("button", { name: "+288 more" })).toBeVisible();
    expect(screen.getByLabelText("Filter tags")).toBeVisible();
  });

  it("keeps a chosen tag on screen even when it is nobody's commonest", () => {
    render(<Harness tags={vocabulary(300)} initial={["tag-299"]} />);

    const names = chipNames();
    // Selected first, and the cap still holds: the rail does not grow by one
    // for every tag a reader turns on.
    expect(names[0]).toContain("#tag-299");
    expect(names).toHaveLength(MAX_VISIBLE_TAGS);
    expect(screen.getByRole("button", { name: "+288 more" })).toBeVisible();
  });

  it("narrows to the substring matches, inside a box that scrolls", async () => {
    const user = userEvent.setup();
    render(<Harness tags={vocabulary(300)} />);

    await user.type(screen.getByLabelText("Filter tags"), "tag-29");

    const names = chipNames();
    // tag-29 and tag-290 through tag-299: eleven, which is past the cap and
    // still bounded, because the box scrolls rather than the page growing.
    expect(names).toHaveLength(11);
    expect(names.every((name) => name.includes("#tag-29"))).toBe(true);
    expect(tagList().className).toContain("overflow-y-auto");
  });

  it("says so quietly when the filter matches no tag at all", async () => {
    const user = userEvent.setup();
    render(<Harness tags={vocabulary(300)} />);

    await user.type(screen.getByLabelText("Filter tags"), "nothing-like-this");

    expect(chipNames()).toHaveLength(0);
    expect(screen.getByText("no tag matches")).toBeVisible();
  });

  it("sends the reader to the filter when they ask what is hidden", async () => {
    const user = userEvent.setup();
    render(<Harness tags={vocabulary(300)} />);

    await user.click(screen.getByRole("button", { name: "+288 more" }));

    expect(screen.getByLabelText("Filter tags")).toHaveFocus();
  });

  it("still turns a tag on and off from the chip itself", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<Harness tags={vocabulary(300)} onChange={onChange} />);

    await user.click(screen.getByRole("button", { name: /#tag-0/ }));
    expect(onChange).toHaveBeenLastCalledWith(["tag-0"]);

    await user.click(screen.getByRole("button", { name: /#tag-0/ }));
    expect(onChange).toHaveBeenLastCalledWith([]);
  });

  it("draws nothing at all when the vocabulary is empty", () => {
    const { container } = render(<Harness tags={[]} />);
    expect(container).toBeEmptyDOMElement();
  });
});
