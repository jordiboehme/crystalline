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
//! The policy itself is one small synchronous function, [`decide`]. Both public
//! answers - [`DomainAccess::right`] for a single domain and
//! [`DomainAccess::hidden_domains`] for the whole set - are folds over it, so
//! the per-domain answer and the bulk filter cannot drift apart. They read the
//! same records differently (one domain's row against one user's whole
//! membership list) purely to keep the bulk path from being N queries, and that
//! is the only difference between them.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::Result;

use crate::rest::{AuthStore, DomainAcl, MemberLevel, Role};

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
fn decide(
    principal: &Principal,
    acl: Option<&DomainAcl>,
    level: Option<MemberLevel>,
) -> DomainRight {
    match principal {
        Principal::Unrestricted => DomainRight::Own,
        Principal::Anonymous => match acl {
            Some(_) => DomainRight::None,
            None => DomainRight::Read,
        },
        Principal::Account { name, role } => {
            if *role == Role::Admin {
                return DomainRight::Own;
            }
            let Some(acl) = acl else {
                return match role {
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
                None => DomainRight::None,
            }
        }
    }
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
}

impl DomainAccess {
    /// Wrap the accounts store. Cheap: this holds a handle and no state of its
    /// own, so every answer is read fresh and a membership change takes effect
    /// on the next request rather than at the next restart.
    pub fn new(auth: Arc<AuthStore>) -> DomainAccess {
        DomainAccess { auth }
    }

    /// What `scope` may do on `domain`.
    pub async fn right(&self, scope: &Scope, domain: &str) -> Result<DomainRight> {
        let principal = self.principal(scope).await?;
        if matches!(principal, Principal::Unrestricted) {
            return Ok(DomainRight::Own);
        }
        let acl = self.auth.domain_visibility(domain).await?;
        let level = match (&acl, &principal) {
            (Some(_), Principal::Account { name, .. }) => self.level_of(name, domain).await?,
            _ => None,
        };
        Ok(decide(&principal, acl.as_ref(), level))
    }

    /// The private domains `scope` may not read.
    ///
    /// `None` means no filtering at all (the machine owner). `Some(set)` is the
    /// set of domain names to *subtract*: every other domain, private or not,
    /// is visible. An empty set is therefore the normal answer on an
    /// installation with no private domains, and the answer an admin always
    /// gets.
    ///
    /// The name says which side of the question this returns, because it is
    /// the side callers need: filtering is a `contains` against a set that is
    /// almost always empty, rather than an intersection with a set of every
    /// domain on the instance. (An earlier draft of the plan called this
    /// `visible_domains`; there is deliberately no such method. This type
    /// holds only the accounts store, so it cannot enumerate the registered
    /// domains, and a method named for the complement of what it returns is
    /// how an inverted filter ships.)
    pub async fn hidden_domains(&self, scope: &Scope) -> Result<Option<HashSet<String>>> {
        let principal = self.principal(scope).await?;
        if matches!(principal, Principal::Unrestricted) {
            return Ok(None);
        }
        let acls = self.auth.private_domains().await?;
        if acls.is_empty() {
            return Ok(Some(HashSet::new()));
        }
        // One read of this caller's whole membership list instead of one read
        // per private domain. The decision below is still `decide`, given the
        // same three inputs `right` gives it.
        let levels: HashMap<String, MemberLevel> = match &principal {
            Principal::Account { name, .. } => {
                self.auth.memberships_of(name).await?.into_iter().collect()
            }
            _ => HashMap::new(),
        };
        let mut hidden = HashSet::new();
        for acl in &acls {
            let level = levels.get(&acl.domain).copied();
            if decide(&principal, Some(acl), level) < DomainRight::Read {
                hidden.insert(acl.domain.clone());
            }
        }
        Ok(Some(hidden))
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
                })
            }
        }
    }

    /// One account's membership level on one domain, if it has one.
    ///
    /// Read out of the account's own membership list rather than through a
    /// per-domain lookup, so this path and [`DomainAccess::hidden_domains`]
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
            ("owner", Role::Editor),
            ("boss", Role::Admin),
            ("mem", Role::Viewer),
            ("out", Role::Editor),
        ] {
            auth.add_user(name, name, None, role, "pw12345678")
                .await
                .unwrap();
        }
    }

    fn user(account: &str, admin: bool) -> Scope {
        Scope::User {
            account: account.into(),
            admin,
        }
    }

    #[tokio::test]
    async fn rights_ladder_owner_admin_members_and_strangers() {
        let dir = tempfile::tempdir().unwrap();
        let auth = store(&dir).await;
        cast(&auth).await;
        auth.set_domain_visibility("lab", true, "owner")
            .await
            .unwrap();
        auth.upsert_domain_member("lab", "mem", MemberLevel::Editor, "owner")
            .await
            .unwrap();
        let access = DomainAccess::new(auth);
        assert_eq!(
            access.right(&user("owner", false), "lab").await.unwrap(),
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
        auth.set_domain_visibility("lab", true, "owner")
            .await
            .unwrap();
        auth.upsert_domain_member("lab", "mem", MemberLevel::Viewer, "owner")
            .await
            .unwrap();
        auth.upsert_domain_member("lab", "out", MemberLevel::Manager, "owner")
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
        auth.set_domain_visibility("lab", true, "owner")
            .await
            .unwrap();
        auth.upsert_domain_member("lab", "mem", MemberLevel::Editor, "owner")
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
    async fn hidden_domains_answers_exactly_what_right_answers() {
        let dir = tempfile::tempdir().unwrap();
        let auth = store(&dir).await;
        cast(&auth).await;
        for domain in ["lab", "vault", "attic"] {
            auth.set_domain_visibility(domain, true, "owner")
                .await
                .unwrap();
        }
        auth.upsert_domain_member("vault", "mem", MemberLevel::Viewer, "owner")
            .await
            .unwrap();
        auth.upsert_domain_member("attic", "out", MemberLevel::Manager, "owner")
            .await
            .unwrap();
        auth.transfer_domain("attic", "mem").await.unwrap();
        let access = DomainAccess::new(auth);
        let scopes = [
            Scope::Unrestricted,
            Scope::Anonymous,
            user("owner", false),
            user("boss", true),
            user("mem", false),
            user("out", false),
            user("ghost", false),
        ];
        for scope in &scopes {
            let hidden = access.hidden_domains(scope).await.unwrap();
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
        auth.set_domain_visibility("lab", true, "owner")
            .await
            .unwrap();
        auth.upsert_domain_member("lab", "mem", MemberLevel::Editor, "owner")
            .await
            .unwrap();
        auth.set_domain_visibility("lab", false, "owner")
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

    #[tokio::test]
    async fn case_variants_of_a_login_name_reach_the_same_membership() {
        let dir = tempfile::tempdir().unwrap();
        let auth = store(&dir).await;
        cast(&auth).await;
        auth.set_domain_visibility("lab", true, "Owner")
            .await
            .unwrap();
        auth.upsert_domain_member("lab", "MEM", MemberLevel::Editor, "owner")
            .await
            .unwrap();
        let access = DomainAccess::new(auth);
        assert_eq!(
            access.right(&user("OWNER", false), "lab").await.unwrap(),
            DomainRight::Own
        );
        assert_eq!(
            access.right(&user("Mem", false), "lab").await.unwrap(),
            DomainRight::Write
        );
    }
}
