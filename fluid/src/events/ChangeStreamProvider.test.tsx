import type { InvalidateQueryFilters, Query } from "@tanstack/react-query";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, render } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ME_QUERY_KEY } from "../auth/keys";
import {
  ChangeStreamProvider,
  subscribeToChanges,
} from "./ChangeStreamProvider";
import { IgnoredEngramsContext, useIgnoredEngram } from "./ignored";
import { FakeEventSource, fakeStreamFactory } from "./testSupport";

function mount(client: QueryClient, children: ReactNode = <div />) {
  return render(
    <QueryClientProvider client={client}>
      <ChangeStreamProvider streamFactory={fakeStreamFactory}>
        {children}
      </ChangeStreamProvider>
    </QueryClientProvider>,
  );
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

/** The one source the mounted provider opened. */
function theSource(): FakeEventSource {
  const source = FakeEventSource.instances[0];
  if (!source) throw new Error("no source was opened");
  return source;
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
    vi.useRealTimers();
  });

  it("opens one source per mount with credentials and closes it on unmount", () => {
    const view = mount(client);
    expect(FakeEventSource.instances).toHaveLength(1);
    expect(theSource().url).toBe("/api/v1/events");
    view.unmount();
    expect(theSource().readyState).toBe(2);
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

  it("a closed source invalidates the capability probe, a reconnecting one does nothing", () => {
    mount(client);
    const source = theSource();
    act(() => {
      source.fail(0);
    });
    expect(invalidate).not.toHaveBeenCalled();
    act(() => {
      source.fail(2);
    });
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ME_QUERY_KEY });
  });

  it("an ignored engram keeps its detail and graph keys while the rest of the row still fires", () => {
    function Editor() {
      useIgnoredEngram("eng", "alpha");
      return null;
    }
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

  it("a domain event's engram and graph prefixes pass over an ignored engram and reach every other one", () => {
    // Catches a domain event (a pull collapsed past the threshold, a
    // MANIFEST save) refetching the open editor's detail under its room:
    // the exemption stands while the registration does, whatever the event.
    function Editor() {
      useIgnoredEngram("eng", "alpha");
      return null;
    }
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
