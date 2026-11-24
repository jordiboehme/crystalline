//! Redeeming invite links over the JSON API: the confirm page's answer, a
//! new local account, a signed-in account, the forwarded identity of a
//! proxy, every refusal, the re-checks, a read-only instance and the
//! throttle. Single sign-on redemption is in `auth::oidc`.

use crystalline_core::config::LoginConfig;
use crystalline_service::rest::{InviteLimits, MemberLevel, NewInvite, Role};
use reqwest::Method;
use serde_json::{Value, json};

use super::rest_invites::{Fixture, Options, call, client, create, login, serve};

/// `POST /api/v1{path}` with no session at all, plus `extra` headers: the
/// status, the `set-cookie` values and the body.
async fn post_bare(
    fx: &Fixture,
    path: &str,
    body: Value,
    extra: &[(&str, &str)],
) -> (u16, Vec<String>, Value) {
    let mut request = client()
        .post(format!("http://{}/api/v1{path}", fx.addr))
        .json(&body);
    for (name, value) in extra {
        request = request.header(*name, *value);
    }
    let resp = request.send().await.unwrap();
    let status = resp.status().as_u16();
    let cookies = resp
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(str::to_string)
        .collect();
    let text = resp.text().await.unwrap();
    let value = serde_json::from_str(&text).unwrap_or(Value::String(text));
    (status, cookies, value)
}

async fn confirm(fx: &Fixture, token: &str) -> (u16, Value) {
    let (status, _, body) = post_bare(fx, "/invites/confirm", json!({"token": token}), &[]).await;
    (status, body)
}

/// An admin's link to `team`, made over the API: the token.
async fn admin_link(fx: &Fixture, body: Value) -> String {
    let root = login(fx, "root").await;
    let (status, made) = create(fx, &root, body).await;
    assert_eq!(status, 201, "{made}");
    made["token"].as_str().unwrap().to_string()
}

async fn level_in_team(fx: &Fixture, account: &str) -> Option<MemberLevel> {
    fx.auth
        .domain_members("team")
        .await
        .unwrap()
        .into_iter()
        .find(|m| m.principal == account)
        .map(|m| m.level)
}

