//! The nginx Fluid image under a path: the template rendered with and without
//! `CRYSTALLINE_BASE_PATH`, the way the image's envsubst renders it. Without
//! the variable the locations are exactly the 0.23.0 ones and the optional
//! snippets are not there; with it every location sits under the prefix, the
//! base tag is rewritten to the same bytes the daemon writes, and the two host
//! root OAuth documents are forwarded. `nginx -t` over both renderings runs in
//! the fluid-image CI job. Reads `fluid/` at run time and skips without it.

use std::path::PathBuf;

use crystalline_service::ui::BASE_TAG;

fn fluid(path: &str) -> Option<String> {
    let full = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fluid")
        .join(path);
    match std::fs::read_to_string(&full) {
        Ok(text) => Some(text),
        Err(_) => {
            eprintln!("note: skipping, {} not found", full.display());
            None
        }
    }
}

/// envsubst over the two names the image defines.
fn render(template: &str, base: &str) -> String {
    template
        .replace("${CRYSTALLINE_UPSTREAM}", "crystalline:7411")
        .replace("${CRYSTALLINE_BASE_PATH}", base)
}

fn locations(conf: &str) -> Vec<String> {
    conf.lines()
        .map(str::trim)
        .filter(|line| line.starts_with("location "))
        .map(|line| line.trim_end_matches('{').trim().to_string())
        .collect()
}

#[test]
fn without_a_base_path_the_locations_are_the_0_23_0_ones() {
    let Some(template) = fluid("nginx.conf.template") else {
        return;
    };
    let conf = render(&template, "");
    assert_eq!(
        locations(&conf),
        vec![
            "location /assets/",
            "location = /index.html",
            "location /api/v1/collab/",
            "location ~ ^/api/v1/domains/[^/]+/archive(/|$)",
            "location /api/",
            "location /",
        ]
    );
    assert!(
        !conf.contains("well-known"),
        "an unset image forwards no OAuth document"
    );
    assert!(!conf.contains("${"), "every variable is rendered");
}

#[test]
fn with_a_base_path_every_location_sits_under_it() {
    let Some(template) = fluid("nginx.conf.template") else {
        return;
    };
    let conf = render(&template, "/crystalline");
    assert_eq!(
        locations(&conf),
        vec![
            "location /crystalline/assets/",
            "location = /crystalline/index.html",
            "location /crystalline/api/v1/collab/",
            "location ~ ^/crystalline/api/v1/domains/[^/]+/archive(/|$)",
            "location /crystalline/api/",
            "location /crystalline/",
        ]
    );
    assert!(conf.contains("include /etc/nginx/conf.d/crystalline/server*.conf;"));
    assert!(conf.contains("include /etc/nginx/conf.d/crystalline/index*.conf;"));
    assert!(conf.contains("try_files $uri /crystalline/index.html;"));
}

#[test]
fn the_prefixed_template_serves_the_bare_prefix_without_a_redirect() {
    let Some(server) = fluid("nginx/server-base-path.conf.template") else {
        return;
    };
    let conf = render(&server, "/crystalline");
    let locations = locations(&conf);
    assert!(
        locations.contains(&"location = /crystalline".to_string()),
        "{locations:?}"
    );
    assert!(conf.contains("rewrite ^ /crystalline/index.html last;"));
    assert!(
        !conf.contains("return 301") && !conf.contains("permanent"),
        "no redirect"
    );
    for root in [
        "location = /.well-known/oauth-protected-resource/crystalline",
        "location = /.well-known/oauth-authorization-server/crystalline",
        "location /crystalline/.well-known/",
    ] {
        assert!(
            locations.contains(&root.to_string()),
            "{root}: {locations:?}"
        );
    }
    assert!(
        conf.contains("proxy_pass http://crystalline:7411;"),
        "no URI part, the path reaches the daemon as sent"
    );
}

#[test]
fn the_sub_filter_matches_the_bytes_the_daemon_rewrites() {
    let Some(index) = fluid("nginx/index-base-path.conf.template") else {
        return;
    };
    let conf = render(&index, "/crystalline");
    assert!(
        conf.contains(&format!(
            "sub_filter '{BASE_TAG}' '<base href=\"/crystalline/\" />';"
        )),
        "{conf}"
    );
}

#[test]
fn the_dockerfile_defines_the_variable_and_installs_the_script() {
    let Some(dockerfile) = fluid("Dockerfile") else {
        return;
    };
    assert!(
        dockerfile.contains("CRYSTALLINE_BASE_PATH="),
        "defined, so envsubst renders it empty"
    );
    assert!(dockerfile.contains("/docker-entrypoint.d/15-crystalline-base-path.sh"));
}
