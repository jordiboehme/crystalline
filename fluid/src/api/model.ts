/**
 * Names for the payload shapes the app passes around.
 *
 * `types.ts` is generated and addresses everything through
 * `components["schemas"][...]`, which is precise and unreadable at a call
 * site. These aliases are the readable half: one place that says which
 * generated schema a screen means, so a renamed schema is one edit here rather
 * than a search across the app. Nothing is redefined - every alias points at
 * the generated type, so the OpenAPI document stays the single source.
 */

import type { components } from "./types";

/** One account, as every route hands it back. Carries no password material. */
export type User = components["schemas"]["User"];

/** What an account may do: `viewer`, `editor` or `admin`. */
export type Role = components["schemas"]["Role"];

/** The capability probe: who the caller is and what this instance allows. */
export type MeResponse = components["schemas"]["MeResponse"];

/** What a successful `POST /auth/login` answers with. */
export type LoginResponse = components["schemas"]["LoginResponse"];

/** The RFC 9457 body every failure on this API carries. */
export type ProblemDetail = components["schemas"]["ProblemDetail"];

/** The `{"users": [...]}` envelope `GET /users` answers with. */
export type UsersResponse = components["schemas"]["UsersResponse"];

/** The `{"user": ...}` envelope every user mutation answers with. */
export type UserResponse = components["schemas"]["UserResponse"];

/** What `POST /users` takes. */
export type CreateUserBody = components["schemas"]["CreateBody"];

/** What `PATCH /users/{name}` takes. */
export type PatchUserBody = components["schemas"]["PatchBody"];
