/**
 * The data layer, mounted.
 *
 * The client is built per mount rather than once per module, so no cache
 * outlives the app that filled it: a second mount in a test starts empty, and
 * so would a remount in the browser. Its policies live in `client.ts`.
 */

import { QueryClientProvider } from "@tanstack/react-query";
import { useState } from "react";
import type { ReactNode } from "react";

import { createQueryClient } from "./client";

export function QueryProvider({ children }: { children: ReactNode }) {
  const [client] = useState(createQueryClient);
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
}
