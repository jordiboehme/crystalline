//! A domain named in a request path by any of its names.
//!
//! A domain answers to its local name, to the canonical name its MANIFEST
//! declares and to a machine-local alias. The routes under `/domains/{domain}`
//! and `/collab/{domain}` take the local name, like everything below them, so
//! this rewrites the path segment to the local name before the router matches
//! it. It has to run before routing: a path parameter is captured when the
//! route matches, so a rewrite after that changes nothing a handler reads.
//!
//! A spelling that names a domain the caller may not see is left exactly as
//! typed. The handler then refuses it the way it refuses a name nobody
//! registered, in the caller's own words, and the local name is never said.

use axum::extract::{Request, State};
use axum::http::{HeaderMap, Uri};
use axum::middleware::Next;
use axum::response::Response;
use percent_encoding::percent_decode_str;

use crate::RestState;

/// The path prefixes whose next segment is a domain name, relative to the
/// `/api/v1` mount.
const DOMAIN_PREFIXES: [&str; 2] = ["/domains/", "/collab/"];

/// Rewrite the domain segment of a domain path to the local name it means,
/// when that is a different name and a domain this caller may see. Every
/// other request passes through untouched.
pub(crate) async fn localize_domain_path(
    State(state): State<RestState>,
    mut req: Request,
    next: Next,
) -> Response {
    // The URI and the headers rather than the request: a body is not
    // `Sync`, so a borrow of the whole request held across an await would
    // make this future unsendable.
    let mut resolved = None;
    let rewritten = localized_uri(&state, req.uri(), req.headers(), &mut resolved).await;
    if let Some(uri) = rewritten {
        *req.uri_mut() = uri;
    }
    // The caller this step resolved, handed to the guard, which would
    // otherwise resolve it a second time.
    if let Some(resolved) = resolved {
        req.extensions_mut()
            .insert(crate::auth::ResolvedIdentity::new(resolved));
    }
    next.run(req).await
}

/// The request's URI with its domain segment spelled as the local name, or
/// `None` when there is nothing to rewrite. A caller it had to resolve to
/// decide lands in `resolved`, whatever the resolution said.
async fn localized_uri(
    state: &RestState,
    uri: &Uri,
    headers: &HeaderMap,
    resolved: &mut Option<Result<crate::auth::Identity, crate::error::ApiError>>,
) -> Option<Uri> {
    let path = uri.path();
    let (start, end) = domain_segment(path)?;
    let typed = percent_decode_str(&path[start..end]).decode_utf8().ok()?;
    // The in-memory check first, so a path that already names a local name
    // (nearly every request) costs no identity lookup.
    let local = state.engine.local_domain_name(&typed).await?;
    if local == typed {
        return None;
    }
    // Who is asking, resolved the way the guard resolves it. A request the
    // guard is going to refuse keeps its path; the guard answers it.
    let identity = resolved.insert(crate::auth::resolve(state, headers).await);
    let identity = identity.as_ref().ok()?;
    let hidden = state.engine.hidden_for(&identity.scope()).await.ok()?;
    if hidden.contains(&local) {
        return None;
    }
    let mut rewritten = String::with_capacity(uri.path().len() + local.len());
    rewritten.push_str(&path[..start]);
    rewritten.push_str(&crystalline_engine::web_url::encode_segment(&local));
    rewritten.push_str(&path[end..]);
    if let Some(query) = uri.query() {
        rewritten.push('?');
        rewritten.push_str(query);
    }
    let mut parts = uri.clone().into_parts();
    parts.path_and_query = Some(rewritten.parse().ok()?);
    Uri::from_parts(parts).ok()
}

/// The byte range of the domain segment in a mount-relative path, when the
/// path names a domain.
fn domain_segment(path: &str) -> Option<(usize, usize)> {
    let start = DOMAIN_PREFIXES
        .iter()
        .find(|prefix| path.starts_with(*prefix))?
        .len();
    let end = path[start..]
        .find('/')
        .map_or(path.len(), |slash| start + slash);
    (end > start).then_some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(path: &str) -> Option<&str> {
        domain_segment(path).map(|(start, end)| &path[start..end])
    }

    #[test]
    fn the_domain_segment_follows_the_domains_and_collab_prefixes() {
        assert_eq!(segment("/domains/eng/tree"), Some("eng"));
        assert_eq!(segment("/domains/eng"), Some("eng"));
        assert_eq!(segment("/domains/old%2Deng/engrams/a/b"), Some("old%2Deng"));
        assert_eq!(segment("/collab/eng/ops/runbook"), Some("eng"));
        assert_eq!(segment("/domains"), None);
        assert_eq!(segment("/domains/"), None);
        assert_eq!(segment("/search"), None);
        assert_eq!(segment("/draft-links/accept"), None);
    }
}