/// The confirm page's answer, then a new local account: made with the
/// link's role, a member at its level, signed in, and the link used. Used,
/// revoked, expired and unknown links all read the same on confirm.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_link_makes_a_local_account_and_signs_it_in() {
    let fx = serve(Options::default()).await;
    let token = admin_link(
        &fx,
        json!({"role": "editor", "domain": "team", "level": "editor"}),
    )
    .await;
    let (status, offer) = confirm(&fx, &token).await;
    assert_eq!(status, 200, "{offer}");
    assert_eq!(offer["inviter"], "root");
    assert_eq!(offer["inviter_display"], "root");
    assert_eq!(offer["domain"], "team");
    assert_eq!(offer["level"], "editor");
    assert_eq!(offer["role"], "editor");
    assert_eq!(offer["sign_in"], "local");
    assert_eq!(offer["local_allowed"], true);
    assert!(offer["expires_at"].as_str().is_some(), "{offer}");

    let (status, _, body) = post_bare(&fx, "/invites/redeem", json!({"token": token}), &[]).await;
    assert_eq!(
        status, 422,
        "a local account needs a login and a password: {body}"
    );
    let (status, cookies, done) = post_bare(
        &fx,
        "/invites/redeem",
        json!({"token": token, "login": "Nina", "password": "pw12345678", "display_name": "Nina N"}),
        &[],
    )
    .await;
    assert_eq!(status, 200, "{done}");
    assert_eq!(done["outcome"], "redeemed");
    assert_eq!(done["created"], true);
    assert_eq!(done["user"]["name"], "nina");
    assert_eq!(done["user"]["display"], "Nina N");
    assert_eq!(done["user"]["role"], "editor");
    assert_eq!(done["domain"], "team");
    assert_eq!(done["level"], "editor");
    assert_eq!(level_in_team(&fx, "nina").await, Some(MemberLevel::Editor));
    let session = cookies
        .iter()
        .find(|c| c.starts_with("fluid_session="))
        .expect("the new account is signed in")
        .split(';')
        .next()
        .unwrap()
        .trim_start_matches("fluid_session=")
        .to_string();
    let csrf = done["csrf"].as_str().unwrap().to_string();
    let (status, _, me) = call(&fx, &(session, csrf), Method::GET, "/api/v1/auth/me", None).await;
    assert_eq!(status, 200);
    assert_eq!(me["user"]["name"], "nina", "{me}");

    // Spent: confirm says the one sentence, redeem says why.
    let (status, again) = confirm(&fx, &token).await;
    assert_eq!(status, 404);
    let not_valid = again["detail"].clone();
    assert_eq!(not_valid, "This invite link is not valid any more.");
    let (status, _, body) = post_bare(
        &fx,
        "/invites/redeem",
        json!({"token": token, "login": "nora", "password": "pw12345678"}),
        &[],
    )
    .await;
    assert_eq!(status, 410, "{body}");
    assert!(fx.auth.user("nora").await.unwrap().is_none());

    // Revoked, expired and unknown read the same on confirm.
    let revoked = admin_link(&fx, json!({"domain": "team"})).await;
    let id = fx.auth.invites(None, None).await.unwrap()[0].id;
    assert!(fx.auth.revoke_invite(id, None).await.unwrap());
    for token in [revoked.as_str(), "cin_nothing"] {
        let (status, body) = confirm(&fx, token).await;
        assert_eq!(status, 404);
        assert_eq!(body["detail"], not_valid);
    }
    let (status, _, body) = post_bare(&fx, "/invites/redeem", json!({"token": revoked}), &[]).await;
    assert_eq!(status, 410, "{body}");
    let (status, _, body) =
        post_bare(&fx, "/invites/redeem", json!({"token": "cin_nothing"}), &[]).await;
    assert_eq!(status, 404, "{body}");
    assert_eq!(body["detail"], not_valid);
    // A query string carries no token: the body is the only way in.
    let (status, _, _) = post_bare(&fx, "/invites/confirm?token=x", json!({}), &[]).await;
    assert_eq!(status, 422);
}

/// A signed-in person redeems for their own account: the membership raised
/// and never lowered, the role kept, another login refused, the CSRF token
/// required, and a link without a domain refused without being spent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_signed_in_account_gets_the_membership_only() {
    let fx = serve(Options::default()).await;
    let token = admin_link(
        &fx,
        json!({"role": "editor", "domain": "team", "level": "editor"}),
    )
    .await;
    let vera = login(&fx, "vera").await;
    let (status, _, body) = call(
        &fx,
        &vera,
        Method::POST,
        "/api/v1/invites/redeem",
        Some(json!({"token": token, "login": "somebody"})),
    )
    .await;
    assert_eq!(status, 403, "{body}");
    // Without the CSRF token.
    let resp = client()
        .post(format!("http://{}/api/v1/invites/redeem", fx.addr))
        .header("cookie", format!("fluid_session={}", vera.0))
        .json(&json!({"token": token}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);
    let (status, _, done) = call(
        &fx,
        &vera,
        Method::POST,
        "/api/v1/invites/redeem",
        Some(json!({"token": token, "login": "Vera"})),
    )
    .await;
    assert_eq!(status, 200, "{done}");
    assert_eq!(done["created"], false);
    assert_eq!(
        done["user"]["role"], "viewer",
        "an existing account keeps its role"
    );
    assert_eq!(done["level"], "editor");
    assert!(done.get("csrf").is_none(), "{done}");
    assert_eq!(level_in_team(&fx, "vera").await, Some(MemberLevel::Editor));

    // A manager taking a viewer link stays manager.
    let token = admin_link(&fx, json!({"domain": "team", "level": "viewer"})).await;
    let mona = login(&fx, "mona").await;
    let (status, _, done) = call(
        &fx,
        &mona,
        Method::POST,
        "/api/v1/invites/redeem",
        Some(json!({"token": token})),
    )
    .await;
    assert_eq!(status, 200, "{done}");
    assert_eq!(done["level"], "manager");
    assert_eq!(level_in_team(&fx, "mona").await, Some(MemberLevel::Manager));

    // A link without a domain gives an existing account nothing.
    let token = admin_link(&fx, json!({"role": "viewer"})).await;
    let (status, _, body) = call(
        &fx,
        &vera,
        Method::POST,
        "/api/v1/invites/redeem",
        Some(json!({"token": token})),
    )
    .await;
    assert_eq!(status, 409, "{body}");
    let (status, _) = confirm(&fx, &token).await;
    assert_eq!(status, 200, "the refusal did not spend the link");
}

