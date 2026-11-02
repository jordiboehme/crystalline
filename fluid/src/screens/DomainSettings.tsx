/**
 * A domain's settings, off its front page (spec Q1 A): who may reach it,
 * which way a write goes, the MANIFEST's switches, the configuration
 * sections with the way to start one, the archive round trip and the danger
 * zone. The front page keeps the MANIFEST as the page; these are settled once
 * and read rarely.
 *
 * One reading column of sections divided by hairlines, with an in-page index
 * beside it at wide widths and a select above it on a phone. Every card is
 * the one the front page drew, under the same gates.
 */

import { useQuery } from "@tanstack/react-query";
import { useEffect, useMemo, useRef, useState } from "react";
import type { ReactNode } from "react";
import { Link, useLocation, useNavigate, useParams } from "react-router";

import {
  archiveDownloadUrl,
  fetchSyncStatus,
  syncStatusKey,
} from "../api/admin";
import { problemDetail } from "../api/client";
import { fetchManifest, manifestKey, treeQuery } from "../api/domain";
import { isNoKeptRight, keptQuery } from "../api/kept";
import type { ManifestSections } from "../api/domain";
import { useAuth } from "../auth/AuthContext";
import { navigateTo } from "../base";
import { NO_COMMANDS, useRegisterCommands } from "../commands";
import type { PaletteCommand } from "../commands";
import { BackupCard } from "../components/BackupCard";
import { DangerZoneCard } from "../components/DangerZoneCard";
import { DomainPoliciesCard } from "../components/DomainPoliciesCard";
import { ImportArchiveDialog } from "../components/ImportArchiveDialog";
import { KeptTextsCard } from "../components/KeptTextsCard";
import {
  EmptySection,
  Problems,
  starterFor,
} from "../components/ManifestStarter";
import { MembersCard } from "../components/MembersCard";
import { RenameDomainDialog } from "../components/RenameDomainDialog";
import { ReviewModeCard } from "../components/ReviewModeCard";
import { Loading } from "../components/Loading";
import { CHROME_LINK, FIELD, RAIL_LABEL, SIDE } from "../components/primitives";
import { useDomainWritable } from "../domainWritable";
import { isMissing, useDomainRights } from "../domainRights";
import { engramsRoute, manifestEditRoute } from "../paths";
import { MissingOrReflection } from "./DomainMissing";

export default function DomainSettings() {
  const { domain = "" } = useParams();
  // The tree says whether the domain exists for this caller at all, the way
  // the front page asks: a 404 is a wrong address or a Reflection-only
  // reader, and both are answered as the front page answers them.
  const tree = useQuery(treeQuery(domain, ""));
  if (isMissing(tree.error)) {
    return <MissingOrReflection domain={domain} />;
  }
  // The tree read is one of the answers the screen waits for, so an
  // unknown domain never flashes its cards before the 404 lands.
  return (
    <Settings key={domain} domain={domain} treeAnswered={!tree.isPending} />
  );
}

