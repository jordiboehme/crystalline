//! The OpenAPI schemas of the account types the JSON API sends and takes.
//!
//! The types themselves live with the account store, which has no business
//! knowing about the API description; these copies carry the schema
//! derive in their place. Each one is a verbatim copy of its store type:
//! same name, same fields, same doc comments, same serde spelling, so the
//! published document is exactly what the store types used to produce.
//! `the_copies_match_the_store_types` below keeps them from drifting.
//!
//! The change stream's wire types are copied here for the same reason: they
//! live with the engine's change bus, and the engine has no business knowing
//! about the API description and carries no `utoipa`.

#![allow(dead_code)]

/// What a user may do. Ordered least to most privileged; the REST layer maps
/// each endpoint to the minimum role it accepts.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Sees no engram on any shared domain. What it works on is its
    /// Reflection level and the private domains it is invited into.
    None,
    /// Read only: search, read, browse.
    Viewer,
    /// Everything a viewer may do, plus writing and editing engrams.
    Editor,
    /// Everything an editor may do, plus managing domains and users.
    Admin,
}

/// One account. Carries no password material, so it is safe to hand to a
/// handler and serialize into a response.
#[derive(Clone, Debug, serde::Serialize, utoipa::ToSchema)]
pub struct User {
    /// The login name and primary key. Also the identity the trusted-header
    /// mode provisions against.
    #[schema(example = "ada")]
    pub name: String,
    /// Human-readable name for the UI.
    #[schema(example = "Ada Lovelace")]
    pub display: String,
    /// Optional contact address; never used for login.
    #[schema(example = "ada@example.com")]
    pub email: Option<String>,
    /// What this account may do.
    pub role: Role,
    /// A disabled account keeps its rows but can neither log in nor use an
    /// already-issued session.
    pub disabled: bool,
    /// When this account last resolved a session or arrived through the
    /// trusted header, RFC 3339. Null for an account never seen.
    #[schema(example = "2026-08-08T09:14:22Z")]
    pub last_seen: Option<String>,
    /// The Reflection level on every shared domain.
    pub reflection: ReflectionLevel,
}

/// One identity an external provider asserts, tied to one account.
///
/// `(issuer, subject)` is the durable key: a username, an address and a
/// display name are all mutable presentation data, and none of them may move
/// an account. An account may hold several links (one per issuer), and a link
/// points at exactly one account.
#[derive(Clone, Debug, serde::Serialize, utoipa::ToSchema)]
pub struct IdentityLink {
    /// The provider that asserts this identity, as its ID tokens spell it.
    #[schema(example = "https://login.microsoftonline.com/<tenant>/v2.0")]
    pub issuer: String,
    /// The provider's stable identifier for the person.
    #[schema(example = "0f8fad5b-d9cb-469f-a165-70867728950e")]
    pub subject: String,
    /// When the link was made, RFC 3339.
    #[schema(example = "2026-09-07T09:14:22Z")]
    pub linked_at: String,
    /// Who made it: the account that linked it, an admin's name, or `jit` for
    /// a link a first sign-in created along with its account.
    #[schema(example = "jit")]
    pub linked_by: String,
}

/// One share-link on one draft, as the store holds it. Never carries the token:
/// only its sha256 is written, so a listing can say who holds a link and when
/// it was made and can never hand the link itself back out.
#[derive(Clone, Debug, serde::Serialize, utoipa::ToSchema)]
#[schema(description = "One share-link on one draft: which draft it opens, \
                        who minted it, which account redeemed it, and the two \
                        dates that can end it.")]
pub struct OverlayGrant {
    /// The row id, which is what revokes this link.
    pub id: i64,
    /// The domain the drafted engram lives in.
    #[schema(example = "team")]
    pub domain: String,
    /// The domain-relative path of the draft this link opens.
    #[schema(example = "plan.md")]
    pub path: String,
    /// The account whose draft it is: the actor the overlay entry belongs to.
    #[schema(example = "alice")]
    pub owner: String,
    /// The account this link bound itself to, or null while nobody has opened
    /// it yet. The first account to redeem it is that account for good.
    #[schema(example = "bob")]
    pub grantee: Option<String>,
    /// RFC 3339, when the link was minted.
    pub created_at: String,
    /// RFC 3339, when the link stops working on its own, or null for one that
    /// lasts as long as the draft does.
    pub expires_at: Option<String>,
    /// RFC 3339, when its author took it back, or null while it stands.
    pub revoked_at: Option<String>,
}

