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

/** What `POST /auth/setup` takes: the first admin, and a setup token if one is needed. */
export type SetupBody = components["schemas"]["SetupBody"];

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

/** What `POST /domains/{domain}/engrams` takes. */
export type CreateEngramBody = components["schemas"]["CreateEngramBody"];

/** What `PUT /domains/{domain}/engrams/{permalink}` takes. */
export type SaveEngramBody = components["schemas"]["SaveEngramBody"];

/** What `POST /domains/{domain}/retire` takes. */
export type RetireBody = components["schemas"]["RetireBody"];

/** What `POST /domains/{domain}/move` takes. */
export type MoveBody = components["schemas"]["MoveBody"];

/** What both verbs on `/domains/{domain}/evolve/ack` take. */
export type AckBody = components["schemas"]["AckBody"];

/** What `PUT /domains/{domain}/manifest` takes. */
export type SaveManifestBody = components["schemas"]["SaveManifestBody"];

/** What `GET /settings/github` and every GitHub settings verb answer with. */
export type GithubStatusResponse =
  components["schemas"]["GithubStatusResponse"];

/** What `GET /me/github-identity` and every identity verb answer with. */
export type GithubIdentityResponse =
  components["schemas"]["GithubIdentityResponse"];

/** What an archive preview and an archive import both answer with. */
export type ArchiveReport = components["schemas"]["ArchiveReport"];

/**
 * What `POST /domains` takes.
 *
 * Named for the wire rather than for the caller, because `api/admin.ts`
 * exports a `CreateDomainBody` of its own: the screen's version narrows `mode`
 * to the three modes that exist and leaves out the nulls a JSON body may
 * carry, and it is checked against this one on its way out.
 */
export type CreateDomainWireBody = components["schemas"]["CreateDomainBody"];

/** What `POST /validate` takes. */
export type ValidateBody = components["schemas"]["ValidateBody"];

/** One finding a validation raises. */
export type ValidateFinding = components["schemas"]["ValidateFinding"];

/** What `POST /validate` answers with. */
export type ValidateResponse = components["schemas"]["ValidateResponse"];

/**
 * One row of the caller's own MCP token list: label, when it was issued, when
 * it last resolved a request. Never the token itself - only its hash is
 * stored, so there is nothing to show back after issuance.
 */
export type McpTokenInfo = components["schemas"]["McpTokenInfo"];

/** What `POST /me/mcp-tokens` takes. */
export type IssueMcpTokenBody = components["schemas"]["IssueBody"];

/**
 * What issuing or rotating an MCP token answers with: the secret, readable
 * here and nowhere else, ever.
 */
export type IssuedMcpToken = components["schemas"]["IssuedTokenResponse"];

/** What `GET /domains/{domain}/members` answers with: who owns and is invited into a domain. */
export type MembersResponse = components["schemas"]["MembersResponse"];

/** One membership row: who, at what level, added by whom and when. */
export type DomainMember = components["schemas"]["DomainMember"];

/** What a member may do on one private domain: `viewer`, `editor` or `manager`. */
export type MemberLevel = components["schemas"]["MemberLevel"];

/** What `PUT /domains/{domain}/members/{principal}` takes. */
export type MemberBody = components["schemas"]["MemberBody"];

/** What `PUT /domains/{domain}/owner` takes. */
export type OwnerBody = components["schemas"]["OwnerBody"];

/** What `PUT /domains/{domain}/visibility` takes. */
export type VisibilityBody = components["schemas"]["VisibilityBody"];
