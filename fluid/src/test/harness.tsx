/**
 * What the shell tests share: a way to mount the whole app at a URL, and a way
 * to say what the server answers.
 *
 * The tests mount `App` rather than a hand-built provider stack, because the
 * behavior under test is the composition itself: which screen a given `me`
 * answer leads to. A stack assembled by the test could pass while the real one
 * is wired wrong.
 */

import { render } from "@testing-library/react";
import type { RenderResult } from "@testing-library/react";
import { MemoryRouter } from "react-router";

import App from "../App";
import { ApiProblem } from "../api/client";
import type { MeResponse, User } from "../api/model";
import type { CollabSession } from "../collab/useCollabSession";

/**
 * What a stubbed route answers. Returning a value resolves the call; throwing
 * an `ApiProblem` fails it, exactly as the real client would.
 */
export type Answer = (path: string, init?: RequestInit) => unknown;

/**
 * Turn a path-to-answer table into an `api` implementation.
 *
 * A path with no entry is a 404 rather than a silent `undefined`: a test that
 * forgot to stub a call should say so, not watch a screen render empty.
 */
export function answersFor(routes: Record<string, Answer>) {
  // Async, so an answer that throws an `ApiProblem` rejects the call exactly
  // as the real client would, without the test having to build a promise.
  return async (path: string, init?: RequestInit): Promise<never> => {
    // `split` on a non-empty separator always yields at least one element;
    // the fallback to `path` itself only documents that guarantee to the
    // checker, it never actually triggers.
    const [route = path] = path.split("?");
    const answer = routes[route];
    if (!answer) {
      throw new ApiProblem(404, "not found", `no stub for ${route}`);
    }
    return (await answer(path, init)) as never;
  };
}

/** A `me` answer, defaulting to the anonymous-refused shape: no identity. */
export function meResponse(overrides: Partial<MeResponse> = {}): MeResponse {
  const probe = {
    user: null,
    anonymous: false,
    read_only: false,
    // An instance that has been set up already, which is what every screen
    // under test assumes; the first-run flow overrides it where it matters.
    needs_setup: false,
    // Off by default, the same as every other feature-gate field here: a
    // test that wants the connected-clients card visible says so with an
    // explicit override, the same way a personal-mode test passes
    // `can_share` itself below.
    oauth: false,
    csrf: null,
    version: import.meta.env.VITE_APP_VERSION,
    ...overrides,
  };
  return {
    ...probe,
    // The default mode, which is what every fixture is unless it says
    // otherwise: one machine credential does every share, so an admin may and
    // nobody else does. A personal-mode test passes `can_share` itself.
    can_share: overrides.can_share ?? probe.user?.role === "admin",
  };
}

/** An account, editor by default. */
export function userFixture(overrides: Partial<User> = {}): User {
  return {
    name: "ada",
    display: "Ada Lovelace",
    role: "editor",
    disabled: false,
    ...overrides,
  };
}

/** The domain listing the sidebar reads, in the engine's own shape. */
export function domainsResponse() {
  return {
    behavior: ["Search before answering from memory."],
    domains: [
      {
        name: "eng",
        kind: "file",
        engrams: 4,
        observations: 12,
        relations: 3,
        when_to_use: ["Route here for eng questions."],
      },
    ],
  };
}

/** One policy row of a manifest payload, in the registry's own wire shape. */
export interface PolicyRowFixture {
  key: string;
  declared: string | null;
  effective: string;
  values: string[];
  default: string;
  meaning: string;
  changed_by: string;
}

/** One row of the registry, at whatever this MANIFEST declares for it. */
export function policyRow(
  key: string,
  declared: string | null,
  effective: string,
  values: string[],
  dflt: string,
  meaning: string,
): PolicyRowFixture {
  return {
    key,
    declared,
    effective,
    values,
    default: dflt,
    meaning,
    changed_by: "owner",
  };
}

/**
 * The registry as the server sends it: every key it knows, in its own order,
 * none of them declared.
 *
 * A function rather than a constant so a test may map over a fresh copy - the
 * policies card's fixtures change one row's declaration and leave the rest.
 */
export function defaultPolicyRows(): PolicyRowFixture[] {
  return [
    policyRow(
      "generated_indexes",
      null,
      "local",
      ["local", "shared"],
      "local",
      "Whether the generated folder listings travel with a share.",
    ),
    policyRow(
      "sharing",
      null,
      "proposal",
      ["proposal", "direct"],
      "proposal",
      "Whether a share opens a proposal for review or commits straight to the branch.",
    ),
  ];
}

/**
 * The startable sections as the server sends them: the core registry, whole,
 * in its own order and independent of what any MANIFEST declares.
 *
 * A function rather than a constant for the reason the policy rows are one: a
 * test that wants one section declared maps over a fresh copy.
 */
export function defaultStarterRows() {
  return [
    {
      section: "When to Use",
      meaning:
        "Agents pick this domain by these bullets. Without one, nothing routes here.",
      example:
        "## When to Use\n\n- Route here for questions about how our deployment pipeline works",
    },
    {
      section: "Scope",
      meaning:
        "What belongs in this domain and what does not. Read for routing only when When to Use is empty.",
      example:
        "## Scope\n\n- Infrastructure and deployment, not application code",
    },
    {
      section: "Provisioning",
      meaning:
        "Folders this domain installs into an AI harness: skills, commands, agents or MCP configs.",
      example: "## Provisioning\n\n- skills: skills",
    },
    {
      section: "Tag Aliases",
      meaning:
        "Spellings that fold into one canonical tag, so a search for either finds both.",
      example: "## Tag Aliases\n\n- k8s -> kubernetes",
    },
  ];
}

/**
 * The sections a server reads out of a domain's MANIFEST, in the wire shape.
 *
 * Shared rather than spelled per file: the domain screen and the policies
 * card are two readers of one payload, and a fixture that drifted between
 * them would let one of the two pass against a shape the server never sends.
 */
export function manifestSectionsResponse(
  overrides: Record<string, unknown> = {},
) {
  return {
    scope: [],
    when_to_use: ["Route here for eng questions."],
    routing: "when_to_use",
    missing: ["Scope"],
    provisioning: null,
    tag_aliases: null,
    policies: defaultPolicyRows(),
    starters: defaultStarterRows(),
    ...overrides,
  };
}

/** Mount the app at `entry`, on an in-memory history. */
export function renderApp(entry = "/"): RenderResult {
  return render(
    <MemoryRouter initialEntries={[entry]}>
      <App />
    </MemoryRouter>,
  );
}

/**
 * The editing session as it reads when there is no room to join: the solo
 * surface, exactly as the editor behaved before sessions existed.
 *
 * The editor route opens a session of its own now, so a test that lands on
 * that route without saying otherwise would sit on the connecting skeleton
 * until the socket's own timeout. Every such file mocks
 * `useCollabSession` and returns this; the one spelling lives here so the
 * shape cannot drift file by file.
 */
export function soloCollabSession(): CollabSession {
  return {
    mode: "solo",
    ytext: null,
    awareness: null,
    epoch: null,
    separator: "\n",
    status: "failed",
    saveState: "ok",
    saveDetail: null,
    conflict: null,
    participants: [],
    permalink: "alpha",
    flush: () => undefined,
    resolve: () => undefined,
    closed: false,
    mergeNotice: false,
  };
}
