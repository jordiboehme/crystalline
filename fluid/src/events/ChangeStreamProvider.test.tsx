import type { InvalidateQueryFilters, Query } from "@tanstack/react-query";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, render } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ApiProblem } from "../api/client";
import { ME_QUERY_KEY } from "../auth/keys";
import type { SessionProbe } from "./ChangeStreamProvider";
import {
  ChangeStreamProvider,
  subscribeToChanges,
} from "./ChangeStreamProvider";
import { IgnoredEngramsContext, useIgnoredEngram } from "./ignored";
import { useRecentChange } from "./recent";
import { FakeEventSource, fakeStreamFactory } from "./testSupport";

/** A probe that answers "still signed in as ada". */
const signedIn: SessionProbe = () =>
  Promise.resolve({ user: { name: "ada" }, anonymous: false });

function tree(
  client: QueryClient,
  children: ReactNode,
  sessionProbe: SessionProbe,
) {
  return (
    <QueryClientProvider client={client}>
      <ChangeStreamProvider
        streamFactory={fakeStreamFactory}
        sessionProbe={sessionProbe}
      >
        {children}
      </ChangeStreamProvider>
    </QueryClientProvider>
  );
}

function mount(
  client: QueryClient,
  children: ReactNode = <div />,
  sessionProbe: SessionProbe = signedIn,
) {
  const view = render(tree(client, children, sessionProbe));
  return {
    ...view,
    rerenderWith: (next: ReactNode) => {
      view.rerender(tree(client, next, sessionProbe));
    },
  };
}

function Editor() {
  useIgnoredEngram("eng", "alpha");
  return null;
}

const engram = (permalink: string, kind = "modified") => ({
  domain: "eng",
  permalink,
  path: `${permalink}.md`,
  kind,
  from: null,
  checksum: "9f",
  actor: null,
  draft_of: null,
});

/** The first source the mounted provider opened. */
function theSource(): FakeEventSource {
  const source = FakeEventSource.instances[0];
  if (!source) throw new Error("no source was opened");
  return source;
}

/** The `index`th source, which must exist. */
function sourceAt(index: number): FakeEventSource {
  const source = FakeEventSource.instances[index];
  if (!source) throw new Error(`no source ${String(index)}`);
  return source;
}

/** Let a settled probe's continuation run, without moving the clock. */
async function settle(): Promise<void> {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(0);
  });
}

/** Move the clock inside act, microtasks included. */
async function advance(ms: number): Promise<void> {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
}

