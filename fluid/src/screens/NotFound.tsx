/**
 * An address this app does not answer at.
 *
 * Inside the layout on purpose: a mistyped URL is a wrong turn, not a broken
 * app, so the frame and the domain list stay where they are.
 *
 * The screen is also the easter-egg layer's second act: beneath the honest
 * explanation sits a C64 load attempt for the very path that failed, ending
 * the only way it could. "Memory" is the sanctioned joke here, not product
 * voice: a page about failed recall gets to say so.
 */

import { Link, useLocation } from "react-router";

const REPO_URL = "https://github.com/jordiboehme/crystalline";

/** The failed address as a C64 filename: bare, uppercase, one line. */
function fileName(pathname: string): string {
  const bare = pathname.replace(/^\/+/, "").toUpperCase().slice(0, 22);
  return bare === "" ? "THIS" : bare;
}

export default function NotFound() {
  const { pathname } = useLocation();
  const file = fileName(pathname);
  return (
    <div className="max-w-xl">
      <h1 className="font-mono text-xl font-semibold">
        this memory could not be recalled
      </h1>
      <p className="mt-2 text-sm text-slate-600 dark:text-slate-400">
        The address names nothing this mind still holds. It may have been
        reorganized, retired, or never captured at all.{" "}
        <Link to="/" className="underline underline-offset-2">
          Back to what is remembered
        </Link>
      </p>
      <div className="mt-6 max-w-md bg-[#7c70da] p-3" aria-hidden>
        <div className="relative bg-[#40318d] px-3 py-3 font-mono text-xs leading-5 text-[#7c70da]">
          <div className="crt-scan pointer-events-none absolute inset-0" />
          <p>{`LOAD"${file}",8,1`}</p>
          <p className="mt-3">{`SEARCHING FOR ${file}`}</p>
          <p>?FILE NOT FOUND{"  "}ERROR</p>
          <p>READY.</p>
          <p>
            <span className="crystal-cursor">{"█"}</span>
          </p>
        </div>
      </div>
      <p className="mt-4 text-xs text-slate-500 dark:text-slate-400">
        grown by{" "}
        <a
          href={REPO_URL}
          target="_blank"
          rel="noreferrer"
          className="underline underline-offset-2"
        >
          Jordi Böhme
        </a>
      </p>
    </div>
  );
}
