/**
 * The element popover host on a `markView`, the way a screen renders it: the
 * buffer's element edits feed a piece of state, and the slot draws the host
 * for it inside a router (the wikilink popover's Open navigates), with the
 * title lookup a screen hands it.
 */

import type { EditorView } from "@codemirror/view";
import { QueryClient } from "@tanstack/react-query";
import { act, render } from "@testing-library/react";
import type { ReactElement } from "react";
import { useCallback, useState, useSyncExternalStore } from "react";
import { MemoryRouter } from "react-router";

import type { ElementRequest } from "../editor/elementEdit";
import { ElementPopoverSlot } from "../editor/popovers/ElementPopoverSlot";
import { wikilinkTitleOptions } from "../editor/wikilinkChips";
import type { MarkViewOptions } from "./markKit";
import { markPeers, markView } from "./markKit";

interface Store {
  value: ElementRequest | null;
  listeners: Set<() => void>;
}

function storeOf(): Store {
  return { value: null, listeners: new Set() };
}

function tell(store: Store, value: ElementRequest | null): void {
  store.value = value;
  for (const listener of store.listeners) listener();
}

// A test helper beside the helpers that render it, never hot reloaded.
// eslint-disable-next-line react-refresh/only-export-components
function Host({
  main,
  store,
}: {
  main: EditorView;
  store: Store;
}): ReactElement {
  const request = useSyncExternalStore(
    (listener) => {
      store.listeners.add(listener);
      return () => {
        store.listeners.delete(listener);
      };
    },
    () => store.value,
  );
  const [client] = useState(() => new QueryClient());
  const titles = useCallback(
    (term: string) => wikilinkTitleOptions("eng", client, term),
    [client],
  );
  return <ElementPopoverSlot main={main} request={request} titles={titles} />;
}

function mount(main: EditorView, store: Store): void {
  render(
    <MemoryRouter>
      <Host main={main} store={store} />
    </MemoryRouter>,
  );
}

function listenerFor(store: Store) {
  return (edit: ElementRequest["edit"] | null, view: EditorView) => {
    // Outside React's batching in a test: wrapped so the render lands.
    act(() => {
      tell(store, edit ? { edit, view } : null);
    });
  };
}

/** A `markView` on `text` with the popover host rendered beside it. */
export function popoverView(
  text: string,
  options: Omit<MarkViewOptions, "onElementEdit"> = {},
): EditorView {
  const store = storeOf();
  const view = markView(text, {
    ...options,
    onElementEdit: listenerFor(store),
  });
  mount(view, store);
  return view;
}

/** Two room peers on `text`; the local one has the popover host. */
export function popoverPeers(
  text: string,
  options: Omit<MarkViewOptions, "onElementEdit" | "room" | "ytext"> = {},
): ReturnType<typeof markPeers> {
  const store = storeOf();
  const peers = markPeers(text, {
    ...options,
    onElementEdit: listenerFor(store),
  });
  mount(peers.local, store);
  return peers;
}
