//! The OpenAPI schemas of the account types the JSON API sends and takes.
//!
//! The types themselves live with the account store, which has no business
//! knowing about the API description; these copies carry the schema
//! derive in their place. Each one is a verbatim copy of its store type:
//! same name, same fields, same doc comments, same serde spelling, so the
//! published document is exactly what the store types used to produce.
//! `the_copies_match_the_store_types` below keeps them from drifting.

#![allow(dead_code)]

/// What a user may do. Ordered least to most privileged; the REST layer maps
/// each endpoint to the minimum role it accepts.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Role {
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
    /// before then.
    pub refresh_expires_at: String,
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
    /// Read only: this domain is visible and searchable, nothing more.
    Viewer,
    /// Everything a viewer may do, plus writing and editing its engrams.
    Editor,
    /// Everything an editor may do, plus managing this domain's membership.
    Manager,
}

/// One membership row: who was invited to a private domain, at what level, by
/// whom and when.
#[derive(Clone, Debug, PartialEq, serde::Serialize, utoipa::ToSchema)]
pub struct DomainMember {
    /// The member's login name, folded by [`normalize_account_name`].
    pub principal: String,
    /// What this member may do here.
    pub level: MemberLevel,
    /// Who added or last changed this row. An audit field, stored as given:
    /// it is usually a login name but may name a non-account actor, the same
    /// latitude the identity-link plan gives `linked_by`.
    pub added_by: String,
    /// RFC 3339, when this row was last written.
    pub added_at: String,
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
        use auth_store::{MemberLevel, Role};
        let s = String::new;
        assert_eq!(
            keys([Role::Viewer, Role::Editor, Role::Admin]),
            published::<super::Role>()
        );
        assert_eq!(
            keys([
                MemberLevel::Viewer,
                MemberLevel::Editor,
                MemberLevel::Manager
            ]),
            published::<super::MemberLevel>()
        );
        assert_eq!(
            keys([auth_store::User {
                name: s(),
                display: s(),
                email: None,
                role: Role::Viewer,
                disabled: false,
                last_seen: None,
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
            }]),
            published::<super::OauthGrantInfo>()
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
                added_by: s(),
                added_at: s(),
            }]),
            published::<super::DomainMember>()
        );
    }
}
