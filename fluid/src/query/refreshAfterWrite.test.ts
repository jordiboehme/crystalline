import { QueryClient, QueryObserver } from "@tanstack/react-query";
import { afterEach, describe, expect, it } from "vitest";

import { refreshAfterWrite } from "./refreshAfterWrite";

/** A promise a test settles by hand, so it decides when a read answers. */
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((settle) => {
    resolve = settle;
  });
  return { promise, resolve };
}

/**
 * A server list whose reads take their snapshot when they start and answer
 * only when the test lets them, which is how a slow GET behaves on the wire.
 */
function slowList() {
  const rows: string[] = [];
  const pending: (() => void)[] = [];
  let reads = 0;
  return {
    rows,
    reads: () => reads,
    read: () => {
      reads += 1;
      const snapshot = [...rows];
      const answer = deferred<string[]>();
      pending.push(() => answer.resolve(snapshot));
      return answer.promise;
    },
    /** Let every read on the wire answer with the snapshot it took. */
    flush: () => {
      for (const answer of pending.splice(0)) {
        answer();
      }
    },
  };
}

const KEY = ["rows"] as const;
let client: QueryClient;
const unsubscribes: (() => void)[] = [];

function watch(read: () => Promise<string[]>) {
  const observer = new QueryObserver(client, { queryKey: KEY, queryFn: read });
  unsubscribes.push(observer.subscribe(() => {}));
  return observer;
}

afterEach(() => {
  for (const unsubscribe of unsubscribes.splice(0)) {
    unsubscribe();
  }
  client.clear();
});

describe("refreshAfterWrite", () => {
  it("reads again when the write lands during the first read, where a bare invalidate joins it", async () => {
    client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const list = slowList();
    watch(list.read);
    expect(list.reads()).toBe(1);

    // The write commits while the first read, taken before it, is on the wire.
    list.rows.push("smoke-token");
    const settled = refreshAfterWrite(client, { queryKey: KEY });
    list.flush();
    await settled;

    expect(list.reads()).toBe(2);
    expect(client.getQueryData(KEY)).toEqual(["smoke-token"]);
  });

  it("shows the race it exists for: a bare invalidate keeps the old snapshot", async () => {
    client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const list = slowList();
    watch(list.read);

    list.rows.push("smoke-token");
    const settled = client.invalidateQueries({ queryKey: KEY });
    list.flush();
    await settled;

    // Pinned so a query-core upgrade that fixes this upstream is noticed and
    // the helper can be reconsidered.
    expect(list.reads()).toBe(1);
    expect(client.getQueryData(KEY)).toEqual([]);
  });

  it("restarts a refetch of a query with data once, as a bare invalidate does", async () => {
    client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const list = slowList();
    const observer = watch(list.read);
    list.flush();
    await expect.poll(() => client.getQueryData(KEY)).toEqual([]);

    let waited: string[] | undefined;
    const refetched = observer.refetch().then((result) => {
      waited = result.data;
    });
    expect(list.reads()).toBe(2);
    list.rows.push("smoke-token");
    const settled = refreshAfterWrite(client, { queryKey: KEY });
    // A turn of the event loop before the reads answer: a reverting cancel
    // would hand the waiting refetch the old rows in that gap.
    await new Promise((resolve) => setTimeout(resolve, 0));
    list.flush();
    await settled;
    await refetched;

    expect(list.reads()).toBe(3);
    expect(client.getQueryData(KEY)).toEqual(["smoke-token"]);
    // The silent restart a bare invalidate does: whoever waited on the
    // replaced read gets the new one's answer, not the rows from before.
    expect(waited).toEqual(["smoke-token"]);
  });

  it("leaves a fetchQuery under the same key prefix alone when nothing shows it", async () => {
    client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const answer = deferred<string>();
    const read = client.fetchQuery({
      queryKey: [...KEY, "detail"],
      queryFn: () => answer.promise,
    });

    void refreshAfterWrite(client, { queryKey: KEY });
    answer.resolve("kept");

    await expect(read).resolves.toBe("kept");
  });

  it("settles only once the new read has answered", async () => {
    client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const list = slowList();
    watch(list.read);

    list.rows.push("smoke-token");
    let done = false;
    const settled = refreshAfterWrite(client, { queryKey: KEY }).then(() => {
      done = true;
    });
    // Cancelled or not, the first read is not what the caller waits on.
    await Promise.resolve();
    await Promise.resolve();
    expect(done).toBe(false);

    list.flush();
    await settled;
    expect(done).toBe(true);
    expect(client.getQueryData(KEY)).toEqual(["smoke-token"]);
  });

  it("only marks the queries stale when nothing is to be read again", async () => {
    client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const list = slowList();
    watch(list.read);

    list.rows.push("smoke-token");
    await refreshAfterWrite(client, { queryKey: KEY, refetchType: "none" });

    expect(list.reads()).toBe(1);
    expect(client.getQueryState(KEY)?.fetchStatus).toBe("fetching");
    expect(client.getQueryState(KEY)?.isInvalidated).toBe(true);
  });
});
