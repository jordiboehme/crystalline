/**
 * The boundary every lazy part of Fluid waits behind: `Suspense` with the
 * part's own loading shape while its chunk is on the way, and a short
 * message with a reload button if the chunk never arrives.
 *
 * Only a {@link ChunkLoadError} is caught (see `lazyChunk.ts`). Any other
 * error is thrown on from here, so a bug in a screen still fails a test and
 * still reaches the console the way it did before this boundary existed.
 * React keeps a failed lazy import's rejection for the life of the page, so
 * trying again in place cannot work; a reload fetches the new `index.html`
 * and the chunk names that go with it.
 */

import { Component, Suspense } from "react";
import type { ReactNode } from "react";
import { useLocation } from "react-router";

import { ChunkLoadError } from "../lazyChunk";
import { BUTTON } from "./primitives";

export function LazyBoundary({
  fallback,
  failed,
  children,
}: {
  fallback: ReactNode;
  /** What stands in when the chunk never arrives; the reload prompt by default. */
  failed?: ReactNode;
  children: ReactNode;
}) {
  const { pathname } = useLocation();
  return (
    <ChunkErrorBoundary resetKey={pathname} failed={failed}>
      <Suspense fallback={fallback}>{children}</Suspense>
    </ChunkErrorBoundary>
  );
}

/**
 * The catching half. Every route draws its `LazyBoundary` at the same place
 * under the frame's outlet, and the domain page draws its cards at the same
 * place for every domain, so React keeps one instance of this across a
 * move. A failure is therefore remembered only for the path it happened on:
 * when `resetKey` (the pathname) changes, the error is dropped before the
 * next render, and the new route draws its own content. A key on the
 * boundary would do the same, but it would also remount every screen under
 * it on each path change, which the engram screen, for one, does not do
 * today.
 *
 * React reports an error each boundary catches, so an error this one throws
 * on is logged twice: once here and once where it lands. A boundary cannot
 * decline an error before React reports it, so that second line is the price
 * of catching only chunk failures.
 */
class ChunkErrorBoundary extends Component<
  { resetKey: string; failed?: ReactNode; children: ReactNode },
  { error: unknown; resetKey: string }
> {
  state: { error: unknown; resetKey: string } = {
    error: null,
    resetKey: this.props.resetKey,
  };

  static getDerivedStateFromError(error: unknown): { error: unknown } {
    return { error };
  }

  static getDerivedStateFromProps(
    props: { resetKey: string },
    state: { error: unknown; resetKey: string },
  ): { error: unknown; resetKey: string } | null {
    return props.resetKey === state.resetKey
      ? null
      : { error: null, resetKey: props.resetKey };
  }

  render() {
    const { error } = this.state;
    if (error === null) {
      return this.props.children;
    }
    if (!(error instanceof ChunkLoadError)) {
      // Not ours: pass it up to whatever would have caught it before. React
      // hands a boundary whatever was thrown, typed as unknown; it is thrown
      // on exactly as it arrived.
      throw error as Error;
    }
    return this.props.failed ?? <ChunkFailed />;
  }
}

function ChunkFailed() {
  return (
    <div role="alert" className="flex flex-wrap items-center gap-3">
      <p className="text-sm text-slate-500 dark:text-slate-400">
        This part of Fluid could not load. Reload the page to get the newest
        version.
      </p>
      <button
        type="button"
        onClick={() => {
          location.reload();
        }}
        className={BUTTON.secondary}
      >
        Reload
      </button>
    </div>
  );
}