describe("the change stream", () => {
  let client: QueryClient;
  let invalidate: ReturnType<typeof vi.fn>;
  /** The filters of every `invalidateQueries` call, in order. */
  const filters = () =>
    invalidate.mock.calls.map(
      (call) => call[0] as InvalidateQueryFilters | undefined,
    );
  const invalidatedKeys = () =>
    filters().map((filter) => JSON.stringify(filter?.queryKey));

  beforeEach(() => {
    vi.useFakeTimers();
    FakeEventSource.instances = [];
    client = new QueryClient();
    invalidate = vi.fn().mockResolvedValue(undefined);
    client.invalidateQueries = invalidate as QueryClient["invalidateQueries"];
  });
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  it("opens one source per mount with credentials and closes it on unmount", () => {
    const view = mount(client);
    expect(FakeEventSource.instances).toHaveLength(1);
    expect(theSource().url).toBe("/api/v1/events");
    view.unmount();
    expect(theSource().readyState).toBe(2);
  });

  it("an unmount inside the window drops the pending invalidations", () => {
    // Catches a cleanup that leaves the coalescing timer running: a late
    // invalidation from a provider that is gone.
    const view = mount(client);
    act(() => {
      theSource().emit("engram", engram("alpha"), "1:1");
    });
    view.unmount();
    act(() => {
      vi.advanceTimersByTime(250);
    });
    expect(invalidate).not.toHaveBeenCalled();
  });

  it("coalesces five frames inside the window into one invalidation per distinct key", () => {
    mount(client);
    const source = theSource();
    act(() => {
      for (let i = 0; i < 5; i++) {
        source.emit("engram", engram("alpha"), `1:${i + 1}`);
      }
    });
    expect(invalidate).not.toHaveBeenCalled();
    act(() => {
      vi.advanceTimersByTime(250);
    });
    const keys = invalidatedKeys();
    expect(new Set(keys).size).toBe(keys.length);
    expect(keys).toContain(JSON.stringify(["engram", "eng", "alpha"]));
    expect(keys).toContain(JSON.stringify(["domain-tree", "eng"]));
  });

  it("a reset flushes the pending set and invalidates everything at once", () => {
    mount(client);
    const source = theSource();
    act(() => {
      source.emit("engram", engram("alpha"), "1:1");
      source.emit("reset", {});
    });
    expect(invalidate).toHaveBeenCalledTimes(1);
    expect(invalidate).toHaveBeenCalledWith();
    act(() => {
      vi.advanceTimersByTime(250);
    });
    expect(invalidate).toHaveBeenCalledTimes(1);
    // The flushed set stays flushed: the next window carries only what came
    // after the reset, never the keys the reset already covered.
    act(() => {
      source.emit("engram", engram("beta"), "1:2");
      vi.advanceTimersByTime(250);
    });
    expect(invalidatedKeys()).toContain(
      JSON.stringify(["engram", "eng", "beta"]),
    );
    expect(invalidatedKeys()).not.toContain(
      JSON.stringify(["engram", "eng", "alpha"]),
    );
  });

  it("a source that reconnects on its own is left to the browser", async () => {
    const probe = vi.fn(signedIn);
    mount(client, <div />, probe);
    act(() => {
      theSource().fail(0);
    });
    await advance(60_000);
    expect(probe).not.toHaveBeenCalled();
    expect(invalidate).not.toHaveBeenCalled();
    expect(FakeEventSource.instances).toHaveLength(1);
  });

  it("a source closed by a 502 during a restart asks the probe and reopens with backoff, then resets (ruling C1b)", async () => {
    vi.spyOn(Math, "random").mockReturnValue(0.5);
    const seen: string[] = [];
    const unsubscribe = subscribeToChanges((event) => seen.push(event.event));
    // The daemon is still down at the first probe and up at the second.
    const probe = vi
      .fn<SessionProbe>()
      .mockRejectedValueOnce(new ApiProblem(502, "bad gateway", ""))
      .mockImplementation(signedIn);
    mount(client, <div />, probe);
    act(() => {
      theSource().fail(2);
    });
    await settle();
    expect(probe).toHaveBeenCalledTimes(1);
    expect(theSource().readyState, "the closed one is let go").toBe(2);
    await advance(999);
    expect(probe).toHaveBeenCalledTimes(1);
    await advance(1);
    expect(probe, "probed again after one second").toHaveBeenCalledTimes(2);
    expect(FakeEventSource.instances).toHaveLength(1);
    await advance(1_999);
    expect(FakeEventSource.instances).toHaveLength(1);
    await advance(1);
    expect(
      FakeEventSource.instances,
      "reopened two seconds later",
    ).toHaveLength(2);
    const next = sourceAt(1);
    expect(next.url).toBe("/api/v1/events");
    expect(
      invalidate,
      "nothing until the new source opens",
    ).not.toHaveBeenCalled();
    act(() => {
      next.open();
    });
    // The new source cannot send the last id it saw, so the gap is covered
    // the way the server covers a lost id: a reset, for the pages and for
    // every subscriber.
    expect(invalidate).toHaveBeenCalledWith();
    expect(seen).toEqual(["reset"]);
    expect(invalidate).not.toHaveBeenCalledWith({ queryKey: ME_QUERY_KEY });
    act(() => {
      next.emit("engram", engram("alpha"), "2:1");
      vi.advanceTimersByTime(250);
    });
    expect(invalidatedKeys()).toContain(
      JSON.stringify(["engram", "eng", "alpha"]),
    );
    expect(seen).toEqual(["reset", "engram"]);
    unsubscribe();
  });

  it("the stream cap's 503 backs off 1, 2, 4 seconds and starts over once a source opens", async () => {
    vi.spyOn(Math, "random").mockReturnValue(0.5);
    mount(client);
    const refuse = async (index: number, wait: number) => {
      act(() => {
        sourceAt(index).fail(2);
      });
      await settle();
      await advance(wait - 1);
      expect(
        FakeEventSource.instances,
        `still waiting at ${String(wait - 1)} ms`,
      ).toHaveLength(index + 1);
      await advance(1);
      expect(FakeEventSource.instances).toHaveLength(index + 2);
    };
    await refuse(0, 1_000);
    await refuse(1, 2_000);
    await refuse(2, 4_000);
    // A source that opens clears the count: the next refusal waits a second.
    act(() => {
      sourceAt(3).open();
    });
    await refuse(3, 1_000);
  });

  it("a 401 from the probe ends it: the shell re-asks who is signed in and nothing reopens", async () => {
    const probe = vi
      .fn<SessionProbe>()
      .mockRejectedValue(new ApiProblem(401, "unauthorized", ""));
    mount(client, <div />, probe);
    act(() => {
      theSource().fail(2);
    });
    await settle();
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ME_QUERY_KEY });
    await advance(120_000);
    expect(FakeEventSource.instances).toHaveLength(1);
    expect(probe).toHaveBeenCalledTimes(1);
  });

  it("a probe that names nobody, or somebody else, is a session that ended", async () => {
    // The probe answers 200 with no identity for an ended cookie session,
    // and as the anonymous viewer where an instance allows one.
    client.setQueryData(ME_QUERY_KEY, { user: { name: "ada" } });
    for (const answer of [
      { user: null, anonymous: false },
      { user: null, anonymous: true },
      { user: { name: "bob" }, anonymous: false },
    ]) {
      cleanup();
      invalidate.mockClear();
      FakeEventSource.instances = [];
      mount(client, <div />, () => Promise.resolve(answer));
      act(() => {
        theSource().fail(2);
      });
      await settle();
      expect(invalidate, JSON.stringify(answer)).toHaveBeenCalledWith({
        queryKey: ME_QUERY_KEY,
      });
      await advance(60_000);
      expect(FakeEventSource.instances, JSON.stringify(answer)).toHaveLength(1);
    }
  });

  it("an unmount while the probe is out or the backoff runs never opens a source", async () => {
    let answer: (value: unknown) => void = () => undefined;
    const view = mount(
      client,
      <div />,
      () =>
        new Promise((resolve) => {
          answer = resolve;
        }),
    );
    act(() => {
      theSource().fail(2);
    });
    view.unmount();
    answer({ user: { name: "ada" } });
    await settle();
    await advance(60_000);
    expect(
      FakeEventSource.instances,
      "unmounted during the probe",
    ).toHaveLength(1);

    FakeEventSource.instances = [];
    const second = mount(client);
    act(() => {
      theSource().fail(2);
    });
    await settle();
    second.unmount();
    await advance(60_000);
    expect(
      FakeEventSource.instances,
      "unmounted during the backoff",
    ).toHaveLength(1);

    // A probe that comes back after the unmount is not acted on at all:
    // not even an ended session re-asks who is signed in.
    FakeEventSource.instances = [];
    let refuse: (reason: unknown) => void = () => undefined;
    const third = mount(
      client,
      <div />,
      () =>
        new Promise((_resolve, reject) => {
          refuse = reject;
        }),
    );
    act(() => {
      theSource().fail(2);
    });
    third.unmount();
    refuse(new ApiProblem(401, "unauthorized", ""));
    await settle();
    expect(invalidate).not.toHaveBeenCalledWith({ queryKey: ME_QUERY_KEY });
  });

  it("an ignored engram keeps its detail and graph keys while the rest of the row still fires", () => {
    mount(client, <Editor />);
    const source = theSource();
    act(() => {
      source.emit("engram", engram("alpha"), "1:1");
      vi.advanceTimersByTime(250);
    });
    const keys = invalidatedKeys();
    expect(keys).not.toContain(JSON.stringify(["engram", "eng", "alpha"]));
    expect(keys).not.toContain(JSON.stringify(["graph", "eng", "alpha"]));
    expect(keys).toContain(JSON.stringify(["domain-tree", "eng"]));
    void IgnoredEngramsContext;
  });

  it("the exemption ends with the editor: a closed editor's engram refetches again", () => {
    // Catches a registration that is never released, which would leave the
    // engram's detail stale on the reading page for the life of the tab.
    const view = mount(client, <Editor />);
    view.rerenderWith(<div />);
    act(() => {
      theSource().emit("engram", engram("alpha"), "1:1");
      vi.advanceTimersByTime(250);
    });
    expect(invalidatedKeys()).toContain(
      JSON.stringify(["engram", "eng", "alpha"]),
    );
    expect(invalidatedKeys()).toContain(
      JSON.stringify(["graph", "eng", "alpha"]),
    );
  });

  it("two registrations of one engram hold it until both are gone", () => {
    const view = mount(
      client,
      <>
        <Editor />
        <Editor />
      </>,
    );
    const emitAndFlush = () => {
      invalidate.mockClear();
      act(() => {
        theSource().emit("engram", engram("alpha"), "1:1");
        vi.advanceTimersByTime(250);
      });
      return invalidatedKeys();
    };
    view.rerenderWith(<Editor />);
    expect(emitAndFlush(), "one left, still exempt").not.toContain(
      JSON.stringify(["engram", "eng", "alpha"]),
    );
    view.rerenderWith(<div />);
    expect(emitAndFlush(), "none left").toContain(
      JSON.stringify(["engram", "eng", "alpha"]),
    );
  });

  it("an editor that opens inside the window is exempt from the frame that came just before it", () => {
    // Catches the exemption decided when a frame is queued rather than when
    // the window flushes.
    const view = mount(client);
    act(() => {
      theSource().emit("engram", engram("alpha"), "1:1");
    });
    view.rerenderWith(<Editor />);
    act(() => {
      vi.advanceTimersByTime(250);
    });
    const keys = invalidatedKeys();
    expect(keys).not.toContain(JSON.stringify(["engram", "eng", "alpha"]));
    expect(keys).not.toContain(JSON.stringify(["graph", "eng", "alpha"]));
    expect(keys).toContain(JSON.stringify(["domain-tree", "eng"]));
  });

  it("an engram frame lands in the recent store the reading page reads", () => {
    // Catches the provider no longer noting frames: the status line and the
    // follow on a move would never appear.
    function Line() {
      const recent = useRecentChange("eng", "alpha");
      return <p>{recent ? `${recent.kind} ${recent.actor ?? ""}` : "none"}</p>;
    }
    const view = mount(client, <Line />);
    expect(view.container.textContent).toBe("none");
    act(() => {
      theSource().emit("engram", { ...engram("alpha"), actor: "ada" }, "1:1");
    });
    expect(view.container.textContent).toBe("updated ada");
  });

  it("a domain event's engram and graph prefixes pass over an ignored engram and reach every other one", () => {
    // Catches a domain event (a pull collapsed past the threshold, a
    // MANIFEST save) refetching the open editor's detail under its room:
    // the exemption stands while the registration does, whatever the event.
    mount(client, <Editor />);
    act(() => {
      theSource().emit("domain", { domain: "eng", actor: null }, "1:1");
      vi.advanceTimersByTime(250);
    });
    const query = (queryKey: readonly unknown[]) =>
      ({ queryKey }) as unknown as Query;
    for (const prefix of ["engram", "graph"]) {
      const filter = filters().find(
        (f) => JSON.stringify(f?.queryKey) === JSON.stringify([prefix, "eng"]),
      );
      const predicate = filter?.predicate;
      expect(predicate, prefix).toBeTypeOf("function");
      expect(predicate?.(query([prefix, "eng", "alpha", 1])), prefix).toBe(
        false,
      );
      expect(predicate?.(query([prefix, "eng", "beta", 1])), prefix).toBe(true);
    }
    expect(invalidatedKeys()).toContain(JSON.stringify(["domain-tree", "eng"]));
  });

  it("subscribeToChanges shares the one source: every frame reaches a listener registered outside the provider (Jordi, 2026-09-27, Section J (m))", () => {
    const seen: string[] = [];
    const unsubscribe = subscribeToChanges((event) => seen.push(event.event));
    mount(client);
    expect(FakeEventSource.instances, "no second connection").toHaveLength(1);
    const source = theSource();
    act(() => {
      source.emit("engram", engram("alpha"), "1:1");
      source.emit("domain", { domain: "eng", actor: null }, "1:2");
    });
    expect(seen).toEqual(["engram", "domain"]);
    unsubscribe();
    act(() => {
      source.emit("reset", {});
    });
    expect(seen, "unsubscribed, so the reset never reaches it").toEqual([
      "engram",
      "domain",
    ]);
  });

  it("a listener that throws neither stops the others nor the invalidation", () => {
    // Catches one subscriber's bug (the game's, later) taking the page's own
    // refresh down with it.
    const error = vi
      .spyOn(console, "error")
      .mockImplementation(() => undefined);
    const seen: string[] = [];
    const first = subscribeToChanges(() => {
      throw new Error("boom");
    });
    const second = subscribeToChanges((event) => seen.push(event.event));
    mount(client);
    act(() => {
      theSource().emit("engram", engram("alpha"), "1:1");
      vi.advanceTimersByTime(250);
    });
    expect(seen).toEqual(["engram"]);
    expect(invalidatedKeys()).toContain(
      JSON.stringify(["engram", "eng", "alpha"]),
    );
    first();
    second();
    error.mockRestore();
  });
});
