/**
 * The queue on its own, for what `MermaidDiagram` never hands it: its paper
 * render catches its own failure, so a work that rejects or throws only
 * reaches the queue from here. It must still go on to the next copy.
 */

import { expect, it, vi } from "vitest";

import { queuePaper } from "./paperQueue";

/** Long enough for the 200 ms first wait and a task after it. */
function past(ms: number) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

it("goes on past a work that rejects or throws", async () => {
  const after = vi.fn(() => Promise.resolve());
  queuePaper(() => Promise.reject(new Error("no paper")));
  queuePaper(() => {
    throw new Error("no paper either");
  });
  queuePaper(after);
  await past(300);
  expect(after).toHaveBeenCalledOnce();
});