/// One row of an account's OAuth grant list: which client is connected, since
/// when, and until when it may keep refreshing. Never carries a token - only
/// hashes are stored, so there is nothing to show back.
#[derive(Clone, Debug, serde::Serialize, utoipa::ToSchema)]
pub struct OauthGrantInfo {
    /// The grant's id, which is what revokes it.
    pub id: i64,
    /// The registration this grant belongs to.
    pub client_id: String,
    /// The name that registration gave for itself, or a stand-in when the
    /// registration is gone.
    pub client_name: String,
    /// The host the client is redirected back to, for a person deciding
    /// whether they recognize this connection.
    pub redirect_host: String,
    /// RFC 3339, when the grant was created.
    pub created_at: String,
    /// RFC 3339, when one of its access tokens last resolved a request.
    pub last_used: Option<String>,
    /// RFC 3339, when the refresh token stops working unless it is rotated
    /// before then. For an ended grant, when it ended.
    pub refresh_expires_at: String,
    /// Set when the grant has ended and this row is only the record of why,
    /// listed for seven days; null for a live grant.
    pub ended: Option<OauthGrantEnded>,
}

/// When and why a grant ended, so a person can tell why a client is gone.
#[derive(Clone, Debug, serde::Serialize, utoipa::ToSchema)]
pub struct OauthGrantEnded {
    /// RFC 3339, when the grant ended.
    pub at: String,
    /// Why it ended.
    pub reason: OauthGrantEndReason,
}

/// Why a grant ended. Only the endings a person could not otherwise explain
/// are recorded: an expired grant was simply not used for thirty days, and a
/// removed account has nobody left to tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum OauthGrantEndReason {
    /// A rotated refresh token came back after the grace window, the replay
    /// rule of a refresh.
    Replayed,
    /// The account itself revoked it, on its Connected clients card.
    Revoked,
    /// The client revoked it with one of its own tokens (RFC 7009), for
    /// example on sign-out.
    ClientRevoked,
    /// The account's password was reset, which signs every client out.
    PasswordReset,
}

/// One row of an account's MCP token list, for a management UI or CLI. Never
/// carries the token itself - only the hash is stored, so there is nothing to
/// show back after issuance.
#[derive(Clone, Debug, serde::Serialize, utoipa::ToSchema)]
pub struct McpTokenInfo {
    /// The row id, used to revoke or rotate this token.
    pub id: i64,
    /// The caller-chosen label.
    pub label: String,
    /// RFC 3339, when this token was issued.
    pub created_at: String,
    /// RFC 3339, when this token last resolved a request. `None` if it has
    /// never been used.
    pub last_used: Option<String>,
}

/// What a member may do on one private domain. Ordered least to most
/// privileged, exactly as [`Role`] is, and deliberately a separate ladder: an
/// account's instance role says what it may do on the installation, this says
/// what it may do on one domain somebody invited it to.
///
/// `Manager` is the level that may invite and change other members' levels. It
/// may not flip the domain back to shared, and it may not hand the domain to
/// someone else: those two stay with the owner (and with an admin), which is
/// what keeps "who can see this at all" a decision the owner made.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum MemberLevel {
    /// Sees no engram of this domain; its Reflection level decides what it
    /// works on.
    None,
    /// Read only: this domain is visible and searchable, nothing more.
    Viewer,
    /// Everything a viewer may do, plus writing and editing its engrams.
    Editor,
    /// Everything an editor may do, plus managing this domain's membership.
    Manager,
}

/// The Reflection level an account holds on every shared domain, or a member
/// on one private domain, beside its knowledge level. It only ever raises
/// what the knowledge level gives: `Default` is "as knowledge",
/// stored as SQL NULL so no sentinel value is written. There is no stored
/// curator: curating follows from writing the engrams.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    utoipa::ToSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum ReflectionLevel {
    /// As the knowledge level gives: a viewer reads and comments, `none`
    /// sees nothing.
    #[default]
    Default,
    /// Reads the Reflection and comments on it.
    Reader,
    /// Also edits reflection text and creates, moves and deletes reflections.
    Contributor,
}

