/**
 * The warning for a browser holding a different build than the server it is
 * talking to.
 *
 * It happens for one ordinary reason: Crystalline was upgraded while a tab
 * stayed open, and that tab is now running the old assets against the new API.
 * Nothing is blocked, because most of the surface will still work, so this is
 * a dismissible notice rather than a wall - but a symptom nobody can explain is
 * worse than a sentence naming both versions.
 */

import { useState } from "react";
import { Toast } from "radix-ui";

import { useAuth } from "../auth/AuthContext";

/** The build this bundle was made from, frozen in at build time. */
const BUILD_VERSION = import.meta.env.VITE_APP_VERSION;

export function VersionSkewToast() {
  const { capabilities } = useAuth();
  const [dismissed, setDismissed] = useState(false);

  const serverVersion = capabilities.serverVersion;
  const skewed = serverVersion !== "" && serverVersion !== BUILD_VERSION;
  if (!skewed || dismissed) {
    return null;
  }

  return (
    <Toast.Provider duration={Infinity}>
      <Toast.Root
        open
        onOpenChange={(open) => {
          if (!open) {
            setDismissed(true);
          }
        }}
        className="flex items-start gap-3 rounded border border-amber-300 bg-amber-50 px-4 py-3 text-sm shadow-lg dark:border-amber-800 dark:bg-amber-950"
      >
        <Toast.Title className="font-medium text-amber-900 dark:text-amber-100">
          Fluid {BUILD_VERSION} is talking to Crystalline {serverVersion}
        </Toast.Title>
        <Toast.Description className="sr-only">
          Reload this tab to pick up the matching build.
        </Toast.Description>
        <Toast.Close
          aria-label="Dismiss"
          className="ml-auto rounded px-1 text-amber-900 hover:bg-amber-100 focus-visible:ring-2 focus-visible:ring-amber-500 focus-visible:outline-none dark:text-amber-100 dark:hover:bg-amber-900"
        >
          <span aria-hidden="true">x</span>
        </Toast.Close>
      </Toast.Root>
      <Toast.Viewport className="fixed bottom-4 left-1/2 z-50 flex w-full max-w-md -translate-x-1/2 flex-col gap-2 px-4 outline-none" />
    </Toast.Provider>
  );
}
