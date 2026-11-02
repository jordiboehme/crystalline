/**
 * Where a caller reaches a Reflection, and in which state it is.
 *
 * Eager on purpose and kept small: Home, the frame and the domain page read
 * it on every visit. Everything else about the Reflection lives in
 * `reflection.ts`, which only the lazy Reflection screens import.
 *
 * `GET /reflection/domains` is how a person with knowledge `none` finds
 * their domains at all: `GET /domains` leaves out every domain whose engrams
 * the caller cannot read, and such a person reads none. `GET /domains/{d}/reflection` never
 * answers 409; it is how the domain page learns off, pending, on and
 * elsewhere. Anything this client does not recognize reads as off and as no
 * right, which draws nothing rather than a door that will not open.
 */

import type { QueryClient } from "@tanstack/react-query";

import { api, encodeSegment } from "./client";
import { asArray, asNumber, asObject, asString, asStrings } from "./json";

/**
 * The reflection right, least to most (spec B5): `read` reads and comments,
 * `edit` also edits text and creates, moves and deletes pages, `curate`
 * (every knowledge editor and above) also writes sources and `PACT.md`.
 */
export type ReflectionRight = "none" | "read" | "edit" | "curate";
const RIGHTS: readonly ReflectionRight[] = ["none", "read", "edit", "curate"];

/** The domain's agreement on how its pages are written. */
export const PACT_PATH = "PACT.md";

/**
 * `homeless`: a GitHub-connected domain whose `reflection_home` is missing or
 * invalid; switching Reflection on again at the team server fills it. It is
 * never `off`, and never drawn as off.
 */
export type ReflectionState =
  "off" | "pending" | "homeless" | "hosted_elsewhere" | "on" | "elsewhere";
/**
 * `hosted_elsewhere`: on a shared Postgres database another instance holds
 * the domain's host lock and serves its Reflection (backend Task 47); the
 * status names it in `host_label`. Listed here because the listing reader
 * reads an unknown word as `on`.
 */
const STATES: readonly ReflectionState[] = [
  "off",
  "pending",
  "homeless",
  "hosted_elsewhere",
  "on",
  "elsewhere",
];

export type ReflectionHomeKind = "here" | "local" | "elsewhere";
const HOMES: readonly ReflectionHomeKind[] = ["here", "local", "elsewhere"];

/** A home word, or null: the server answers null for off, pending, homeless and hosted_elsewhere. */
function homeOf(value: unknown): ReflectionHomeKind | null {
  return typeof value === "string" &&
    (HOMES as readonly string[]).includes(value)
    ? (value as ReflectionHomeKind)
    : null;
}

/** Whether `held` is `needed` or more, on the contract's ladder. */
export function atLeast(
  held: ReflectionRight,
  needed: ReflectionRight,
): boolean {
  return RIGHTS.indexOf(held) >= RIGHTS.indexOf(needed);
}

/**
 * Whether `right` may edit the page at `path`: `edit` for a page, `curate`
 * for `PACT.md`, which only curators write. Everyone who reads it may
 * propose a change through a thread on it.
 */
export function mayEditPage(right: ReflectionRight, path: string): boolean {
  return atLeast(right, path === PACT_PATH ? "curate" : "edit");
}

function oneOf<T extends string>(
  value: unknown,
  allowed: readonly T[],
  fallback: T,
): T {
  return typeof value === "string" &&
    (allowed as readonly string[]).includes(value)
    ? (value as T)
    : fallback;
}

export interface ReflectionStatus {
  domain: string;
  state: ReflectionState;
  home: ReflectionHomeKind | null;
  /** Set when the Reflection lives on another instance. */
  homeUrl: string | null;
  /** While `hosted_elsewhere`: the label of the instance that serves it. */
  hostLabel: string | null;
  right: ReflectionRight;
  /** Whether this caller reads the domain's engrams at all. */
  engramsVisible: boolean;
  /** Whether this caller holds a membership here it may leave (G5). */
  member: boolean;
  /** `github` when access to the domain follows the GitHub App. */
  governance: "github" | "crystalline";
  /** Whether the domain is private. */
  private: boolean;
  /** While `pending`: the engram paths still under `reflections/`. */
  pendingEngrams: string[];
}