/// One membership row: who was invited to a private domain, at what level, by
/// whom and when.
#[derive(Clone, Debug, PartialEq, serde::Serialize, utoipa::ToSchema)]
pub struct DomainMember {
    /// The member's login name, folded by [`normalize_account_name`].
    pub principal: String,
    /// What this member may do here.
    pub level: MemberLevel,
    /// The Reflection level on this domain.
    pub reflection: ReflectionLevel,
    /// Who added or last changed this row. An audit field, stored as given:
    /// it is usually a login name but may name a non-account actor, the same
    /// latitude the identity-link plan gives `linked_by`.
    pub added_by: String,
    /// RFC 3339, when this row was last written.
    pub added_at: String,
}

/// One per-domain Reflection collaborator: an account given the Reflection
/// reader or contributor level on one domain. A grant only raises the
/// Reflection right and never opens an engram.
#[derive(Clone, Debug, PartialEq, serde::Serialize, utoipa::ToSchema)]
pub struct ReflectionGrant {
    /// The account's login name, folded.
    pub account: String,
    /// `reader` or `contributor`.
    pub level: ReflectionLevel,
    /// Who added or last changed this grant.
    pub added_by: String,
    /// RFC 3339, when this grant was last written.
    pub added_at: String,
}

/// One person GitHub lists on a governed domain's repository.
#[derive(Clone, Debug, PartialEq, serde::Serialize, utoipa::ToSchema)]
pub struct GithubPerson {
    /// The login GitHub lists.
    pub login: String,
    /// Their role on the repository: `admin`, `maintain`, `write`,
    /// `triage` or `read`.
    pub github_role: String,
    /// The level that role gives on this domain.
    pub level: MemberLevel,
    /// The account linked to that GitHub login here, or `null` when nobody
    /// here is linked to it.
    pub account: Option<String>,
}

/// A domain closed because the GitHub App is not installed on its
/// repository, or GitHub does not show the repository to the App.
#[derive(Clone, Debug, PartialEq, serde::Serialize, utoipa::ToSchema)]
pub struct ClosedDomain {
    /// The domain.
    pub domain: String,
    /// Its repository, `owner/name`.
    pub repo: String,
    /// What to do about it, in one sentence.
    pub reason: String,
}

/// What happened to the engram: the four things the sync engine counts, and
/// `files` for an attachment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Moved,
    /// An attachment under `assets/` was written, replaced or deleted; the
    /// event's `path` and `permalink` both carry the attachment path.
    Files,
}

/// Where a moved engram came from.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
pub struct MovedFrom {
    /// Domain-relative, forward slashes, `.md`.
    pub path: String,
    pub permalink: String,
}

/// One engram changed, as every subscriber hears it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
pub struct EngramChanged {
    pub domain: String,
    /// After the change; for `deleted`, what the row had before it went.
    pub permalink: String,
    /// Domain-relative, forward slashes, `.md`; after the change.
    pub path: String,
    pub kind: ChangeKind,
    /// `moved` only.
    pub from: Option<MovedFrom>,
    /// Lowercase hex SHA-256 of the new content, the value every write
    /// receipt reports as `checksum`. Absent for `deleted`.
    pub checksum: Option<String>,
    /// Who made the change, as a display label. Null for a change the watcher
    /// found on disk and for a write nobody signed.
    pub actor: Option<String>,
    /// Set when the change landed in this actor's draft overlay. Delivered to
    /// that actor's own sessions and to nobody else.
    pub draft_of: Option<String>,
    /// Who may hear this event, when that was captured for its domain's name:
    /// set by the bus, never by a feed point, while the domain is being
    /// renamed or removed (ruled 2026-09-27: a path under the old name leaks
    /// the name as surely as the domain event does). `None` keeps the
    /// ordinary per-session check. Never on the wire.
    #[serde(skip)]
    pub audience: Option<DomainAudience>,
}

