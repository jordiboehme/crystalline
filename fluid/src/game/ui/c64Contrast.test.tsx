/**
 * The C64 screens' text against their blue, WCAG AA for normal text
 * (4.5:1): the refusal, the connecting screen and the pause screen, and
 * the pause screen's reverse video.
 */

import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router";
import { describe, expect, it, vi } from "vitest";

import { classColours, contrastRatio } from "../../test/contrast";
import { CONNECTING_LABEL, Connecting } from "./Connecting";
import { C64_BLUE, C64_TEXT, DeviceRefusal } from "./DeviceRefusal";
import { CONT_LABEL, PAUSE_LABEL, PauseScreen } from "./PauseScreen";

const AA = 4.5;

/** The inner screen of a C64 frame: the element on the blue. */
function blueBox(root: HTMLElement): HTMLElement {
  const box = [root, ...root.querySelectorAll<HTMLElement>("div")].find(
    (el) =>
      el.style.background !== "" &&
      contrastRatio(el.style.background, C64_BLUE) === 1,
  );
  if (!box) throw new Error("no blue box");
  return box;
}

describe("the C64 screens' contrast (WCAG AA)", () => {
  // Mutation caught: the text drawn in the palette's own light blue
  // (2.3:1 on the blue), on any of the three screens.
  it("draws the text at 4.5:1 or more on the blue", () => {
    expect(contrastRatio(C64_TEXT, C64_BLUE)).toBeGreaterThanOrEqual(AA);

    const refusal = render(
      <MemoryRouter>
        <DeviceRefusal />
      </MemoryRouter>,
    );
    const box = blueBox(refusal.container);
    expect(
      contrastRatio(box.style.color, box.style.background),
    ).toBeGreaterThanOrEqual(AA);
    refusal.unmount();

    const connecting = render(
      <Connecting number="555-0142" name="ENG" onDone={vi.fn()} />,
    );
    const dial = blueBox(
      screen.getByRole("dialog", { name: CONNECTING_LABEL }),
    );
    expect(
      contrastRatio(dial.style.color, dial.style.background),
    ).toBeGreaterThanOrEqual(AA);
    connecting.unmount();

    render(
      <PauseScreen
        where="ALPHA"
        lockEndedAt={-Infinity}
        onContinue={vi.fn()}
        onLeave={vi.fn()}
      />,
    );
    const pause = blueBox(screen.getByRole("dialog", { name: PAUSE_LABEL }));
    expect(
      contrastRatio(pause.style.color, pause.style.background),
    ).toBeGreaterThanOrEqual(AA);
    // The reverse video of a focused or hovered button.
    const cont = screen.getByRole("button", { name: CONT_LABEL });
    for (const state of ["hover:", "focus:"]) {
      const [bg] = classColours(cont.className, `${state}bg-`);
      const [fg] = classColours(cont.className, `${state}text-`);
      expect(contrastRatio(fg!, bg!)).toBeGreaterThanOrEqual(AA);
    }
  });
});
