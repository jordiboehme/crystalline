/**
 * The name a tab, the history and a saved PDF give the page on screen.
 *
 * A screen hands over the title it shows and nothing else: the hook writes
 * `<title> - Fluid` to `document.title`, and `Fluid` while the page has no
 * title yet (loading, not found) or after it leaves. No screen touches
 * `document.title` itself.
 *
 * While the page prints, the title is the bare page title, because the
 * browser suggests it as the file name of a saved PDF and `- Fluid` on the
 * end of it is noise there. That holds for Cmd+P as much as for a print
 * button, since it rides on `beforeprint` and `afterprint`.
 */

import { useEffect } from "react";

/** The app's own name: the title of a page that has none of its own. */
export const APP_TITLE = "Fluid";

/**
 * Name the page `title`; `null` (or only blanks) means it has no title yet.
 * `printTitle` replaces the bare title while printing.
 */
export function usePageTitle(
  title: string | null | undefined,
  printTitle?: string,
): void {
  const bare = title?.trim() ?? "";
  const forPrint = printTitle?.trim() ?? "";
  useEffect(() => {
    if (bare === "") {
      document.title = APP_TITLE;
      return;
    }
    const shown = `${bare} - ${APP_TITLE}`;
    const printed = forPrint === "" ? bare : forPrint;
    document.title = shown;
    const start = () => {
      document.title = printed;
    };
    const end = () => {
      document.title = shown;
    };
    window.addEventListener("beforeprint", start);
    window.addEventListener("afterprint", end);
    return () => {
      window.removeEventListener("beforeprint", start);
      window.removeEventListener("afterprint", end);
      document.title = APP_TITLE;
    };
  }, [bare, forPrint]);
}