/// The identities that may read a domain, captured once by the engine at the
/// moment of a change that takes the domain's name out of the privacy records
/// (a removal, and the old name of a rename; ruled 2026-09-27).
///
/// `Everyone` when the domain is not private. `Accounts` when it is, naming
/// every account that may read it: its owner, every member who reads engrams
/// there (a member at knowledge `none` does not) and every instance admin by
/// the role the accounts store records, disabled accounts left out. The names
/// are the store's folded spelling (trimmed and lowercased), so a check folds
/// `Scope::User.account` the same way, and a scope that carries `admin: true`
/// reads too, since the surface's own admin flag outranks the stored role.
/// The machine owner always reads, so it is never in the set. When the engine
/// cannot read the records (no resolver installed yet, or a read error) it
/// captures `Accounts` with nobody in it: the owner still hears the event,
/// nobody else does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DomainAudience {
    Everyone,
    Accounts(std::collections::HashSet<String>),
}

/// A whole domain moved: refetch everything of it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
pub struct DomainChanged {
    pub domain: String,
    pub actor: Option<String>,
    /// `Some`, resolved eagerly, only for a rename's old name and a removal:
    /// that name's privacy record leaves with the change, so the route must
    /// filter against this snapshot rather than a session's own lazy check.
    /// `None` keeps the ordinary per-session check for every other domain
    /// event. Never on the wire.
    #[serde(skip)]
    pub audience: Option<DomainAudience>,
}

/// What happened in a domain's Reflection, as the `reflection` event says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReflectionChangeKind {
    /// A page's text or bookkeeping changed: a write, a settle, a room's
    /// save, or a link that followed another page or file.
    Changed,
    /// A page moved; `from` is where it was.
    Moved,
    Deleted,
    /// A thread on the page was opened, answered, resolved or reopened.
    Threads,
    /// A file under `reflections/assets/` was added, replaced, removed or
    /// moved; `path` is the file, `from` its old path on a move.
    Files,
    /// The switch moved; `path` is empty.
    Switched,
}

/// One change in a domain's Reflection, as a session that may read it hears
/// it. A path and a kind, never text and never an engram name, so a person
/// with knowledge `none` may hear it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
pub struct ReflectionChanged {
    pub domain: String,
    /// Inside `reflections/`, after the change; empty for `switched`, and
    /// for `changed` when a sync found many paths at once (the whole
    /// Reflection, then).
    pub path: String,
    pub kind: ReflectionChangeKind,
    /// `moved`, and `files` on a move: the path before.
    pub from: Option<String>,
    /// Who made the change, as a display label; null for a link that
    /// followed, a room save with no single typist, the machine owner and a
    /// change found on disk.
    pub actor: Option<String>,
}

/// The kind of a draft conflict: a copy of the engine's `DraftConflictKind`,
/// which carries no `utoipa`. The draft conflict routes answer the engine's
/// JSON as it is; these copies only describe it.
#[derive(Debug, Clone, Copy, serde::Serialize, utoipa::ToSchema)]
#[schema(description = "Why a draft does not fit the team's text: `EditEdit` \
                        both changed the same lines, `EditDelete` the team \
                        deleted a page you drafted, `DeleteEdit` you deleted a \
                        page the team changed, `AddAdd` the team has a file \
                        where you did not start from one, `UnknownStart` a \
                        draft from before 0.32.0 with no record of its start, \
                        `AddressTaken` another team page holds your draft's \
                        address.")]
pub enum DraftConflictKind {
    EditEdit,
    EditDelete,
    DeleteEdit,
    AddAdd,
    UnknownStart,
    AddressTaken,
}

/// One of the caller's draft conflicts, as `GET .../drafts/conflicts` lists it.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
#[schema(description = "One of your drafts that does not fit the team's text.")]
pub struct DraftConflictEntry {
    /// `d` and seven hex digits, stable while the draft stands.
    #[schema(example = "d3f9a1c2")]
    pub id: String,
    /// The draft's path, or the deletion's when it stands alone.
    #[schema(example = "notes/plan.md")]
    pub path: String,
    pub kind: DraftConflictKind,
    /// Always true: the entry is a draft's, never the folder's.
    pub draft: bool,
    /// The path a moved draft left.
    #[schema(required = true)]
    pub moved_from: Option<String>,
    /// A file beside the pages rather than a page.
    pub file: bool,
    /// For `AddressTaken`: the team's path that holds the address.
    #[schema(required = true)]
    pub other: Option<String>,
    /// For `AddressTaken`: the page's own conflict, when the team also
    /// changed or deleted the page you drafted. Null otherwise. A merged text
    /// with a new address settles the address only; this one is still to
    /// settle after it.
    #[schema(required = true)]
    pub page_kind: Option<DraftConflictKind>,
}

