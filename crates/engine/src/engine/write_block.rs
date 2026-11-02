//! Why a caller cannot write one domain, as one answer every surface asks.
//!
//! The write gates (MCP `refuse_unwritable`, REST `require_domain_write`) and
//! the domain rows that say `read_only: true` all need the same question
//! answered the same way: may this caller write here, and if not, which of
//! the reasons is it. The reasons are ordered. A locked domain refuses
//! everybody, so the lock is named first, even to an account whose write list
//! names the domain.

use std::collections::HashMap;

use super::{Engine, OVERLAY_NEEDS_IDENTITY, Result, read_only_sentence};
use crate::scope::{DomainRight, Scope, WriteRights};

/// Why a write into one domain is refused for one caller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WriteBlock {
    /// The domain's MANIFEST says `read_only: on`.
    DomainLocked,
    /// The caller's role or membership allows reading only.
    Role,
    /// The caller's write list does not name the domain, and nothing else
    /// would refuse the write.
    WriteList {
        /// The account whose list it is, as `crystalline users writes` takes it.
        account: String,
    },
    /// The open tier (anonymous, `auth.mcp` off) on a domain that reviews
    /// changes: a draft needs an identity to belong to.
    Reviewing,
}

impl WriteBlock {
    /// The sentence a surface refuses with, naming `domain`.
    pub fn sentence(&self, domain: &str) -> String {
        match self {
            WriteBlock::DomainLocked => read_only_sentence(domain),
            // Visible and not writable means Read: the resolver never answers
            // None for a domain the caller can see, so the level word is
            // always "viewer". Byte for byte the MCP gate's old sentence.
            WriteBlock::Role => format!(
                "your access to '{domain}' is viewer, and editor access is required to change it"
            ),
            WriteBlock::WriteList { account } => format!(
                "'{domain}' is not on the write list of '{account}'; an instance admin changes it \
                 with crystalline users writes"
            ),
            WriteBlock::Reviewing => OVERLAY_NEEDS_IDENTITY.to_string(),
        }
    }
}

impl Engine {
    /// Why `scope` cannot write `domain`, or `None` when it can.
    ///
    /// Asked of a domain the caller may see; the surface's `require_domain`
    /// runs first. The domain lock comes first, then
    /// [`Engine::write_access_block`]. A domain listing marks a row
    /// `read_only: true` exactly when this is `Some`; when it errors, a read
    /// leaves the mark out ([`Engine::read_only_mark`]).
    ///
    /// The machine owner ([`Scope::Unrestricted`]) and an engine with no
    /// resolver installed are answered `None` or [`WriteBlock::DomainLocked`],
    /// never an error: both resolve to Own on every domain.
    pub async fn write_block(&self, scope: &Scope, domain: &str) -> Result<Option<WriteBlock>> {
        if self.domain_is_read_only(domain).await {
            return Ok(Some(WriteBlock::DomainLocked));
        }
        self.write_access_block(scope, domain).await
    }

    /// Whether a read marks `domain` read-only for `scope`. A read never
    /// fails because this could not be decided: the row is served without
    /// the mark and a warning is logged. A write still runs its own check,
    /// which refuses on the same error.
    ///
    /// Public but hidden for one reason: the service crate's tests compare
    /// every batched listing mark with this per-row answer, and they live
    /// outside this crate.
    #[doc(hidden)]
    pub async fn read_only_mark(&self, scope: &Scope, domain: &str) -> bool {
        match self.write_block(scope, domain).await {
            Ok(block) => block.is_some(),
            Err(e) => {
                tracing::warn!(
                    domain,
                    "the read_only mark could not be decided, so the read is served without it: {e}"
                );
                false
            }
        }
    }

