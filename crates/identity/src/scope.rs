//! Who is asking, and what that entitles them to on one domain.
//!
//! Two pieces sit here. [`Scope`] is what every surface resolves once per
//! request and threads inward: the CLI, the control socket and the local stdio
//! MCP stack are the machine owner and pass [`Scope::Unrestricted`]; an
//! authenticated HTTP caller passes [`Scope::User`]; an HTTP caller on an
//! instance with authentication off is [`Scope::Anonymous`]. [`DomainAccess`]
//! is the resolver that turns one of those, plus the membership records in the
//! auth database, into a [`DomainRight`].
//!
//! The policy itself is one small synchronous function, `decide`. Both public
//! answers - [`DomainAccess::right`] for a single domain and
//! [`DomainAccess::visibility`] for the whole set - are folds over it, so
//! the per-domain answer and the bulk filter cannot drift apart. They read the
//! same records differently (one domain's row against one user's whole
//! membership list) purely to keep the bulk path from being N queries, and that
//! is the only difference between them. An account's write list (`crystalline
//! users writes`) caps both answers at [`DomainRight::Read`] on every domain
//! it does not name, in `resolved_right`; visibility is not affected.
//!
//! Beside the engram ladder sits the Reflection's: [`ReflectionRight`],
//! decided by `decide_reflection` from the knowledge right and the
//! Reflection level that applies (the account's on a shared domain, the
//! membership row's on a private one). An explicit level only raises, every
//! knowledge editor curates, and the write list caps it at Read as well.
//!
//! On a domain connected to GitHub through the GitHub App (a "governed"
//! domain, named by the [`KnowledgeGovernor`] the engine installs with
//! [`DomainAccess::with_governor`]) both ladders follow the person's GitHub
//! role on the repository instead of the domain's owner and membership
//! rows (`decide_governed`). Crystalline's own shared/private switch still
//! says who the domain is for: a shared one is read by everyone signed in
//! at instance viewer or above, a private one only by GitHub readers.
//! Writes there need a GitHub link and GitHub write or more, are not capped
//! by the instance role and still by the write list. Without a governor
//! every answer is what it was before.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::Result;

use crate::auth_store::{AuthStore, DomainAcl, GithubLink, MemberLevel, ReflectionLevel, Role};

/// The fixed identity name the machine owner's personal credential is stored
/// under - the CLI and stdio MCP have no account to be, so they share one local
/// name rather than inventing one per machine. `crystalline connect github
/// --personal` with no `--as` writes exactly this slot.
///
/// It is also the actor key the machine owner's private drafts carry in a
/// domain that reviews changes: the overlay rows, the journal folder, the files
/// overlay inside it. So it is a reserved login, refused case-folded by
/// [`crate::auth_store::normalize_new_account_name`], because an account
/// holding this name would be acting in the machine owner's own unshared work
/// everywhere at once.
pub const OWNER_IDENTITY_NAME: &str = "owner";

/// Who a request is acting as.
///
/// Resolved once, at the edge, by whichever surface accepted the request, and
/// passed down from there. Nothing below the edge re-derives it, so there is
/// exactly one place per surface that decides who somebody is.
#[derive(Clone, Debug, PartialEq)]
pub enum Scope {
    /// The machine owner: the `crystalline` CLI, the control socket, and a
    /// local stdio MCP session. Whoever can run those already has the files on
    /// disk, so there is nothing here for a check to protect.
    Unrestricted,
    /// A signed-in account. `admin` is the instance role the surface already
    /// resolved, carried along so a check does not have to read it back.
    User {
        /// The login name, as the auth store keys it.
        account: String,
        /// Whether that account holds the instance-wide admin role.
        admin: bool,
    },
    /// Nobody in particular: the `auth.anonymous` viewer tier, and an HTTP MCP
    /// session on an instance that has MCP authentication switched off.
    Anonymous,
    /// The MCP open tier acting as the account `github.agent_identity`
    /// names, on a domain GitHub governs. Never resolved at an edge: the
    /// engine puts it in place of [`Scope::Anonymous`] for one question
    /// about one governed domain. It is that account with an instance role
    /// of at most editor, so an admin named here gives the unauthenticated
    /// open tier its GitHub grant and nothing more.
    Agent {
        /// The login name, as the auth store keys it.
        account: String,
    },
}

/// Whose draft overlay a caller acts in on a domain that reviews changes
/// before they land, or `None` when the caller has no identity for a draft to
/// belong to.
///
/// One function, so every write verb and every read verb asks the same
/// question the same way. The mapping is the whole of the policy:
///
/// * the machine owner drafts as [`OWNER_IDENTITY_NAME`]. Whoever runs
///   the CLI, the control socket or a local stdio session is one actor with
///   one name, and naming them makes the owner's own work reviewable beside
///   everybody else's rather than a special case that bypasses review;
/// * a signed-in account drafts under its own login name, which is the key the
///   membership rows and the overlay rows already share. The instance admin
///   flag is deliberately not read: a role widens what an account may reach,
///   never whose draft its writing is;
/// * nobody in particular holds nothing. There is no identity for a draft to
///   belong to, so there is no draft, and the write verbs turn this into a
///   refusal that teaches how to connect rather than into a silent write.
pub fn overlay_actor(scope: &Scope) -> Option<String> {
    match scope {
        Scope::Unrestricted => Some(OWNER_IDENTITY_NAME.to_string()),
        Scope::User { account, .. } => Some(account.clone()),
        // The open tier's writes run as anonymous; the agent stand-in only
        // answers rights questions, so it holds no draft.
        Scope::Anonymous | Scope::Agent { .. } => None,
    }
}

/// What presenting a share-link opened: which draft, and how long the link
/// goes on opening it.
///
/// The redeeming caller needs both - the address to route the work to, and the
/// window to stamp onto the join it opens - and neither is meaningful without
/// the other here.
pub struct RedeemedLink {
    /// The domain the draft lives in.
    pub domain: String,
    /// Whose draft it is: the actor whose overlay the writes land in.
    pub owner: String,
    /// The domain-relative path of the draft the link was minted on.
    pub path: String,
    /// RFC 3339, when the link stops working on its own, or `None` for one
    /// that lasts as long as the draft does.
    pub expires_at: Option<String>,
}

/// What a scope may do on one domain. Ordered least to most privileged, so a
/// caller writes `right >= DomainRight::Write` rather than matching every arm.
///
/// This answers the *domain* question only. An instance-role gate can still
/// refuse afterwards: an account that is a domain editor on a private domain
/// but only a viewer on the instance is refused by the role gate the REST write
/// routes already carry, and that is deliberate - a domain invitation widens
/// what an account may reach, never what its instance role lets it do.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub enum DomainRight {
    /// Not visible at all. Every surface answers as though the domain did not
    /// exist, because its existence is the secret being kept.
    None,
    /// May read: search, browse, open engrams.
    Read,
    /// May read and write content.
    Write,
    /// May read, write, and change who else is a member.
    Manage,
    /// Everything, including making the domain shared again and handing it on.
    Own,
}

/// What a caller holding this right is called on the domain, for a refusal
/// that has to name it. Only [`DomainRight::Read`] reaches a message today;
/// the rest are spelled out so the mapping is complete rather than a default
/// arm that would print "viewer" for something else one day.
pub fn member_level_word(right: DomainRight) -> &'static str {
    match right {
        DomainRight::None => "none",
        DomainRight::Read => "viewer",
        DomainRight::Write => "editor",
        DomainRight::Manage => "manager",
        DomainRight::Own => "owner",
    }
}

/// What a scope may do with one domain's Reflection. Beside [`DomainRight`]
/// rather than inside it, so the engram ladder stays untouched: a person with
/// knowledge `none` and Reflection `contributor` holds `DomainRight::None`
/// and `ReflectionRight::Edit`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReflectionRight {
    /// Nothing: the Reflection is not there for this caller.
    None,
    /// Read reflections and comment: open, answer, resolve and reopen
    /// threads. Reading and commenting are one level; only the anonymous
    /// visitor of a public domain reads without commenting
    /// ([`ReflectionCaller::may_comment`]).
    Read,
    /// Also edit reflection text, create, move and delete reflections.
    Edit,
    /// Also write sections with their source engrams, settle feedback items and
    /// edit PACT.md. Held by every knowledge editor and above, never granted
    /// on its own.
    Curate,
}

/// The wire word for a reflection right.
pub fn reflection_right_word(right: ReflectionRight) -> &'static str {
    match right {
        ReflectionRight::None => "none",
        ReflectionRight::Read => "read",
        ReflectionRight::Edit => "edit",
        ReflectionRight::Curate => "curate",
    }
}

/// What GitHub says one account may do on one governed domain's repository,
/// as [`KnowledgeGovernor::knowledge`] answers it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GithubKnowledge {
    /// The account is linked to a GitHub login and holds this level on the
    /// repository: admin and maintain are [`MemberLevel::Manager`], write is
    /// [`MemberLevel::Editor`], triage and read are [`MemberLevel::Viewer`],
    /// and no access (or an answer GitHub could not give in time) is
    /// [`MemberLevel::None`].
    Level(MemberLevel),
    /// The account has no GitHub link, so GitHub grants it nothing.
    Unlinked,
}

/// Who decides engram access on the domains connected to GitHub through the
/// GitHub App ("governed" domains). The engine implements it; this crate
/// only asks it, in its own terms: account names as the accounts store
/// spells them, domain names as the engine registers them, and
/// [`MemberLevel`].
///
/// On a governed domain the domain's own `domain_member` rows and its
/// `domain_acl` owner are not read: what a person may do follows their
/// GitHub role on the repository, and the `domain_acl` row only says
/// whether the domain is private (only GitHub readers) or shared (everyone
/// signed in). Without a governor every answer is what it was before.
#[async_trait::async_trait]
pub trait KnowledgeGovernor: Send + Sync {
    /// The names of the domains whose access follows GitHub right now.
    fn governed_domains(&self) -> HashSet<String>;
    /// What GitHub grants `account` on `domain`'s repository.
    async fn knowledge(&self, account: &str, domain: &str) -> GithubKnowledge;
    /// Whether `domain`'s repository is public on GitHub. `false` when it
    /// is not known.
    async fn repository_public(&self, domain: &str) -> bool;
    /// The linked accounts with GitHub read or more on `domain`'s
    /// repository, from one listing of the repository's collaborators.
    ///
    /// It must equal the set of accounts whose
    /// [`KnowledgeGovernor::knowledge`] on `domain` is
    /// `Level(Viewer | Editor | Manager)`, answered from the same cached
    /// snapshot, so the reader sets and the per-account answers never
    /// disagree. Names are the accounts store's own folded spelling: they
    /// are compared byte for byte with the store's names, so any other
    /// spelling matches no account and fails closed. `None` when the
    /// listing is not known right now; every caller then fails closed.
    async fn readers(&self, domain: &str) -> Option<HashSet<String>>;
}

/// The GitHub half of [`decide`]'s inputs on a governed domain: what GitHub
/// grants the asking account, and whether the repository is public (which
/// is what the anonymous caller is decided by).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Governed {
    /// The account's GitHub grant. [`GithubKnowledge::Unlinked`] for every
    /// principal that is not an account.
    knowledge: GithubKnowledge,
    /// Whether the repository is public on GitHub. Only read for an
    /// anonymous caller on a shared domain.
    public: bool,
}

/// The engram right a GitHub grant gives on a governed domain.
fn github_right(knowledge: GithubKnowledge) -> DomainRight {
    match knowledge {
        GithubKnowledge::Level(MemberLevel::Manager) => DomainRight::Manage,
        GithubKnowledge::Level(MemberLevel::Editor) => DomainRight::Write,
        GithubKnowledge::Level(MemberLevel::Viewer) => DomainRight::Read,
        GithubKnowledge::Level(MemberLevel::None) | GithubKnowledge::Unlinked => DomainRight::None,
    }
}

/// The scope resolved against the accounts table: what [`decide`] actually
/// needs, with every question that requires a database read already answered.
#[derive(Clone, Debug, PartialEq)]
enum Principal {
    /// [`Scope::Unrestricted`], carried through so [`decide`] is total.
    Unrestricted,
    /// A live account and the instance role it acts with.
    Account {
        /// The store's own spelling of the login name, which is what the acl
        /// and membership rows are keyed on.
        name: String,
        /// The instance role. [`Scope::User::admin`] is folded in here: a scope
        /// that says admin resolves as [`Role::Admin`] whatever the row says,
        /// so the surface that authenticated the caller stays the authority on
        /// that flag.
        role: Role,
        /// The account's Reflection level on every shared domain.
        reflection: ReflectionLevel,
    },
    /// Nobody in particular.
    Anonymous,
}

/// The whole policy, as a pure function of one caller and one domain.
///
/// `acl` is `None` for a shared domain and `Some` for a private one; `level` is
/// the caller's membership row on that domain, if any (never consulted for a
/// shared domain, which has no membership).
///
/// The ladder, in the order it is decided:
///
/// * the machine owner and any instance admin get [`DomainRight::Own`]
///   everywhere, private domains included. An admin already manages every
///   account on the installation, so a domain it could not see would be a
///   secret kept from the person who can simply grant themselves the account
///   that holds it;
/// * on a shared domain everyone gets their instance role's right, and an
///   anonymous caller reads. There is no owner on a shared domain and none is
///   invented;
/// * on a private domain the owner owns it, a member gets its level, and
///   everybody else - anonymous callers included - gets nothing.
///
/// `github` is `Some` exactly when the domain is governed by GitHub (see
/// [`KnowledgeGovernor`]); `decide_governed` answers it then, and `level`
/// and the acl's owner are not read. `None` leaves the ladder above
/// untouched.
fn decide(
    principal: &Principal,
    acl: Option<&DomainAcl>,
    level: Option<MemberLevel>,
    github: Option<Governed>,
) -> DomainRight {
    if let Some(governed) = github {
        return decide_governed(principal, acl.is_some(), governed);
    }
    match principal {
        Principal::Unrestricted => DomainRight::Own,
        Principal::Anonymous => match acl {
            Some(_) => DomainRight::None,
            None => DomainRight::Read,
        },
        Principal::Account { name, role, .. } => {
            if *role == Role::Admin {
                return DomainRight::Own;
            }
            let Some(acl) = acl else {
                // The `Admin` arm cannot be reached from here today: the
                // early return above already answered every admin. It is
                // spelled out rather than folded into a catch-all so this
                // ladder stays a complete statement of what each instance
                // role gets on a shared domain, and so that removing or
                // narrowing that early return changes what an admin gets in
                // one obvious place instead of silently dropping it to the
                // wrong rung.
                return match role {
                    Role::None => DomainRight::None,
                    Role::Viewer => DomainRight::Read,
                    Role::Editor => DomainRight::Write,
                    Role::Admin => DomainRight::Own,
                };
            };
            if acl.owner == *name {
                return DomainRight::Own;
            }
            match level {
                Some(MemberLevel::Manager) => DomainRight::Manage,
                Some(MemberLevel::Editor) => DomainRight::Write,
                Some(MemberLevel::Viewer) => DomainRight::Read,
                // A knowledge-`none` member reads no engram here; its
                // Reflection level is decided by `decide_reflection`.
                Some(MemberLevel::None) => DomainRight::None,
                None => DomainRight::None,
            }
        }
    }
}

/// [`decide`] on a domain governed by GitHub. Only whether the domain is
/// private (a `domain_acl` row exists) is read from Crystalline's records;
/// the acl's owner and every `domain_member` row are not.
///
/// The ladder, in the order it is decided:
///
/// * the machine owner and any instance admin get [`DomainRight::Own`],
///   whatever GitHub says;
/// * an account at instance knowledge `none` gets nothing, whatever GitHub
///   says;
/// * an anonymous caller reads a shared domain whose repository is public
///   on GitHub, and nothing else. It never writes;
/// * every other account gets the higher of the domain's base (Read on a
///   shared domain, nothing on a private one) and its GitHub grant (manager
///   manages, editor writes, viewer reads, GitHub none or no link gives
///   nothing). The instance role does not raise it: an unlinked instance
///   editor reads a shared GitHub domain and writes nothing there. It does
///   not cap it either ([`DomainAccess::write_right`] skips the instance
///   cap here): an instance viewer with GitHub write writes.
fn decide_governed(principal: &Principal, private: bool, governed: Governed) -> DomainRight {
    match principal {
        Principal::Unrestricted => DomainRight::Own,
        Principal::Anonymous => {
            if !private && governed.public {
                DomainRight::Read
            } else {
                DomainRight::None
            }
        }
        Principal::Account { role, .. } => match role {
            Role::Admin => DomainRight::Own,
            Role::None => DomainRight::None,
            Role::Viewer | Role::Editor => {
                let base = if private {
                    DomainRight::None
                } else {
                    DomainRight::Read
                };
                let github = github_right(governed.knowledge);
                if github > base { github } else { base }
            }
        },
    }
}

/// The most a principal's instance role lets it do on any domain at all.
///
/// The other half of [`DomainAccess::write_right`]: [`decide`] answers what the
/// domain grants, this answers what the instance permits, and a write gate
/// takes the lesser of the two. Spelled as a total match rather than a
/// comparison against one role so adding a role has to answer this question.
fn instance_cap(principal: &Principal) -> DomainRight {
    match principal {
        Principal::Unrestricted => DomainRight::Own,
        Principal::Anonymous => DomainRight::Read,
        Principal::Account { role, .. } => match role {
            // It never writes an engram. `Read`, not `None`, so a write gate
            // refuses it as forbidden on a private domain it reads instead of
            // answering it as unknown.
            Role::None => DomainRight::Read,
            Role::Viewer => DomainRight::Read,
            Role::Editor => DomainRight::Write,
            Role::Admin => DomainRight::Own,
        },
    }
}

/// The Reflection half of the policy: the knowledge right first ([`decide`],
/// before the instance cap), then the Reflection level that applies here.
///
/// * the machine owner curates; an anonymous caller reads what it may read
///   as engrams (a public domain) and nothing else;
/// * knowledge `Write` or more curates, whatever the explicit level says;
/// * below that the right is the higher of the default (knowledge `Read`
///   gives `Read`, `None` gives `None`) and the explicit level: it raises,
///   never lowers;
/// * the explicit level is the account's instance level on a shared domain
///   and the membership row's level on a private one; an account with no
///   membership row on a private domain has none;
/// * `off_list`: the account's write list leaves this domain out. The
///   answer is then at most `Read`, whatever the knowledge right and the
///   explicit level say, and `None` stays `None` (a cap, never a grant).
///
/// On a domain governed by GitHub (`github` is `Some`) the knowledge right
/// is `decide_governed`'s, so a GitHub writer curates, a GitHub reader
/// reads and comments, and an unlinked account or one at GitHub none
/// follows the domain's base: read and comment on a shared domain, nothing
/// on a private one. The explicit level is the account's instance level on
/// a shared domain and none on a private one: a membership row and its
/// Reflection level are not read. An account at instance knowledge `none`
/// gets nothing there at all, its instance Reflection level included.
fn decide_reflection(
    principal: &Principal,
    acl: Option<&DomainAcl>,
    level: Option<MemberLevel>,
    member_reflection: ReflectionLevel,
    github: Option<Governed>,
    off_list: bool,
) -> ReflectionRight {
    let knowledge = decide(principal, acl, level, github);
    let reflection = match principal {
        Principal::Unrestricted => return ReflectionRight::Curate,
        Principal::Anonymous => {
            return if knowledge >= DomainRight::Read {
                ReflectionRight::Read
            } else {
                ReflectionRight::None
            };
        }
        Principal::Account { role, .. } if github.is_some() && *role == Role::None => {
            return ReflectionRight::None;
        }
        Principal::Account { reflection, .. } if github.is_some() => match acl {
            None => *reflection,
            Some(_) => ReflectionLevel::Default,
        },
        Principal::Account { reflection, .. } => match (acl, level) {
            (None, _) => *reflection,
            (Some(_), Some(_)) => member_reflection,
            (Some(_), None) => ReflectionLevel::Default,
        },
    };
    let right = if knowledge >= DomainRight::Write {
        ReflectionRight::Curate
    } else {
        let default = if knowledge >= DomainRight::Read {
            ReflectionRight::Read
        } else {
            ReflectionRight::None
        };
        let raised = match reflection {
            ReflectionLevel::Default => ReflectionRight::None,
            ReflectionLevel::Reader => ReflectionRight::Read,
            ReflectionLevel::Contributor => ReflectionRight::Edit,
        };
        default.max(raised)
    };
    // 0.25.0's write list caps the whole account, the Reflection too.
    if off_list {
        right.min(ReflectionRight::Read)
    } else {
        right
    }
}

/// Who read a Reflection page when a person shared text on it on purpose
/// ([`DomainAccess::page_readers`]): what a confirmation is held against
/// later ([`DomainAccess::readers_cover_for`]).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PageReaders {
    /// A shared domain's page: everyone who reaches the instance, anonymous
    /// callers and accounts made later included.
    Everyone,
    /// A private domain's page: these accounts, by their folded names.
    Accounts(std::collections::BTreeSet<String>),
}

/// Who calls a reflection verb. Every reflection verb takes one, because
/// three things travel together: the scope the rights are decided for, the
/// name a thread entry and the `changed_by` bookkeeping record, and whether
/// the call is the MCP open tier (`auth.mcp` off), which keeps writing what
/// it always wrote on every domain it can see.
#[derive(Clone, Debug, PartialEq)]
pub struct ReflectionCaller {
    /// Who the rights are decided for.
    pub scope: Scope,
    /// Who shows up in threads and in the bookkeeping.
    pub author: crystalline_core::threads::Author,
    /// The MCP open tier: curate every domain the scope can see.
    pub open_tier: bool,
}