/// What a link gives is checked again when it is taken: the account cap, a
/// manager who no longer manages, and `auth.invites.managers` turned off.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_redemption_re_checks_the_cap_and_the_maker() {
    let fx = serve(Options {
        managers: true,
        max_users: Some(7),
        ..Options::default()
    })
    .await;
    let token = admin_link(&fx, json!({"domain": "team"})).await;
    fx.auth
        .add_user("late", "late", None, Role::Viewer, "pw12345678")
        .await
        .unwrap();
    let (status, _, body) = post_bare(
        &fx,
        "/invites/redeem",
        json!({"token": token, "login": "nina", "password": "pw12345678"}),
        &[],
    )
    .await;
    assert_eq!(status, 403, "the accounts fill the cap: {body}");
    assert!(
        !body["detail"].as_str().unwrap().contains("max_users"),
        "the setting stays in the log: {body}"
    );
    let eddy = login(&fx, "eddy").await;
    let (status, _, body) = call(
        &fx,
        &eddy,
        Method::POST,
        "/api/v1/invites/redeem",
        Some(json!({"token": token})),
    )
    .await;
    assert_eq!(status, 200, "an existing account is no new one: {body}");
    fx.auth.remove_user("late").await.unwrap();

    // A manager's link, then she is made an editor there.
    let mona = login(&fx, "mona").await;
    let (status, made) = create(&fx, &mona, json!({"domain": "team", "level": "editor"})).await;
    assert_eq!(status, 201, "{made}");
    let token = made["token"].as_str().unwrap().to_string();
    fx.auth
        .upsert_domain_member("team", "mona", MemberLevel::Editor, None, "olga")
        .await
        .unwrap();
    let (status, body) = confirm(&fx, &token).await;
    assert_eq!(status, 404, "{body}");
    let (status, _, body) = call(
        &fx,
        &login(&fx, "vera").await,
        Method::POST,
        "/api/v1/invites/redeem",
        Some(json!({"token": token})),
    )
    .await;
    assert_eq!(status, 409, "{body}");

    // The owner's link on an instance that no longer lets owners invite.
    let issued = fx
        .auth
        .create_invite(
            &NewInvite {
                role: Role::None,
                domain: Some("team"),
                level: Some(MemberLevel::Viewer),
                label: None,
                local_account: false,
                created_by: "olga",
                ttl_secs: 3_600,
            },
            InviteLimits {
                per_creator: 10,
                max_users: 100,
            },
        )
        .await
        .unwrap();
    let (status, _) = confirm(&fx, &issued.token).await;
    assert_eq!(status, 200, "an owner's link works while the setting is on");
}

/// With `auth.invites.managers` off, a link an owner or a manager made is
/// not redeemed; an admin's still is.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_managers_link_waits_for_the_setting() {
    let fx = serve(Options::default()).await;
    let limits = InviteLimits {
        per_creator: 10,
        max_users: 100,
    };
    let owners = fx
        .auth
        .create_invite(
            &NewInvite {
                role: Role::None,
                domain: Some("team"),
                level: Some(MemberLevel::Viewer),
                label: None,
                local_account: false,
                created_by: "olga",
                ttl_secs: 3_600,
            },
            limits,
        )
        .await
        .unwrap();
    let (status, body) = confirm(&fx, &owners.token).await;
    assert_eq!(status, 404, "{body}");
    let (status, _, body) = post_bare(
        &fx,
        "/invites/redeem",
        json!({"token": owners.token, "login": "nina", "password": "pw12345678"}),
        &[],
    )
    .await;
    assert_eq!(status, 409, "{body}");
    assert!(fx.auth.user("nina").await.unwrap().is_none());
}

