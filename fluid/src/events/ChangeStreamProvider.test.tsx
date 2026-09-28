import type { InvalidateQueryFilters, Query } from "@tanstack/react-query";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, render } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ApiProblem } from "../api/client";
import { ME_QUERY_KEY } from "../auth/keys";
import {
  ChangeStreamProvider,
  subscribeToChanges,
} from "./ChangeStreamProvider";
import { CLAIM_MS, LEADER_TIMEOUT_MS } from "./election";
import type { HubDeps, SessionProbe } from "./hub";
import { ChangeHub, STREAM_NAME } from "./hub";
import { IgnoredEngramsContext, useIgnoredEngram } from "./ignored";
import { useRecentChange } from "./recent";
import {
  FakeBroadcastChannel,
  FakeChannelBus,
  FakeEventSource,
  FakeLocks,
  fakeStreamFactory,
} from "./testSupport";

/** A probe that answers "still signed in as ada". */
const signedIn: SessionProbe = () =>
  Promise.resolve({ user: { name: "ada" }, anonymous: false });

/**
 * One fake browser: its tabs share a lock manager (or none) and a channel
 * bus. Each `tab` is that tab's hub.
 */
function browser({ locks = true }: { locks?: boolean } = {}) {
  const bus = new FakeChannelBus();
  const lockManager = locks ? new FakeLocks() : null;
  return {
    bus,
    locks: lockManager,
    tab(deps: HubDeps = {}): ChangeHub {
      return new ChangeHub({
        streamFactory: fakeStreamFactory,
        sessionProbe: signedIn,
        locks: lockManager,
        channel: (name) => new FakeBroadcastChannel(name, bus),
        random: () => 0.5,
        ...deps,
      });
    },
  };
}

/** A lone tab, the shape most cases here need. */
function lone(deps: HubDeps = {}): ChangeHub {
  return browser().tab(deps);
}

function tree(client: QueryClient, children: ReactNode, hub: ChangeHub) {
  return (
    <QueryClientProvider client={client}>
      <ChangeStreamProvider hub={hub}>{children}</ChangeStreamProvider>
    </QueryClientProvider>
  );
}

/** Let promise continuations run (a lock grant, a probe), clock untouched. */
async function settle(): Promise<void> {
  await act(async () => {
    for (let i = 0; i < 20; i++) await Promise.resolve();
  });
}

/** Move the clock inside act, microtasks included. */
async function advance(ms: number): Promise<void> {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
  await settle();
}