/// What `GET /domains/{domain}/drafts/conflicts` answers.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
#[schema(description = "Your own draft conflicts in this domain, and the \
                        drafts an update merged team changes into since they \
                        last settled.")]
pub struct DraftConflictsResponse {
    /// Ordered by path.
    pub conflicts: Vec<DraftConflictEntry>,
    /// Paths of your drafts a merge changed, sorted.
    #[schema(example = json!(["notes/plan.md"]))]
    pub merged: Vec<String>,
    /// What you may not do with these conflicts here, in a sentence to show:
    /// on a read-only server or without write access none can be settled,
    /// and on a read-only domain or an archived repository only the team's
    /// text can be taken. Null when every choice is open or nothing is listed.
    #[schema(required = true)]
    pub settle_note: Option<String>,
}

/// What `GET /domains/{domain}/drafts/conflicts/{id}` answers.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
#[schema(description = "One of your draft conflicts with its three sides: \
                        `base` is the text your draft started from, `local` \
                        your draft and `upstream` the team's text. For \
                        `AddressTaken` `upstream` is the page that holds your \
                        address, and `page_upstream` the team's text of your \
                        own page when `page_kind` says it changed too. A side \
                        with no text is null and `note` says why. A file has \
                        no text sides, and carries `file: true`.")]
pub struct DraftConflictSides {
    #[schema(example = "d3f9a1c2")]
    pub id: String,
    #[schema(example = "notes/plan.md")]
    pub path: String,
    pub kind: DraftConflictKind,
    /// Always true.
    pub draft: bool,
    /// Always null: a draft conflict is read from what stands, never stored.
    #[schema(required = true)]
    pub detected_at: Option<String>,
    #[schema(required = true)]
    pub base: Option<String>,
    #[schema(required = true)]
    pub local: Option<String>,
    #[schema(required = true)]
    pub upstream: Option<String>,
    /// What the team's side is: the SHA-256 of its bytes (a page's text or a
    /// file's), `absent` when the team holds nothing there, `unreadable` when
    /// its copy cannot be read. With a `page_kind` it is one token for both
    /// team sides. Pass it back as `expected_upstream_sha256` when settling,
    /// so a team change made since is not overwritten.
    #[schema(example = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08")]
    pub upstream_sha256: String,
    #[schema(required = true)]
    pub moved_from: Option<String>,
    #[schema(required = true)]
    pub other: Option<String>,
    /// For `AddressTaken`: the page's own conflict, as in the list.
    #[schema(required = true)]
    pub page_kind: Option<DraftConflictKind>,
    /// For `AddressTaken` with a `page_kind`: the team's text of your own
    /// page. Null when the team deleted it, or when there is no `page_kind`.
    #[schema(required = true)]
    pub page_upstream: Option<String>,
    /// A draft from before 0.32.0, with no record of its start: taking the
    /// team's side keeps a copy of it first.
    pub old_draft: bool,
    /// For your deletion of a page or file the team moved: where it went.
    /// Keeping your side then removes nothing, since the team's copy stands
    /// there.
    #[schema(required = true)]
    pub moved_to: Option<String>,
    #[schema(required = true)]
    pub note: Option<String>,
    /// Present, and true, only for a file.
    pub file: Option<bool>,
}

