/**
 * The room's presence row: a chip a screen reader announces once.
 *
 * The server already appends "(agent)" onto a participant's name (Task 15 fix
 * round 1, M2), so the glyph beside an agent's chip is decoration for sighted
 * readers rather than a second announcement of the same word.
 */

import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import type { CollabParticipant } from "./useCollabSession";
import { PresenceChips } from "./PresenceChips";

const PARTICIPANTS: CollabParticipant[] = [
  { name: "Ada (agent)", color: "#0ea5e9", self: false, agent: true },
  { name: "Jordi", color: "#22c55e", self: true, agent: false },
];

describe("PresenceChips", () => {
  it("hides the agent glyph from assistive tech and lets the chip text carry the word", () => {
    render(<PresenceChips participants={PARTICIPANTS} />);
    const glyph = document.querySelector("svg");
    expect(glyph).not.toBeNull();
    expect(glyph).toHaveAttribute("aria-hidden", "true");
    expect(glyph).not.toHaveAttribute("role");
    expect(glyph).not.toHaveAttribute("aria-label");
    expect(screen.getByText("Ada (agent)")).toBeInTheDocument();
  });
});