function Settings({
  domain,
  treeAnswered,
}: {
  domain: string;
  treeAnswered: boolean;
}) {
  const { user, capabilities } = useAuth();
  const { canWrite, canShare } = useDomainWritable(domain);
  const navigate = useNavigate();
  const location = useLocation();
  const { summary, listing, members, own, isRenaming } =
    useDomainRights(domain);
  // The front page's palette row "Unregister domain" lands here armed: the
  // question is asked by the card that owns it, never skipped. The arming is
  // taken once and cleared from the history entry, so a reload or a step
  // back and forth does not ask again.
  const [confirmingUnregister, setConfirmingUnregister] = useState(
    (location.state as { unregister?: boolean } | null)?.unregister === true,
  );
  const armedOnArrival = useRef(confirmingUnregister);
  useEffect(() => {
    if (armedOnArrival.current) {
      armedOnArrival.current = false;
      void navigate(
        { pathname: location.pathname, hash: location.hash },
        { replace: true, state: null },
      );
    }
  }, [location.hash, location.pathname, navigate]);
  const [importing, setImporting] = useState(false);
  const [renamingOpen, setRenamingOpen] = useState(false);
  const manifest = useQuery({
    queryKey: manifestKey(domain),
    queryFn: () => fetchManifest(domain),
  });
  // The branch a direct share would commit onto, for the policies card; the
  // same read the sync banner makes, under the same gate.
  const syncStatus = useQuery({
    queryKey: syncStatusKey(domain),
    queryFn: () => fetchSyncStatus(domain),
    retry: false,
    enabled: canShare,
  });
  // The texts the server kept for this domain. A refusal (no right, signed
  // out) is not an error here: the section is simply not drawn.
  const kept = useQuery(keptQuery(domain, user !== null));
  const keptFailed = kept.error !== null && !isNoKeptRight(kept.error);
  const sections = manifest.data?.sections ?? null;
  const manifestEditable =
    manifest.data !== undefined || isMissing(manifest.error);
  const onStart =
    capabilities.canAdminister && canWrite && manifestEditable && !isRenaming
      ? (section: string) => {
          void navigate(manifestEditRoute(domain), {
            state: { seedSection: section },
          });
        }
      : null;

  // Everything a block on this screen waits for has answered, one way or
  // the other: a refused members read is an answer too. Latched, because a
  // card mounting below asks a refused read again, and the screen must not
  // fold back into its loading state (and unmount that card) while it does.
  const answered =
    treeAnswered &&
    !listing.isPending &&
    !members.isPending &&
    !manifest.isPending &&
    !(canShare && syncStatus.isPending) &&
    !(user !== null && kept.isPending);
  const [settled, setSettled] = useState(answered);
  if (answered && !settled) {
    setSettled(true);
  }
  // A link from the front page's rail opens at its block, once the screen
  // has settled: every block is drawn in the same commit, so the target is
  // there then, or the reader may not see it at all.
  const hash = location.hash;
  useEffect(() => {
    if (hash === "" || !settled) {
      return;
    }
    document.getElementById(hash.slice(1))?.scrollIntoView?.({
      block: "start",
    });
  }, [hash, settled]);

  const commands = useMemo<readonly PaletteCommand[]>(() => {
    const rows: PaletteCommand[] = [];
    if (capabilities.canAdminister) {
      rows.push({
        id: "download-archive",
        title: "Download archive",
        run: () => {
          navigateTo(archiveDownloadUrl(domain));
        },
      });
      if (canWrite && !isRenaming) {
        rows.push({
          id: "import-archive",
          title: "Import archive",
          run: () => {
            setImporting(true);
          },
        });
      }
    }
    if (own && !isRenaming) {
      rows.push({
        id: "unregister-domain",
        title: "Unregister domain",
        run: () => {
          setConfirmingUnregister(true);
        },
      });
    }
    return rows.length === 0 ? NO_COMMANDS : rows;
  }, [capabilities.canAdminister, canWrite, domain, isRenaming, own]);
  useRegisterCommands(commands);

  const header = (
    <header>
      <p className="mb-1 text-sm font-semibold text-accent-700 dark:text-accent-300">
        Settings
      </p>
      <h1 className="text-display">{domain}</h1>
      {/*
        The engram view rather than the domain's address: settings are
        opened from the engram view's rail, so "back" returns there even when
        the domain would open on its Reflection.
      */}
      <Link
        to={engramsRoute(domain)}
        className={`mt-2 inline-block text-sm ${CHROME_LINK}`}
      >
        Back to the domain
      </Link>
    </header>
  );
  if (!settled) {
    return (
      <div className="flex max-w-[720px] flex-col gap-8">
        {header}
        <Loading label="Loading the settings" subject={`${domain} settings`} />
      </div>
    );
  }

  const index: SectionLink[] = [
    { id: "members", label: "Members", shown: members.data !== undefined },
    {
      id: "review-mode",
      label: "Review mode",
      shown: capabilities.canShare && summary !== undefined,
    },
    {
      id: "policies",
      label: "Domain policies",
      shown: sections !== null && sections.policies.length > 0,
    },
    { id: "provisioning", label: "Provisioning", shown: sections !== null },
    { id: "tag-aliases", label: "Tag aliases", shown: sections !== null },
    {
      id: "kept-texts",
      label: "Kept texts",
      shown: kept.data !== undefined || keptFailed,
    },
    { id: "backup", label: "Backup", shown: capabilities.canAdminister },
    {
      id: "danger-zone",
      label: "Danger zone",
      shown: capabilities.canAdminister || own,
    },
  ].filter((link) => link.shown);

  return (
    <div className="flex flex-col gap-8">
      {header}
      <div className="flex flex-col gap-6 lg:grid lg:grid-cols-[11rem_minmax(0,720px)] lg:gap-14">
        <SectionIndex links={index} hash={hash} />
        <div className="flex min-w-0 flex-col divide-y divide-hair">
          {members.data !== undefined && (
            <div
              id="members"
              className="scroll-mt-20 py-8 first:pt-0 last:pb-0"
            >
              <MembersCard domain={domain} />
            </div>
          )}

          {capabilities.canShare && summary !== undefined && (
            <div
              id="review-mode"
              className="scroll-mt-20 py-8 first:pt-0 last:pb-0"
            >
              <ReviewModeCard
                domain={domain}
                reviewing={summary.review !== null}
              />
            </div>
          )}

          {sections !== null && sections.policies.length > 0 && (
            <div
              id="policies"
              className="scroll-mt-20 py-8 first:pt-0 last:pb-0"
            >
              <DomainPoliciesCard
                domain={domain}
                policies={sections.policies}
                branch={syncStatus.data?.branch ?? null}
                onRename={() => {
                  setRenamingOpen(true);
                }}
                renaming={isRenaming}
              />
            </div>
          )}

          {/*
            A refused MANIFEST read, said the way the front page says it, so
            the missing policies and sections below are not read as "none
            declared".
          */}
          {manifest.error !== null && !isMissing(manifest.error) && (
            <div className="py-8 first:pt-0 last:pb-0">
              <p
                role="alert"
                className="rounded bg-red-50 px-3 py-2 text-sm text-red-800 dark:bg-red-950 dark:text-red-200"
              >
                {problemDetail(manifest.error)}
              </p>
            </div>
          )}
          {sections !== null && (
            <ConfigSections sections={sections} onStart={onStart} />
          )}

          {(kept.data !== undefined || keptFailed) && (
            <div
              id="kept-texts"
              className="scroll-mt-20 py-8 first:pt-0 last:pb-0"
            >
              {kept.data !== undefined ? (
                <KeptTextsCard domain={domain} entries={kept.data} />
              ) : (
                <section
                  aria-labelledby="domain-kept"
                  className="flex flex-col gap-4"
                >
                  <h2 id="domain-kept" className="text-section">
                    Kept texts
                  </h2>
                  <p
                    role="alert"
                    className="rounded bg-red-50 px-3 py-2 text-sm text-red-800 dark:bg-red-950 dark:text-red-200"
                  >
                    {kept.error !== null && problemDetail(kept.error)}
                  </p>
                </section>
              )}
            </div>
          )}

          {capabilities.canAdminister && (
            <div id="backup" className="scroll-mt-20 py-8 first:pt-0 last:pb-0">
              <BackupCard
                domain={domain}
                onImport={() => {
                  setImporting(true);
                }}
                canImport={canWrite}
                renaming={isRenaming}
              />
            </div>
          )}

          {(capabilities.canAdminister || own) && (
            <div
              id="danger-zone"
              className="scroll-mt-20 py-8 first:pt-0 last:pb-0"
            >
              <DangerZoneCard
                domain={domain}
                kind={summary?.kind ?? null}
                confirming={confirmingUnregister}
                onConfirmingChange={setConfirmingUnregister}
                renaming={isRenaming}
              />
            </div>
          )}
        </div>
      </div>

      {importing && (
        <ImportArchiveDialog
          domain={domain}
          onClose={() => {
            setImporting(false);
          }}
        />
      )}
      {renamingOpen && (
        <RenameDomainDialog
          domain={domain}
          onClose={() => {
            setRenamingOpen(false);
          }}
        />
      )}
    </div>
  );
}