export function readReflectionStatus(
  payload: unknown,
  domain: string,
): ReflectionStatus {
  const record = asObject(payload);
  return {
    domain: asString(record?.domain) ?? domain,
    state: oneOf(record?.state, STATES, "off"),
    home: homeOf(record?.home),
    homeUrl: asString(record?.home_url),
    hostLabel: asString(record?.host_label),
    right: oneOf(record?.right, RIGHTS, "none"),
    engramsVisible: record?.engrams_visible === true,
    member: record?.member === true,
    governance: record?.governance === "github" ? "github" : "crystalline",
    private: record?.private === true,
    pendingEngrams: asStrings(record?.pending_engrams),
  };
}

export interface ReflectionDomain {
  domain: string;
  /** Never `off`: the listing leaves those out. */
  state: ReflectionState;
  right: ReflectionRight;
  engramsVisible: boolean;
  home: ReflectionHomeKind | null;
  homeUrl: string | null;
  /** While `hosted_elsewhere`: the label of the instance that serves it. */
  hostLabel: string | null;
  openThreads: number;
}

export function readReflectionDomains(payload: unknown): ReflectionDomain[] {
  const out: ReflectionDomain[] = [];
  for (const row of asArray(asObject(payload)?.domains)) {
    const record = asObject(row);
    const domain = asString(record?.domain);
    const right = oneOf(record?.right, RIGHTS, "none");
    if (domain === null || right === "none") {
      continue;
    }
    out.push({
      domain,
      state: oneOf(record?.state, STATES, "on"),
      right,
      engramsVisible: record?.engrams_visible === true,
      home: homeOf(record?.home),
      homeUrl: asString(record?.home_url),
      hostLabel: asString(record?.host_label),
      openThreads: asNumber(record?.open_threads) ?? 0,
    });
  }
  return out;
}

export const REFLECTION_DOMAINS_KEY = ["reflection-domains"] as const;

/**
 * The prefix of every Reflection query of one domain (tree, page, threads,
 * review, search; `api/reflection.ts` builds them all on it). Here, in the
 * eager module, so the change-event reader (Task 1a) can invalidate it
 * without pulling the lazy module into the entry.
 */
export function reflectionKey(domain: string): readonly unknown[] {
  return ["reflection", domain];
}

export function reflectionStatusKey(domain: string): readonly unknown[] {
  return ["reflection-status", domain];
}

/**
 * Every query a change to a domain's Reflection can make stale: the
 * Reflection itself, its status (outside that prefix) and the listing whose
 * rows count open threads. The change-event reader and the writes of the
 * screens both read this one list, so the two cannot drift apart.
 */
export function reflectionKeys(domain: string): (readonly unknown[])[] {
  return [
    reflectionKey(domain),
    reflectionStatusKey(domain),
    REFLECTION_DOMAINS_KEY,
  ];
}

/** Refresh everything {@link reflectionKeys} lists, after a create, move or delete. */
export function invalidateReflection(
  queryClient: QueryClient,
  domain: string,
): void {
  for (const queryKey of reflectionKeys(domain)) {
    void queryClient.invalidateQueries({ queryKey });
  }
}

export async function fetchReflectionStatus(
  domain: string,
): Promise<ReflectionStatus> {
  return readReflectionStatus(
    await api<unknown>(`/domains/${encodeSegment(domain)}/reflection`),
    domain,
  );
}

export async function fetchReflectionDomains(): Promise<ReflectionDomain[]> {
  return readReflectionDomains(await api<unknown>("/reflection/domains"));
}
