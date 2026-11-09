/**
 * Who hears Back before the router does: an open rename field, which has to write
 * first (R95). It has to be in the first bundle, imported before the app mounts.
 * The router's listener goes in at mount and unmounts the editor in its own
 * dispatch, and a listener a lazy screen adds later runs after it, whatever its
 * capture flag says (seen in Chromium, on a popstate at the window).
 */

export const backGates = new Set<(event: PopStateEvent) => void>();

window.addEventListener("popstate", (event) => {
  for (const gate of [...backGates]) {
    gate(event);
  }
});