    /// Why `scope` may not write `domain` whatever the domain lock says:
    /// [`WriteBlock::Role`], [`WriteBlock::WriteList`] or
    /// [`WriteBlock::Reviewing`], never [`WriteBlock::DomainLocked`].
    ///
    /// The MCP write gate asks this rather than [`Engine::write_block`]. The
    /// lock's own sentence is left to the engine's write view and to the MCP
    /// unlock refusal, but a locked domain must not open the gate: the
    /// collaboration verbs (share, resolve, withdraw, discard) go through
    /// origin paths that do not read the lock, and REST's write gate refuses
    /// a viewer and an off-list account there too.
    ///
    /// In order: the open tier (anonymous with `auth.mcp` off), which writes
    /// through the MCP carve-out wherever `edit_engram` writes and so is
    /// refused only by a reviewing domain, except on a domain GitHub governs,
    /// where it is decided as `github.agent_identity` (or as anonymous
    /// without one, which never writes); then the capped write right, with
    /// the write list named only when it is the reason
    /// ([`crate::scope::DomainAccess::write_list_excludes`]). `None` for
    /// [`Scope::Unrestricted`] and with no resolver installed, never an error.
    pub async fn write_access_block(
        &self,
        scope: &Scope,
        domain: &str,
    ) -> Result<Option<WriteBlock>> {
        if self.open_tier_carve_out(scope, domain) {
            return Ok(self
                .reviews_changes(domain)
                .then_some(WriteBlock::Reviewing));
        }
        // The open tier on a domain GitHub governs is decided as
        // `github.agent_identity`, or as anonymous without one.
        let scope = &self.scope_on(scope, domain);
        if self.write_right(scope, domain).await? >= DomainRight::Write {
            return Ok(None);
        }
        Ok(Some(
            match self.write_list_refusal(scope, domain, None).await? {
                Some(account) => WriteBlock::WriteList { account },
                None => WriteBlock::Role,
            },
        ))
    }
}

/// One caller's write answers for a whole listing: the rights read once,
/// and each domain's lock read at most once per listing. Built by
/// [`Engine::write_marks`] for one request and asked through
/// [`Engine::mark`]; never kept, since a lock or a right can change between
/// requests.
pub(crate) struct WriteMarks {
    /// The caller's rights, read once. `None` in three cases that answer
    /// differently: no resolver is installed (the machine owner writes
    /// everywhere, so no row is marked for a right), the open tier (decided
    /// by [`Engine::reviews_changes`] instead), or the rights could not be
    /// read (then `per_row` is set).
    rights: Option<WriteRights>,
    /// Set when the rights could not be read (logged once for the listing):
    /// every row then asks [`Engine::read_only_mark`] on its own, so a
    /// failed batch read gives exactly the per-row answer, a Viewer's mark
    /// included.
    per_row: bool,
    scope: Scope,
    /// The MCP open tier: anonymous on an instance with `auth.mcp` off, as
    /// [`Engine::write_access_block`] decides it.
    open_tier: bool,
    /// Each domain's lock, once read (or seeded from a MANIFEST the listing
    /// already read for its routing bullets).
    locks: HashMap<String, bool>,
}

impl WriteMarks {
    /// Record `domain`'s lock as read from its MANIFEST elsewhere, so
    /// [`Engine::mark`] does not read the file again.
    pub(crate) fn seed_lock(&mut self, domain: &str, locked: bool) {
        self.locks.insert(domain.to_string(), locked);
    }
}

impl Engine {
    /// The rights half of [`Engine::write_block`] for every row of one
    /// listing, read from the accounts store once (B-3) rather than once or
    /// twice per row. Never fails: a store that cannot answer leaves each
    /// row to [`Engine::read_only_mark`], which serves it without the mark
    /// where the answer needs the broken read.
    pub(crate) async fn write_marks(&self, scope: &Scope) -> WriteMarks {
        let open_tier = matches!(scope, Scope::Anonymous) && !self.auth_mcp();
        let mut per_row = false;
        let rights = match self.domain_access.get() {
            Some(access) if !open_tier => match access.write_rights(scope).await {
                Ok(rights) => Some(rights),
                Err(e) => {
                    tracing::warn!(
                        "the read_only marks could not be read in one go, so each row is decided on its own: {e:#}"
                    );
                    per_row = true;
                    None
                }
            },
            _ => None,
        };
        WriteMarks {
            rights,
            per_row,
            scope: scope.clone(),
            open_tier,
            locks: HashMap::new(),
        }
    }

    /// Whether `domain` is marked read-only in this listing: the same answer
    /// as `write_block(..).is_some()`, or as [`Engine::read_only_mark`] when
    /// the rights could not be read.
    pub(crate) async fn mark(&self, marks: &mut WriteMarks, domain: &str) -> bool {
        if marks.per_row {
            return self.read_only_mark(&marks.scope, domain).await;
        }
        let locked = match marks.locks.get(domain) {
            Some(locked) => *locked,
            None => {
                let locked = self.domain_is_read_only(domain).await;
                marks.locks.insert(domain.to_string(), locked);
                locked
            }
        };
        if locked {
            return true;
        }
        if marks.open_tier {
            // A domain GitHub governs has no carve-out: decided on its own.
            if self.github_governs(domain) {
                return self.read_only_mark(&marks.scope, domain).await;
            }
            return self.reviews_changes(domain);
        }
        match &marks.rights {
            Some(rights) => rights.write_right(domain) < DomainRight::Write,
            None => false,
        }
    }
}