/// What `POST /domains/{domain}/drafts/conflicts/{id}/resolve` answers.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
#[schema(description = "Where the resolution landed: in your draft.")]
pub struct DraftResolveReceipt {
    /// The draft's path. A move is settled whole, at its draft.
    #[schema(example = "notes/plan.md")]
    pub resolved: String,
    /// How many of your conflicts in this domain are left, or null when they
    /// could not be counted.
    #[schema(required = true)]
    pub remaining: Option<u64>,
    /// Always true.
    pub draft: bool,
    /// Present, and true, only for a file.
    pub file: Option<bool>,
    /// `mine`, `theirs` or `merged`.
    #[schema(example = "mine")]
    pub kept: String,
    /// Your side was a deletion: keeping it removes the team's page or file
    /// when you share, unless `moved_to` is set.
    pub deletion: bool,
    /// For a kept deletion of a page or file the team moved: where it went.
    /// Your deletion then removes nothing, and it ends.
    pub moved_to: Option<String>,
    /// Whether a copy of your text or file was kept before taking the
    /// team's.
    pub kept_copy: bool,
    /// For a file that was kept: where to find it.
    pub kept_note: Option<String>,
    /// Always null over this API: a place on the server's disk is named only
    /// to the machine owner.
    pub kept_file: Option<String>,
    /// Present when the draft landed but its copy on this server's disk did
    /// not.
    pub draft_warning: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::super::auth_store;
    use utoipa::openapi::RefOr;
    use utoipa::openapi::schema::Schema;

    /// The property names a copy publishes, or its enum values.
    fn published<T: utoipa::PartialSchema>() -> Vec<String> {
        let mut names: Vec<String> = match T::schema() {
            RefOr::T(Schema::Object(object)) => match object.enum_values {
                Some(values) => values
                    .iter()
                    .map(|v| v.as_str().unwrap().to_string())
                    .collect(),
                None => object.properties.keys().cloned().collect(),
            },
            other => panic!(
                "an unexpected schema shape: {}",
                serde_json::to_string(&other).unwrap()
            ),
        };
        names.sort();
        names
    }

    /// The keys (or, for an enum, the spellings) a store value serializes to.
    fn keys<T: serde::Serialize>(values: impl IntoIterator<Item = T>) -> Vec<String> {
        let mut keys: Vec<String> = Vec::new();
        for value in values {
            match serde_json::to_value(value).unwrap() {
                serde_json::Value::Object(map) => keys.extend(map.keys().cloned()),
                serde_json::Value::String(s) => keys.push(s),
                other => panic!("{other}"),
            }
        }
        keys.sort();
        keys
    }