impl ReflectionCaller {
    /// The machine owner: the CLI, the control socket, a stdio session.
    pub fn owner() -> ReflectionCaller {
        ReflectionCaller {
            scope: Scope::Unrestricted,
            author: crystalline_core::threads::Author {
                name: OWNER_IDENTITY_NAME.to_string(),
                agent: false,
            },
            open_tier: false,
        }
    }

    /// A person, through the JSON API.
    pub fn person(scope: Scope, name: &str) -> ReflectionCaller {
        ReflectionCaller {
            scope,
            author: crystalline_core::threads::Author {
                name: crystalline_core::threads::sanitize_name(name),
                agent: false,
            },
            open_tier: false,
        }
    }

    /// An agent, through MCP.
    pub fn agent(scope: Scope, name: &str, open_tier: bool) -> ReflectionCaller {
        ReflectionCaller {
            scope,
            author: crystalline_core::threads::Author {
                name: crystalline_core::threads::sanitize_name(name),
                agent: true,
            },
            open_tier,
        }
    }

    /// Whether this caller may open, answer, resolve or reopen a thread when
    /// it holds `Read`. Everyone may, except the anonymous visitor of a
    /// public domain: it reads, and a comment needs somebody to sign it.
    pub fn may_comment(&self) -> bool {
        self.open_tier || !matches!(self.scope, Scope::Anonymous)
    }
}

/// `right` under the instance cap of `principal`: an invitation widens what
/// an account may reach, never what its role lets it do.
fn capped(principal: &Principal, right: DomainRight) -> DomainRight {
    let cap = instance_cap(principal);
    if right < cap { right } else { cap }
}

/// [`capped`] on a domain Crystalline decides, `right` itself on one GitHub
/// governs: there the GitHub role is the whole grant and the instance role
/// does not cap it (`decide_governed` already gives instance `none` and the
/// anonymous caller nothing to write).
fn write_capped(principal: &Principal, right: DomainRight, governed: bool) -> DomainRight {
    if governed {
        right
    } else {
        capped(principal, right)
    }
}

/// What one caller may write, read from the store once: the principal, the
/// private domains with this account's levels on them, its write list and,
/// with a governor, what GitHub grants it on each governed domain.
/// Answers [`DomainAccess::write_right`] and
/// [`DomainAccess::write_list_excludes`] for any domain with no further store
/// read, so a listing asks the store a fixed number of times, not per row.
/// Built by [`DomainAccess::write_rights`]; a snapshot, so it is asked
/// within one request and never kept.
pub struct WriteRights {
    principal: Principal,
    /// Every private domain's acl row, keyed by its stored (trimmed) name.
    acls: HashMap<String, DomainAcl>,
    /// This account's membership levels, keyed the same way. Empty for any
    /// principal that is not an account.
    levels: HashMap<String, MemberLevel>,
    /// This account's write list, `None` when it has none, is no account, or
    /// when the role cannot write anywhere (the list is not read).
    write_list: Option<Vec<String>>,
    /// The GitHub input for every governed domain, keyed by its trimmed
    /// name. Empty without a governor.
    governed: HashMap<String, Governed>,
}

impl WriteRights {
    /// [`DomainAccess::write_right`] for `domain`.
    pub fn write_right(&self, domain: &str) -> DomainRight {
        let right = self.decided(domain);
        let right = if right > DomainRight::Read && self.off_list(domain) {
            DomainRight::Read
        } else {
            right
        };
        write_capped(&self.principal, right, self.is_governed(domain))
    }

    /// [`DomainAccess::write_list_excludes`] for `domain`.
    pub fn write_list_excludes(&self, domain: &str) -> Option<String> {
        let right = write_capped(
            &self.principal,
            self.decided(domain),
            self.is_governed(domain),
        );
        if right < DomainRight::Write {
            return None;
        }
        match &self.principal {
            Principal::Account { name, .. } if self.off_list(domain) => Some(name.clone()),
            _ => None,
        }
    }

    /// [`DomainAccess::decided_right`] from the rows already read. The name
    /// is trimmed as the store trims it before an acl or membership lookup.
    fn decided(&self, domain: &str) -> DomainRight {
        if matches!(self.principal, Principal::Unrestricted) {
            return DomainRight::Own;
        }
        let domain = domain.trim();
        let acl = self.acls.get(domain);
        if let Some(governed) = self.governed.get(domain) {
            return decide(&self.principal, acl, None, Some(*governed));
        }
        let level = match (acl, &self.principal) {
            (Some(_), Principal::Account { .. }) => self.levels.get(domain).copied(),
            _ => None,
        };
        decide(&self.principal, acl, level, None)
    }

    /// Whether GitHub governs `domain`.
    fn is_governed(&self, domain: &str) -> bool {
        self.governed.contains_key(domain.trim())
    }

    /// Whether this account has a write list that does not name `domain`.
    fn off_list(&self, domain: &str) -> bool {
        matches!(&self.write_list, Some(patterns) if !write_list_matches(patterns, domain))
    }
}

/// Whether a write list names `domain`: an entry equal to it, or an entry
/// ending in `*` whose stem it starts with.
///
/// The same comparison a `domain_acl` or `domain_member` row gets
/// (`AuthStore::domain_visibility`): the domain is trimmed and then compared
/// byte for byte, never case folded, because the engine keys its domain map
/// on the literal name and `Lab` and `lab` are two registrations there.
pub fn write_list_matches(patterns: &[String], domain: &str) -> bool {
    let domain = domain.trim();
    patterns
        .iter()
        .any(|pattern| match pattern.strip_suffix('*') {
            Some(stem) => domain.starts_with(stem),
            None => pattern == domain,
        })
}

/// Resolves a [`Scope`] against the membership records in the auth database.
///
/// Held by the engine (behind a `OnceLock`, installed when the HTTP surface
/// starts) and by the REST state. An engine with none installed - the embedded
/// stdio stack and every one-shot CLI command - filters nothing, which is the
/// same answer it would get for the [`Scope::Unrestricted`] those surfaces
/// pass anyway.
pub struct DomainAccess {
    auth: Arc<AuthStore>,
    /// Who decides the domains connected to GitHub through the GitHub App,
    /// `None` when no App is configured (every answer is then the accounts
    /// store's alone).
    governor: Option<Arc<dyn KnowledgeGovernor>>,
}

/// How an account is named on the commits it makes through the GitHub App:
/// its display name and, when it gave one, its contact address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountContact {
    /// The account's display name.
    pub display: String,
    /// The account's contact address, `None` when it has none.
    pub email: Option<String>,
}

/// What one read of the visibility records says: which domains are private,
/// and which of those the asking scope may not read.
///
/// The two sets answer two different questions and only one of them is a
/// secret. `private` is a property of the domain, true for every private
/// domain on the instance whoever is asking; `hidden` is a property of the
/// caller: `None` for the machine owner and otherwise the private names to
/// subtract (plus the governed domains it may not read, see the field). A
/// listing subtracts `hidden` first and marks what is left from
/// `private`, so no row it keeps was ever a name this caller may not learn.
///
/// `hidden` alone is not the whole screen: for an account with
/// `shared_hidden` every shared domain is hidden too, and this type cannot
/// name them. [`DomainVisibility::screen`] folds the three fields into the
/// one answer a filter can apply without the registrations.
#[derive(Clone, Debug, PartialEq)]
pub struct DomainVisibility {
    /// Every private domain on the instance, by name.
    pub private: HashSet<String>,
    /// The private domains this scope may not read, or `None` for no
    /// filtering at all. With a [`KnowledgeGovernor`] it also names every
    /// governed domain this scope may not read: a private one without a
    /// GitHub grant and, for an anonymous caller, a shared one whose
    /// repository is not public.
    pub hidden: Option<HashSet<String>>,
    /// Whether this scope reads no shared domain at all: an account whose
    /// instance knowledge level is `none`. This type cannot list the shared
    /// domains (it holds only the accounts store), so the engine, which holds
    /// the registrations, adds them to `hidden` (`Engine::hidden_domains`).
    pub shared_hidden: bool,
}

/// Which domain names one scope may hear about, as a filter that needs no
/// list of the registered domains. Built by [`DomainVisibility::screen`].
#[derive(Clone, Debug, PartialEq)]
pub enum DomainScreen {
    /// Every domain: the machine owner.
    All,
    /// Every domain but these (private domains this scope may not read).
    Except(HashSet<String>),
    /// These domains and no other: an account whose instance knowledge level
    /// is `none` reads no shared domain, so only the private domains it may
    /// read are named. A domain registered after the screen was built is not
    /// among them, so it stays out until the screen is built again: the
    /// filter fails closed.
    Only(HashSet<String>),
}

impl DomainScreen {
    /// Whether `domain` passes the screen.
    pub fn admits(&self, domain: &str) -> bool {
        match self {
            DomainScreen::All => true,
            DomainScreen::Except(hidden) => !hidden.contains(domain),
            DomainScreen::Only(readable) => readable.contains(domain),
        }
    }

    /// Whether this scope reads the shared domains at all. `false` only for
    /// [`DomainScreen::Only`]: a change whose audience is every reader of a
    /// shared domain does not reach such a scope.
    pub fn admits_shared(&self) -> bool {
        !matches!(self, DomainScreen::Only(_))
    }
}

impl DomainVisibility {
    /// The one filter these records make: everything for no filtering at all,
    /// the private domains this scope may read for an account with
    /// `shared_hidden`, and every domain but `hidden` otherwise.
    pub fn screen(&self) -> DomainScreen {
        match &self.hidden {
            None => DomainScreen::All,
            Some(hidden) if self.shared_hidden => DomainScreen::Only(
                self.private
                    .iter()
                    .filter(|name| !hidden.contains(*name))
                    .cloned()
                    .collect(),
            ),
            Some(hidden) => DomainScreen::Except(hidden.clone()),
        }
    }
}

/// Which domains GitHub governs, read once from the governor for one
/// public answer (`DomainAccess::governance`), and the governor to ask
/// about them. Without a governor it governs nothing.
struct Governance<'a> {
    governor: Option<&'a dyn KnowledgeGovernor>,
    /// The governed domain names, trimmed as the store trims a name.
    domains: HashSet<String>,
}

impl<'a> Governance<'a> {
    /// The governor, when it governs `domain`.
    fn of(&self, domain: &str) -> Option<&'a dyn KnowledgeGovernor> {
        self.governor
            .filter(|_| self.domains.contains(domain.trim()))
    }

    /// The GitHub input to [`decide`] for `principal` on `domain`, or `None`
    /// when GitHub does not govern it (see `github_input`).
    async fn input(&self, principal: &Principal, domain: &str, private: bool) -> Option<Governed> {
        let governor = self.of(domain)?;
        Some(github_input(governor, principal, domain.trim(), private).await)
    }

    /// Whether an anonymous caller reads `domain`'s engrams (`private` when
    /// it has an acl row): a shared domain Crystalline decides, or a shared
    /// one GitHub governs whose repository is public. Counted whether or not
    /// the `auth.anonymous` tier is on, so a later switch opens no leak.
    async fn anonymous_reads(&self, domain: &str, private: bool) -> bool {
        if private {
            return false;
        }
        match self.of(domain) {
            Some(governor) => governor.repository_public(domain.trim()).await,
            None => true,
        }
    }
}

/// What a per-account loop over one domain's readers reads for each
/// account ([`DomainAccess::readers_of`], [`DomainAccess::page_readers`],
/// [`DomainAccess::readers_cover`]).
enum ReaderSide {
    /// A domain Crystalline decides: its membership rows (knowledge level
    /// and Reflection level by folded name), empty for a shared domain.
    Members(HashMap<String, (MemberLevel, ReflectionLevel)>),
    /// A domain GitHub governs: the accounts with GitHub read or more on
    /// its repository, empty on a shared one (where the shared base already
    /// reads) and when the listing is not known and the caller fails closed.
    Governed(HashSet<String>),
}

impl ReaderSide {
    /// The inputs `decide` and `decide_reflection` take for `name`: its
    /// membership level, its Reflection level on the domain and the GitHub
    /// input. A GitHub reader counts as [`MemberLevel::Viewer`]: the listing
    /// cannot tell a writer from a reader, and every caller here asks only
    /// whether an account reads.
    fn inputs(&self, name: &str) -> (Option<MemberLevel>, ReflectionLevel, Option<Governed>) {
        match self {
            ReaderSide::Members(members) => match members.get(name) {
                Some((level, reflection)) => (Some(*level), *reflection, None),
                None => (None, ReflectionLevel::Default, None),
            },
            ReaderSide::Governed(readers) => {
                let knowledge = if readers.contains(name) {
                    GithubKnowledge::Level(MemberLevel::Viewer)
                } else {
                    GithubKnowledge::Unlinked
                };
                (
                    None,
                    ReflectionLevel::Default,
                    Some(Governed {
                        knowledge,
                        public: false,
                    }),
                )
            }
        }
    }
}

/// The GitHub input to [`decide`] for `principal` on `domain`, which
/// `governor` governs. Asks GitHub only what `decide_governed` reads: the
/// grant of an account whose instance role is viewer or editor, and whether
/// the repository is public for an anonymous caller on a shared domain.
async fn github_input(
    governor: &dyn KnowledgeGovernor,
    principal: &Principal,
    domain: &str,
    private: bool,
) -> Governed {
    match principal {
        Principal::Account {
            name,
            role: Role::Viewer | Role::Editor,
            ..
        } => Governed {
            knowledge: governor.knowledge(name, domain).await,
            public: false,
        },
        Principal::Anonymous if !private => Governed {
            knowledge: GithubKnowledge::Unlinked,
            public: governor.repository_public(domain).await,
        },
        _ => Governed {
            knowledge: GithubKnowledge::Unlinked,
            public: false,
        },
    }
}

impl DomainAccess {
    /// Wrap the accounts store. Cheap: this holds a handle and no state of its
    /// own, so every answer is read fresh and a membership change takes effect
    /// on the next request rather than at the next restart.
    pub fn new(auth: Arc<AuthStore>) -> DomainAccess {
        DomainAccess {
            auth,
            governor: None,
        }
    }

    /// This resolver with `governor` deciding the domains it governs: on
    /// those, engram and Reflection access follow the person's GitHub role
    /// on the repository instead of the domain's membership rows (see
    /// [`KnowledgeGovernor`]).
    pub fn with_governor(self, governor: Arc<dyn KnowledgeGovernor>) -> DomainAccess {
        DomainAccess {
            governor: Some(governor),
            ..self
        }
    }