/**
 * The MANIFEST's configuration sections, as the core crate reads them, with
 * the way to start one that is not declared. Read-only on purpose: the
 * document is the source and the editor is where a change goes.
 */
function ConfigSections({
  sections,
  onStart,
}: {
  sections: ManifestSections;
  onStart: ((section: string) => void) | null;
}) {
  return (
    <>
      <Facet id="provisioning" title="Provisioning">
        {sections.provisioning === null ||
        sections.provisioning.decls.length === 0 ? (
          <EmptySection
            starter={starterFor(sections, "Provisioning")}
            fallback="Nothing declared"
            onStart={onStart}
          />
        ) : (
          <>
            <ul className="flex flex-col gap-1 text-sm">
              {sections.provisioning.decls.map((decl) => (
                <li key={decl.kind} className="font-mono">
                  {`${decl.kind}: ${decl.path}`}
                </li>
              ))}
            </ul>
          </>
        )}
        {sections.provisioning !== null && (
          <Problems items={sections.provisioning.problems} />
        )}
      </Facet>
      <Facet id="tag-aliases" title="Tag aliases">
        {sections.tagAliases === null ||
        sections.tagAliases.decls.length === 0 ? (
          <EmptySection
            starter={starterFor(sections, "Tag Aliases")}
            fallback="No aliases"
            onStart={onStart}
          />
        ) : (
          <>
            <ul className="flex flex-col gap-1 text-sm">
              {sections.tagAliases.decls.map((decl) => (
                <li key={decl.alias} className="font-mono">
                  {`${decl.alias} -> ${decl.canonical}`}
                </li>
              ))}
            </ul>
          </>
        )}
        {sections.tagAliases !== null && (
          <Problems items={sections.tagAliases.problems} />
        )}
      </Facet>
    </>
  );
}