    /// A copy that drifts from its store type publishes a document that lies
    /// about the wire: every field and every enum value must be the same.
    #[test]
    fn the_copies_match_the_store_types() {
        use auth_store::{MemberLevel, ReflectionLevel, Role};
        let s = String::new;
        assert_eq!(
            keys([Role::None, Role::Viewer, Role::Editor, Role::Admin]),
            published::<super::Role>()
        );
        assert_eq!(
            keys([
                MemberLevel::None,
                MemberLevel::Viewer,
                MemberLevel::Editor,
                MemberLevel::Manager
            ]),
            published::<super::MemberLevel>()
        );
        assert_eq!(
            keys([
                ReflectionLevel::Default,
                ReflectionLevel::Reader,
                ReflectionLevel::Contributor
            ]),
            published::<super::ReflectionLevel>()
        );
        assert_eq!(
            keys([auth_store::User {
                name: s(),
                display: s(),
                email: None,
                role: Role::Viewer,
                disabled: false,
                last_seen: None,
                reflection: ReflectionLevel::Default,
            }]),
            published::<super::User>()
        );
        assert_eq!(
            keys([auth_store::IdentityLink {
                issuer: s(),
                subject: s(),
                linked_at: s(),
                linked_by: s(),
            }]),
            published::<super::IdentityLink>()
        );
        assert_eq!(
            keys([auth_store::OverlayGrant {
                id: 0,
                domain: s(),
                path: s(),
                owner: s(),
                grantee: None,
                created_at: s(),
                expires_at: None,
                revoked_at: None,
            }]),
            published::<super::OverlayGrant>()
        );
        assert_eq!(
            keys([auth_store::OauthGrantInfo {
                id: 0,
                client_id: s(),
                client_name: s(),
                redirect_host: s(),
                created_at: s(),
                last_used: None,
                refresh_expires_at: s(),
                ended: None,
            }]),
            published::<super::OauthGrantInfo>()
        );
        assert_eq!(
            keys([auth_store::OauthGrantEnded {
                at: s(),
                reason: auth_store::OauthGrantEndReason::Revoked,
            }]),
            published::<super::OauthGrantEnded>()
        );
        assert_eq!(
            keys([
                auth_store::OauthGrantEndReason::Replayed,
                auth_store::OauthGrantEndReason::Revoked,
                auth_store::OauthGrantEndReason::ClientRevoked,
                auth_store::OauthGrantEndReason::PasswordReset,
            ]),
            published::<super::OauthGrantEndReason>()
        );
        assert_eq!(
            keys([auth_store::McpTokenInfo {
                id: 0,
                label: s(),
                created_at: s(),
                last_used: None,
            }]),
            published::<super::McpTokenInfo>()
        );
        assert_eq!(
            keys([auth_store::DomainMember {
                principal: s(),
                level: MemberLevel::Viewer,
                reflection: ReflectionLevel::Default,
                added_by: s(),
                added_at: s(),
            }]),
            published::<super::DomainMember>()
        );
        assert_eq!(
            keys([auth_store::ReflectionGrant {
                account: s(),
                level: ReflectionLevel::Reader,
                added_by: s(),
                added_at: s(),
            }]),
            published::<super::ReflectionGrant>()
        );
        assert_eq!(
            keys([crystalline_engine::engine::GithubPerson {
                login: s(),
                github_role: s(),
                level: MemberLevel::Viewer,
                account: None,
            }]),
            published::<super::GithubPerson>()
        );
        assert_eq!(
            keys([crystalline_engine::engine::ClosedDomain {
                domain: s(),
                repo: s(),
                reason: s(),
            }]),
            published::<super::ClosedDomain>()
        );
        use crystalline_engine::engine::{DraftConflict, DraftConflictKind as Kind};
        assert_eq!(
            keys([
                Kind::EditEdit,
                Kind::EditDelete,
                Kind::DeleteEdit,
                Kind::AddAdd,
                Kind::UnknownStart,
                Kind::AddressTaken
            ]),
            published::<super::DraftConflictKind>()
        );
        // The list adds `draft` to each conflict, as the status does.
        let mut listed = keys([DraftConflict {
            id: s(),
            path: s(),
            kind: Kind::EditEdit,
            moved_from: None,
            file: false,
            other: None,
            page_kind: None,
        }]);
        listed.push("draft".to_string());
        listed.sort();
        assert_eq!(listed, published::<super::DraftConflictEntry>());
        use crystalline_engine::changes as bus;
        assert_eq!(
            keys([
                bus::ChangeKind::Added,
                bus::ChangeKind::Modified,
                bus::ChangeKind::Deleted,
                bus::ChangeKind::Moved,
                bus::ChangeKind::Files
            ]),
            published::<super::ChangeKind>()
        );
        assert_eq!(
            keys([bus::MovedFrom {
                path: s(),
                permalink: s()
            }]),
            published::<super::MovedFrom>()
        );
        assert_eq!(
            keys([bus::EngramChanged {
                domain: s(),
                permalink: s(),
                path: s(),
                kind: bus::ChangeKind::Moved,
                from: None,
                checksum: None,
                actor: None,
                draft_of: None,
                audience: None,
            }]),
            published::<super::EngramChanged>()
        );
        assert_eq!(
            keys([bus::DomainChanged {
                domain: s(),
                actor: None,
                audience: None
            }]),
            published::<super::DomainChanged>()
        );
        assert_eq!(
            keys([
                bus::ReflectionChangeKind::Changed,
                bus::ReflectionChangeKind::Moved,
                bus::ReflectionChangeKind::Deleted,
                bus::ReflectionChangeKind::Threads,
                bus::ReflectionChangeKind::Files,
                bus::ReflectionChangeKind::Switched,
            ]),
            published::<super::ReflectionChangeKind>()
        );
        assert_eq!(
            keys([bus::ReflectionChanged {
                domain: s(),
                path: s(),
                kind: bus::ReflectionChangeKind::Moved,
                from: None,
                actor: None,
                checksum: None,
                audience: None,
            }]),
            published::<super::ReflectionChanged>()
        );
    }
}
