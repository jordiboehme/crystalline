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
  return {
    user: null,
    anonymous: false,
    read_only: false,
    csrf: null,
    version: import.meta.env.VITE_APP_VERSION,
    ...overrides,
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

/** Mount the app at `entry`, on an in-memory history. */
export function renderApp(entry = "/"): RenderResult {
  return render(
    <MemoryRouter initialEntries={[entry]}>
      <App />
    </MemoryRouter>,
  );
}