/** One section: a labelled region, so each is reachable by its name. */
function Facet({
  id,
  title,
  children,
}: {
  id: string;
  title: string;
  children: ReactNode;
}) {
  const headingId = `manifest-facet-${id}`;
  return (
    <div id={id} className="scroll-mt-20 py-8 first:pt-0 last:pb-0">
      <section aria-labelledby={headingId} className="flex flex-col gap-4">
        <h2 id={headingId} className="text-section">
          {title}
        </h2>
        {children}
      </section>
    </div>
  );
}

interface SectionLink {
  id: string;
  label: string;
  shown: boolean;
}

/**
 * The in-page index: a rail of links at wide widths, a select on a phone.
 * The current section follows the scroll, and a press names it at once.
 */
function SectionIndex({ links, hash }: { links: SectionLink[]; hash: string }) {
  const navigate = useNavigate();
  const ids = links.map((link) => link.id).join(" ");
  const [seen, setSeen] = useState<string | null>(null);
  // A pressed or hashed section stays current until the reader scrolls on
  // their own: a short last section never reaches the observer's band, and
  // the scroll a press causes must not move the highlight off it.
  const [picked, setPicked] = useState<string | null>(
    () => links.find((link) => `#${link.id}` === hash)?.id ?? null,
  );
  useEffect(() => {
    const order = ids.split(" ");
    const release = () => {
      setPicked(null);
    };
    const scrollKeys = new Set([
      "ArrowUp",
      "ArrowDown",
      "PageUp",
      "PageDown",
      "Home",
      "End",
      " ",
    ]);
    const onKey = (event: KeyboardEvent) => {
      if (scrollKeys.has(event.key)) {
        release();
      }
    };
    const onScroll = () => {
      const root = document.scrollingElement ?? document.documentElement;
      const last = order[order.length - 1];
      if (
        last !== undefined &&
        root.scrollTop > 0 &&
        root.scrollTop + root.clientHeight >= root.scrollHeight - 2
      ) {
        setSeen(last);
      }
    };
    window.addEventListener("wheel", release, { passive: true });
    window.addEventListener("touchmove", release, { passive: true });
    window.addEventListener("keydown", onKey);
    window.addEventListener("scroll", onScroll, { passive: true });
    let observer: IntersectionObserver | null = null;
    if (typeof IntersectionObserver !== "undefined") {
      const inView = new Set<string>();
      observer = new IntersectionObserver(
        (entries) => {
          for (const entry of entries) {
            if (entry.isIntersecting) {
              inView.add(entry.target.id);
            } else {
              inView.delete(entry.target.id);
            }
          }
          const first = order.find((id) => inView.has(id));
          if (first !== undefined) {
            setSeen(first);
          }
        },
        { rootMargin: "-80px 0px -60% 0px" },
      );
      for (const id of order) {
        const el = document.getElementById(id);
        if (el !== null) {
          observer.observe(el);
        }
      }
    }
    return () => {
      observer?.disconnect();
      window.removeEventListener("wheel", release);
      window.removeEventListener("touchmove", release);
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("scroll", onScroll);
    };
  }, [ids]);
  const current = picked ?? seen ?? links[0]?.id ?? null;
  // Scrolls to the section itself, because pressing the section the hash
  // already names changes nothing the screen's hash effect could see, and
  // hands the focus to its heading so a keyboard reader lands there too.
  function go(id: string) {
    setPicked(id);
    void navigate({ hash: `#${id}` }, { replace: true });
    const el = document.getElementById(id);
    el?.scrollIntoView?.({ block: "start" });
    const heading = el?.querySelector<HTMLElement>("h2") ?? null;
    if (heading !== null) {
      heading.tabIndex = -1;
      heading.focus({ preventScroll: true });
    }
  }
  if (links.length === 0) {
    return null;
  }
  return (
    <>
      <div className="lg:hidden">
        <label htmlFor="settings-jump" className="sr-only">
          Jump to a section
        </label>
        <select
          id="settings-jump"
          value={current ?? ""}
          onChange={(event) => {
            go(event.target.value);
          }}
          className={`w-full ${FIELD}`}
        >
          {links.map((link) => (
            <option key={link.id} value={link.id}>
              {link.label}
            </option>
          ))}
        </select>
      </div>
      <nav
        aria-label="Settings sections"
        className="sticky top-20 hidden self-start lg:block"
      >
        <p className={`mb-2 px-2.5 ${RAIL_LABEL}`}>On this page</p>
        <ul className="flex flex-col gap-0.5">
          {links.map((link) => (
            <li key={link.id}>
              <Link
                to={{ hash: `#${link.id}` }}
                replace
                aria-current={link.id === current ? "location" : undefined}
                onClick={(event) => {
                  event.preventDefault();
                  go(link.id);
                }}
                className={`${SIDE.row} ${link.id === current ? SIDE.current : ""}`}
              >
                {link.label}
              </Link>
            </li>
          ))}
        </ul>
      </nav>
    </>
  );
}
