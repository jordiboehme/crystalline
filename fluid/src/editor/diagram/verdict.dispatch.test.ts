// The sequence parser is watched with a spy, so a flowchart's verdict is seen
// not to scan its body twice: the scan's regexes can cost seconds on a crafted
// line, and the verdict runs on every changed body.

import { beforeEach, describe, expect, test, vi } from "vitest";

import { requestVerdict } from "./verdict";
import { parseSequence } from "./sequence/parse";

vi.mock("./sequence/parse", async (original) => {
  const real = await original<typeof import("./sequence/parse")>();
  return { ...real, parseSequence: vi.fn(real.parseSequence) };
});

describe("the verdict's dispatch", () => {
  beforeEach(() => {
    vi.mocked(parseSequence).mockClear();
  });

  test("a flowchart never reaches the sequence parser", async () => {
    const verdict = await requestVerdict("flowchart TD\n  A --> B", false);
    expect(verdict.ok).toBe(true);
    expect(parseSequence).not.toHaveBeenCalled();
  });

  test("a sequence diagram does", async () => {
    const verdict = await requestVerdict("sequenceDiagram\n  A->>B: x", false);
    expect(verdict).toMatchObject({ ok: true, kind: "sequence" });
    expect(parseSequence).toHaveBeenCalledTimes(1);
  });

  test("a flowchart that names the keyword in a label is still a flowchart", async () => {
    const verdict = await requestVerdict(
      "flowchart TD\n  A[sequenceDiagram] --> B",
      false,
    );
    expect(verdict).toMatchObject({ ok: true, kind: "flowchart" });
  });
});
