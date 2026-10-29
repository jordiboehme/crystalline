//! Where the automation verbs go: the local daemon or a server, and the
//! MCP config the agent reaches the same place with.

use std::sync::atomic::{AtomicBool, Ordering};

use super::*;

/// Where the automation verbs go.
pub enum AutomationClient {
    /// The daemon on this machine, over its control socket. The flag turns
    /// true once a call was answered.
    Local(AtomicBool),
    /// A server, over the ctl route, as the token's account.
    Remote(Box<Connection>),
}

/// No daemon answered on this machine.
#[derive(Debug)]
pub(super) struct NoDaemon;

impl std::fmt::Display for NoDaemon {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("no Crystalline daemon is running. Start the daemon (crystalline serve).")
    }
}

impl std::error::Error for NoDaemon {}

impl AutomationClient {
    /// The server from `CRYSTALLINE_REMOTE_URL` and `CRYSTALLINE_REMOTE_TOKEN`
    /// when both are set, else the local daemon.
    pub async fn from_env() -> anyhow::Result<AutomationClient> {
        let Some(source) =
            crystalline_remote::env_source(crystalline_core::secret_env::process_var, &[])
        else {
            return Ok(AutomationClient::Local(AtomicBool::new(false)));
        };
        let dir = crystalline_remote::remote_dir()?;
        let connection = tokio::task::spawn_blocking(move || Connection::open(source, &dir))
            .await??
            .with_limit(REMOTE_VERB_LIMIT);
        Ok(AutomationClient::Remote(Box::new(connection)))
    }

    /// Send automation verb `cmd` (a ctl request with `v` and `cmd`) and
    /// return its `data`. A refusal comes back as its bare sentence.
    ///
    /// Locally the daemon is settled by the first call that is answered:
    /// that one may replace a daemon older than this binary, as other
    /// commands do. Every later call only attaches to the daemon that is
    /// there, so a renew, a share or a record never replaces the daemon in
    /// the middle of a run, which would cut off the agent's own MCP session.
    pub async fn call(&self, cmd: Value) -> anyhow::Result<Value> {
        match self {
            AutomationClient::Local(settled) => {
                let answer = if settled.load(Ordering::SeqCst) {
                    crystalline_service::ctl_if_running_passive(cmd).await?
                } else {
                    crystalline_service::ctl_if_running(cmd).await?
                };
                match answer {
                    Some(data) => {
                        settled.store(true, Ordering::SeqCst);
                        Ok(data)
                    }
                    None => Err(NoDaemon.into()),
                }
            }
            AutomationClient::Remote(connection) => {
                Ok(connection.ctl_data(cmd).await.map_err(anyhow::Error::new)?)
            }
        }
    }

    pub(super) async fn verb(&self, name: &str, lp: Loop, rest: Value) -> anyhow::Result<Value> {
        let mut request = rest;
        request["v"] = json!(1);
        request["cmd"] = json!(name);
        request["loop"] = json!(lp);
        self.call(request).await
    }

    /// The status of `lp` on `domains`, or on every domain in scope. With
    /// `force` a loop that backs off after failed runs counts as due when it
    /// has work.
    pub(super) async fn status(
        &self,
        lp: Loop,
        domains: &[String],
        force: bool,
    ) -> anyhow::Result<Value> {
        let mut rest = if domains.is_empty() {
            json!({})
        } else {
            json!({ "domains": domains })
        };
        if force {
            rest["force"] = json!(true);
        }
        self.verb("automation_status", lp, rest).await
    }

    /// The MCP config the agent connects with. The remote one holds the
    /// bearer, so it is only ever written to [`PrivateConfig`].
    pub(super) async fn mcp_config(&self, domain: &str) -> anyhow::Result<Value> {
        match self {
            AutomationClient::Local(_) => {
                let exe = std::env::current_exe()?;
                Ok(local_mcp_config(&exe, domain))
            }
            AutomationClient::Remote(connection) => {
                let token = connection.bearer().await.map_err(anyhow::Error::new)?;
                Ok(remote_mcp_config(&connection.source().url, &token, domain))
            }
        }
    }
}

/// Whether `e` is the other side's answer (a refusal), not a failure to
/// reach it. A refused renew means the lease is lost; an unreachable one is
/// retried on the next tick.
pub(super) fn is_refusal(e: &anyhow::Error) -> bool {
    if let Some(failure) = e.downcast_ref::<RemoteFailure>() {
        return matches!(
            failure,
            RemoteFailure::Refused(_) | RemoteFailure::Invalid(_)
        );
    }
    e.downcast_ref::<NoDaemon>().is_none()
        && e.downcast_ref::<std::io::Error>().is_none()
        && e.downcast_ref::<serde_json::Error>().is_none()
}

/// The stdio entry for the local daemon: this binary's `mcp` verb, marked
/// `--unattended` and bound to the run's domain with `--domain`: nobody is
/// behind a headless run to answer a question, so its session is never
/// asked to share Reflection text on purpose, and it reads only what every
/// reader of its domain may read.
///
/// It carries no `--harness`, unlike the entry `crystalline install`
/// registers. With `--harness claude-code` on a machine where Claude Code's
/// session hooks are installed the server leaves the `skills` tool out of
/// its list, since that harness reads the skills from disk. A headless run
/// loads no settings, hooks or skills from disk (`--setting-sources ""`), and
/// its prompt starts with the `skills` tool, so it needs the served surface.
pub(super) fn local_mcp_config(exe: &Path, domain: &str) -> Value {
    json!({
        "mcpServers": {
            "crystalline": {
                "command": exe.to_string_lossy(),
                "args": ["mcp", "--unattended", "--domain", domain],
            }
        }
    })
}

/// The HTTP entry for a server: its MCP endpoint is the root of `url` (the
/// transport answers there, under any base path), with the bearer header and
/// the run's domain, which the server believes only while the bearer's
/// account holds the run's lease there.
pub(super) fn remote_mcp_config(url: &str, token: &str, domain: &str) -> Value {
    json!({
        "mcpServers": {
            "crystalline": {
                "type": "http",
                "url": format!("{}/", url.trim_end_matches('/')),
                "headers": {
                    "Authorization": format!("Bearer {token}"),
                    crystalline_service::mcp::RUN_DOMAIN_HEADER: domain,
                },
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mcp_configs_have_their_pinned_shapes() {
        assert_eq!(
            local_mcp_config(Path::new("/opt/bin/crystalline"), "eng"),
            json!({ "mcpServers": { "crystalline": {
                "command": "/opt/bin/crystalline",
                "args": ["mcp", "--unattended", "--domain", "eng"] } } })
        );
        assert_eq!(
            remote_mcp_config("https://team.example.com/base/", "cmt_secret", "eng"),
            json!({ "mcpServers": { "crystalline": {
                "type": "http",
                "url": "https://team.example.com/base/",
                "headers": {
                    "Authorization": "Bearer cmt_secret",
                    "X-Crystalline-Run-Domain": "eng" } } } })
        );
    }
}