async function mount(
  client: QueryClient,
  children: ReactNode = <div />,
  hub: ChangeHub = lone(),
) {
  const view = render(tree(client, children, hub));
  await settle();
  return {
    ...view,
    rerenderWith: (next: ReactNode) => {
      view.rerender(tree(client, next, hub));
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

/** The first source any tab opened. */
function theSource(): FakeEventSource {
  return sourceAt(0);
}

/** The `index`th source, which must exist. */
function sourceAt(index: number): FakeEventSource {
  const source = FakeEventSource.instances[index];
  if (!source) throw new Error(`no source ${String(index)}`);
  return source;
}

/** The sources still open, across every tab. */
function openSources(): FakeEventSource[] {
  return FakeEventSource.instances.filter((source) => source.readyState !== 2);
}

/** A query client whose invalidations are recorded rather than run. */
function recordingClient() {
  const client = new QueryClient();
  const invalidate = vi.fn().mockResolvedValue(undefined);
  client.invalidateQueries = invalidate as QueryClient["invalidateQueries"];
  const filters = () =>
    invalidate.mock.calls.map(
      (call) => call[0] as InvalidateQueryFilters | undefined,
    );
  return {
    client,
    invalidate,
    filters,
    keys: () => filters().map((filter) => JSON.stringify(filter?.queryKey)),
    /** How often everything was invalidated at once. */
    resets: () =>
      invalidate.mock.calls.filter((call) => call.length === 0).length,
  };
}

beforeEach(() => {
  vi.useFakeTimers();
  FakeEventSource.instances = [];
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

describe("the change stream in one tab", () => {
  let client: QueryClient;
  let invalidate: ReturnType<typeof vi.fn>;
  let filters: () => (InvalidateQueryFilters | undefined)[];
  let invalidatedKeys: () => string[];

  beforeEach(() => {
    const recording = recordingClient();
    client = recording.client;
    invalidate = recording.invalidate;
    filters = recording.filters;
    invalidatedKeys = recording.keys;
  });

  it("opens one source per mount and closes it on unmount", async () => {
    const view = await mount(client);
    expect(FakeEventSource.instances).toHaveLength(1);
    expect(theSource().url).toBe("/api/v1/events");
    view.unmount();
    expect(theSource().readyState).toBe(2);
  });

  it("an unmount inside the window drops the pending invalidations", async () => {
    // Catches a cleanup that leaves the coalescing timer running: a late
    // invalidation from a provider that is gone.
    const view = await mount(client);
    act(() => {
      theSource().emit("engram", engram("alpha"), "1:1");
    });
    view.unmount();
    act(() => {
      vi.advanceTimersByTime(250);
    });
    expect(invalidate).not.toHaveBeenCalled();
  });

  it("coalesces five frames inside the window into one invalidation per distinct key", async () => {
    await mount(client);
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

  it("a reset flushes the pending set and invalidates everything at once", async () => {
    await mount(client);
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

  it("the first source to open resets nothing: there was no gap before it", async () => {
    await mount(client);
    act(() => {
      theSource().open();
    });
    expect(invalidate).not.toHaveBeenCalled();
  });

  it("a source that reconnects on its own is left to the browser", async () => {
    const probe = vi.fn(signedIn);
    await mount(client, <div />, lone({ sessionProbe: probe }));
    act(() => {
      theSource().fail(0);
    });
    await advance(60_000);
    expect(probe).not.toHaveBeenCalled();
    expect(invalidate).not.toHaveBeenCalled();
    expect(FakeEventSource.instances).toHaveLength(1);
  });

  it("a source closed by a 502 during a restart asks the probe and reopens with backoff, then resets", async () => {
    // A closed source is not proof the session ended: a proxy's 502 closes
    // it too. The probe decides, and a new source cannot send the last id
    // it saw, so it resets once it opens.
    const seen: string[] = [];
    // The daemon is still down at the first probe and up at the second.
    const probe = vi
      .fn<SessionProbe>()
      .mockRejectedValueOnce(new ApiProblem(502, "bad gateway", ""))
      .mockImplementation(signedIn);
    const hub = lone({ sessionProbe: probe });
    const unsubscribe = hub.subscribe((event) => seen.push(event.event));
    await mount(client, <div />, hub);
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
    await mount(client);
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
    await mount(client, <div />, lone({ sessionProbe: probe }));
    act(() => {
      theSource().fail(2);
    });
    await settle();
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ME_QUERY_KEY });
    await advance(120_000);
    expect(FakeEventSource.instances).toHaveLength(1);
    expect(probe).toHaveBeenCalledTimes(1);
  });

  it("a probe that names nobody is a session that ended", async () => {
    // The probe answers 200 with no identity for an ended cookie session.
    client.setQueryData(ME_QUERY_KEY, { user: { name: "ada" } });
    await mount(
      client,
      <div />,
      lone({
        sessionProbe: () => Promise.resolve({ user: null, anonymous: false }),
      }),
    );
    act(() => {
      theSource().fail(2);
    });
    await settle();
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ME_QUERY_KEY });
    await advance(60_000);
    expect(FakeEventSource.instances).toHaveLength(1);
  });

  it("a probe that names another account is no ended session: the tab re-asks who is signed in and the stream reopens as that account", async () => {
    // Catches a switched account read as a logout, which left every tab
    // without live refresh until a reload.
    client.setQueryData(ME_QUERY_KEY, { user: { name: "ada" } });
    for (const answer of [
      { user: { name: "bob" }, anonymous: false },
      { user: null, anonymous: true },
    ]) {
      cleanup();
      invalidate.mockClear();
      FakeEventSource.instances = [];
      await mount(
        client,
        <div />,
        lone({ sessionProbe: () => Promise.resolve(answer) }),
      );
      act(() => {
        theSource().fail(2);
      });
      await settle();
      expect(invalidate, JSON.stringify(answer)).toHaveBeenCalledWith({
        queryKey: ME_QUERY_KEY,
      });
      await advance(1_000);
      expect(FakeEventSource.instances, JSON.stringify(answer)).toHaveLength(2);
    }
  });

  it("an unmount while the probe is out or the backoff runs never opens a source", async () => {
    let answer: (value: unknown) => void = () => undefined;
    const view = await mount(
      client,
      <div />,
      lone({
        sessionProbe: () =>
          new Promise((resolve) => {
            answer = resolve;
          }),
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
    const second = await mount(client);
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
    const third = await mount(
      client,
      <div />,
      lone({
        sessionProbe: () =>
          new Promise((_resolve, reject) => {
            refuse = reject;
          }),
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

  it("an ignored engram keeps its detail and graph keys while the rest of the row still fires", async () => {
    await mount(client, <Editor />);
    act(() => {
      theSource().emit("engram", engram("alpha"), "1:1");
      vi.advanceTimersByTime(250);
    });
    const keys = invalidatedKeys();
    expect(keys).not.toContain(JSON.stringify(["engram", "eng", "alpha"]));
    expect(keys).not.toContain(JSON.stringify(["graph", "eng", "alpha"]));
    expect(keys).toContain(JSON.stringify(["domain-tree", "eng"]));
    void IgnoredEngramsContext;
  });

  it("the exemption ends with the editor: a closed editor's engram refetches again", async () => {
    // Catches a registration that is never released, which would leave the
    // engram's detail stale on the reading page for the life of the tab.
    const view = await mount(client, <Editor />);
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

  it("two registrations of one engram hold it until both are gone", async () => {
    const view = await mount(
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

  it("an editor that opens inside the window is exempt from the frame that came just before it", async () => {
    // Catches the exemption decided when a frame is queued rather than when
    // the window flushes.
    const view = await mount(client);
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

  it("an engram frame lands in the recent store the reading page reads", async () => {
    // Catches the provider no longer noting frames: the status line and the
    // follow on a move would never appear.
    function Line() {
      const recent = useRecentChange("eng", "alpha");
      return <p>{recent ? `${recent.kind} ${recent.actor ?? ""}` : "none"}</p>;
    }
    const view = await mount(client, <Line />);
    expect(view.container.textContent).toBe("none");
    act(() => {
      theSource().emit("engram", { ...engram("alpha"), actor: "ada" }, "1:1");
    });
    expect(view.container.textContent).toBe("updated ada");
  });

  it("a domain event's engram and graph prefixes pass over an ignored engram and reach every other one", async () => {
    // Catches a domain event (a pull collapsed past the threshold, a
    // MANIFEST save) refetching the open editor's detail under its room:
    // the exemption stands while the registration does, whatever the event.
    await mount(client, <Editor />);
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

  it("a listener that throws neither stops the others nor the invalidation", async () => {
    // Catches one subscriber's bug taking the page's own refresh down with
    // it.
    const error = vi
      .spyOn(console, "error")
      .mockImplementation(() => undefined);
    const seen: string[] = [];
    const hub = lone();
    const first = hub.subscribe(() => {
      throw new Error("boom");
    });
    const second = hub.subscribe((event) => seen.push(event.event));
    await mount(client, <div />, hub);
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

describe("subscribeToChanges", () => {
  it("shares this tab's one source with the shell: every frame reaches a listener registered outside the provider (Jordi, 2026-09-27)", async () => {
    // The module's own hub, on the setup's fake Web Locks and channel.
    vi.stubGlobal("EventSource", FakeEventSource);
    const { client } = recordingClient();
    const seen: string[] = [];
    const unsubscribe = subscribeToChanges((event) => seen.push(event.event));
    await settle();
    expect(FakeEventSource.instances, "a listener alone opens it").toHaveLength(
      1,
    );
    const view = render(
      <QueryClientProvider client={client}>
        <ChangeStreamProvider>
          <div />
        </ChangeStreamProvider>
      </QueryClientProvider>,
    );
    await settle();
    expect(FakeEventSource.instances, "no second connection").toHaveLength(1);
    const source = theSource();
    expect(source.withCredentials, "the session cookie rides along").toBe(true);
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
    view.unmount();
    await settle();
    expect(source.readyState, "nobody wants it any more").toBe(2);
  });
});

describe("one stream per browser", () => {
  it("two tabs share one source, and every frame reaches both", async () => {
    const fake = browser();
    const first = recordingClient();
    const second = recordingClient();
    const seen: string[] = [];
    const hubA = fake.tab();
    const hubB = fake.tab();
    await mount(first.client, <div />, hubA);
    const unsubscribe = hubB.subscribe((event) => seen.push(`${event.event}`));
    await mount(second.client, <div />, hubB);
    expect(FakeEventSource.instances, "one connection for both").toHaveLength(
      1,
    );
    act(() => {
      theSource().emit("engram", engram("alpha"), "1:1");
    });
    await settle();
    act(() => {
      vi.advanceTimersByTime(250);
    });
    for (const tab of [first, second]) {
      expect(tab.keys()).toContain(JSON.stringify(["engram", "eng", "alpha"]));
      expect(tab.keys()).toContain(JSON.stringify(["domain-tree", "eng"]));
    }
    expect(seen, "a listener in the tab without the source").toEqual([
      "engram",
    ]);
    unsubscribe();
  });

  it("a tab with a listener and no shell follows the leader, and leads when it is alone", async () => {
    const fake = browser();
    const shell = recordingClient();
    const view = await mount(shell.client, <div />, fake.tab());
    const seen: string[] = [];
    const unsubscribe = fake.tab().subscribe((event) => seen.push(event.event));
    await settle();
    act(() => {
      theSource().emit("domain", { domain: "eng", actor: null }, "1:1");
    });
    await settle();
    expect(seen).toEqual(["domain"]);
    view.unmount();
    await settle();
    expect(openSources(), "the listener's tab took over").toHaveLength(1);
    expect(FakeEventSource.instances).toHaveLength(2);
    unsubscribe();
  });

  it("when the leading tab closes another takes over, and every tab resets once", async () => {
    const fake = browser();
    const tabs = [recordingClient(), recordingClient(), recordingClient()];
    const hubs = tabs.map(() => fake.tab());
    const resets = hubs.map(() => 0);
    const leaves = hubs.map((hub, index) =>
      hub.subscribe((event) => {
        if (event.event === "reset") resets[index] = (resets[index] ?? 0) + 1;
      }),
    );
    const views = [];
    for (const [index, tab] of tabs.entries()) {
      views.push(await mount(tab.client, <div />, hubs[index]));
    }
    expect(FakeEventSource.instances).toHaveLength(1);
    act(() => {
      theSource().open();
    });
    await settle();
    expect(
      tabs.map((tab) => tab.resets()),
      "the first leader had no gap",
    ).toEqual([0, 0, 0]);

    views[0]?.unmount();
    leaves[0]?.();
    await settle();
    expect(theSource().readyState, "the old leader's source is gone").toBe(2);
    expect(openSources(), "exactly one source again").toHaveLength(1);
    expect(FakeEventSource.instances).toHaveLength(2);
    act(() => {
      sourceAt(1).open();
    });
    await settle();
    expect(
      tabs.slice(1).map((tab) => tab.resets()),
      "each surviving tab refetches once",
    ).toEqual([1, 1]);
    expect(resets.slice(1), "and each tab's listeners hear one reset").toEqual([
      1, 1,
    ]);
    for (const leave of leaves.slice(1)) leave();
  });

  it("an ended session reaches every tab, and none of them reopens", async () => {
    const fake = browser();
    const unauthorized = () =>
      Promise.reject(new ApiProblem(401, "unauthorized", ""));
    const first = recordingClient();
    const second = recordingClient();
    await mount(
      first.client,
      <div />,
      fake.tab({ sessionProbe: unauthorized }),
    );
    const secondHub = fake.tab({ sessionProbe: unauthorized });
    await mount(second.client, <div />, secondHub);
    act(() => {
      theSource().fail(2);
    });
    await settle();
    for (const tab of [first, second]) {
      expect(tab.invalidate).toHaveBeenCalledWith({ queryKey: ME_QUERY_KEY });
    }
    await advance(120_000);
    expect(
      FakeEventSource.instances,
      "no tab takes the lock to hear a 401 again",
    ).toHaveLength(1);
    expect(fake.locks?.isHeld(STREAM_NAME)).toBe(false);
    // A listener added while the session is over does not bring it back.
    const leave = secondHub.subscribe(() => undefined);
    await advance(60_000);
    expect(FakeEventSource.instances, "still out of the running").toHaveLength(
      1,
    );
    // The shell mounting again is a new sign-in: that tab takes part again.
    await mount(recordingClient().client, <div />, secondHub);
    expect(FakeEventSource.instances, "back after a sign-in").toHaveLength(2);
    leave();
  });

  describe("when the tabs disagree about who is signed in", () => {
    const me = (name: string) => ({ user: { name } });

    /** Two tabs of one browser whose cookie now belongs to `cookie`. */
    async function twoTabs(leaderShows: string, followerShows: string) {
      const fake = browser();
      let cookie = "bob";
      const probe: SessionProbe = () => Promise.resolve(me(cookie));
      const leader = recordingClient();
      const follower = recordingClient();
      leader.client.setQueryData(ME_QUERY_KEY, me(leaderShows));
      follower.client.setQueryData(ME_QUERY_KEY, me(followerShows));
      const leaderHub = fake.tab({ sessionProbe: probe });
      const followerHub = fake.tab({ sessionProbe: probe });
      const leaderView = await mount(leader.client, <div />, leaderHub);
      const heard: string[] = [];
      const leave = followerHub.subscribe((event) => heard.push(event.event));
      const followerView = await mount(follower.client, <div />, followerHub);
      return {
        fake,
        leader,
        follower,
        leaderHub,
        followerHub,
        leaderView,
        followerView,
        heard,
        leave,
        setCookie: (name: string) => {
          cookie = name;
        },
      };
    }

    /** Emit one frame on `source` and let every tab's window flush. */
    async function frameOn(source: FakeEventSource, permalink: string) {
      act(() => {
        source.emit("engram", engram(permalink), "9:1");
      });
      await settle();
      act(() => {
        vi.advanceTimersByTime(250);
      });
      await settle();
    }

    it("a leader whose shell is stale reconnects as the cookie's account and the other tab keeps its live refresh", async () => {
      // The reviewer's case: the leader shows ada, the follower bob, the
      // cookie is bob's, and the leader's source closes during a restart.
      const tabs = await twoTabs("ada", "bob");
      act(() => {
        theSource().fail(2);
      });
      await settle();
      expect(
        tabs.leader.invalidate,
        "the stale tab re-asks who is signed in",
      ).toHaveBeenCalledWith({ queryKey: ME_QUERY_KEY });
      expect(
        tabs.follower.invalidate,
        "the follower's session did not end",
      ).not.toHaveBeenCalledWith({ queryKey: ME_QUERY_KEY });
      await advance(1_000);
      expect(FakeEventSource.instances, "reopened as bob").toHaveLength(2);
      act(() => {
        sourceAt(1).open();
      });
      await settle();
      expect(tabs.follower.resets(), "bob's tab resets once").toBe(1);
      expect(
        tabs.leader.resets(),
        "the stale tab dropped the reset streamed as bob, and resets on its own",
      ).toBe(1);
      tabs.follower.invalidate.mockClear();
      await frameOn(sourceAt(1), "alpha");
      expect(tabs.follower.keys()).toContain(
        JSON.stringify(["engram", "eng", "alpha"]),
      );
      expect(tabs.heard).toEqual(["reset", "engram"]);
      tabs.leave();
    });

    it("a frame streamed as ada never reaches bob's tab, and bob's tab gets the leader to reconnect as bob", async () => {
      const tabs = await twoTabs("ada", "bob");
      await frameOn(theSource(), "alpha");
      expect(tabs.follower.keys(), "no query of bob's tab").not.toContain(
        JSON.stringify(["engram", "eng", "alpha"]),
      );
      expect(tabs.heard, "no listener of bob's tab").toEqual([]);
      expect(tabs.leader.keys(), "ada's own tab still takes it").toContain(
        JSON.stringify(["engram", "eng", "alpha"]),
      );
      // Bob's tab asked the probe, found the leader stale and asked it to
      // re-check; the leader found bob and reopened.
      expect(FakeEventSource.instances).toHaveLength(2);
      expect(theSource().readyState).toBe(2);
      act(() => {
        sourceAt(1).open();
      });
      await settle();
      expect(tabs.follower.resets()).toBe(1);
      tabs.follower.invalidate.mockClear();
      await frameOn(sourceAt(1), "beta");
      expect(tabs.follower.keys()).toContain(
        JSON.stringify(["engram", "eng", "beta"]),
      );
      expect(tabs.heard).toEqual(["reset", "engram"]);
      tabs.leave();
    });

    it("a logout and a new sign-in in another tab: the old account's frames stop at that tab until the leader follows", async () => {
      const tabs = await twoTabs("ada", "ada");
      tabs.setCookie("ada");
      await frameOn(theSource(), "alpha");
      expect(tabs.follower.keys()).toContain(
        JSON.stringify(["engram", "eng", "alpha"]),
      );
      // In the follower's tab, ada logs out: its shell goes. Bob signs in
      // there, and the shell comes back showing bob.
      tabs.followerView.unmount();
      tabs.leave();
      tabs.setCookie("bob");
      const bob = recordingClient();
      bob.client.setQueryData(ME_QUERY_KEY, me("bob"));
      const heard: string[] = [];
      const leave = tabs.followerHub.subscribe((event) =>
        heard.push(event.event),
      );
      await mount(bob.client, <div />, tabs.followerHub);
      expect(FakeEventSource.instances, "still the one stream").toHaveLength(1);
      await frameOn(theSource(), "gamma");
      expect(bob.keys()).not.toContain(
        JSON.stringify(["engram", "eng", "gamma"]),
      );
      expect(heard).toEqual([]);
      expect(
        FakeEventSource.instances,
        "the leader reopened as bob",
      ).toHaveLength(2);
      act(() => {
        sourceAt(1).open();
      });
      await settle();
      expect(bob.resets()).toBe(1);
      bob.invalidate.mockClear();
      await frameOn(sourceAt(1), "delta");
      expect(bob.keys()).toContain(JSON.stringify(["engram", "eng", "delta"]));
      expect(heard).toEqual(["reset", "engram"]);
      leave();
    });
  });

  it("unmounting every tab lets go of the lock, the channels and the source", async () => {
    const fake = browser();
    const views = [
      await mount(recordingClient().client, <div />, fake.tab()),
      await mount(recordingClient().client, <div />, fake.tab()),
    ];
    expect(fake.locks?.isHeld(STREAM_NAME)).toBe(true);
    for (const view of views) view.unmount();
    await settle();
    expect(fake.locks?.isHeld(STREAM_NAME)).toBe(false);
    expect(fake.bus.channels.size).toBe(0);
    expect(openSources()).toHaveLength(0);
    expect(vi.getTimerCount()).toBe(0);
  });

  describe("without Web Locks", () => {
    it("elects one leader over the channel, hands over when it leaves, and resets once", async () => {
      const fake = browser({ locks: false });
      const first = recordingClient();
      const second = recordingClient();
      const viewA = await mount(
        first.client,
        <div />,
        fake.tab({ random: () => 0.1 }),
      );
      await advance(CLAIM_MS);
      expect(FakeEventSource.instances, "a lone tab leads").toHaveLength(1);
      await mount(second.client, <div />, fake.tab({ random: () => 0.2 }));
      await advance(CLAIM_MS);
      await advance(LEADER_TIMEOUT_MS * 2);
      expect(FakeEventSource.instances, "the second tab follows").toHaveLength(
        1,
      );
      act(() => {
        theSource().emit("engram", engram("alpha"), "1:1");
      });
      await settle();
      act(() => {
        vi.advanceTimersByTime(250);
      });
      expect(second.keys()).toContain(
        JSON.stringify(["engram", "eng", "alpha"]),
      );

      viewA.unmount();
      await settle();
      await advance(CLAIM_MS);
      expect(openSources(), "the follower took over").toHaveLength(1);
      expect(FakeEventSource.instances).toHaveLength(2);
      act(() => {
        sourceAt(1).open();
      });
      await settle();
      expect(second.resets()).toBe(1);
    });

    it("takes over from a leader that went silent without saying so", async () => {
      // A closed tab runs no cleanup: its heartbeats just stop.
      const fake = browser({ locks: false });
      await mount(
        recordingClient().client,
        <div />,
        fake.tab({ random: () => 0.1 }),
      );
      await advance(CLAIM_MS);
      const [leaderElection] = [...fake.bus.channels].filter((channel) =>
        channel.name.endsWith(":election"),
      );
      if (!leaderElection) throw new Error("no election channel");
      const second = recordingClient();
      await mount(second.client, <div />, fake.tab({ random: () => 0.2 }));
      await advance(CLAIM_MS);
      await advance(LEADER_TIMEOUT_MS);
      expect(FakeEventSource.instances).toHaveLength(1);
      leaderElection.postMessage = () => undefined;
      await advance(LEADER_TIMEOUT_MS + CLAIM_MS);
      expect(
        FakeEventSource.instances,
        "the follower claimed the silent leader's place",
      ).toHaveLength(2);
      act(() => {
        sourceAt(1).open();
      });
      await settle();
      expect(second.resets()).toBe(1);
    });
  });
});