    /// One snapshot of which domains GitHub governs, taken once per public
    /// answer so every step of that answer sees the same set.
    fn governance(&self) -> Governance<'_> {
        match &self.governor {
            Some(governor) => Governance {
                governor: Some(governor.as_ref()),
                domains: governor
                    .governed_domains()
                    .iter()
                    .map(|name| name.trim().to_string())
                    .collect(),
            },
            None => Governance {
                governor: None,
                domains: HashSet::new(),
            },
        }
    }

    /// Whose draft `account` was granted at one path, or `None` for the
    /// ordinary answer of nobody's.
    ///
    /// A narrow delegation rather than an accessor that hands out the whole
    /// accounts store, and the narrowness is the point: this resolver is the
    /// only way the engine reaches that database, and one question it can ask
    /// is one question a later change cannot widen into a listing. Share-links
    /// are the only cross-overlay visibility there is, and this is the seam
    /// they reach through.
    pub async fn overlay_grant_for(
        &self,
        account: &str,
        domain: &str,
        path: &str,
    ) -> Result<Option<String>> {
        self.auth.overlay_grant_for(account, domain, path).await
    }

    /// Every live share-link `account` holds in one domain. See
    /// [`AuthStore::overlay_grants_held`] for the one caller and why it is a
    /// list rather than a lookup.
    pub async fn overlay_grants_held(
        &self,
        account: &str,
        domain: &str,
    ) -> Result<Vec<(String, String)>> {
        Ok(self
            .auth
            .overlay_grants_held(account, domain)
            .await?
            .into_iter()
            .map(|grant| (grant.path, grant.owner))
            .collect())
    }

    /// Which domain a share-link is about, without redeeming it.
    ///
    /// The screen that has to come BEFORE the irreversible half: redeeming
    /// binds a link to its first presenter for good, so an account that turns
    /// out not to be allowed to read the domain would otherwise burn the link
    /// permanently, and the person it was meant for could never open it. One
    /// row read, and it says nothing but the name.
    pub async fn overlay_grant_domain(&self, token: &str) -> Result<Option<String>> {
        self.auth.overlay_grant_domain(token).await
    }

    /// Present a share-link as `account` and answer the draft it opens, with
    /// the window the link itself stands on.
    ///
    /// Binds the link to that account the first time and answers the same
    /// thing every time after; `None` for an unknown, revoked, expired or
    /// already-taken link, deliberately one answer for all four. Four named
    /// fields rather than the row, for the reason every delegation here is
    /// narrow: this resolver is the engine's only way to that database, and
    /// one question it can ask is one question a later change cannot widen.
    ///
    /// **The window travels with the address** because a join has to end when
    /// the link does, and the join registry measures in this process's memory:
    /// stamping the moment onto the join at open time is what keeps the
    /// co-editing saver's four-times-a-second pass from re-asking the database
    /// something the grant answered once. See [`crate::join::Join::expires_at`].
    pub async fn redeem_overlay_grant(
        &self,
        token: &str,
        account: &str,
    ) -> Result<Option<RedeemedLink>> {
        Ok(self
            .auth
            .redeem_overlay_grant(token, account)
            .await?
            .map(|grant| RedeemedLink {
                domain: grant.domain,
                owner: grant.owner,
                path: grant.path,
                expires_at: grant.expires_at,
            }))
    }

    /// End every share-link standing on one draft, because that draft has
    /// ended. Answers how many were standing.
    pub async fn end_overlay_grants(&self, domain: &str, owner: &str, path: &str) -> Result<u64> {
        self.auth.end_overlay_grants(domain, owner, path).await
    }

    /// End every share-link in one domain, because every draft in it has
    /// ended. Answers how many were standing.
    pub async fn end_domain_overlay_grants(&self, domain: &str) -> Result<u64> {
        self.auth.end_domain_overlay_grants(domain).await
    }

    /// The display name and contact address of `account`, or `None` for a
    /// name that is not an account. A blank address reads as none.
    ///
    /// Narrow for the same reason as every delegation here: a share needs the
    /// two fields it names a commit's author with, not the account row.
    pub async fn account_contact(&self, account: &str) -> Result<Option<AccountContact>> {
        Ok(self.auth.user(account).await?.map(|user| AccountContact {
            display: user.display,
            email: user
                .email
                .map(|email| email.trim().to_string())
                .filter(|email| !email.is_empty()),
        }))
    }

    /// The GitHub link of `account`, or `None` when it has none.
    ///
    /// The link delegations below are narrow for the same reason as every
    /// delegation here: the engine links, reads, marks and unlinks one
    /// account's row. Only the GitHub governor lists the table
    /// ([`DomainAccess::github_links`]), to join a repository's
    /// collaborators to accounts in one read.
    pub async fn github_link_of(&self, account: &str) -> Result<Option<GithubLink>> {
        self.auth.github_link_of(account).await
    }

    /// Every GitHub link, by account. For the GitHub governor, which joins
    /// one listing of a repository's collaborators to the accounts by GitHub
    /// id.
    pub async fn github_links(&self) -> Result<Vec<GithubLink>> {
        self.auth.github_links().await
    }

    /// Store `login` as the login last seen for the GitHub id `account` is
    /// linked to (a renamed GitHub account). Answers whether a row changed.
    pub async fn set_github_login(&self, account: &str, login: &str) -> Result<bool> {
        self.auth.set_github_login(account, login).await
    }

    /// Whether `account` names an enabled instance admin.
    pub async fn account_is_admin(&self, account: &str) -> Result<bool> {
        Ok(self
            .auth
            .user(account)
            .await?
            .is_some_and(|user| !user.disabled && user.role == Role::Admin))
    }

    /// Whether `account` names an account that is not disabled.
    pub async fn account_enabled(&self, account: &str) -> Result<bool> {
        Ok(self
            .auth
            .user(account)
            .await?
            .is_some_and(|user| !user.disabled))
    }

    /// The names of every account that is not disabled, in the store's own
    /// spelling.
    pub async fn enabled_accounts(&self) -> Result<HashSet<String>> {
        Ok(self
            .auth
            .list_users()
            .await?
            .into_iter()
            .filter(|user| !user.disabled)
            .map(|user| user.name)
            .collect())
    }

    /// Link `account` to the GitHub account `github_id` on `host` (see
    /// [`AuthStore::link_github`]: another account holding the id is refused
    /// with [`crate::auth_store::RefusalKind::GithubIdTaken`]). `host` is the
    /// bare host the App runs on, never a URL.
    pub async fn link_github(
        &self,
        account: &str,
        host: &str,
        github_id: u64,
        login: &str,
    ) -> Result<GithubLink> {
        self.auth.link_github(account, host, github_id, login).await
    }

    /// Say that the stored user token of `account` is gone (revoked or
    /// expired); the link stays. Answers whether a row was marked.
    pub async fn mark_github_token_gone(&self, account: &str) -> Result<bool> {
        self.auth.mark_github_token_gone(account).await
    }

    /// Delete the GitHub link of `account`. Answers whether there was one.
    pub async fn unlink_github(&self, account: &str) -> Result<bool> {
        self.auth.unlink_github(account).await
    }

    /// What `scope` may do on `domain`.
    pub async fn right(&self, scope: &Scope, domain: &str) -> Result<DomainRight> {
        Ok(self.resolved_right(scope, domain).await?.1)
    }

    /// What `scope` may do on `domain` **when the request is a write**: the
    /// domain answer, capped by what the instance role lets this account do
    /// anywhere.
    ///
    /// One rule, one spelling, both surfaces. A domain invitation widens what
    /// an account may *reach*, never what its instance role lets it *do*, so an
    /// instance viewer invited into a private domain as an editor reads it and
    /// writes nothing - the answer the JSON API has always given (through
    /// `Identity::require_editor`, which still runs there and still answers
    /// first) and, since this method exists, the answer the MCP write gate
    /// gives too. Before it, the same person's agent wrote over MCP what their
    /// browser was refused, which is two rules for one question.
    ///
    /// The cap is a floor on nothing: it can only lower the answer. The machine
    /// owner is [`DomainRight::Own`] before any of it, an anonymous caller caps
    /// at [`DomainRight::Read`] (the MCP open tier is carved out by its own
    /// surface, on the setting that creates it, before this is ever called),
    /// and an admin caps at `Own`, which is what `decide` already gave them.
    ///
    /// On a domain GitHub governs the instance role does not cap: the GitHub
    /// role is the whole grant there (`decide_governed`), so an instance
    /// viewer with GitHub write writes. The write list still restricts.
    pub async fn write_right(&self, scope: &Scope, domain: &str) -> Result<DomainRight> {
        let (principal, right, governed) = self.decided_right(scope, domain).await?;
        // The write list can only lower a right, so a role that already reads
        // at most has its answer without it, and a broken list table cannot
        // change it.
        let right = write_capped(&principal, right, governed);
        if right > DomainRight::Read && self.off_list_account(&principal, domain).await?.is_some() {
            return Ok(DomainRight::Read);
        }
        Ok(right)
    }

    /// What `scope` may do with `domain`'s Reflection (the private
    /// `decide_reflection`). One read of the visibility and, for an account
    /// on a private domain, its membership level and its Reflection level;
    /// for an account, its write list (off the list, at most Read).
    pub async fn reflection_right(&self, scope: &Scope, domain: &str) -> Result<ReflectionRight> {
        let principal = self.principal(scope).await?;
        if matches!(principal, Principal::Unrestricted) {
            return Ok(ReflectionRight::Curate);
        }
        let (acl, level, member_reflection, github) =
            self.reflection_inputs(&principal, domain).await?;
        // `off_list_account` is `None` for an account with no list and
        // for every principal that is not an account.
        let off_list = self.off_list_account(&principal, domain).await?.is_some();
        Ok(decide_reflection(
            &principal,
            acl.as_ref(),
            level,
            member_reflection,
            github,
            off_list,
        ))
    }

    /// The account whose write list keeps `scope` from a Reflection step that
    /// needs `need` on `domain`, or `None` when the list is not the
    /// reason: `Some` exactly when the account is off its list there and its
    /// reflection right without the list would be `need` or more. The twin of
    /// [`DomainAccess::write_list_refuses`] for the Reflection.
    pub async fn reflection_list_refuses(
        &self,
        scope: &Scope,
        domain: &str,
        need: ReflectionRight,
    ) -> Result<Option<String>> {
        let principal = self.principal(scope).await?;
        let Some(account) = self.off_list_account(&principal, domain).await? else {
            return Ok(None);
        };
        let (acl, level, member_reflection, github) =
            self.reflection_inputs(&principal, domain).await?;
        let uncapped = decide_reflection(
            &principal,
            acl.as_ref(),
            level,
            member_reflection,
            github,
            false,
        );
        Ok((uncapped >= need).then_some(account))
    }

    /// The domain's acl row and, for an account on a private domain, its
    /// membership level and its Reflection level there; on a domain GitHub
    /// governs, the GitHub input instead of the membership row.
    async fn reflection_inputs(
        &self,
        principal: &Principal,
        domain: &str,
    ) -> Result<(
        Option<DomainAcl>,
        Option<MemberLevel>,
        ReflectionLevel,
        Option<Governed>,
    )> {
        let acl = self.auth.domain_visibility(domain).await?;
        let github = self
            .governance()
            .input(principal, domain, acl.is_some())
            .await;
        if github.is_some() {
            return Ok((acl, None, ReflectionLevel::Default, github));
        }
        let (level, member_reflection) = match (&acl, principal) {
            (Some(_), Principal::Account { name, .. }) => (
                self.level_of(name, domain).await?,
                self.auth.member_reflection(domain, name).await?,
            ),
            _ => (None, ReflectionLevel::Default),
        };
        Ok((acl, level, member_reflection, None))
    }

    /// [`WriteRights`] for `scope`, read from the store once: the principal,
    /// every private domain, this account's memberships and its write list.
    /// A listing asks this once and every row from the answer.
    ///
    /// The write list is read only for a role that can write somewhere, as in
    /// [`DomainAccess::write_right`]: the list can only lower a right, so a
    /// Viewer's answers are the same without it, and a broken list table does
    /// not fail the batch for one. With a domain GitHub governs every account
    /// may write somewhere (an instance viewer with GitHub write writes
    /// there), so the list is read for every account then.
    ///
    /// GitHub is asked once per governed domain, for this caller only.
    pub async fn write_rights(&self, scope: &Scope) -> Result<WriteRights> {
        let principal = self.principal(scope).await?;
        let acls: HashMap<String, DomainAcl> = self
            .auth
            .private_domains()
            .await?
            .into_iter()
            .map(|acl| (acl.domain.clone(), acl))
            .collect();
        let governance = self.governance();
        let mut governed = HashMap::new();
        if let Some(governor) = governance.governor {
            for domain in governance.domains {
                let private = acls.contains_key(&domain);
                let input = github_input(governor, &principal, &domain, private).await;
                governed.insert(domain, input);
            }
        }
        let writes_somewhere = instance_cap(&principal) > DomainRight::Read || !governed.is_empty();
        let (levels, write_list) = match &principal {
            Principal::Account { name, .. } => (
                self.auth.memberships_of(name).await?.into_iter().collect(),
                if writes_somewhere {
                    self.auth.write_list(name).await?
                } else {
                    None
                },
            ),
            _ => (HashMap::new(), None),
        };
        Ok(WriteRights {
            principal,
            acls,
            levels,
            write_list,
            governed,
        })
    }

    /// The account whose write list keeps `scope` from writing `domain`, or
    /// `None` when the list is not the reason.
    ///
    /// `Some(account)` exactly when the caller is an account with a write
    /// list that does not name `domain` **and** its write right without the
    /// list would be [`DomainRight::Write`] or more. A viewer, a viewer member,
    /// an account with no list, the machine owner and the anonymous tier get
    /// `None`, so a surface that refuses keeps the role's sentence for them
    /// and names the list only when changing the list would change the
    /// answer. The name is the store's own folded spelling, the one
    /// `crystalline users writes` takes.
    pub async fn write_list_excludes(&self, scope: &Scope, domain: &str) -> Result<Option<String>> {
        let (principal, right, governed) = self.decided_right(scope, domain).await?;
        if write_capped(&principal, right, governed) < DomainRight::Write {
            return Ok(None);
        }
        self.off_list_account(&principal, domain).await
    }

    /// The account whose write list keeps `scope` from a step that needs
    /// `needed` on `domain` (an owner or manager step), or `None` when the
    /// list is not the reason.
    ///
    /// Compared with the domain answer alone (`decide`, no instance cap),
    /// because owning or managing a domain is what the domain grants: a
    /// private domain's owner who is an instance editor owns it. An instance
    /// editor asking for an owner step on a shared domain gets `None`, since
    /// adding the domain to its list would not make it the owner.
    pub async fn write_list_refuses(
        &self,
        scope: &Scope,
        domain: &str,
        needed: DomainRight,
    ) -> Result<Option<String>> {
        let (principal, right, _) = self.decided_right(scope, domain).await?;
        if right < needed {
            return Ok(None);
        }
        self.off_list_account(&principal, domain).await
    }

    /// The shared tail of the two answers above: the account's own name when
    /// `principal` is an account whose write list exists and does not name
    /// `domain`, `None` for every other principal and for an account with no
    /// list. The caller has already decided that the list matters.
    async fn off_list_account(
        &self,
        principal: &Principal,
        domain: &str,
    ) -> Result<Option<String>> {
        let Principal::Account { name, .. } = principal else {
            return Ok(None);
        };
        Ok(match self.auth.write_list(name).await? {
            Some(patterns) if !write_list_matches(&patterns, domain) => Some(name.clone()),
            _ => None,
        })
    }

    /// The policy answer for one scope on one domain, with the principal it
    /// was decided for. Both public answers are this one, read differently.
    ///
    /// **The write list caps here** for [`DomainAccess::right`] (what the
    /// owner gates read): an account with a write list that does not name
    /// `domain` is a reader there and nothing more. [`DomainAccess::write_right`]
    /// (what a content write reads) carries the same list in another order:
    /// the instance cap first, then the list, read only when the capped right
    /// is still above Read, so a Viewer never waits on it. The cap only
    /// lowers - a domain the caller may not see stays [`DomainRight::None`] -
    /// and visibility never comes through here ([`DomainAccess::visibility`]
    /// folds [`decide`] directly), so every read is unchanged. The machine
    /// owner and the anonymous tier have no list. One more read of the
    /// accounts table, and only when there is a right above Read to cap.
    async fn resolved_right(
        &self,
        scope: &Scope,
        domain: &str,
    ) -> Result<(Principal, DomainRight)> {
        let (principal, right, _) = self.decided_right(scope, domain).await?;
        if right > DomainRight::Read && self.off_list_account(&principal, domain).await?.is_some() {
            return Ok((principal, DomainRight::Read));
        }
        Ok((principal, right))
    }

    /// The policy answer for one scope on one domain before the write list,
    /// with the principal it was decided for.
    /// The third answer is whether GitHub governs `domain`, which is what
    /// decides whether a write is capped by the instance role.
    async fn decided_right(
        &self,
        scope: &Scope,
        domain: &str,
    ) -> Result<(Principal, DomainRight, bool)> {
        let principal = self.principal(scope).await?;
        if matches!(principal, Principal::Unrestricted) {
            return Ok((principal, DomainRight::Own, false));
        }
        let acl = self.auth.domain_visibility(domain).await?;
        let github = self
            .governance()
            .input(&principal, domain, acl.is_some())
            .await;
        // A governed domain's membership rows are not read at all.
        let level = match (&acl, &principal, github) {
            (Some(_), Principal::Account { name, .. }, None) => self.level_of(name, domain).await?,
            _ => None,
        };
        let right = decide(&principal, acl.as_ref(), level, github);
        Ok((principal, right, github.is_some()))
    }

    /// The accounts that may read `domain` right now, or `None` when the
    /// domain is not private and so every scope that reaches the instance
    /// reads it, except an account whose instance knowledge level is `none`
    /// (a listener keeps those out with [`DomainScreen::admits_shared`]).
    ///
    /// The other direction of [`DomainAccess::visibility`]: from one
    /// domain to the accounts, where that method goes from one scope to the
    /// domains. The engine asks it in the instant before a change takes the
    /// domain's name out of these records (a removal, the old name of a
    /// rename), so the change can carry who could read the domain a moment
    /// before it, rather than leaving a listener to ask a registry the change
    /// is emptying. Each account is decided by `decide`, the rule every
    /// other answer here uses: the owner, every member who reads engrams there
    /// (a member at knowledge `none` does not) and every instance admin; a
    /// disabled account reads nothing, as `DomainAccess::principal` rules.
    /// Names are the store's own folded spelling. The machine owner is never
    /// named: it reads every domain.
    ///
    /// On a domain GitHub governs the owner and the members are not read:
    ///
    /// * a private one is read by every enabled account the governor lists
    ///   as a GitHub reader ([`KnowledgeGovernor::readers`], one call) and
    ///   every instance admin, never by an account at instance knowledge
    ///   `none`. When the listing is not known, only the instance admins;
    /// * a shared one whose repository is public is read by everyone
    ///   (`None`); one whose repository is not public is not read by an
    ///   anonymous caller, so it names every enabled account at instance
    ///   viewer or above instead.
    pub async fn readers_of(&self, domain: &str) -> Result<Option<HashSet<String>>> {
        let acl = self.auth.domain_visibility(domain).await?;
        if let Some(governor) = self.governance().of(domain) {
            let side = match &acl {
                None if governor.repository_public(domain.trim()).await => return Ok(None),
                None => ReaderSide::Governed(HashSet::new()),
                // Unknown fails closed: nobody is a GitHub reader.
                Some(_) => {
                    ReaderSide::Governed(governor.readers(domain.trim()).await.unwrap_or_default())
                }
            };
            return Ok(Some(self.engram_readers(acl.as_ref(), &side).await?));
        }
        let Some(acl) = acl else {
            return Ok(None);
        };
        let side = ReaderSide::Members(self.member_rows(domain).await?);
        Ok(Some(self.engram_readers(Some(&acl), &side).await?))
    }

    /// The enabled accounts `decide` lets read a domain with this acl and
    /// these reader inputs.
    async fn engram_readers(
        &self,
        acl: Option<&DomainAcl>,
        side: &ReaderSide,
    ) -> Result<HashSet<String>> {
        let mut readers = HashSet::new();
        for user in self.auth.list_users().await? {
            if user.disabled {
                continue;
            }
            let (level, _, github) = side.inputs(&user.name);
            let principal = Principal::Account {
                name: user.name.clone(),
                role: user.role,
                reflection: user.reflection,
            };
            if decide(&principal, acl, level, github) >= DomainRight::Read {
                readers.insert(user.name);
            }
        }
        Ok(readers)
    }

    /// One domain's membership rows: each member's knowledge level and
    /// Reflection level, keyed by the member's folded name.
    async fn member_rows(
        &self,
        domain: &str,
    ) -> Result<HashMap<String, (MemberLevel, ReflectionLevel)>> {
        Ok(self
            .auth
            .domain_members(domain)
            .await?
            .into_iter()
            .map(|m| (m.principal, (m.level, m.reflection)))
            .collect())
    }

    /// How a per-account loop decides who reads `domain` (private when
    /// `private`): its membership rows, or on a domain GitHub governs the
    /// governor's reader listing (one call, and only for a private domain:
    /// on a shared one the GitHub grant cannot lower the shared base).
    /// `None` when that listing is not known.
    async fn reader_side(
        &self,
        governance: &Governance<'_>,
        domain: &str,
        private: bool,
    ) -> Result<Option<ReaderSide>> {
        if let Some(governor) = governance.of(domain) {
            if !private {
                return Ok(Some(ReaderSide::Governed(HashSet::new())));
            }
            return Ok(governor
                .readers(domain.trim())
                .await
                .map(ReaderSide::Governed));
        }
        Ok(Some(ReaderSide::Members(if private {
            self.member_rows(domain).await?
        } else {
            HashMap::new()
        })))
    }

    /// Whether a Reflection page of domain `page` may carry text drawn from
    /// the engrams of domain `source`: whether every reader of the page may
    /// read those engrams anyway. Text on a page reaches every reader of the
    /// page, so a page read by someone who may not read the source's engrams
    /// would hand that knowledge to them.
    ///
    /// The two sides are measured differently, each by what reaches whom:
    ///
    /// * the page's readers are who may read the page's domain's Reflection
    ///   (the private `decide_reflection` at [`ReflectionRight::Read`] or
    ///   more, before any write list, which only caps writing);
    /// * the source's readers are who may read its engrams (the private
    ///   `decide` at [`DomainRight::Read`] or more). A Reflection level on
    ///   the source's domain does not count: it opens that domain's pages,
    ///   which its curators wrote for that audience, never its engrams.
    ///
    /// Spelled out:
    ///
    /// * a shared domain's Reflection is read by every anonymous caller (the
    ///   `auth.anonymous` tier, counted whether or not it is on, so a later
    ///   switch does not open a leak), every enabled account whose instance
    ///   role is viewer, editor or admin, every account at instance
    ///   knowledge `none` whose instance Reflection level is reader or
    ///   contributor, and every account created later. Its engrams are read
    ///   by the same, less the accounts at instance knowledge `none`. So a
    ///   shared page never covers a private source, and covers another
    ///   shared domain only while no enabled account at instance knowledge
    ///   `none` has an instance Reflection level (an account created later
    ///   with one is caught by the queue, which asks again on every scan);
    /// * a private domain's Reflection is read by its owner, every instance
    ///   admin, every member at knowledge viewer or more, and every member at
    ///   knowledge `none` whose Reflection level on it is reader or
    ///   contributor. Its engrams are read by the same, less those members at
    ///   knowledge `none`. Never by an anonymous caller, a disabled account
    ///   or an account created later. A page there covers a source when each
    ///   of its readers also reads the source's engrams;
    /// * the machine owner and the instance admins read everything, so they
    ///   never make a difference.
    ///
    /// On a domain GitHub governs, the owner and the membership rows are not
    /// read:
    ///
    /// * a private one's Reflection and engrams are read by the enabled
    ///   accounts the governor lists as GitHub readers and the instance
    ///   admins, never by an account at instance knowledge `none`. One
    ///   listing per side ([`KnowledgeGovernor::readers`]); when it is not
    ///   known the answer is "not covered", whichever side it is on;
    /// * a shared one is read as a shared domain is, except that an account
    ///   at instance knowledge `none` reads nothing there (its instance
    ///   Reflection level included) and an anonymous caller reads it only
    ///   while the repository is public. A page an anonymous caller reads
    ///   never covers a source an anonymous caller may not read.
    ///
    /// A page always covers its own domain. Read fresh on every call, so a
    /// membership change counts at once.
    pub async fn readers_cover(&self, page: &str, source: &str) -> Result<bool> {
        self.cover(page, source, None).await
    }

    /// Who reads a Reflection page of domain `page` right now, as a person's
    /// confirmation is recorded for ([`DomainAccess::readers_cover_for`]):
    /// [`PageReaders::Everyone`] for a shared domain, whose Reflection reaches
    /// anonymous callers and accounts not made yet, and for a private one the
    /// enabled accounts that read its Reflection (the reader side of
    /// [`DomainAccess::readers_cover`]). Names are the store's folded
    /// spelling. The machine owner is never named: it reads every domain.
    ///
    /// On a private domain GitHub governs those are the enabled accounts the
    /// governor lists as GitHub readers and the instance admins (not the
    /// owner, not the members); when the listing is not known, only the
    /// instance admins. A shared domain GitHub governs is
    /// [`PageReaders::Everyone`] while its repository is public; while it is
    /// not, no anonymous caller reads the page, so the page names the
    /// enabled accounts that read its Reflection (every account at instance
    /// viewer or above). A confirmation for those is asked again once the
    /// repository goes public ([`DomainAccess::readers_cover_for`]).
    pub async fn page_readers(&self, page: &str) -> Result<PageReaders> {
        let governance = self.governance();
        let acl = self.auth.domain_visibility(page).await?;
        if governance.anonymous_reads(page, acl.is_some()).await {
            return Ok(PageReaders::Everyone);
        }
        // Unknown fails closed: nobody is a GitHub reader.
        let side = match self.reader_side(&governance, page, acl.is_some()).await? {
            Some(side) => side,
            None => ReaderSide::Governed(HashSet::new()),
        };
        let mut readers = std::collections::BTreeSet::new();
        for user in self.auth.list_users().await? {
            if user.disabled {
                continue;
            }
            let (level, reflection, github) = side.inputs(&user.name);
            let principal = Principal::Account {
                name: user.name.clone(),
                role: user.role,
                reflection: user.reflection,
            };
            if decide_reflection(&principal, acl.as_ref(), level, reflection, github, false)
                >= ReflectionRight::Read
            {
                readers.insert(user.name);
            }
        }
        Ok(PageReaders::Accounts(readers))
    }

    /// [`DomainAccess::readers_cover`] for a page whose text drawn from
    /// `source` a person shared on purpose with `confirmed`, the page's
    /// readers when they did ([`DomainAccess::page_readers`]). Covered unless
    /// someone reads the page now who was not among `confirmed` and may not
    /// read the source's engrams: the person agreed to the readers of that
    /// time, so a widening asks again. A reader who lost the source since
    /// changes nothing, since the text reached them already. A page an
    /// anonymous caller may read now (shared since, or a shared GitHub
    /// domain whose repository went public since) is never covered by a
    /// confirmation for named accounts: its readers include anonymous
    /// callers, and none of them was named.
    pub async fn readers_cover_for(
        &self,
        page: &str,
        source: &str,
        confirmed: &PageReaders,
    ) -> Result<bool> {
        match confirmed {
            PageReaders::Everyone => Ok(true),
            PageReaders::Accounts(names) => self.cover(page, source, Some(names)).await,
        }
    }

    /// [`DomainAccess::readers_cover`], with the page readers in `confirmed`
    /// counted as covered whatever they read.
    async fn cover(
        &self,
        page: &str,
        source: &str,
        confirmed: Option<&std::collections::BTreeSet<String>>,
    ) -> Result<bool> {
        if page.trim() == source.trim() {
            return Ok(true);
        }
        let governance = self.governance();
        let page_acl = self.auth.domain_visibility(page).await?;
        let source_acl = self.auth.domain_visibility(source).await?;
        let anonymous_reads_page = governance.anonymous_reads(page, page_acl.is_some()).await;
        // A confirmation for named accounts never covers a page anonymous
        // callers read: none of them was named. Without a governor that is
        // every shared page.
        if anonymous_reads_page && confirmed.is_some() {
            return Ok(false);
        }
        // A shared page reaches accounts not made yet, who never read a
        // private domain. A confirmation for named accounts (possible on a
        // shared page only while anonymous callers cannot read it) is held
        // against the readers of the moment instead, in the loop below.
        if page_acl.is_none() && source_acl.is_some() && confirmed.is_none() {
            return Ok(false);
        }
        // A page anonymous callers read never covers a source they may not
        // read (a shared one GitHub governs whose repository is not public).
        // Without a governor both sides are shared domains they read.
        if anonymous_reads_page
            && !governance
                .anonymous_reads(source, source_acl.is_some())
                .await
        {
            return Ok(false);
        }
        // A reader listing GitHub cannot give right now covers nothing,
        // on either side.
        let Some(page_side) = self
            .reader_side(&governance, page, page_acl.is_some())
            .await?
        else {
            return Ok(false);
        };
        let Some(source_side) = self
            .reader_side(&governance, source, source_acl.is_some())
            .await?
        else {
            return Ok(false);
        };
        // Who reads the page: its domain's Reflection readers.
        let reads_page = |principal: &Principal, name: &str| {
            let (level, reflection, github) = page_side.inputs(name);
            decide_reflection(
                principal,
                page_acl.as_ref(),
                level,
                reflection,
                github,
                false,
            ) >= ReflectionRight::Read
        };
        // Who reads the source: its engrams' readers.
        let reads_source = |principal: &Principal, name: &str| {
            let (level, _, github) = source_side.inputs(name);
            decide(principal, source_acl.as_ref(), level, github) >= DomainRight::Read
        };
        for user in self.auth.list_users().await? {
            if user.disabled {
                continue;
            }
            let principal = Principal::Account {
                name: user.name.clone(),
                role: user.role,
                reflection: user.reflection,
            };
            if reads_page(&principal, &user.name)
                && !reads_source(&principal, &user.name)
                && !confirmed.is_some_and(|names| names.contains(&user.name))
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Retire the visibility and membership records of a domain that no longer
    /// exists.
    ///
    /// **One of the two writes on this type, and it is here rather than beside
    /// the reads by accident of ownership.** Everything else on `DomainAccess`
    /// but [`DomainAccess::rename_domain`] answers a question; this ends the
    /// records the answers are computed from.
    /// It lives here because the engine holds exactly one handle onto the
    /// accounts database - this one - and the removal verb is the engine's, so
    /// the alternative was handing the engine the whole store. A narrow method
    /// is the smaller door. It decides nothing: the caller has already decided
    /// the domain is gone.
    ///
    /// Answers whether the domain had an acl row at all, so a caller can tell
    /// "a private domain's records were retired" from "a shared domain had
    /// none".
    #[doc(hidden)]
    pub async fn forget_domain(&self, domain: &str) -> Result<bool> {
        self.auth.forget_domain(domain).await
    }

    /// Carry the visibility, membership and share-link records of domain
    /// `old` over to `new`, for a domain that is being renamed.
    ///
    /// The second write on this type, here for the reason
    /// [`DomainAccess::forget_domain`] is: the rename verb is the engine's and
    /// this is its one handle onto the accounts database. It decides nothing;
    /// [`AuthStore::rename_domain`] says what moves and why a second call is
    /// fine.
    #[doc(hidden)]
    pub async fn rename_domain(&self, old: &str, new: &str) -> Result<()> {
        self.auth.rename_domain(old, new).await
    }

    /// Which domains are private, and which of those `scope` may not read.
    ///
    /// Both facts from one read of `domain_acl`, for the caller that needs
    /// them together: a domain listing marks each row it kept private or
    /// shared and drops the hidden ones, and reading the two separately would
    /// be two sweeps of the same table for one answer.
    ///
    /// `private` is filled for every scope, the machine owner included:
    /// whether a domain is private is not a secret from anybody who can see
    /// the domain at all, and the machine owner sees all of them. `hidden` is
    /// `None` for the machine owner and otherwise the private names to
    /// subtract (and the domains GitHub governs that it may not read); with
    /// `shared_hidden` the shared domains are hidden too
    /// ([`DomainVisibility::screen`]).
    ///
    /// There is deliberately no public answer that carries `hidden` alone: it
    /// would drop `shared_hidden`, and a filter built on it would show an
    /// account at knowledge `none` every shared domain.
    pub async fn visibility(&self, scope: &Scope) -> Result<DomainVisibility> {
        let principal = self.principal(scope).await?;
        self.resolve(&principal).await
    }

    /// Whether GitHub decides access on `domain` right now (see
    /// [`KnowledgeGovernor`]). Always `false` without a governor.
    pub fn governs(&self, domain: &str) -> bool {
        self.governance().of(domain).is_some()
    }

    /// What the MCP open tier sees when it acts as the account `agent` on
    /// the domains GitHub governs (`github.agent_identity`): what `agent`
    /// sees on those, and what an anonymous caller sees on every other
    /// domain. An `agent` that is no live account sees what an anonymous
    /// caller sees everywhere.
    pub async fn open_tier_visibility(&self, agent: &str) -> Result<DomainVisibility> {
        // One snapshot of the governed set for both folds and the merge, so
        // a domain that leaves it meanwhile is decided one way throughout.
        let governance = self.governance();
        let anonymous = self.resolve_in(&Principal::Anonymous, &governance).await?;
        let principal = self
            .principal(&Scope::Agent {
                account: agent.to_string(),
            })
            .await?;
        let as_agent = self.resolve_in(&principal, &governance).await?;
        let governed = &governance.domains;
        let mut hidden: HashSet<String> = anonymous
            .hidden
            .unwrap_or_default()
            .into_iter()
            .filter(|domain| !governed.contains(domain))
            .collect();
        hidden.extend(
            as_agent
                .hidden
                .unwrap_or_default()
                .into_iter()
                .filter(|domain| governed.contains(domain)),
        );
        // An agent account at knowledge `none` reads no shared domain, and
        // so no shared governed one.
        if as_agent.shared_hidden {
            hidden.extend(
                governed
                    .iter()
                    .filter(|domain| !anonymous.private.contains(*domain))
                    .cloned(),
            );
        }
        Ok(DomainVisibility {
            private: anonymous.private,
            hidden: Some(hidden),
            shared_hidden: false,
        })
    }

    /// The whole visibility fold, over an already-resolved principal.
    ///
    /// One read of `domain_acl` and, for an account, one read of its own
    /// membership list - instead of one read per private domain. The decision
    /// is still [`decide`], given the same three inputs
    /// [`DomainAccess::right`] gives it, which is what keeps the bulk answer
    /// and the per-domain one from drifting apart.
    async fn resolve(&self, principal: &Principal) -> Result<DomainVisibility> {
        self.resolve_in(principal, &self.governance()).await
    }

    /// [`DomainAccess::resolve`] under one given snapshot of which domains
    /// GitHub governs, so two folds can share it.
    async fn resolve_in(
        &self,
        principal: &Principal,
        governance: &Governance<'_>,
    ) -> Result<DomainVisibility> {
        let shared_hidden = matches!(
            principal,
            Principal::Account {
                role: Role::None,
                ..
            }
        );
        let acls = self.auth.private_domains().await?;
        let private: HashSet<String> = acls.iter().map(|acl| acl.domain.clone()).collect();
        if matches!(principal, Principal::Unrestricted) {
            return Ok(DomainVisibility {
                private,
                hidden: None,
                shared_hidden: false,
            });
        }
        let governed = &governance.domains;
        if acls.is_empty() && governed.is_empty() {
            return Ok(DomainVisibility {
                private,
                hidden: Some(HashSet::new()),
                shared_hidden,
            });
        }
        let levels: HashMap<String, MemberLevel> = match principal {
            Principal::Account { name, .. } => {
                self.auth.memberships_of(name).await?.into_iter().collect()
            }
            _ => HashMap::new(),
        };
        let mut hidden = HashSet::new();
        for acl in &acls {
            if governed.contains(&acl.domain) {
                continue;
            }
            let level = levels.get(&acl.domain).copied();
            if decide(principal, Some(acl), level, None) < DomainRight::Read {
                hidden.insert(acl.domain.clone());
            }
        }
        // Every governed domain this scope may not read: a private one
        // without a GitHub grant, and for an anonymous caller a shared one
        // whose repository is not public. An account at viewer or above
        // reads every shared one.
        if let Some(governor) = governance.governor {
            let by_name: HashMap<&str, &DomainAcl> =
                acls.iter().map(|acl| (acl.domain.as_str(), acl)).collect();
            for domain in governed {
                let acl = by_name.get(domain.as_str()).copied();
                let input = github_input(governor, principal, domain, acl.is_some()).await;
                if decide(principal, acl, None, Some(input)) < DomainRight::Read {
                    hidden.insert(domain.clone());
                }
            }
        }
        Ok(DomainVisibility {
            private,
            hidden: Some(hidden),
            shared_hidden,
        })
    }

    /// Resolve a scope against the accounts table.
    ///
    /// An account the store no longer has, or has disabled, resolves as
    /// [`Principal::Anonymous`] - it reads a shared domain and sees no private
    /// one - and its `admin` flag is dropped with it. That is the one rule that
    /// keeps a stale credential, or a membership row that outlived its holder,
    /// from being worth anything: the surfaces already refuse a disabled
    /// account at sign-in, and this is the same answer one table deeper.
    async fn principal(&self, scope: &Scope) -> Result<Principal> {
        match scope {
            Scope::Unrestricted => Ok(Principal::Unrestricted),
            Scope::Anonymous => Ok(Principal::Anonymous),
            Scope::User { account, admin } => {
                let Some(user) = self.auth.user(account).await? else {
                    return Ok(Principal::Anonymous);
                };
                if user.disabled {
                    return Ok(Principal::Anonymous);
                }
                let role = if *admin { Role::Admin } else { user.role };
                Ok(Principal::Account {
                    name: user.name,
                    role,
                    reflection: user.reflection,
                })
            }
            // The open tier as `github.agent_identity`: an admin is clamped
            // to editor, so on a governed domain it gets its GitHub grant
            // and never the instance admin's `Own`.
            Scope::Agent { account } => {
                let Some(user) = self.auth.user(account).await? else {
                    return Ok(Principal::Anonymous);
                };
                if user.disabled {
                    return Ok(Principal::Anonymous);
                }
                let role = match user.role {
                    Role::Admin => Role::Editor,
                    role => role,
                };
                Ok(Principal::Account {
                    name: user.name,
                    role,
                    reflection: user.reflection,
                })
            }
        }
    }

    /// One account's membership level on one domain, if it has one.
    ///
    /// Read out of the account's own membership list rather than through a
    /// per-domain lookup, so this path and [`DomainAccess::visibility`]
    /// read the same rows the same way. A membership list is a handful of rows
    /// even for a heavy user.
    async fn level_of(&self, account: &str, domain: &str) -> Result<Option<MemberLevel>> {
        let domain = domain.trim();
        Ok(self
            .auth
            .memberships_of(account)
            .await?
            .into_iter()
            .find(|(d, _)| d == domain)
            .map(|(_, level)| level))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl DomainAccess {
        /// The `hidden` half of [`DomainAccess::visibility`], which is what
        /// most tests here compare against `right`. Test only: a caller that
        /// filters on it alone drops `shared_hidden`.
        async fn hidden_domains(&self, scope: &Scope) -> Result<Option<HashSet<String>>> {
            Ok(self.visibility(scope).await?.hidden)
        }
    }

    async fn store(dir: &tempfile::TempDir) -> Arc<AuthStore> {
        Arc::new(
            AuthStore::open(&dir.path().join("web-auth.db"))
                .await
                .unwrap(),
        )
    }

    /// The cast every test here works with: an owner, an admin, a member and a
    /// stranger, all instance editors except where the ladder needs otherwise.
    async fn cast(auth: &AuthStore) {
        for (name, role) in [
            ("keeper", Role::Editor),
            ("boss", Role::Admin),
            ("mem", Role::Viewer),
            ("out", Role::Editor),
        ] {
            auth.add_user(name, name, None, role, "pw12345678")
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn account_contact_answers_the_display_name_and_the_email() {
        let dir = tempfile::tempdir().unwrap();
        let auth = store(&dir).await;
        auth.add_user(
            "ada",
            "Ada Lovelace",
            Some("ada@example.com"),
            Role::Editor,
            "pw12345678",
        )
        .await
        .unwrap();
        auth.add_user("bob", "Bob", None, Role::Editor, "pw12345678")
            .await
            .unwrap();
        let access = DomainAccess::new(auth);

        assert_eq!(
            access.account_contact("ada").await.unwrap(),
            Some(AccountContact {
                display: "Ada Lovelace".to_string(),
                email: Some("ada@example.com".to_string()),
            })
        );
        assert_eq!(
            access.account_contact("bob").await.unwrap(),
            Some(AccountContact {
                display: "Bob".to_string(),
                email: None,
            })
        );
        assert_eq!(access.account_contact("nobody").await.unwrap(), None);
    }

    fn user(account: &str, admin: bool) -> Scope {
        Scope::User {
            account: account.into(),
            admin,
        }
    }

    /// The readers of a private domain are its owner, its members and the
    /// instance admins, and nobody else; a shared domain has no list at all.
    /// Catches a reader list that drops the owner or the admins (who read by
    /// role, not by membership) or keeps a disabled member.
    #[tokio::test]
    async fn the_readers_of_a_private_domain_are_owner_members_and_admins() {
        let dir = tempfile::tempdir().unwrap();
        let auth = store(&dir).await;
        cast(&auth).await;
        auth.add_user("gone", "gone", None, Role::Viewer, "pw12345678")
            .await
            .unwrap();
        auth.set_domain_visibility("lab", true, "keeper")
            .await
            .unwrap();
        for member in ["mem", "gone"] {
            auth.upsert_domain_member("lab", member, MemberLevel::Viewer, None, "keeper")
                .await
                .unwrap();
        }
        auth.set_disabled("gone", true).await.unwrap();
        let access = DomainAccess::new(auth);
        assert_eq!(access.readers_of("shared").await.unwrap(), None);
        let readers = access.readers_of("lab").await.unwrap().unwrap();
        let expected: HashSet<String> = ["keeper", "boss", "mem"]
            .into_iter()
            .map(str::to_string)
            .collect();
        assert_eq!(readers, expected);
    }

    /// `lab` and `vault` private to keeper and `mem` a viewer of both; `ann`
    /// an instance viewer who is no member yet. Instance roles from [`cast`].
    async fn covering(auth: &AuthStore) {
        cast(auth).await;
        auth.add_user("ann", "ann", None, Role::Viewer, "pw12345678")
            .await
            .unwrap();
        for domain in ["lab", "vault"] {
            auth.set_domain_visibility(domain, true, "keeper")
                .await
                .unwrap();
            auth.upsert_domain_member(domain, "mem", MemberLevel::Viewer, None, "keeper")
                .await
                .unwrap();
        }
    }

    /// A shared page never covers a private source, however few accounts
    /// there are: anonymous callers and every account signed up later read
    /// a shared domain. A private page covers a shared source, and every
    /// page covers its own domain.
    #[tokio::test]
    async fn a_shared_page_never_covers_a_private_source() {
        let dir = tempfile::tempdir().unwrap();
        let auth = store(&dir).await;
        covering(&auth).await;
        let access = DomainAccess::new(auth);
        assert!(!access.readers_cover("shared", "lab").await.unwrap());
        assert!(access.readers_cover("lab", "shared").await.unwrap());
        assert!(access.readers_cover("shared", "other").await.unwrap());
        assert!(access.readers_cover("lab", "lab").await.unwrap());
    }

    /// A private page covers a private source exactly when every account
    /// that reads the page reads the source too: the same readers, or more
    /// on the source, cover; one reader of the page the source lacks does
    /// not. Instance admins and the owner read both and change nothing.
    #[tokio::test]
    async fn a_private_page_covers_a_source_whose_readers_include_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let auth = store(&dir).await;
        covering(&auth).await;
        let access = DomainAccess::new(auth.clone());
        assert!(access.readers_cover("lab", "vault").await.unwrap());
        // vault gains a reader lab lacks: still covered.
        auth.upsert_domain_member("vault", "out", MemberLevel::Viewer, None, "keeper")
            .await
            .unwrap();
        assert!(access.readers_cover("lab", "vault").await.unwrap());
        // So a page in vault no longer covers lab.
        assert!(!access.readers_cover("vault", "lab").await.unwrap());
        // A disabled account reads nothing, so it is nobody's reader.
        auth.set_disabled("out", true).await.unwrap();
        assert!(access.readers_cover("vault", "lab").await.unwrap());
    }

    /// The page's readers are its Reflection's readers, the source's are its
    /// engrams' readers: a member at knowledge `none` with a Reflection
    /// level reads the page and is not a reader of the source, not even
    /// with the same levels on the source's domain.
    #[tokio::test]
    async fn a_reflection_only_member_counts_as_a_reader_of_the_page() {
        let dir = tempfile::tempdir().unwrap();
        let auth = store(&dir).await;
        covering(&auth).await;
        let access = DomainAccess::new(auth.clone());
        assert!(access.readers_cover("lab", "vault").await.unwrap());
        auth.upsert_domain_member(
            "lab",
            "ann",
            MemberLevel::None,
            Some(ReflectionLevel::Reader),
            "keeper",
        )
        .await
        .unwrap();
        assert!(!access.readers_cover("lab", "vault").await.unwrap());
        // Given the same on vault, ann reads vault's Reflection and still no
        // engram there: a source is measured by who reads its engrams.
        auth.upsert_domain_member(
            "vault",
            "ann",
            MemberLevel::None,
            Some(ReflectionLevel::Reader),
            "keeper",
        )
        .await
        .unwrap();
        assert!(!access.readers_cover("lab", "vault").await.unwrap());
        // As a viewer of vault she reads its engrams.
        auth.upsert_domain_member("vault", "ann", MemberLevel::Viewer, None, "keeper")
            .await
            .unwrap();
        assert!(access.readers_cover("lab", "vault").await.unwrap());
    }

    /// An account at instance knowledge `none` reads no shared domain unless
    /// its instance Reflection level says so. As a member of a private page's
    /// domain it keeps that page from covering a shared source.
    #[tokio::test]
    async fn an_instance_none_member_keeps_a_private_page_from_covering_a_shared_source() {
        let dir = tempfile::tempdir().unwrap();
        let auth = store(&dir).await;
        covering(&auth).await;
        auth.add_user("nora", "nora", None, Role::None, "pw12345678")
            .await
            .unwrap();
        auth.upsert_domain_member("lab", "nora", MemberLevel::Viewer, None, "keeper")
            .await
            .unwrap();
        let access = DomainAccess::new(auth.clone());
        assert!(!access.readers_cover("lab", "shared").await.unwrap());
        // An instance Reflection level lets nora read shared Reflections,
        // never a shared domain's engrams: still not covered.
        auth.set_reflection_level("nora", ReflectionLevel::Reader)
            .await
            .unwrap();
        assert!(!access.readers_cover("lab", "shared").await.unwrap());
    }

    /// A shared page is read by every account at instance knowledge `none`
    /// that has an instance Reflection level, and such an account reads no
    /// shared domain's engrams: while one exists, a shared page covers no
    /// other shared domain.
    #[tokio::test]
    async fn an_instance_none_reflection_reader_keeps_shared_from_covering_shared() {
        let dir = tempfile::tempdir().unwrap();
        let auth = store(&dir).await;
        covering(&auth).await;
        let access = DomainAccess::new(auth.clone());
        assert!(access.readers_cover("shared", "other").await.unwrap());
        auth.add_user("nora", "nora", None, Role::None, "pw12345678")
            .await
            .unwrap();
        assert!(
            access.readers_cover("shared", "other").await.unwrap(),
            "without a Reflection level nora reads neither"
        );
        auth.set_reflection_level("nora", ReflectionLevel::Reader)
            .await
            .unwrap();
        assert!(!access.readers_cover("shared", "other").await.unwrap());
    }

    /// Who reads a page is named for a confirmation: everyone for a shared
    /// domain, the accounts that read its Reflection for a private one
    /// (keeper the owner, boss the admin, mem the member).
    #[tokio::test]
    async fn a_page_is_read_by_everyone_or_by_named_accounts() {
        let dir = tempfile::tempdir().unwrap();
        let auth = store(&dir).await;
        covering(&auth).await;
        let access = DomainAccess::new(auth);
        assert_eq!(
            access.page_readers("shared").await.unwrap(),
            PageReaders::Everyone
        );
        let PageReaders::Accounts(readers) = access.page_readers("lab").await.unwrap() else {
            panic!("a private page has named readers");
        };
        assert!(
            readers.contains("mem") && readers.contains("keeper"),
            "{readers:?}"
        );
        assert!(!readers.contains("ann"), "{readers:?}");
    }

    /// A page confirmed for its readers covers a source as long as no reader
    /// came that was not one then and may not read the source: a reader who
    /// lost the source afterwards changes nothing, a new one does, and so
    /// does the page becoming shared. A confirmation for everyone always
    /// covers.
    #[tokio::test]
    async fn a_confirmation_covers_the_readers_it_was_given_for() {
        let dir = tempfile::tempdir().unwrap();
        let auth = store(&dir).await;
        covering(&auth).await;
        let access = DomainAccess::new(auth.clone());
        // ann reads lab and not vault: lab does not cover vault.
        auth.upsert_domain_member("lab", "ann", MemberLevel::Viewer, None, "keeper")
            .await
            .unwrap();
        assert!(!access.readers_cover("lab", "vault").await.unwrap());
        let confirmed = access.page_readers("lab").await.unwrap();
        assert!(
            access
                .readers_cover_for("lab", "vault", &confirmed)
                .await
                .unwrap()
        );
        // mem loses vault: mem read the page then, so nothing changes.
        auth.remove_domain_member("vault", "mem").await.unwrap();
        assert!(
            access
                .readers_cover_for("lab", "vault", &confirmed)
                .await
                .unwrap()
        );
        // A new reader of the page who may not read vault.
        auth.add_user("lee", "lee", None, Role::Viewer, "pw12345678")
            .await
            .unwrap();
        auth.upsert_domain_member("lab", "lee", MemberLevel::Viewer, None, "keeper")
            .await
            .unwrap();
        assert!(
            !access
                .readers_cover_for("lab", "vault", &confirmed)
                .await
                .unwrap()
        );
        // Shared now: anonymous and later accounts read it.
        auth.remove_domain_member("lab", "lee").await.unwrap();
        assert!(
            access
                .readers_cover_for("lab", "vault", &confirmed)
                .await
                .unwrap()
        );
        auth.set_domain_visibility("lab", false, "keeper")
            .await
            .unwrap();
        assert!(
            !access
                .readers_cover_for("lab", "vault", &confirmed)
                .await
                .unwrap()
        );
        assert!(
            access
                .readers_cover_for("lab", "vault", &PageReaders::Everyone)
                .await
                .unwrap()
        );
    }

    /// A renamed domain keeps who may do what: the rights answered for the
    /// new name are the ones the old name had, and the old name is an
    /// ordinary shared domain afterwards.
    #[tokio::test]
    async fn a_renamed_domain_keeps_its_rights_under_the_new_name() {
        let dir = tempfile::tempdir().unwrap();
        let auth = store(&dir).await;
        cast(&auth).await;
        auth.set_domain_visibility("eng", true, "keeper")
            .await
            .unwrap();
        auth.upsert_domain_member("eng", "mem", MemberLevel::Editor, None, "keeper")
            .await
            .unwrap();
        let access = DomainAccess::new(auth);
        access.rename_domain("eng", "platform").await.unwrap();
        assert_eq!(
            access
                .right(&user("keeper", false), "platform")
                .await
                .unwrap(),
            DomainRight::Own
        );
        assert_eq!(
            access.right(&user("mem", false), "platform").await.unwrap(),
            DomainRight::Write
        );
        assert_eq!(
            access.right(&user("out", false), "platform").await.unwrap(),
            DomainRight::None
        );
        assert_eq!(
            access.right(&user("out", false), "eng").await.unwrap(),
            DomainRight::Write,
            "nothing private is left under the old name"
        );
    }

    #[tokio::test]
    async fn rights_ladder_owner_admin_members_and_strangers() {
        let dir = tempfile::tempdir().unwrap();
        let auth = store(&dir).await;
        cast(&auth).await;
        auth.set_domain_visibility("lab", true, "keeper")
            .await
            .unwrap();
        auth.upsert_domain_member("lab", "mem", MemberLevel::Editor, None, "keeper")
            .await
            .unwrap();
        let access = DomainAccess::new(auth);
        assert_eq!(
            access.right(&user("keeper", false), "lab").await.unwrap(),
            DomainRight::Own
        );
        assert_eq!(
            access.right(&user("boss", true), "lab").await.unwrap(),
            DomainRight::Own
        );
        assert_eq!(
            access.right(&user("mem", false), "lab").await.unwrap(),
            DomainRight::Write
        );
        assert_eq!(
            access.right(&user("out", false), "lab").await.unwrap(),
            DomainRight::None
        );
        assert_eq!(
            access.right(&Scope::Anonymous, "lab").await.unwrap(),
            DomainRight::None
        );
        assert_eq!(
            access.right(&Scope::Unrestricted, "lab").await.unwrap(),
            DomainRight::Own
        );
        // a non-private domain follows the instance role
        assert_eq!(
            access.right(&user("out", false), "open").await.unwrap(),
            DomainRight::Write
        );
        let hidden = access
            .hidden_domains(&user("out", false))
            .await
            .unwrap()
            .unwrap();
        assert!(hidden.contains("lab"));
        assert!(
            access
                .hidden_domains(&Scope::Unrestricted)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_shared_domain_answers_every_instance_role_and_anonymous() {
        let dir = tempfile::tempdir().unwrap();
        let auth = store(&dir).await;
        cast(&auth).await;
        let access = DomainAccess::new(auth);
        assert_eq!(
            access.right(&user("mem", false), "open").await.unwrap(),
            DomainRight::Read,
            "an instance viewer reads a shared domain"
        );
        assert_eq!(
            access.right(&user("out", false), "open").await.unwrap(),
            DomainRight::Write,
            "an instance editor writes it"
        );
        assert_eq!(
            access.right(&user("boss", false), "open").await.unwrap(),
            DomainRight::Own,
            "the stored admin role counts even when the scope did not say so"
        );
        assert_eq!(
            access.right(&Scope::Anonymous, "open").await.unwrap(),
            DomainRight::Read,
            "an anonymous caller reads what is shared"
        );
        assert!(
            access
                .hidden_domains(&Scope::Anonymous)
                .await
                .unwrap()
                .unwrap()
                .is_empty(),
            "with nothing private, nothing is hidden from anyone"
        );
    }

    #[tokio::test]
    async fn a_manager_manages_and_a_viewer_member_only_reads() {
        let dir = tempfile::tempdir().unwrap();
        let auth = store(&dir).await;
        cast(&auth).await;
        auth.set_domain_visibility("lab", true, "keeper")
            .await
            .unwrap();
        auth.upsert_domain_member("lab", "mem", MemberLevel::Viewer, None, "keeper")
            .await
            .unwrap();
        auth.upsert_domain_member("lab", "out", MemberLevel::Manager, None, "keeper")
            .await
            .unwrap();
        let access = DomainAccess::new(auth);
        assert_eq!(
            access.right(&user("mem", false), "lab").await.unwrap(),
            DomainRight::Read
        );
        assert_eq!(
            access.right(&user("out", false), "lab").await.unwrap(),
            DomainRight::Manage
        );
        assert!(
            DomainRight::Manage > DomainRight::Write,
            "the ladder is ordered, so callers can compare"
        );
    }

    #[tokio::test]
    async fn a_disabled_or_removed_account_resolves_as_nobody() {
        let dir = tempfile::tempdir().unwrap();
        let auth = store(&dir).await;
        cast(&auth).await;
        auth.set_domain_visibility("lab", true, "keeper")
            .await
            .unwrap();
        auth.upsert_domain_member("lab", "mem", MemberLevel::Editor, None, "keeper")
            .await
            .unwrap();
        auth.set_disabled("mem", true).await.unwrap();
        let access = DomainAccess::new(auth);
        assert_eq!(
            access.right(&user("mem", false), "lab").await.unwrap(),
            DomainRight::None,
            "a disabled member is nobody, even with the row still there"
        );
        assert_eq!(
            access.right(&user("mem", false), "open").await.unwrap(),
            DomainRight::Read,
            "and reads a shared domain exactly as an anonymous caller does"
        );
        assert_eq!(
            access.right(&user("ghost", true), "lab").await.unwrap(),
            DomainRight::None,
            "an account the store does not have carries no admin flag"
        );
        assert!(
            access
                .hidden_domains(&user("ghost", true))
                .await
                .unwrap()
                .unwrap()
                .contains("lab")
        );
    }

    #[tokio::test]
    async fn a_re_added_owner_name_does_not_inherit_the_domain() {
        let dir = tempfile::tempdir().unwrap();
        let auth = store(&dir).await;
        cast(&auth).await;
        auth.set_domain_visibility("lab", true, "keeper")
            .await
            .unwrap();
        auth.upsert_domain_member("lab", "mem", MemberLevel::Editor, None, "keeper")
            .await
            .unwrap();
        auth.remove_user("keeper").await.unwrap();
        // A different person, sitting down at a login name that was freed.
        auth.add_user("keeper", "keeper", None, Role::Editor, "pw12345678")
            .await
            .unwrap();
        let access = DomainAccess::new(auth);
        assert_eq!(
            access.right(&user("keeper", false), "lab").await.unwrap(),
            DomainRight::None,
            "the name is not the person: a re-added account inherits nothing"
        );
        assert!(
            access
                .hidden_domains(&user("keeper", false))
                .await
                .unwrap()
                .unwrap()
                .contains("lab")
        );
        assert_eq!(
            access.right(&user("boss", true), "lab").await.unwrap(),
            DomainRight::Own,
            "an ownerless private domain is administered by admins"
        );
        assert_eq!(
            access.right(&user("mem", false), "lab").await.unwrap(),
            DomainRight::Write,
            "and the people invited into it keep their levels"
        );
    }

    #[tokio::test]
    async fn a_scope_that_claims_admin_outranks_the_stored_role() {
        let dir = tempfile::tempdir().unwrap();
        let auth = store(&dir).await;
        cast(&auth).await;
        auth.set_domain_visibility("lab", true, "keeper")
            .await
            .unwrap();
        let access = DomainAccess::new(auth);
        // `out` is a stored editor and no member of `lab`. The surface that
        // authenticated the caller is the authority on the admin flag, so a
        // scope carrying it outranks the row - the one place in this policy
        // where a scope widens what the database says, pinned here rather than
        // left to be discovered.
        assert_eq!(
            access.right(&user("out", false), "lab").await.unwrap(),
            DomainRight::None
        );
        assert_eq!(
            access.right(&user("out", true), "lab").await.unwrap(),
            DomainRight::Own
        );
        assert!(
            access
                .hidden_domains(&user("out", true))
                .await
                .unwrap()
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn hidden_domains_answers_exactly_what_right_answers() {
        let dir = tempfile::tempdir().unwrap();
        let auth = store(&dir).await;
        cast(&auth).await;
        for domain in ["lab", "vault", "attic"] {
            auth.set_domain_visibility(domain, true, "keeper")
                .await
                .unwrap();
        }
        auth.upsert_domain_member("vault", "mem", MemberLevel::Viewer, None, "keeper")
            .await
            .unwrap();
        auth.upsert_domain_member("attic", "out", MemberLevel::Manager, None, "keeper")
            .await
            .unwrap();
        auth.transfer_domain("attic", "mem").await.unwrap();
        let access = DomainAccess::new(auth);
        let scopes = [
            Scope::Unrestricted,
            Scope::Anonymous,
            user("keeper", false),
            user("boss", true),
            user("mem", false),
            user("out", false),
            user("ghost", false),
        ];
        let every_private: HashSet<String> = ["lab", "vault", "attic"]
            .into_iter()
            .map(str::to_string)
            .collect();
        for scope in &scopes {
            let hidden = access.hidden_domains(scope).await.unwrap();
            // The third public answer, held against the other two. The module
            // doc's claim is that none of them can drift apart, and it is only
            // a claim while something reads them side by side: `visibility`
            // must say exactly what `hidden_domains` says about who may see
            // what, and exactly the same private set to everybody, since which
            // domains are private is a fact about the domains rather than
            // about the caller. `Scope::Anonymous` is in this cast, which is
            // how the anonymous tier reaches the new path at all.
            let seen = access.visibility(scope).await.unwrap();
            assert_eq!(
                seen.hidden, hidden,
                "visibility and hidden_domains disagree for {scope:?}"
            );
            assert_eq!(
                seen.private, every_private,
                "every scope learns the same private set: {scope:?}"
            );
            for domain in ["lab", "vault", "attic", "open"] {
                let right = access.right(scope, domain).await.unwrap();
                match &hidden {
                    None => assert_eq!(
                        right,
                        DomainRight::Own,
                        "an unfiltered scope owns everything: {scope:?} on {domain}"
                    ),
                    Some(hidden) => assert_eq!(
                        hidden.contains(domain),
                        right < DomainRight::Read,
                        "hidden and right disagree for {scope:?} on {domain}"
                    ),
                }
            }
        }
    }

    #[tokio::test]
    async fn a_domain_made_shared_again_is_hidden_from_nobody() {
        let dir = tempfile::tempdir().unwrap();
        let auth = store(&dir).await;
        cast(&auth).await;
        auth.set_domain_visibility("lab", true, "keeper")
            .await
            .unwrap();
        auth.upsert_domain_member("lab", "mem", MemberLevel::Editor, None, "keeper")
            .await
            .unwrap();
        auth.set_domain_visibility("lab", false, "keeper")
            .await
            .unwrap();
        let access = DomainAccess::new(auth);
        assert_eq!(
            access.right(&user("out", false), "lab").await.unwrap(),
            DomainRight::Write,
            "the instance role decides again"
        );
        assert!(
            access
                .hidden_domains(&Scope::Anonymous)
                .await
                .unwrap()
                .unwrap()
                .is_empty()
        );
    }

    /// Whose draft a write joins, per scope. The machine owner is one actor
    /// like any other and drafts under the owner key; a signed-in account
    /// drafts under its own name; nobody in particular holds no draft at all,
    /// because there is no identity for one to belong to.
    #[test]
    fn overlay_actor_names_the_owner_the_account_and_nobody() {
        assert_eq!(
            overlay_actor(&Scope::Unrestricted).as_deref(),
            Some(OWNER_IDENTITY_NAME)
        );
        assert_eq!(
            overlay_actor(&Scope::User {
                account: "ada".into(),
                admin: false
            })
            .as_deref(),
            Some("ada")
        );
        assert_eq!(
            overlay_actor(&Scope::User {
                account: "boss".into(),
                admin: true
            })
            .as_deref(),
            Some("boss"),
            "an admin drafts as itself; the role widens what it may reach, never whose draft it is"
        );
        assert_eq!(overlay_actor(&Scope::Anonymous), None);
    }

    #[tokio::test]
    async fn case_variants_of_a_login_name_reach_the_same_membership() {
        let dir = tempfile::tempdir().unwrap();
        let auth = store(&dir).await;
        cast(&auth).await;
        auth.set_domain_visibility("lab", true, "Keeper")
            .await
            .unwrap();
        auth.upsert_domain_member("lab", "MEM", MemberLevel::Editor, None, "keeper")
            .await
            .unwrap();
        let access = DomainAccess::new(auth);
        assert_eq!(
            access.right(&user("KEEPER", false), "lab").await.unwrap(),
            DomainRight::Own
        );
        assert_eq!(
            access.right(&user("Mem", false), "lab").await.unwrap(),
            DomainRight::Write
        );
    }

    fn list(entries: &[&str]) -> Vec<String> {
        entries.iter().map(|e| e.to_string()).collect()
    }

    /// The match itself: an exact name, or a prefix ending in one `*`.
    #[test]
    fn write_list_matches_exact_names_and_star_prefixes() {
        let entries = list(&["agents-bot", "shared-*"]);
        assert!(write_list_matches(&entries, "agents-bot"));
        assert!(write_list_matches(&entries, "shared-"));
        assert!(write_list_matches(&entries, "shared-notes"));
        assert!(
            write_list_matches(&entries, " agents-bot "),
            "the domain is trimmed like an acl lookup"
        );
        assert!(
            !write_list_matches(&entries, "agents-bot2"),
            "an exact entry is not a prefix"
        );
        assert!(
            !write_list_matches(&entries, "shared"),
            "the prefix includes the hyphen"
        );
        assert!(!write_list_matches(&entries, "open"));
        assert!(!write_list_matches(&[], "open"));
    }

    /// An entry compares with a domain name exactly as a `domain_acl` row
    /// does: byte for byte after a trim, so case matters on both. Pinned on
    /// both tables side by side, so the two comparisons cannot drift apart.
    #[tokio::test]
    async fn write_list_entry_case_follows_domain_acl() {
        let dir = tempfile::tempdir().unwrap();
        let auth = store(&dir).await;
        cast(&auth).await;
        // The acl side: a row for `Lab` does not make `lab` private.
        auth.set_domain_visibility("Lab", true, "keeper")
            .await
            .unwrap();
        // The write-list side: `Agents-Bot` names `Agents-Bot` only.
        auth.set_write_list("OUT", &list(&["Agents-Bot", "Team-*"]))
            .await
            .unwrap();
        let access = DomainAccess::new(auth);
        let out = user("out", false);
        assert_eq!(access.right(&out, "Lab").await.unwrap(), DomainRight::None);
        assert_eq!(
            access.right(&out, "lab").await.unwrap(),
            DomainRight::Read,
            "`lab` is not the private `Lab`; it is shared, and off the list"
        );
        assert_eq!(
            access.write_right(&out, "Agents-Bot").await.unwrap(),
            DomainRight::Write
        );
        assert_eq!(
            access.write_right(&out, "agents-bot").await.unwrap(),
            DomainRight::Read,
            "a case variant of an entry is another domain"
        );
        assert_eq!(
            access.write_right(&out, "Team-x").await.unwrap(),
            DomainRight::Write
        );
        assert_eq!(
            access.write_right(&out, "team-x").await.unwrap(),
            DomainRight::Read,
            "a `*` prefix is case-sensitive too"
        );
    }

    /// The cap matrix of spec B-8: no list, an exact entry, a `*` entry, a
    /// viewer with a list, an admin with a list, the machine owner and the
    /// anonymous tier, on both public answers.
    #[tokio::test]
    async fn write_list_caps_off_list_domains_for_both_rights() {
        let dir = tempfile::tempdir().unwrap();
        let auth = store(&dir).await;
        cast(&auth).await;
        auth.add_user("star", "star", None, Role::Editor, "pw12345678")
            .await
            .unwrap();
        auth.set_domain_visibility("lab", true, "keeper")
            .await
            .unwrap();
        auth.set_write_list("out", &list(&["agents-out"]))
            .await
            .unwrap();
        auth.set_write_list("star", &list(&["agents-*"]))
            .await
            .unwrap();
        auth.set_write_list("mem", &list(&["open"])).await.unwrap();
        auth.set_write_list("boss", &list(&["agents-boss"]))
            .await
            .unwrap();
        auth.set_write_list("keeper", &list(&["agents-keeper"]))
            .await
            .unwrap();
        let access = DomainAccess::new(auth);
        let both = |right: DomainRight, write: DomainRight| (right, write);
        let ask = |scope: Scope, domain: &'static str| {
            let access = &access;
            async move {
                (
                    access.right(&scope, domain).await.unwrap(),
                    access.write_right(&scope, domain).await.unwrap(),
                )
            }
        };
        // An editor with an exact entry.
        assert_eq!(
            ask(user("out", false), "agents-out").await,
            both(DomainRight::Write, DomainRight::Write)
        );
        assert_eq!(
            ask(user("out", false), "open").await,
            both(DomainRight::Read, DomainRight::Read)
        );
        // An editor with a `*` entry.
        assert_eq!(
            ask(user("star", false), "agents-bot").await,
            both(DomainRight::Write, DomainRight::Write)
        );
        assert_eq!(
            ask(user("star", false), "agentsbot").await,
            both(DomainRight::Read, DomainRight::Read)
        );
        // A viewer with a list writes nothing, the listed domain included.
        assert_eq!(
            ask(user("mem", false), "open").await,
            both(DomainRight::Read, DomainRight::Read)
        );
        // An admin with a list: owner on the listed domain, a reader elsewhere,
        // private domains included, whether the scope or the row says admin.
        assert_eq!(
            ask(user("boss", true), "agents-boss").await,
            both(DomainRight::Own, DomainRight::Own)
        );
        assert_eq!(
            ask(user("boss", true), "open").await,
            both(DomainRight::Read, DomainRight::Read)
        );
        assert_eq!(
            ask(user("boss", false), "lab").await,
            both(DomainRight::Read, DomainRight::Read)
        );
        // A private domain's owner whose list does not name it reads it and
        // no more; it stays visible.
        assert_eq!(
            ask(user("keeper", false), "lab").await,
            both(DomainRight::Read, DomainRight::Read)
        );
        assert!(
            !access
                .hidden_domains(&user("keeper", false))
                .await
                .unwrap()
                .unwrap()
                .contains("lab")
        );
        // No list, the machine owner and the anonymous tier are unchanged.
        assert_eq!(
            ask(user("keeper", false), "agents-keeper").await,
            both(DomainRight::Write, DomainRight::Write)
        );
        assert_eq!(
            ask(Scope::Unrestricted, "open").await,
            both(DomainRight::Own, DomainRight::Own)
        );
        assert_eq!(
            ask(Scope::Anonymous, "open").await,
            both(DomainRight::Read, DomainRight::Read)
        );
    }

    /// The list only lowers: a stranger with a list still has nothing on a
    /// private domain, and the domain stays hidden from it.
    #[tokio::test]
    async fn a_stranger_with_a_write_list_still_sees_no_private_domain() {
        let dir = tempfile::tempdir().unwrap();
        let auth = store(&dir).await;
        cast(&auth).await;
        auth.set_domain_visibility("lab", true, "keeper")
            .await
            .unwrap();
        auth.set_write_list("out", &list(&["lab"])).await.unwrap();
        let access = DomainAccess::new(auth);
        let out = user("out", false);
        assert_eq!(access.right(&out, "lab").await.unwrap(), DomainRight::None);
        assert_eq!(
            access.write_right(&out, "lab").await.unwrap(),
            DomainRight::None
        );
        assert!(
            access
                .hidden_domains(&out)
                .await
                .unwrap()
                .unwrap()
                .contains("lab")
        );
    }

    /// Entries that name no domain anybody registered match nothing, so an
    /// account whose list holds only such entries writes nowhere.
    #[tokio::test]
    async fn unknown_entries_match_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let auth = store(&dir).await;
        cast(&auth).await;
        auth.set_write_list("out", &list(&["ghost", "nobody-*"]))
            .await
            .unwrap();
        let access = DomainAccess::new(auth);
        for domain in ["open", "second", "agents-bot"] {
            assert_eq!(
                access
                    .write_right(&user("out", false), domain)
                    .await
                    .unwrap(),
                DomainRight::Read,
                "{domain}"
            );
        }
    }

    /// `write_list_excludes` names the account exactly when the list is what
    /// refuses: an editor off its list. A viewer, a viewer member of a listed
    /// private domain, no list, the machine owner and the anonymous tier get
    /// `None`, so a surface keeps the role's sentence for them.
    #[tokio::test]
    async fn write_list_excludes_names_the_account_only_when_the_list_refuses() {
        let dir = tempfile::tempdir().unwrap();
        let auth = store(&dir).await;
        cast(&auth).await;
        auth.add_user("watcher", "watcher", None, Role::Editor, "pw12345678")
            .await
            .unwrap();
        auth.set_domain_visibility("lab", true, "keeper")
            .await
            .unwrap();
        auth.upsert_domain_member("lab", "watcher", MemberLevel::Viewer, None, "keeper")
            .await
            .unwrap();
        auth.set_write_list("out", &list(&["second"]))
            .await
            .unwrap();
        auth.set_write_list("mem", &list(&["second"]))
            .await
            .unwrap();
        auth.set_write_list("watcher", &list(&["lab"]))
            .await
            .unwrap();
        auth.set_write_list("boss", &list(&["second"]))
            .await
            .unwrap();
        let access = DomainAccess::new(auth);
        assert_eq!(
            access
                .write_list_excludes(&user("OUT", false), "open")
                .await
                .unwrap(),
            Some("out".to_string()),
            "the store's folded spelling"
        );
        assert_eq!(
            access
                .write_list_excludes(&user("out", false), "second")
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            access
                .write_list_excludes(&user("boss", true), "open")
                .await
                .unwrap(),
            Some("boss".to_string())
        );
        assert_eq!(
            access
                .write_list_excludes(&user("mem", false), "open")
                .await
                .unwrap(),
            None,
            "an instance viewer is refused by its role, not by its list"
        );
        assert_eq!(
            access
                .write_list_excludes(&user("watcher", false), "lab")
                .await
                .unwrap(),
            None,
            "a viewer member of a listed domain is refused by its level"
        );
        assert_eq!(
            access
                .write_list_excludes(&user("keeper", false), "open")
                .await
                .unwrap(),
            None,
            "no list"
        );
        assert_eq!(
            access
                .write_list_excludes(&Scope::Unrestricted, "open")
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            access
                .write_list_excludes(&Scope::Anonymous, "open")
                .await
                .unwrap(),
            None
        );
    }

    /// The owner-step answer reads the domain's own grant: a private domain's
    /// owner who is an instance editor is refused by its list, an instance
    /// editor on a shared domain is not (it never owned it), and an admin is.
    #[tokio::test]
    async fn write_list_refuses_an_owner_step_only_where_the_caller_would_own() {
        let dir = tempfile::tempdir().unwrap();
        let auth = store(&dir).await;
        cast(&auth).await;
        auth.set_domain_visibility("lab", true, "keeper")
            .await
            .unwrap();
        for name in ["keeper", "out", "boss"] {
            auth.set_write_list(name, &list(&["second"])).await.unwrap();
        }
        let access = DomainAccess::new(auth);
        let own = DomainRight::Own;
        assert_eq!(
            access
                .write_list_refuses(&user("keeper", false), "lab", own)
                .await
                .unwrap(),
            Some("keeper".to_string())
        );
        assert_eq!(
            access
                .write_list_refuses(&user("out", false), "open", own)
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            access
                .write_list_refuses(&user("boss", true), "open", own)
                .await
                .unwrap(),
            Some("boss".to_string())
        );
        assert_eq!(
            access
                .write_list_refuses(&user("boss", true), "second", own)
                .await
                .unwrap(),
            None
        );
    }

    /// B-3: [`WriteRights`], read once, answers exactly what the per-domain
    /// [`DomainAccess::write_right`] and [`DomainAccess::write_list_excludes`]
    /// answer, for every caller and every domain: the write-list fixture
    /// above, plus a private domain with an editor and a manager member, an
    /// account the store does not have, a padded name and a name that
    /// differs only in case.
    #[tokio::test]
    async fn write_rights_answer_what_the_per_domain_questions_answer() {
        let dir = tempfile::tempdir().unwrap();
        let auth = store(&dir).await;
        cast(&auth).await;
        for name in ["watcher", "crew", "lead"] {
            auth.add_user(name, name, None, Role::Editor, "pw12345678")
                .await
                .unwrap();
        }
        auth.set_domain_visibility("lab", true, "keeper")
            .await
            .unwrap();
        auth.upsert_domain_member("lab", "watcher", MemberLevel::Viewer, None, "keeper")
            .await
            .unwrap();
        auth.upsert_domain_member("lab", "crew", MemberLevel::Editor, None, "keeper")
            .await
            .unwrap();
        auth.upsert_domain_member("lab", "lead", MemberLevel::Manager, None, "keeper")
            .await
            .unwrap();
        auth.upsert_domain_member("lab", "mem", MemberLevel::Editor, None, "keeper")
            .await
            .unwrap();
        auth.set_write_list("out", &list(&["second"]))
            .await
            .unwrap();
        auth.set_write_list("mem", &list(&["second"]))
            .await
            .unwrap();
        auth.set_write_list("watcher", &list(&["lab"]))
            .await
            .unwrap();
        auth.set_write_list("boss", &list(&["second"]))
            .await
            .unwrap();
        auth.set_write_list("crew", &list(&["la*"])).await.unwrap();
        let access = DomainAccess::new(auth);
        let scopes = [
            Scope::Unrestricted,
            Scope::Anonymous,
            user("keeper", false),
            user("boss", true),
            user("boss", false),
            user("mem", false),
            user("OUT", false),
            user("out", false),
            user("watcher", false),
            user("crew", false),
            user("lead", false),
            user("nobody", false),
        ];
        let domains = ["open", "second", "lab", " lab ", "Lab", "agents-x"];
        for scope in &scopes {
            let rights = access.write_rights(scope).await.unwrap();
            for domain in domains {
                assert_eq!(
                    rights.write_right(domain),
                    access.write_right(scope, domain).await.unwrap(),
                    "write_right for {scope:?} on {domain:?}"
                );
                assert_eq!(
                    rights.write_list_excludes(domain),
                    access.write_list_excludes(scope, domain).await.unwrap(),
                    "write_list_excludes for {scope:?} on {domain:?}"
                );
            }
        }
    }

    /// The whole two-axis table as a pure function: knowledge level x explicit
    /// Reflection level x shared or private. Each row lists the reflection
    /// right for the explicit levels `default`, `reader`, `contributor`, in
    /// that order. An explicit level only raises; a knowledge editor always
    /// curates; the instance level never reaches a private domain.
    #[test]
    fn the_reflection_rights_table() {
        use ReflectionLevel as L;
        use ReflectionRight as R;
        let acl = DomainAcl {
            domain: "lab".into(),
            owner: "keeper".into(),
        };
        let acct = |name: &str, role: Role, reflection: L| Principal::Account {
            name: name.into(),
            role,
            reflection,
        };
        let levels = [L::Default, L::Reader, L::Contributor];

        // Shared: the instance role and the instance Reflection level decide.
        let shared: Vec<(&str, Role, [R; 3])> = vec![
            ("shared none", Role::None, [R::None, R::Read, R::Edit]),
            ("shared viewer", Role::Viewer, [R::Read, R::Read, R::Edit]),
            (
                "shared editor",
                Role::Editor,
                [R::Curate, R::Curate, R::Curate],
            ),
            (
                "shared admin",
                Role::Admin,
                [R::Curate, R::Curate, R::Curate],
            ),
        ];
        for (name, role, want) in shared {
            for (l, want) in levels.iter().zip(want) {
                let p = acct("mem", role, *l);
                assert_eq!(
                    decide_reflection(&p, None, None, L::Default, None, false),
                    want,
                    "{name}, {l}"
                );
            }
        }

        // Private: the membership row and its Reflection level decide; the
        // principal's instance level is `contributor` on purpose and must not
        // count.
        // (row name, account, instance role, membership level, expected).
        type PrivateRow<'a> = (&'a str, &'a str, Role, Option<MemberLevel>, [R; 3]);
        let private: Vec<PrivateRow> = vec![
            (
                "no membership",
                "mem",
                Role::Viewer,
                None,
                [R::None, R::None, R::None],
            ),
            (
                "member none",
                "mem",
                Role::Viewer,
                Some(MemberLevel::None),
                [R::None, R::Read, R::Edit],
            ),
            (
                "member viewer",
                "mem",
                Role::Viewer,
                Some(MemberLevel::Viewer),
                [R::Read, R::Read, R::Edit],
            ),
            (
                "member viewer, instance none",
                "mem",
                Role::None,
                Some(MemberLevel::Viewer),
                [R::Read, R::Read, R::Edit],
            ),
            (
                "member editor, instance viewer",
                "mem",
                Role::Viewer,
                Some(MemberLevel::Editor),
                [R::Curate, R::Curate, R::Curate],
            ),
            (
                "member manager",
                "mem",
                Role::Editor,
                Some(MemberLevel::Manager),
                [R::Curate, R::Curate, R::Curate],
            ),
            (
                "owner, instance viewer",
                "keeper",
                Role::Viewer,
                None,
                [R::Curate, R::Curate, R::Curate],
            ),
            (
                "admin",
                "boss",
                Role::Admin,
                None,
                [R::Curate, R::Curate, R::Curate],
            ),
        ];
        for (name, who, role, level, want) in private {
            for (l, want) in levels.iter().zip(want) {
                let p = acct(who, role, L::Contributor);
                assert_eq!(
                    decide_reflection(&p, Some(&acl), level, *l, None, false),
                    want,
                    "{name}, {l}"
                );
            }
        }

        // Nobody in particular, and the machine owner.
        assert_eq!(
            decide_reflection(&Principal::Anonymous, None, None, L::Default, None, false),
            R::Read,
            "anonymous reads a public domain"
        );
        assert_eq!(
            decide_reflection(
                &Principal::Anonymous,
                Some(&acl),
                None,
                L::Default,
                None,
                false
            ),
            R::None
        );
        assert_eq!(
            decide_reflection(
                &Principal::Unrestricted,
                Some(&acl),
                None,
                L::Default,
                None,
                false
            ),
            R::Curate
        );

        // The write list caps the whole account (a cap, never a grant).
        // Off the list, every right is at most Read, whatever the knowledge
        // right and the explicit level say; None stays None.
        for l in levels {
            assert_eq!(
                decide_reflection(
                    &acct("mem", Role::Editor, l),
                    None,
                    None,
                    L::Default,
                    None,
                    true
                ),
                R::Read,
                "shared editor off its list, {l}"
            );
            assert_eq!(
                decide_reflection(
                    &acct("boss", Role::Admin, l),
                    None,
                    None,
                    L::Default,
                    None,
                    true
                ),
                R::Read,
                "admin with a list, shared, {l}"
            );
            assert_eq!(
                decide_reflection(
                    &acct("keeper", Role::Viewer, L::Default),
                    Some(&acl),
                    None,
                    l,
                    None,
                    true
                ),
                R::Read,
                "owner off its list, {l}"
            );
            assert_eq!(
                decide_reflection(
                    &acct("mem", Role::Viewer, L::Default),
                    Some(&acl),
                    Some(MemberLevel::Editor),
                    l,
                    None,
                    true
                ),
                R::Read,
                "private member editor off its list, {l}"
            );
        }
        // The instance level never counts on a private domain, so this row is
        // asserted once, not once per level.
        assert_eq!(
            decide_reflection(
                &acct("boss", Role::Admin, L::Default),
                Some(&acl),
                None,
                L::Default,
                None,
                true
            ),
            R::Read,
            "admin with a list, private"
        );
        assert_eq!(
            decide_reflection(
                &acct("mem", Role::Viewer, L::Default),
                Some(&acl),
                Some(MemberLevel::Viewer),
                L::Contributor,
                None,
                true
            ),
            R::Read,
            "a private contributor member at knowledge viewer, off its list"
        );
        assert_eq!(
            decide_reflection(
                &acct("mem", Role::Viewer, L::Default),
                Some(&acl),
                Some(MemberLevel::None),
                L::Contributor,
                None,
                true
            ),
            R::Read,
            "a private contributor member at knowledge none, off its list"
        );
        assert_eq!(
            decide_reflection(
                &acct("mem", Role::Viewer, L::Contributor),
                None,
                None,
                L::Default,
                None,
                true
            ),
            R::Read,
            "shared viewer contributor, off its list"
        );
        assert_eq!(
            decide_reflection(
                &acct("mem", Role::None, L::Contributor),
                None,
                None,
                L::Default,
                None,
                true
            ),
            R::Read,
            "shared none contributor, off its list"
        );
        assert_eq!(
            decide_reflection(
                &acct("mem", Role::None, L::Default),
                None,
                None,
                L::Default,
                None,
                true
            ),
            R::None,
            "a cap never grants"
        );
        assert_eq!(
            decide_reflection(
                &acct("mem", Role::Viewer, L::Default),
                Some(&acl),
                None,
                L::Contributor,
                None,
                true
            ),
            R::None,
            "no membership stays None"
        );

        // The knowledge axis: `none` reads no engram, and never writes one.
        assert_eq!(
            decide(&acct("mem", Role::None, L::Default), None, None, None),
            DomainRight::None
        );
        assert_eq!(
            decide(
                &acct("mem", Role::Viewer, L::Default),
                Some(&acl),
                Some(MemberLevel::None),
                None
            ),
            DomainRight::None
        );
        assert_eq!(
            decide(
                &acct("mem", Role::None, L::Default),
                Some(&acl),
                Some(MemberLevel::Viewer),
                None
            ),
            DomainRight::Read
        );
        assert_eq!(
            instance_cap(&acct("mem", Role::None, L::Default)),
            DomainRight::Read
        );

        assert_eq!(reflection_right_word(R::Read), "read");
        assert!(R::None < R::Read && R::Read < R::Edit && R::Edit < R::Curate);
    }

    /// Reading and commenting are one level; the anonymous visitor of a
    /// public domain is the one reader who never comments.
    #[test]
    fn only_the_anonymous_visitor_reads_without_commenting() {
        assert!(!ReflectionCaller::person(Scope::Anonymous, "anonymous").may_comment());
        assert!(
            ReflectionCaller::agent(Scope::Anonymous, "agent", true).may_comment(),
            "the MCP open tier"
        );
        assert!(ReflectionCaller::person(user("mem", false), "mem").may_comment());
        assert!(ReflectionCaller::owner().may_comment());
    }

    /// Through the store: a member with knowledge `none` is hidden from engram
    /// listings and edits the Reflection; an instance-`none` account hides
    /// every shared domain.
    #[tokio::test]
    async fn knowledge_none_is_hidden_as_engrams_and_reaches_the_reflection() {
        let dir = tempfile::tempdir().unwrap();
        let auth = store(&dir).await;
        cast(&auth).await;
        auth.set_domain_visibility("lab", true, "keeper")
            .await
            .unwrap();
        auth.upsert_domain_member(
            "lab",
            "mem",
            MemberLevel::None,
            Some(ReflectionLevel::Contributor),
            "keeper",
        )
        .await
        .unwrap();
        let access = DomainAccess::new(auth.clone());
        let mem = user("mem", false);
        assert_eq!(access.right(&mem, "lab").await.unwrap(), DomainRight::None);
        assert!(
            access
                .hidden_domains(&mem)
                .await
                .unwrap()
                .unwrap()
                .contains("lab")
        );
        assert_eq!(
            access.reflection_right(&mem, "lab").await.unwrap(),
            ReflectionRight::Edit
        );
        assert_eq!(
            access
                .reflection_right(&Scope::Unrestricted, "lab")
                .await
                .unwrap(),
            ReflectionRight::Curate
        );

        auth.set_role("out", Role::None).await.unwrap();
        auth.set_reflection_level("out", ReflectionLevel::Reader)
            .await
            .unwrap();
        let out = user("out", false);
        assert_eq!(access.right(&out, "open").await.unwrap(), DomainRight::None);
        assert!(access.visibility(&out).await.unwrap().shared_hidden);
        assert!(!access.visibility(&mem).await.unwrap().shared_hidden);
        // The screen a filter applies fails closed for out: no shared domain,
        // known or registered later, and no private one it does not read.
        let screen = access.visibility(&out).await.unwrap().screen();
        assert_eq!(screen, DomainScreen::Only(HashSet::new()));
        assert!(!screen.admits("open") && !screen.admits("lab") && !screen.admits("later"));
        assert!(!screen.admits_shared());
        let screen = access.visibility(&mem).await.unwrap().screen();
        assert!(screen.admits("open") && !screen.admits("lab") && screen.admits_shared());
        assert_eq!(
            access
                .visibility(&Scope::Unrestricted)
                .await
                .unwrap()
                .screen(),
            DomainScreen::All
        );
        assert_eq!(
            access.reflection_right(&out, "open").await.unwrap(),
            ReflectionRight::Read
        );
        assert_eq!(
            access.reflection_right(&out, "lab").await.unwrap(),
            ReflectionRight::None,
            "the instance level stays off a private domain"
        );

        // Through the store: the write list caps the Reflection.
        let keeper = user("keeper", false);
        assert_eq!(
            access.reflection_right(&keeper, "open").await.unwrap(),
            ReflectionRight::Curate
        );
        auth.set_write_list("keeper", &["agents-*".to_string()])
            .await
            .unwrap();
        assert_eq!(
            access.reflection_right(&keeper, "open").await.unwrap(),
            ReflectionRight::Read,
            "a shared domain off the list"
        );
        assert_eq!(
            access.reflection_right(&keeper, "lab").await.unwrap(),
            ReflectionRight::Read,
            "the owner off its list too"
        );
        assert_eq!(
            access.reflection_right(&keeper, "agents-1").await.unwrap(),
            ReflectionRight::Curate,
            "a domain on the list"
        );
        // Below Write too: mem (member none, Reflection contributor on lab)
        // and out (instance none, Reflection reader) are capped by their lists.
        assert_eq!(
            access.reflection_right(&mem, "lab").await.unwrap(),
            ReflectionRight::Edit,
            "no list, nothing changes"
        );
        auth.set_write_list("mem", &["agents-*".to_string()])
            .await
            .unwrap();
        assert_eq!(
            access.reflection_right(&mem, "lab").await.unwrap(),
            ReflectionRight::Read,
            "contributor capped at Read"
        );
        auth.set_write_list("out", &["agents-*".to_string()])
            .await
            .unwrap();
        assert_eq!(
            access.reflection_right(&out, "open").await.unwrap(),
            ReflectionRight::Read
        );
        assert_eq!(
            access.reflection_right(&out, "lab").await.unwrap(),
            ReflectionRight::None,
            "a cap never grants"
        );
        assert_eq!(
            access
                .reflection_list_refuses(&mem, "lab", ReflectionRight::Edit)
                .await
                .unwrap()
                .as_deref(),
            Some("mem"),
            "the list is why mem may not edit"
        );
        assert_eq!(
            access
                .reflection_list_refuses(&mem, "lab", ReflectionRight::Curate)
                .await
                .unwrap(),
            None,
            "mem never curated"
        );
        assert_eq!(
            access
                .reflection_list_refuses(&keeper, "lab", ReflectionRight::Curate)
                .await
                .unwrap()
                .as_deref(),
            Some("keeper")
        );
    }

    // ---- GitHub-governed domains (Task 3.2) ----

    /// A [`KnowledgeGovernor`] answering from fixed tables. `readers` is
    /// derived from the grants unless a test pins it (or pins it unknown).
    #[derive(Default)]
    struct FakeGovernor {
        governed: HashSet<String>,
        grants: HashMap<(String, String), GithubKnowledge>,
        public: HashSet<String>,
        readers: HashMap<String, Option<HashSet<String>>>,
    }

    impl FakeGovernor {
        fn govern(mut self, domain: &str) -> Self {
            self.governed.insert(domain.to_string());
            self
        }

        fn grant(mut self, account: &str, domain: &str, level: MemberLevel) -> Self {
            self.grants.insert(
                (account.to_string(), domain.to_string()),
                GithubKnowledge::Level(level),
            );
            self
        }

        fn public(mut self, domain: &str) -> Self {
            self.public.insert(domain.to_string());
            self
        }

        fn readers(mut self, domain: &str, readers: Option<&[&str]>) -> Self {
            self.readers.insert(
                domain.to_string(),
                readers.map(|names| names.iter().map(|n| n.to_string()).collect()),
            );
            self
        }
    }

    #[async_trait::async_trait]
    impl KnowledgeGovernor for FakeGovernor {
        fn governed_domains(&self) -> HashSet<String> {
            self.governed.clone()
        }

        async fn knowledge(&self, account: &str, domain: &str) -> GithubKnowledge {
            self.grants
                .get(&(account.to_string(), domain.to_string()))
                .copied()
                .unwrap_or(GithubKnowledge::Unlinked)
        }

        async fn repository_public(&self, domain: &str) -> bool {
            self.public.contains(domain)
        }

        async fn readers(&self, domain: &str) -> Option<HashSet<String>> {
            if let Some(pinned) = self.readers.get(domain) {
                return pinned.clone();
            }
            Some(
                self.grants
                    .iter()
                    .filter(|((_, d), k)| d == domain && github_right(**k) >= DomainRight::Read)
                    .map(|((a, _), _)| a.clone())
                    .collect(),
            )
        }
    }

    /// [`cast`] plus `ann` (instance viewer) and `nora` (instance `none`);
    /// `gh-lab` and `gh-vault` private to keeper, `lab` private to keeper
    /// without GitHub. `gh-open` is a shared domain (no acl row).
    async fn github_cast(auth: &AuthStore) {
        cast(auth).await;
        auth.add_user("ann", "ann", None, Role::Viewer, "pw12345678")
            .await
            .unwrap();
        auth.add_user("nora", "nora", None, Role::None, "pw12345678")
            .await
            .unwrap();
        for domain in ["gh-lab", "gh-vault", "lab"] {
            auth.set_domain_visibility(domain, true, "keeper")
                .await
                .unwrap();
        }
    }

    fn base_governor() -> FakeGovernor {
        FakeGovernor::default()
            .govern("gh-open")
            .govern("gh-lab")
            .govern("gh-vault")
    }

    async fn governed_access(dir: &tempfile::TempDir, governor: FakeGovernor) -> DomainAccess {
        let auth = store(dir).await;
        github_cast(&auth).await;
        DomainAccess::new(auth).with_governor(Arc::new(governor))
    }

    fn names(list: &[&str]) -> HashSet<String> {
        list.iter().map(|n| n.to_string()).collect()
    }

    fn governed(knowledge: GithubKnowledge) -> Option<Governed> {
        Some(Governed {
            knowledge,
            public: false,
        })
    }

    #[test]
    fn github_roles_map_to_rights_on_a_private_domain() {
        let acl = DomainAcl {
            domain: "gh-lab".into(),
            owner: "keeper".into(),
        };
        let viewer = Principal::Account {
            name: "mem".into(),
            role: Role::Viewer,
            reflection: ReflectionLevel::Default,
        };
        for (knowledge, want) in [
            (
                GithubKnowledge::Level(MemberLevel::Manager),
                DomainRight::Manage,
            ),
            (
                GithubKnowledge::Level(MemberLevel::Editor),
                DomainRight::Write,
            ),
            (
                GithubKnowledge::Level(MemberLevel::Viewer),
                DomainRight::Read,
            ),
            (GithubKnowledge::Level(MemberLevel::None), DomainRight::None),
            (GithubKnowledge::Unlinked, DomainRight::None),
        ] {
            assert_eq!(
                decide(&viewer, Some(&acl), None, governed(knowledge)),
                want,
                "{knowledge:?}"
            );
        }
    }

    #[tokio::test]
    async fn an_unlinked_viewer_reads_a_shared_github_domain() {
        let dir = tempfile::tempdir().unwrap();
        let access = governed_access(&dir, base_governor()).await;
        let ann = user("ann", false);
        assert_eq!(
            access.right(&ann, "gh-open").await.unwrap(),
            DomainRight::Read
        );
        assert_eq!(
            access.write_right(&ann, "gh-open").await.unwrap(),
            DomainRight::Read
        );
        assert!(
            access
                .visibility(&ann)
                .await
                .unwrap()
                .screen()
                .admits("gh-open")
        );
        // An unlinked instance editor reads it too, and no more.
        let out = user("out", false);
        assert_eq!(
            access.right(&out, "gh-open").await.unwrap(),
            DomainRight::Read
        );
    }

    #[tokio::test]
    async fn a_github_none_account_reads_a_shared_github_domain() {
        let dir = tempfile::tempdir().unwrap();
        let governor = base_governor()
            .grant("mem", "gh-open", MemberLevel::None)
            .grant("out", "gh-open", MemberLevel::None);
        let access = governed_access(&dir, governor).await;
        // An instance viewer and an instance editor, both GitHub none.
        for who in ["mem", "out"] {
            let scope = user(who, false);
            assert_eq!(
                access.right(&scope, "gh-open").await.unwrap(),
                DomainRight::Read,
                "{who}"
            );
            assert!(
                access
                    .visibility(&scope)
                    .await
                    .unwrap()
                    .screen()
                    .admits("gh-open"),
                "{who}"
            );
        }
    }

    #[tokio::test]
    async fn an_unlinked_account_reads_nothing_on_a_private_github_domain() {
        let dir = tempfile::tempdir().unwrap();
        let auth = store(&dir).await;
        github_cast(&auth).await;
        // An old member row from before the domain followed GitHub.
        auth.upsert_domain_member("gh-lab", "out", MemberLevel::Editor, None, "keeper")
            .await
            .unwrap();
        let access = DomainAccess::new(auth).with_governor(Arc::new(base_governor()));
        // `out` is an instance editor: the role reaches no private domain.
        for who in ["ann", "out"] {
            let scope = user(who, false);
            assert_eq!(
                access.right(&scope, "gh-lab").await.unwrap(),
                DomainRight::None,
                "{who}"
            );
            assert!(
                access
                    .hidden_domains(&scope)
                    .await
                    .unwrap()
                    .unwrap()
                    .contains("gh-lab"),
                "{who}"
            );
        }
    }

    #[tokio::test]
    async fn a_github_reader_reads_a_private_github_domain() {
        let dir = tempfile::tempdir().unwrap();
        let governor = base_governor().grant("ann", "gh-lab", MemberLevel::Viewer);
        let access = governed_access(&dir, governor).await;
        let ann = user("ann", false);
        assert_eq!(
            access.right(&ann, "gh-lab").await.unwrap(),
            DomainRight::Read
        );
        assert_eq!(
            access.write_right(&ann, "gh-lab").await.unwrap(),
            DomainRight::Read
        );
        let hidden = access.hidden_domains(&ann).await.unwrap().unwrap();
        assert!(!hidden.contains("gh-lab"), "{hidden:?}");
        assert!(hidden.contains("gh-vault"), "{hidden:?}");
    }

    #[tokio::test]
    async fn an_instance_viewer_with_github_write_writes() {
        let dir = tempfile::tempdir().unwrap();
        let governor = base_governor()
            .grant("ann", "gh-lab", MemberLevel::Editor)
            .grant("ann", "gh-open", MemberLevel::Editor);
        let access = governed_access(&dir, governor).await;
        let ann = user("ann", false);
        let rights = access.write_rights(&ann).await.unwrap();
        for domain in ["gh-lab", "gh-open"] {
            assert_eq!(
                access.write_right(&ann, domain).await.unwrap(),
                DomainRight::Write,
                "{domain}"
            );
            assert_eq!(rights.write_right(domain), DomainRight::Write, "{domain}");
        }
        // A shared domain without GitHub keeps the instance cap.
        assert_eq!(
            access.write_right(&ann, "open").await.unwrap(),
            DomainRight::Read
        );
        assert_eq!(rights.write_right("open"), DomainRight::Read);
    }

    #[tokio::test]
    async fn an_unlinked_viewer_never_writes_a_shared_github_domain() {
        let dir = tempfile::tempdir().unwrap();
        let access = governed_access(&dir, base_governor()).await;
        // `out` is an instance editor and unlinked: it writes `open`, never
        // `gh-open`.
        for who in ["ann", "out"] {
            let scope = user(who, false);
            assert_eq!(
                access.write_right(&scope, "gh-open").await.unwrap(),
                DomainRight::Read,
                "{who}"
            );
            assert_eq!(
                access
                    .write_rights(&scope)
                    .await
                    .unwrap()
                    .write_right("gh-open"),
                DomainRight::Read,
                "{who}"
            );
        }
        assert_eq!(
            access
                .write_right(&user("out", false), "open")
                .await
                .unwrap(),
            DomainRight::Write
        );
    }

    #[tokio::test]
    async fn the_write_list_still_restricts_a_github_editor() {
        let dir = tempfile::tempdir().unwrap();
        let auth = store(&dir).await;
        github_cast(&auth).await;
        let governor = base_governor()
            .grant("ann", "gh-lab", MemberLevel::Editor)
            .grant("ann", "gh-open", MemberLevel::Manager);
        let access = DomainAccess::new(auth.clone()).with_governor(Arc::new(governor));
        auth.set_write_list("ann", &["agents-*".to_string()])
            .await
            .unwrap();
        let ann = user("ann", false);
        let rights = access.write_rights(&ann).await.unwrap();
        for domain in ["gh-lab", "gh-open"] {
            assert_eq!(
                access.write_right(&ann, domain).await.unwrap(),
                DomainRight::Read,
                "{domain}"
            );
            assert_eq!(access.right(&ann, domain).await.unwrap(), DomainRight::Read);
            assert_eq!(rights.write_right(domain), DomainRight::Read, "{domain}");
            assert_eq!(
                access
                    .write_list_excludes(&ann, domain)
                    .await
                    .unwrap()
                    .as_deref(),
                Some("ann"),
                "{domain}"
            );
            assert_eq!(
                rights.write_list_excludes(domain).as_deref(),
                Some("ann"),
                "{domain}"
            );
            assert_eq!(
                access.reflection_right(&ann, domain).await.unwrap(),
                ReflectionRight::Read,
                "{domain}"
            );
        }
    }

    /// The MCP open tier acting as `github.agent_identity`: that account's
    /// view on the governed domains, an anonymous caller's on the rest (so
    /// the agent's own private domain `lab` stays hidden).
    #[tokio::test]
    async fn the_open_tier_sees_governed_domains_as_its_agent_and_the_rest_as_anonymous() {
        let dir = tempfile::tempdir().unwrap();
        let governor = base_governor()
            .grant("keeper", "gh-lab", MemberLevel::Viewer)
            .grant("nora", "gh-lab", MemberLevel::Manager);
        let access = governed_access(&dir, governor).await;
        assert!(access.governs("gh-lab") && access.governs(" gh-open "));
        assert!(!access.governs("lab"));
        let hidden = |agent: &'static str| {
            let access = &access;
            async move { access.open_tier_visibility(agent).await.unwrap().screen() }
        };
        let keeper = hidden("keeper").await;
        assert!(keeper.admits("gh-lab") && keeper.admits("gh-open"));
        assert!(!keeper.admits("gh-vault"), "no GitHub grant");
        assert!(!keeper.admits("lab"), "keeper's own, but not governed");
        assert!(keeper.admits("open"), "shared and not governed");
        // Instance knowledge `none`: no shared governed domain.
        let nora = hidden("nora").await;
        assert!(!nora.admits("gh-open"));
        assert!(!nora.admits("gh-lab"), "instance none gets nothing");
        assert!(nora.admits("open"), "anonymous reads shared elsewhere");
        // No such account: anonymous everywhere.
        let ghost = hidden("ghost").await;
        assert!(!ghost.admits("gh-open") && !ghost.admits("gh-lab") && !ghost.admits("lab"));
        assert!(ghost.admits("open"));
    }

    /// An instance admin named as `github.agent_identity` gives the open
    /// tier its GitHub grant and nothing more: as `Scope::Agent` it is an
    /// editor, so it never owns a governed domain and never sees a private
    /// one it has no GitHub read on.
    #[tokio::test]
    async fn an_admin_agent_identity_gets_only_its_github_role() {
        let dir = tempfile::tempdir().unwrap();
        let governor = base_governor().grant("boss", "gh-lab", MemberLevel::Viewer);
        let access = governed_access(&dir, governor).await;
        let agent = Scope::Agent {
            account: "boss".to_string(),
        };
        assert_eq!(
            access.right(&agent, "gh-lab").await.unwrap(),
            DomainRight::Read
        );
        assert_eq!(
            access.write_right(&agent, "gh-lab").await.unwrap(),
            DomainRight::Read
        );
        assert_eq!(
            access.right(&agent, "gh-vault").await.unwrap(),
            DomainRight::None
        );
        assert_eq!(
            access.right(&agent, "gh-open").await.unwrap(),
            DomainRight::Read
        );
        assert!(
            access.reflection_right(&agent, "gh-vault").await.unwrap() == ReflectionRight::None
        );
        let screen = access.open_tier_visibility("boss").await.unwrap().screen();
        assert!(screen.admits("gh-lab"));
        assert!(!screen.admits("gh-vault"), "no GitHub read");
        // The same account signed in is the admin it is.
        assert_eq!(
            access
                .right(&user("boss", false), "gh-vault")
                .await
                .unwrap(),
            DomainRight::Own
        );
    }

    /// A governor whose governed set is `gh-lab` on the first ask and empty
    /// after: the App found out mid-answer that it is not installed there.
    struct FlippingGovernor {
        inner: FakeGovernor,
        asked: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl KnowledgeGovernor for FlippingGovernor {
        fn governed_domains(&self) -> HashSet<String> {
            if self.asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                self.inner.governed_domains()
            } else {
                HashSet::new()
            }
        }
        async fn knowledge(&self, account: &str, domain: &str) -> GithubKnowledge {
            self.inner.knowledge(account, domain).await
        }
        async fn repository_public(&self, domain: &str) -> bool {
            self.inner.repository_public(domain).await
        }
        async fn readers(&self, domain: &str) -> Option<HashSet<String>> {
            KnowledgeGovernor::readers(&self.inner, domain).await
        }
    }

    /// The open tier's view decides every domain under one governed set:
    /// `gh-lab` is keeper's own private domain, governed when the answer
    /// began, and keeper has no GitHub read on it, so it stays hidden even
    /// though the governed set changes while the answer is put together.
    #[tokio::test]
    async fn the_open_tier_view_takes_one_governed_set() {
        let dir = tempfile::tempdir().unwrap();
        let auth = store(&dir).await;
        github_cast(&auth).await;
        let access = DomainAccess::new(auth).with_governor(Arc::new(FlippingGovernor {
            inner: FakeGovernor::default().govern("gh-lab"),
            asked: std::sync::atomic::AtomicUsize::new(0),
        }));
        let screen = access
            .open_tier_visibility("keeper")
            .await
            .unwrap()
            .screen();
        assert!(!screen.admits("gh-lab"));
    }

    #[tokio::test]
    async fn knowledge_none_account_gets_nothing_with_github_admin() {
        let dir = tempfile::tempdir().unwrap();
        let auth = store(&dir).await;
        github_cast(&auth).await;
        auth.set_reflection_level("nora", ReflectionLevel::Contributor)
            .await
            .unwrap();
        let governor = base_governor()
            .grant("nora", "gh-lab", MemberLevel::Manager)
            .grant("nora", "gh-open", MemberLevel::Manager);
        let access = DomainAccess::new(auth).with_governor(Arc::new(governor));
        let nora = user("nora", false);
        for domain in ["gh-lab", "gh-open"] {
            assert_eq!(
                access.right(&nora, domain).await.unwrap(),
                DomainRight::None,
                "{domain}"
            );
            assert_eq!(
                access.write_right(&nora, domain).await.unwrap(),
                DomainRight::None,
                "{domain}"
            );
            assert_eq!(
                access
                    .write_rights(&nora)
                    .await
                    .unwrap()
                    .write_right(domain),
                DomainRight::None,
                "{domain}"
            );
            assert_eq!(
                access.reflection_right(&nora, domain).await.unwrap(),
                ReflectionRight::None,
                "{domain}: no Reflection either"
            );
            assert!(
                !access
                    .visibility(&nora)
                    .await
                    .unwrap()
                    .screen()
                    .admits(domain)
            );
        }
        assert!(
            !access
                .readers_of("gh-lab")
                .await
                .unwrap()
                .unwrap()
                .contains("nora")
        );
    }

    #[tokio::test]
    async fn an_instance_admin_owns_a_governed_domain_whatever_github_says() {
        let dir = tempfile::tempdir().unwrap();
        let governor = base_governor()
            .grant("boss", "gh-lab", MemberLevel::None)
            .readers("gh-lab", Some(&[]));
        let access = governed_access(&dir, governor).await;
        let boss = user("boss", false);
        for domain in ["gh-lab", "gh-open", "gh-vault"] {
            assert_eq!(access.right(&boss, domain).await.unwrap(), DomainRight::Own);
            assert_eq!(
                access.write_right(&boss, domain).await.unwrap(),
                DomainRight::Own
            );
            assert_eq!(
                access.reflection_right(&boss, domain).await.unwrap(),
                ReflectionRight::Curate
            );
        }
        assert!(
            access
                .hidden_domains(&boss)
                .await
                .unwrap()
                .unwrap()
                .is_empty()
        );
        assert!(
            access
                .readers_of("gh-lab")
                .await
                .unwrap()
                .unwrap()
                .contains("boss")
        );
        assert_eq!(
            access.right(&Scope::Unrestricted, "gh-lab").await.unwrap(),
            DomainRight::Own
        );
    }

    #[tokio::test]
    async fn the_acl_owner_of_a_private_governed_domain_gets_only_the_github_role() {
        let dir = tempfile::tempdir().unwrap();
        let governor = base_governor().grant("keeper", "gh-lab", MemberLevel::Viewer);
        let access = governed_access(&dir, governor).await;
        let keeper = user("keeper", false);
        assert_eq!(
            access.right(&keeper, "gh-lab").await.unwrap(),
            DomainRight::Read
        );
        assert_eq!(
            access.write_right(&keeper, "gh-lab").await.unwrap(),
            DomainRight::Read
        );
        assert_eq!(
            access.right(&keeper, "gh-vault").await.unwrap(),
            DomainRight::None,
            "owner, unlinked"
        );
        assert!(
            access
                .hidden_domains(&keeper)
                .await
                .unwrap()
                .unwrap()
                .contains("gh-vault")
        );
        // The owner without GitHub still owns its plain private domain.
        assert_eq!(
            access.right(&keeper, "lab").await.unwrap(),
            DomainRight::Own
        );
    }

    #[tokio::test]
    async fn old_member_rows_are_ignored_on_a_governed_domain() {
        let dir = tempfile::tempdir().unwrap();
        let auth = store(&dir).await;
        github_cast(&auth).await;
        auth.upsert_domain_member("gh-lab", "ann", MemberLevel::Manager, None, "keeper")
            .await
            .unwrap();
        auth.upsert_domain_member("gh-lab", "mem", MemberLevel::Viewer, None, "keeper")
            .await
            .unwrap();
        let governor = base_governor().grant("mem", "gh-lab", MemberLevel::Editor);
        let access = DomainAccess::new(auth).with_governor(Arc::new(governor));
        let ann = user("ann", false);
        let mem = user("mem", false);
        assert_eq!(
            access.right(&ann, "gh-lab").await.unwrap(),
            DomainRight::None
        );
        assert!(
            access
                .hidden_domains(&ann)
                .await
                .unwrap()
                .unwrap()
                .contains("gh-lab")
        );
        assert_eq!(
            access
                .write_rights(&ann)
                .await
                .unwrap()
                .write_right("gh-lab"),
            DomainRight::None
        );
        assert_eq!(
            access.right(&mem, "gh-lab").await.unwrap(),
            DomainRight::Write
        );
        assert_eq!(
            access.write_right(&mem, "gh-lab").await.unwrap(),
            DomainRight::Write
        );
        // The pure rule ignores a membership level it is handed by mistake.
        let principal = Principal::Account {
            name: "ann".into(),
            role: Role::Viewer,
            reflection: ReflectionLevel::Default,
        };
        let acl = DomainAcl {
            domain: "gh-lab".into(),
            owner: "ann".into(),
        };
        assert_eq!(
            decide(
                &principal,
                Some(&acl),
                Some(MemberLevel::Manager),
                governed(GithubKnowledge::Unlinked)
            ),
            DomainRight::None
        );
    }

    #[tokio::test]
    async fn an_old_member_reflection_level_is_ignored_on_a_governed_domain() {
        let dir = tempfile::tempdir().unwrap();
        let auth = store(&dir).await;
        github_cast(&auth).await;
        auth.upsert_domain_member(
            "gh-lab",
            "ann",
            MemberLevel::None,
            Some(ReflectionLevel::Contributor),
            "keeper",
        )
        .await
        .unwrap();
        let access = DomainAccess::new(auth).with_governor(Arc::new(base_governor()));
        assert_eq!(
            access
                .reflection_right(&user("ann", false), "gh-lab")
                .await
                .unwrap(),
            ReflectionRight::None
        );
        let PageReaders::Accounts(readers) = access.page_readers("gh-lab").await.unwrap() else {
            panic!("a private page has named readers");
        };
        assert!(!readers.contains("ann"), "{readers:?}");
    }

    #[tokio::test]
    async fn anonymous_reads_a_shared_domain_of_a_public_repository() {
        let dir = tempfile::tempdir().unwrap();
        let access = governed_access(&dir, base_governor().public("gh-open")).await;
        assert_eq!(
            access.right(&Scope::Anonymous, "gh-open").await.unwrap(),
            DomainRight::Read
        );
        assert!(
            access
                .visibility(&Scope::Anonymous)
                .await
                .unwrap()
                .screen()
                .admits("gh-open")
        );
        assert_eq!(
            access
                .reflection_right(&Scope::Anonymous, "gh-open")
                .await
                .unwrap(),
            ReflectionRight::Read
        );
        assert_eq!(access.readers_of("gh-open").await.unwrap(), None);
    }

    #[tokio::test]
    async fn anonymous_reads_nothing_when_the_repository_is_private() {
        let dir = tempfile::tempdir().unwrap();
        let access = governed_access(&dir, base_governor()).await;
        assert_eq!(
            access.right(&Scope::Anonymous, "gh-open").await.unwrap(),
            DomainRight::None
        );
        assert_eq!(
            access
                .reflection_right(&Scope::Anonymous, "gh-open")
                .await
                .unwrap(),
            ReflectionRight::None
        );
        assert!(
            !access
                .visibility(&Scope::Anonymous)
                .await
                .unwrap()
                .screen()
                .admits("gh-open")
        );
        // Who hears about it: every enabled account at viewer or above,
        // never an anonymous listener.
        assert_eq!(
            access.readers_of("gh-open").await.unwrap(),
            Some(names(&["keeper", "boss", "mem", "out", "ann"]))
        );
        // The same with no private domain on the instance at all.
        let dir = tempfile::tempdir().unwrap();
        let auth = store(&dir).await;
        cast(&auth).await;
        let access = DomainAccess::new(auth).with_governor(Arc::new(base_governor()));
        assert!(
            access
                .hidden_domains(&Scope::Anonymous)
                .await
                .unwrap()
                .unwrap()
                .contains("gh-open")
        );
    }

    #[tokio::test]
    async fn anonymous_reads_nothing_on_a_private_domain() {
        let dir = tempfile::tempdir().unwrap();
        let access = governed_access(&dir, base_governor().public("gh-lab")).await;
        assert_eq!(
            access.right(&Scope::Anonymous, "gh-lab").await.unwrap(),
            DomainRight::None
        );
        assert!(
            access
                .hidden_domains(&Scope::Anonymous)
                .await
                .unwrap()
                .unwrap()
                .contains("gh-lab")
        );
    }

    #[tokio::test]
    async fn anonymous_never_writes_a_governed_domain() {
        let dir = tempfile::tempdir().unwrap();
        let access = governed_access(&dir, base_governor().public("gh-open")).await;
        assert_eq!(
            access
                .write_right(&Scope::Anonymous, "gh-open")
                .await
                .unwrap(),
            DomainRight::Read
        );
        assert_eq!(
            access
                .write_rights(&Scope::Anonymous)
                .await
                .unwrap()
                .write_right("gh-open"),
            DomainRight::Read
        );
        assert_eq!(
            decide(
                &Principal::Anonymous,
                None,
                None,
                Some(Governed {
                    knowledge: GithubKnowledge::Level(MemberLevel::Manager),
                    public: true,
                })
            ),
            DomainRight::Read,
            "a grant never reaches the anonymous caller"
        );
    }

    #[tokio::test]
    async fn a_signed_in_account_never_gets_less_than_anonymous() {
        let dir = tempfile::tempdir().unwrap();
        let governor = base_governor().public("gh-open").public("gh-lab").grant(
            "mem",
            "gh-open",
            MemberLevel::None,
        );
        let access = governed_access(&dir, governor).await;
        for domain in ["gh-open", "gh-lab", "gh-vault"] {
            let anonymous = access.right(&Scope::Anonymous, domain).await.unwrap();
            // Instance viewer and editor, unlinked or GitHub none; the owner.
            for who in ["ann", "mem", "out", "keeper"] {
                let right = access.right(&user(who, false), domain).await.unwrap();
                assert!(right >= anonymous, "{who} on {domain}: {right:?}");
            }
        }
    }

    #[tokio::test]
    async fn the_instance_role_still_caps_writes_on_a_domain_without_github() {
        let dir = tempfile::tempdir().unwrap();
        let auth = store(&dir).await;
        github_cast(&auth).await;
        auth.upsert_domain_member("lab", "ann", MemberLevel::Editor, None, "keeper")
            .await
            .unwrap();
        // A grant on a domain that is not governed counts for nothing.
        let governor = base_governor()
            .grant("ann", "lab", MemberLevel::Manager)
            .grant("ann", "open", MemberLevel::Manager);
        let access = DomainAccess::new(auth).with_governor(Arc::new(governor));
        let ann = user("ann", false);
        assert_eq!(access.right(&ann, "lab").await.unwrap(), DomainRight::Write);
        assert_eq!(
            access.write_right(&ann, "lab").await.unwrap(),
            DomainRight::Read
        );
        assert_eq!(
            access.write_right(&ann, "open").await.unwrap(),
            DomainRight::Read
        );
        let rights = access.write_rights(&ann).await.unwrap();
        assert_eq!(rights.write_right("lab"), DomainRight::Read);
        assert_eq!(rights.write_right("open"), DomainRight::Read);
    }

    #[tokio::test]
    async fn readers_of_a_private_github_domain_are_its_github_readers() {
        let dir = tempfile::tempdir().unwrap();
        let auth = store(&dir).await;
        github_cast(&auth).await;
        auth.add_user("gone", "gone", None, Role::Viewer, "pw12345678")
            .await
            .unwrap();
        auth.set_disabled("gone", true).await.unwrap();
        // An old member row: not a reader any more.
        auth.upsert_domain_member("gh-lab", "out", MemberLevel::Viewer, None, "keeper")
            .await
            .unwrap();
        let governor = base_governor()
            .grant("mem", "gh-lab", MemberLevel::Viewer)
            .grant("ann", "gh-lab", MemberLevel::Editor)
            .grant("nora", "gh-lab", MemberLevel::Manager)
            .grant("gone", "gh-lab", MemberLevel::Viewer)
            .grant("ghost", "gh-lab", MemberLevel::Viewer);
        let access = DomainAccess::new(auth).with_governor(Arc::new(governor));
        assert_eq!(
            access.readers_of("gh-lab").await.unwrap(),
            Some(names(&["boss", "mem", "ann"]))
        );
        // A plain private domain is unchanged.
        assert_eq!(
            access.readers_of("lab").await.unwrap(),
            Some(names(&["keeper", "boss"]))
        );
    }

    #[tokio::test]
    async fn without_a_governor_every_answer_is_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let auth = store(&dir).await;
        github_cast(&auth).await;
        auth.upsert_domain_member("gh-lab", "mem", MemberLevel::Editor, None, "keeper")
            .await
            .unwrap();
        auth.upsert_domain_member(
            "lab",
            "ann",
            MemberLevel::None,
            Some(ReflectionLevel::Reader),
            "keeper",
        )
        .await
        .unwrap();
        auth.set_write_list("out", &["gh-*".to_string()])
            .await
            .unwrap();
        let plain = DomainAccess::new(auth.clone());
        // A governor that governs nothing must not change a single answer,
        // and neither may grants on domains it does not govern.
        let idle = DomainAccess::new(auth).with_governor(Arc::new(
            FakeGovernor::default()
                .grant("ann", "gh-lab", MemberLevel::Manager)
                .grant("nora", "open", MemberLevel::Manager)
                .public("lab"),
        ));
        let scopes = [
            Scope::Unrestricted,
            Scope::Anonymous,
            user("keeper", false),
            user("boss", false),
            user("mem", false),
            user("out", false),
            user("ann", false),
            user("nora", false),
            user("mem", true),
            user("nobody", false),
        ];
        let domains = ["gh-lab", "gh-vault", "lab", "open", "gh-open"];
        for scope in &scopes {
            assert_eq!(
                plain.visibility(scope).await.unwrap(),
                idle.visibility(scope).await.unwrap(),
                "{scope:?}"
            );
            let (a, b) = (
                plain.write_rights(scope).await.unwrap(),
                idle.write_rights(scope).await.unwrap(),
            );
            for domain in domains {
                let at = format!("{scope:?} on {domain}");
                assert_eq!(
                    plain.right(scope, domain).await.unwrap(),
                    idle.right(scope, domain).await.unwrap(),
                    "{at}"
                );
                assert_eq!(
                    plain.write_right(scope, domain).await.unwrap(),
                    idle.write_right(scope, domain).await.unwrap(),
                    "{at}"
                );
                assert_eq!(
                    plain.reflection_right(scope, domain).await.unwrap(),
                    idle.reflection_right(scope, domain).await.unwrap(),
                    "{at}"
                );
                assert_eq!(a.write_right(domain), b.write_right(domain), "{at}");
                for needed in [DomainRight::Write, DomainRight::Manage, DomainRight::Own] {
                    assert_eq!(
                        plain
                            .write_list_refuses(scope, domain, needed)
                            .await
                            .unwrap(),
                        idle.write_list_refuses(scope, domain, needed)
                            .await
                            .unwrap(),
                        "{at}, {needed:?}"
                    );
                }
                for need in [
                    ReflectionRight::Read,
                    ReflectionRight::Edit,
                    ReflectionRight::Curate,
                ] {
                    assert_eq!(
                        plain
                            .reflection_list_refuses(scope, domain, need)
                            .await
                            .unwrap(),
                        idle.reflection_list_refuses(scope, domain, need)
                            .await
                            .unwrap(),
                        "{at}, {need:?}"
                    );
                }
                assert_eq!(
                    a.write_list_excludes(domain),
                    b.write_list_excludes(domain),
                    "{at}"
                );
                assert_eq!(
                    plain.write_list_excludes(scope, domain).await.unwrap(),
                    idle.write_list_excludes(scope, domain).await.unwrap(),
                    "{at}"
                );
            }
        }
        for page in domains {
            assert_eq!(
                plain.readers_of(page).await.unwrap(),
                idle.readers_of(page).await.unwrap(),
                "{page}"
            );
            assert_eq!(
                plain.page_readers(page).await.unwrap(),
                idle.page_readers(page).await.unwrap(),
                "{page}"
            );
            for source in domains {
                assert_eq!(
                    plain.readers_cover(page, source).await.unwrap(),
                    idle.readers_cover(page, source).await.unwrap(),
                    "{page} <- {source}"
                );
                for confirmed in [
                    PageReaders::Everyone,
                    PageReaders::Accounts(Default::default()),
                    PageReaders::Accounts(
                        ["ann", "mem", "out"]
                            .into_iter()
                            .map(str::to_string)
                            .collect(),
                    ),
                ] {
                    assert_eq!(
                        plain
                            .readers_cover_for(page, source, &confirmed)
                            .await
                            .unwrap(),
                        idle.readers_cover_for(page, source, &confirmed)
                            .await
                            .unwrap(),
                        "{page} <- {source}, {confirmed:?}"
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn a_github_writer_curates_the_reflection_of_a_governed_domain() {
        let dir = tempfile::tempdir().unwrap();
        let governor = base_governor()
            .grant("ann", "gh-lab", MemberLevel::Editor)
            .grant("ann", "gh-open", MemberLevel::Manager);
        let access = governed_access(&dir, governor).await;
        let ann = user("ann", false);
        for domain in ["gh-lab", "gh-open"] {
            assert_eq!(
                access.reflection_right(&ann, domain).await.unwrap(),
                ReflectionRight::Curate,
                "{domain}"
            );
        }
    }

    #[tokio::test]
    async fn a_github_reader_comments_on_the_reflection() {
        let dir = tempfile::tempdir().unwrap();
        let governor = base_governor().grant("ann", "gh-lab", MemberLevel::Viewer);
        let access = governed_access(&dir, governor).await;
        let ann = user("ann", false);
        assert_eq!(
            access.reflection_right(&ann, "gh-lab").await.unwrap(),
            ReflectionRight::Read
        );
        assert!(ReflectionCaller::person(ann.clone(), "ann").may_comment());
        // Unlinked on a shared domain: the shared default, read and comment;
        // on a private one: nothing.
        assert_eq!(
            access.reflection_right(&ann, "gh-open").await.unwrap(),
            ReflectionRight::Read
        );
        assert_eq!(
            access.reflection_right(&ann, "gh-vault").await.unwrap(),
            ReflectionRight::None
        );
        // An unlinked instance editor reads and comments on a shared one,
        // and never curates it.
        assert_eq!(
            access
                .reflection_right(&user("out", false), "gh-open")
                .await
                .unwrap(),
            ReflectionRight::Read
        );
    }

    #[tokio::test]
    async fn readers_cover_counts_github_readers_of_a_private_governed_source() {
        let dir = tempfile::tempdir().unwrap();
        let auth = store(&dir).await;
        github_cast(&auth).await;
        auth.upsert_domain_member("lab", "mem", MemberLevel::Viewer, None, "keeper")
            .await
            .unwrap();
        let governor = base_governor()
            .grant("mem", "gh-lab", MemberLevel::Viewer)
            .grant("keeper", "gh-lab", MemberLevel::Viewer)
            .grant("mem", "gh-vault", MemberLevel::Viewer)
            .grant("keeper", "gh-vault", MemberLevel::Editor)
            .grant("ann", "gh-vault", MemberLevel::Viewer);
        let access = DomainAccess::new(auth.clone()).with_governor(Arc::new(governor));
        // gh-vault's readers include gh-lab's: covered one way only.
        assert!(access.readers_cover("gh-lab", "gh-vault").await.unwrap());
        assert!(!access.readers_cover("gh-vault", "gh-lab").await.unwrap());
        // lab (keeper, mem) cites gh-lab (keeper, mem by GitHub): covered.
        assert!(access.readers_cover("lab", "gh-lab").await.unwrap());
        // An old member row on gh-lab does not make ann a reader of it.
        auth.upsert_domain_member("gh-lab", "ann", MemberLevel::Viewer, None, "keeper")
            .await
            .unwrap();
        auth.upsert_domain_member("lab", "ann", MemberLevel::Viewer, None, "keeper")
            .await
            .unwrap();
        assert!(!access.readers_cover("lab", "gh-lab").await.unwrap());
        // A shared page reached by anonymous callers never covers a shared
        // GitHub domain anonymous may not read.
        assert!(!access.readers_cover("open", "gh-open").await.unwrap());
        assert!(access.readers_cover("gh-open", "open").await.unwrap());
        let public =
            DomainAccess::new(auth).with_governor(Arc::new(base_governor().public("gh-open")));
        assert!(public.readers_cover("open", "gh-open").await.unwrap());
    }

    #[tokio::test]
    async fn page_readers_of_a_private_governed_domain_are_its_linked_github_readers() {
        let dir = tempfile::tempdir().unwrap();
        let auth = store(&dir).await;
        github_cast(&auth).await;
        auth.upsert_domain_member("gh-lab", "out", MemberLevel::Viewer, None, "keeper")
            .await
            .unwrap();
        let governor = base_governor()
            .grant("mem", "gh-lab", MemberLevel::Viewer)
            .grant("ann", "gh-lab", MemberLevel::Editor)
            .grant("nora", "gh-lab", MemberLevel::Viewer);
        let access = DomainAccess::new(auth).with_governor(Arc::new(governor));
        assert_eq!(
            access.page_readers("gh-lab").await.unwrap(),
            PageReaders::Accounts(
                ["boss", "mem", "ann"]
                    .into_iter()
                    .map(str::to_string)
                    .collect()
            )
        );
        // A shared one of a private repository names its readers too (see
        // `a_confirmation_on_a_governed_shared_page_is_asked_again_...`):
        // every account at instance viewer or above, never nora.
        let PageReaders::Accounts(shared) = access.page_readers("gh-open").await.unwrap() else {
            panic!("no anonymous caller reads a private repository's page");
        };
        assert!(
            shared.contains("out") && !shared.contains("nora"),
            "{shared:?}"
        );
    }

    #[tokio::test]
    async fn an_unknown_reader_listing_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let auth = store(&dir).await;
        github_cast(&auth).await;
        let governor = base_governor()
            .grant("mem", "gh-lab", MemberLevel::Viewer)
            .grant("mem", "gh-vault", MemberLevel::Viewer)
            .readers("gh-lab", None);
        let access = DomainAccess::new(auth).with_governor(Arc::new(governor));
        assert_eq!(
            access.readers_of("gh-lab").await.unwrap(),
            Some(names(&["boss"]))
        );
        assert_eq!(
            access.page_readers("gh-lab").await.unwrap(),
            PageReaders::Accounts(std::iter::once("boss".to_string()).collect())
        );
        // Unknown on either side: not covered, whichever way the gap would
        // have pointed.
        assert!(!access.readers_cover("gh-lab", "gh-vault").await.unwrap());
        assert!(!access.readers_cover("gh-vault", "gh-lab").await.unwrap());
        assert!(!access.readers_cover("gh-lab", "open").await.unwrap());
        assert!(!access.readers_cover("lab", "gh-lab").await.unwrap());
        assert!(
            !access
                .readers_cover_for(
                    "gh-lab",
                    "gh-vault",
                    &PageReaders::Accounts(std::iter::once("boss".to_string()).collect())
                )
                .await
                .unwrap()
        );
        // A page still covers its own domain.
        assert!(access.readers_cover("gh-lab", "gh-lab").await.unwrap());
    }

    /// The bulk screen and the per-domain answer agree on every governed
    /// domain for every kind of caller, the write snapshot agrees with the
    /// per-domain write answer, and a governed listing is never wider than
    /// what `right` lets one read.
    #[tokio::test]
    async fn governed_visibility_answers_exactly_what_right_answers() {
        let dir = tempfile::tempdir().unwrap();
        let auth = store(&dir).await;
        github_cast(&auth).await;
        auth.upsert_domain_member("gh-vault", "out", MemberLevel::Manager, None, "keeper")
            .await
            .unwrap();
        auth.set_write_list("mem", &["gh-o*".to_string()])
            .await
            .unwrap();
        let governor = base_governor()
            .public("gh-open")
            .public("gh-vault")
            .grant("mem", "gh-lab", MemberLevel::Editor)
            .grant("mem", "gh-open", MemberLevel::Manager)
            .grant("ann", "gh-vault", MemberLevel::Viewer)
            .grant("nora", "gh-lab", MemberLevel::Manager)
            .grant("keeper", "gh-lab", MemberLevel::None);
        let access = DomainAccess::new(auth).with_governor(Arc::new(governor));
        let scopes = [
            Scope::Anonymous,
            user("keeper", false),
            user("boss", false),
            user("mem", false),
            user("out", false),
            user("ann", false),
            user("nora", false),
            user("nobody", false),
        ];
        for scope in &scopes {
            let screen = access.visibility(scope).await.unwrap().screen();
            let rights = access.write_rights(scope).await.unwrap();
            for domain in ["gh-open", "gh-lab", "gh-vault", "lab", "open"] {
                let at = format!("{scope:?} on {domain}");
                let right = access.right(scope, domain).await.unwrap();
                assert_eq!(screen.admits(domain), right >= DomainRight::Read, "{at}");
                assert_eq!(
                    rights.write_right(domain),
                    access.write_right(scope, domain).await.unwrap(),
                    "{at}"
                );
                assert_eq!(
                    rights.write_list_excludes(domain),
                    access.write_list_excludes(scope, domain).await.unwrap(),
                    "{at}"
                );
            }
        }
    }

    /// A confirmation on a shared GitHub page of a private repository names
    /// the accounts that read it then, and covers only while anonymous
    /// callers still cannot read the page: once the repository is public,
    /// the person is asked again.
    #[tokio::test]
    async fn a_confirmation_on_a_governed_shared_page_is_asked_again_when_its_repository_goes_public()
     {
        let dir = tempfile::tempdir().unwrap();
        let auth = store(&dir).await;
        github_cast(&auth).await;
        let governor = base_governor().grant("mem", "gh-lab", MemberLevel::Viewer);
        let access = DomainAccess::new(auth.clone()).with_governor(Arc::new(governor));
        let confirmed = access.page_readers("gh-open").await.unwrap();
        assert_eq!(
            confirmed,
            PageReaders::Accounts(
                ["keeper", "boss", "mem", "out", "ann"]
                    .into_iter()
                    .map(str::to_string)
                    .collect()
            ),
            "every enabled account at viewer or above, never instance none"
        );
        assert!(!access.readers_cover("gh-open", "gh-lab").await.unwrap());
        assert!(
            access
                .readers_cover_for("gh-open", "gh-lab", &confirmed)
                .await
                .unwrap()
        );
        let public = DomainAccess::new(auth).with_governor(Arc::new(
            base_governor()
                .grant("mem", "gh-lab", MemberLevel::Viewer)
                .public("gh-open"),
        ));
        assert_eq!(
            public.page_readers("gh-open").await.unwrap(),
            PageReaders::Everyone
        );
        assert!(
            !public
                .readers_cover_for("gh-open", "gh-lab", &confirmed)
                .await
                .unwrap()
        );
    }

    /// A confirmation on a shared GitHub page of a private repository is
    /// held against the readers of the moment: an account made later at
    /// instance viewer reads the page and not the private source, so it is
    /// asked again; one made later at instance knowledge `none` reads
    /// nothing there and keeps the cover.
    #[tokio::test]
    async fn a_later_account_on_a_governed_shared_page_is_asked_for_only_when_it_reads_the_page() {
        let dir = tempfile::tempdir().unwrap();
        let auth = store(&dir).await;
        github_cast(&auth).await;
        let governor = base_governor().grant("mem", "gh-lab", MemberLevel::Viewer);
        let access = DomainAccess::new(auth.clone()).with_governor(Arc::new(governor));
        let confirmed = access.page_readers("gh-open").await.unwrap();
        assert!(
            access
                .readers_cover_for("gh-open", "gh-lab", &confirmed)
                .await
                .unwrap()
        );
        auth.add_user("nina", "nina", None, Role::None, "pw12345678")
            .await
            .unwrap();
        assert!(
            access
                .readers_cover_for("gh-open", "gh-lab", &confirmed)
                .await
                .unwrap(),
            "an instance-none account reads no governed page"
        );
        auth.add_user("vic", "vic", None, Role::Viewer, "pw12345678")
            .await
            .unwrap();
        assert!(
            !access
                .readers_cover_for("gh-open", "gh-lab", &confirmed)
                .await
                .unwrap(),
            "a later viewer reads the page and not the source"
        );
    }
}
