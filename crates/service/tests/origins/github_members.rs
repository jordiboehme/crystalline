//! The Members card's REST answers on a domain GitHub governs, end to end
//! through the HTTP router: who GitHub lists, the old Crystalline grants
//! shown as overridden, the membership routes refusing with a 409, the "who
//! may read" switch for a GitHub maintainer or admin, the admin banner's
//! route, and the Own-only steps that stay with instance admins.
//!
//! They live here rather than beside `domains/domain_access.rs` because the
//! forge that answers the collaborator listing is this binary's
//! (`origins::github_app`).

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use crystalline_service::Engine;
use crystalline_service::daemon::http_router;
use crystalline_service::rest::{AuthStore, MemberLevel, Role};
use serde_json::{Value, json};

use super::github_app::{Forge, app_env, connect, engine, repo_with_manifest, spawn_forge};

const MAINT_ID: u64 = 2001;
const WRI_ID: u64 = 2002;
const NOBODY_ID: u64 = 3003;

struct Fixture {
    addr: std::net::SocketAddr,
    forge: Arc<Forge>,
    eng: Arc<Engine>,
    auth: Arc<AuthStore>,
}

/// An instance with an App, `acme/kb` connected as `kb` (shared), and the
/// accounts `root` (instance admin), `maint` (GitHub maintain), `wri`
/// (GitHub write) and `ada` (not linked). GitHub also lists `nobody-gh`,
/// who has no account here. Served over HTTP.
async fn serve(tmp: &Path, extra_env: &[(&str, &str)]) -> Fixture {
    let (forge, url) = spawn_forge(
        &[("acme/kb", repo_with_manifest(""))],
        &[("acme/kb", 1)],
        &[(1, "ghs_test")],
    )
    .await;
    let mut env = app_env();
    env.extend_from_slice(extra_env);
    let eng = Arc::new(engine(tmp, &url, &env).await);
    if extra_env.is_empty() {
        connect(&eng, "acme/kb", "kb", &tmp.join("kb")).await;
    }
    let host = eng.github_app().unwrap().credentials().host.clone();
    let auth = Arc::new(AuthStore::open(&tmp.join("auth.db")).await.unwrap());
    for (name, role) in [
        ("root", Role::Admin),
        ("maint", Role::Editor),
        ("wri", Role::Editor),
        ("ada", Role::Editor),
    ] {
        auth.add_user(name, name, None, role, "pw12345678")
            .await
            .unwrap();
    }
    auth.link_github("maint", &host, MAINT_ID, "maint-gh")
        .await
        .unwrap();
    auth.link_github("wri", &host, WRI_ID, "wri-gh")
        .await
        .unwrap();
    forge.collaborators.lock().unwrap().insert(
        "acme/kb".to_string(),
        vec![
            json!({"id": MAINT_ID, "login": "maint-gh", "role_name": "maintain"}),
            json!({"id": WRI_ID, "login": "wri-gh", "role_name": "write"}),
            json!({"id": NOBODY_ID, "login": "nobody-gh", "role_name": "read"}),
        ],
    );
    let router = http_router(
        eng.clone(),
        Arc::new(AtomicUsize::new(0)),
        &[],
        auth.clone(),
        None,
    )
    .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::from_std(listener).unwrap();
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    Fixture {
        addr,
        forge,
        eng,
        auth,
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

/// Log in, returning (session cookie value, csrf token).
async fn login(addr: std::net::SocketAddr, name: &str) -> (String, String) {
    let resp = client()
        .post(format!("http://{addr}/api/v1/auth/login"))
        .json(&json!({"name": name, "password": "pw12345678"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "login as {name} must succeed");
    let cookie = resp
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with("fluid_session="))
        .and_then(|v| v.split(';').next())
        .and_then(|v| v.strip_prefix("fluid_session="))
        .unwrap()
        .to_string();
    let body: Value = resp.json().await.unwrap();
    (cookie, body["csrf"].as_str().unwrap().to_string())
}

/// One request as `session`; answers the status and the body as JSON
/// (`null` for an empty one).
async fn call(
    fx: &Fixture,
    session: &(String, String),
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let mut request = client()
        .request(method, format!("http://{}{path}", fx.addr))
        .header("cookie", format!("fluid_session={}", session.0))
        .header("x-csrf-token", &session.1);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let resp = request.send().await.unwrap();
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap();
    let value = if text.is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&text).unwrap_or(Value::String(text))
    };
    (status, value)
}

/// Makes `kb` private the way an older instance did (an acl row owned by
/// `ada`) and leaves `wri` a Crystalline member there: the grants GitHub
/// now overrides.
async fn old_grants(fx: &Fixture) {
    fx.auth
        .set_domain_visibility("kb", true, "ada")
        .await
        .unwrap();
    fx.auth
        .upsert_domain_member("kb", "wri", MemberLevel::Viewer, None, "ada")
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn members_of_a_governed_domain_list_github_people_and_overridden_grants() {
    let tmp = tempfile::tempdir().unwrap();
    let fx = serve(tmp.path(), &[]).await;
    old_grants(&fx).await;
    let maint = login(fx.addr, "maint").await;
    let (status, body) = call(
        &fx,
        &maint,
        reqwest::Method::GET,
        "/api/v1/domains/kb/members",
        None,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["governance"], "github", "{body}");
    assert_eq!(body["visibility"], "private", "{body}");
    // The old acl owner holds nothing special here: GitHub decides.
    assert_eq!(body["owner"], Value::Null, "{body}");
    assert_eq!(body["members"], json!([]), "{body}");
    let overridden = body["overridden"].as_array().unwrap();
    assert_eq!(overridden.len(), 1, "{body}");
    assert_eq!(overridden[0]["principal"], "wri");
    let people = body["github_people"].as_array().unwrap();
    let logins: Vec<&str> = people
        .iter()
        .map(|p| p["login"].as_str().unwrap())
        .collect();
    assert_eq!(logins, ["maint-gh", "nobody-gh", "wri-gh"], "{body}");
    assert_eq!(people[0]["github_role"], "maintain");
    assert_eq!(people[0]["level"], "manager");
    assert_eq!(people[0]["account"], "maint");
    assert_eq!(people[1]["github_role"], "read");
    assert_eq!(people[1]["level"], "viewer");
    assert_eq!(people[1]["account"], Value::Null);
    assert_eq!(people[2]["level"], "editor");
    assert_eq!(people[2]["account"], "wri");
    // A manager sees the Reflection collaborators.
    assert_eq!(body["reflection_collaborators"], json!([]), "{body}");

    // A GitHub writer reads the same list but not the collaborators.
    let wri = login(fx.addr, "wri").await;
    let (status, body) = call(
        &fx,
        &wri,
        reqwest::Method::GET,
        "/api/v1/domains/kb/members",
        None,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["github_people"].as_array().unwrap().len(), 3);
    assert_eq!(body["reflection_collaborators"], Value::Null, "{body}");

    // A domain GitHub does not govern answers as it always did.
    fx.eng.domain_add_virtual("plain").await.unwrap();
    let root = login(fx.addr, "root").await;
    let (status, body) = call(
        &fx,
        &root,
        reqwest::Method::GET,
        "/api/v1/domains/plain/members",
        None,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["governance"], "crystalline", "{body}");
    assert_eq!(body["github_people"], Value::Null, "{body}");
    assert_eq!(body["overridden"], json!([]), "{body}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn setting_a_member_on_a_governed_domain_is_409() {
    let tmp = tempfile::tempdir().unwrap();
    let fx = serve(tmp.path(), &[]).await;
    old_grants(&fx).await;
    let root = login(fx.addr, "root").await;
    let maint = login(fx.addr, "maint").await;
    for session in [&root, &maint] {
        let (status, body) = call(
            &fx,
            session,
            reqwest::Method::PUT,
            "/api/v1/domains/kb/members/ada",
            Some(json!({"level": "editor"})),
        )
        .await;
        assert_eq!(status, 409, "{body}");
        assert!(
            body["detail"]
                .as_str()
                .unwrap()
                .contains("access to this domain follows GitHub; change it on GitHub"),
            "{body}"
        );
        let (status, body) = call(
            &fx,
            session,
            reqwest::Method::DELETE,
            "/api/v1/domains/kb/members/wri",
            None,
        )
        .await;
        assert_eq!(status, 409, "{body}");
    }
    // Leaving an overridden row is the same answer: the row is kept for the
    // day the App is removed.
    let wri = login(fx.addr, "wri").await;
    let (status, body) = call(
        &fx,
        &wri,
        reqwest::Method::DELETE,
        "/api/v1/domains/kb/members/wri",
        None,
    )
    .await;
    assert_eq!(status, 409, "{body}");
    // No owner to hand on either.
    let (status, body) = call(
        &fx,
        &root,
        reqwest::Method::PUT,
        "/api/v1/domains/kb/owner",
        Some(json!({"owner": "maint"})),
    )
    .await;
    assert_eq!(status, 409, "{body}");
    let members = fx.auth.domain_members("kb").await.unwrap();
    assert_eq!(members.len(), 1, "no row was written or removed");
    assert_eq!(members[0].principal, "wri");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_github_maintainer_switches_visibility() {
    let tmp = tempfile::tempdir().unwrap();
    let fx = serve(tmp.path(), &[]).await;
    old_grants(&fx).await;
    let maint = login(fx.addr, "maint").await;
    // Shared: the acl row goes, the overridden membership rows stay.
    let (status, body) = call(
        &fx,
        &maint,
        reqwest::Method::PUT,
        "/api/v1/domains/kb/visibility",
        Some(json!({"private": false})),
    )
    .await;
    assert_eq!(status, 204, "{body}");
    assert!(fx.auth.domain_visibility("kb").await.unwrap().is_none());
    assert_eq!(
        fx.auth.domain_members("kb").await.unwrap().len(),
        1,
        "making a governed domain shared keeps its membership rows"
    );
    // Private again, recorded by the maintainer.
    let (status, body) = call(
        &fx,
        &maint,
        reqwest::Method::PUT,
        "/api/v1/domains/kb/visibility",
        Some(json!({"private": true})),
    )
    .await;
    assert_eq!(status, 204, "{body}");
    assert!(fx.auth.domain_visibility("kb").await.unwrap().is_some());
    assert_eq!(fx.auth.domain_members("kb").await.unwrap().len(), 1);
    // The maintainer still reads it: GitHub decides who reads it.
    let (status, body) = call(
        &fx,
        &maint,
        reqwest::Method::GET,
        "/api/v1/domains/kb/members",
        None,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["visibility"], "private");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_github_writer_cannot_switch_visibility() {
    let tmp = tempfile::tempdir().unwrap();
    let fx = serve(tmp.path(), &[]).await;
    let wri = login(fx.addr, "wri").await;
    for private in [true, false] {
        let (status, body) = call(
            &fx,
            &wri,
            reqwest::Method::PUT,
            "/api/v1/domains/kb/visibility",
            Some(json!({"private": private})),
        )
        .await;
        assert_eq!(status, 403, "private={private}: {body}");
    }
    assert!(fx.auth.domain_visibility("kb").await.unwrap().is_none());
    // A stranger to a private governed domain learns nothing: privatizing
    // answers what an unregistered name answers to a non-admin.
    fx.auth.set_governed_private("kb", "owner").await.unwrap();
    let ada = login(fx.addr, "ada").await;
    let (hidden, _) = call(
        &fx,
        &ada,
        reqwest::Method::PUT,
        "/api/v1/domains/kb/visibility",
        Some(json!({"private": true})),
    )
    .await;
    let (unregistered, _) = call(
        &fx,
        &ada,
        reqwest::Method::PUT,
        "/api/v1/domains/nowhere/visibility",
        Some(json!({"private": true})),
    )
    .await;
    assert_eq!(hidden, unregistered);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_env_defined_domain_keeps_its_readers_in_the_environment() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("kb");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("MANIFEST.md"), super::github_app::manifest("")).unwrap();
    let root_str = root.to_str().unwrap().to_string();
    let fx = serve(
        tmp.path(),
        &[
            ("CRYSTALLINE_DOMAIN_KB", &root_str),
            ("CRYSTALLINE_DOMAIN_KB_ORIGIN", "acme/kb"),
            ("CRYSTALLINE_DOMAIN_KB_READERS", "shared"),
        ],
    )
    .await;
    fx.eng.sync(None).await.unwrap();
    let sentence = "Set by CRYSTALLINE_DOMAIN_KB_READERS; change it there.";
    let maint = login(fx.addr, "maint").await;
    let (status, body) = call(
        &fx,
        &maint,
        reqwest::Method::PUT,
        "/api/v1/domains/kb/visibility",
        Some(json!({"private": true})),
    )
    .await;
    assert_eq!(status, 409, "{body}");
    assert!(
        body["detail"].as_str().unwrap().contains(sentence),
        "{body}"
    );
    assert!(fx.auth.domain_visibility("kb").await.unwrap().is_none());
    let (status, body) = call(
        &fx,
        &maint,
        reqwest::Method::GET,
        "/api/v1/domains/kb/members",
        None,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["visibility_fixed_by"], sentence, "{body}");
}

/// A pin: a GitHub admin is `Manage` on a governed domain, never `Own`, so
/// the steps only an owner takes (ending the domain, which disconnects its
/// origin, and switching how its writes reach the origin) stay with
/// instance admins.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_github_admin_cannot_change_the_origin() {
    let tmp = tempfile::tempdir().unwrap();
    let fx = serve(tmp.path(), &[]).await;
    fx.forge.collaborators.lock().unwrap().insert(
        "acme/kb".to_string(),
        vec![json!({"id": MAINT_ID, "login": "maint-gh", "role_name": "admin"})],
    );
    // Old grants named this account the owner; GitHub decides now.
    fx.auth
        .set_domain_visibility("kb", true, "maint")
        .await
        .unwrap();
    let maint = login(fx.addr, "maint").await;
    let (status, body) = call(
        &fx,
        &maint,
        reqwest::Method::PUT,
        "/api/v1/domains/kb/review",
        Some(json!({"mode": "overlay"})),
    )
    .await;
    assert_eq!(status, 403, "{body}");
    let (status, body) = call(
        &fx,
        &maint,
        reqwest::Method::DELETE,
        "/api/v1/domains/kb",
        None,
    )
    .await;
    assert_eq!(status, 403, "{body}");
    assert!(fx.eng.github_governs("kb"), "the domain is still connected");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_github_access_route_names_closed_domains_to_admins_only() {
    let tmp = tempfile::tempdir().unwrap();
    let fx = serve(tmp.path(), &[]).await;
    let root = login(fx.addr, "root").await;
    let (status, body) = call(
        &fx,
        &root,
        reqwest::Method::GET,
        "/api/v1/github/access",
        None,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["stale_since"], Value::Null, "{body}");
    assert_eq!(body["closed"], json!([]), "{body}");
    let maint = login(fx.addr, "maint").await;
    let (status, _) = call(
        &fx,
        &maint,
        reqwest::Method::GET,
        "/api/v1/github/access",
        None,
    )
    .await;
    assert_eq!(status, 403);

    // The App is taken off the repository: the domain closes, and the
    // Members card tells an admin why.
    fx.forge.uninstall("acme/kb");
    fx.eng
        .invalidate_github_access(crystalline_service::engine::Invalidate::All);
    let mut closed = Value::Null;
    for _ in 0..100 {
        // A question about the domain asks GitHub again.
        let _ = call(
            &fx,
            &maint,
            reqwest::Method::GET,
            "/api/v1/domains/kb/members",
            None,
        )
        .await;
        let (_, body) = call(
            &fx,
            &root,
            reqwest::Method::GET,
            "/api/v1/github/access",
            None,
        )
        .await;
        if body["closed"].as_array().is_some_and(|c| !c.is_empty()) {
            closed = body["closed"].clone();
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(closed[0]["domain"], "kb", "{closed}");
    let reason = closed[0]["reason"].as_str().unwrap().to_string();
    assert!(reason.contains("acme/kb"), "{reason}");
    let (status, body) = call(
        &fx,
        &root,
        reqwest::Method::GET,
        "/api/v1/domains/kb/members",
        None,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["closed_reason"], reason, "{body}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_reflection_status_says_github_decides_and_offers_no_leave() {
    let tmp = tempfile::tempdir().unwrap();
    let fx = serve(tmp.path(), &[]).await;
    old_grants(&fx).await;
    let wri = login(fx.addr, "wri").await;
    let (status, body) = call(
        &fx,
        &wri,
        reqwest::Method::GET,
        "/api/v1/domains/kb/reflection",
        None,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["governance"], "github", "{body}");
    assert_eq!(body["private"], true, "{body}");
    // The overridden row cannot be left: the route answers 409 there.
    assert_eq!(body["member"], false, "{body}");
}