/// Behind a forward-auth proxy the forwarded identity is the account: the
/// proxy made it before the link was taken, so it gets the membership only
/// and keeps the role the proxy gave it. A local account is not offered, and
/// a request the proxy named nobody in is told to sign in there.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_forwarded_identity_redeems_for_itself() {
    let fx = serve(Options {
        proxy_headers: true,
        ..Options::default()
    })
    .await;
    let issued = fx
        .auth
        .create_invite(
            &NewInvite {
                role: Role::Editor,
                domain: Some("team"),
                level: Some(MemberLevel::Editor),
                label: None,
                local_account: false,
                created_by: "root",
                ttl_secs: 3_600,
            },
            InviteLimits {
                per_creator: 10,
                max_users: 100,
            },
        )
        .await
        .unwrap();
    let (status, offer) = confirm(&fx, &issued.token).await;
    assert_eq!(status, 200, "{offer}");
    assert_eq!(offer["sign_in"], "proxy");
    assert_eq!(offer["local_allowed"], false);
    let (status, _, body) = post_bare(
        &fx,
        "/invites/redeem",
        json!({"token": issued.token, "login": "nina", "password": "pw12345678"}),
        &[],
    )
    .await;
    assert_eq!(status, 403, "no local account behind a proxy: {body}");
    let (status, _, body) =
        post_bare(&fx, "/invites/redeem", json!({"token": issued.token}), &[]).await;
    assert_eq!(status, 401, "{body}");

    let me = client()
        .get(format!("http://{}/api/v1/auth/me", fx.addr))
        .header("remote-user", "pia")
        .send()
        .await
        .unwrap();
    assert_eq!(me.status(), 200);
    let me: Value = me.json().await.unwrap();
    let csrf = me["csrf"].as_str().unwrap().to_string();
    let (status, _, done) = post_bare(
        &fx,
        "/invites/redeem",
        json!({"token": issued.token}),
        &[("remote-user", "pia"), ("x-csrf-token", &csrf)],
    )
    .await;
    assert_eq!(status, 200, "{done}");
    assert_eq!(done["created"], false);
    assert_eq!(done["user"]["name"], "pia");
    assert_eq!(
        done["user"]["role"], "viewer",
        "the proxy's role, not the link's"
    );
    assert_eq!(level_in_team(&fx, "pia").await, Some(MemberLevel::Editor));
}

/// A read-only instance answers confirm and refuses redeem.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_read_only_instance_refuses_redemption() {
    let fx = serve(Options {
        read_only: true,
        ..Options::default()
    })
    .await;
    let issued = fx
        .auth
        .create_invite(
            &NewInvite {
                role: Role::Viewer,
                domain: Some("team"),
                level: Some(MemberLevel::Viewer),
                label: None,
                local_account: false,
                created_by: "root",
                ttl_secs: 3_600,
            },
            InviteLimits {
                per_creator: 10,
                max_users: 100,
            },
        )
        .await
        .unwrap();
    let (status, _) = confirm(&fx, &issued.token).await;
    assert_eq!(status, 200);
    let (status, _, body) = post_bare(
        &fx,
        "/invites/redeem",
        json!({"token": issued.token, "login": "nina", "password": "pw12345678"}),
        &[],
    )
    .await;
    assert_eq!(status, 403, "{body}");
    assert!(fx.auth.user("nina").await.unwrap().is_none());
}

/// Wrong tokens from one address are slowed and then refused, on the
/// login throttle's settings.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wrong_tokens_are_throttled_per_address() {
    let fx = serve(Options {
        login: Some(LoginConfig {
            free_attempts: Some(1),
            max_delay: Some(1),
        }),
        ..Options::default()
    })
    .await;
    assert_eq!(confirm(&fx, "cin_one").await.0, 404);
    assert_eq!(confirm(&fx, "cin_two").await.0, 404, "after a nap");
    let (status, _, body) =
        post_bare(&fx, "/invites/redeem", json!({"token": "cin_three"}), &[]).await;
    assert_eq!(status, 429, "{body}");
}
