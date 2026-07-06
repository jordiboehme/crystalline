//! Client-side entry points: the `crystalline mcp` stdio bridge, the CLI data
//! commands (over the socket when a daemon runs, else in-process) and the ctl
//! client used by the CLI operator commands.

use std::path::Path;
use std::sync::Arc;

use interprocess::local_socket::tokio::Stream as IpcStream;
use rmcp::model::{CallToolRequestParams, CallToolResult};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::daemon::{open_store, resolve_db};
use crate::engine::{Engine, open_standalone};
use crate::instance::{Connection, acquire_ownership, ensure_daemon, try_attach};
use crate::mcp::McpServer;
use crate::overlay;
use crate::params::*;

/// The `crystalline mcp` stdio entry: attach to (or spawn) a daemon and pump
/// bytes, or run the full stack in-process when embedded or when no daemon can
/// be started.
pub async fn run_mcp(
    embedded: bool,
    db: Option<&Path>,
    config_path: Option<&Path>,
    read_only: bool,
) -> anyhow::Result<()> {
    if embedded {
        return run_embedded_stdio(db, config_path, read_only).await;
    }
    // `read_only` is forwarded only to a daemon this call spawns; attaching to
    // an already-running daemon uses that daemon's own mode.
    match ensure_daemon(true, db, config_path, read_only).await {
        Ok(conn) => {
            let stream = conn.into_mcp().await?;
            pump_stdio(stream).await
        }
        Err(e) => {
            init_tracing();
            tracing::warn!("no daemon available ({e}); running embedded");
            run_embedded_stdio(db, config_path, read_only).await
        }
    }
}

/// Proxy stdin and stdout to the daemon socket. Exits when either side closes.
async fn pump_stdio(stream: IpcStream) -> anyhow::Result<()> {
    let (mut sock_read, mut sock_write) = tokio::io::split(stream);
    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    tokio::select! {
        r = tokio::io::copy(&mut stdin, &mut sock_write) => {
            r?;
            let _ = sock_write.shutdown().await;
        }
        r = tokio::io::copy(&mut sock_read, &mut stdout) => {
            r?;
            let _ = stdout.flush().await;
        }
    }
    Ok(())
}

/// The full in-process stack over stdio. Takes the lock; refuses if held.
/// The effective mode is the explicit flag or `service.read_only`.
async fn run_embedded_stdio(
    db: Option<&Path>,
    config_path: Option<&Path>,
    read_only: bool,
) -> anyhow::Result<()> {
    init_tracing();
    let ownership = acquire_ownership()
        .map_err(|e| anyhow::anyhow!("cannot run an embedded MCP server: {e}"))?;
    let loaded = overlay::load(config_path)?;
    let read_only = read_only || loaded.effective.read_only();
    let db_path = resolve_db(db)?;
    let store = open_store(&loaded.effective, Some(&db_path)).await?;
    // The provider is built in the background so the stdio session is ready and
    // text search works before any model download completes. There is no
    // watcher task in this mode, so the resolved config path only helps a domain
    // added mid-session resolve for data operations, not for picking up external
    // file changes. The engine holds the file config and the overlay apart, so
    // its effective config drives reads while persistence stays env-free.
    let engine = Arc::new(
        Engine::new(store, loaded.file.clone(), None, Some(loaded.path.clone()))
            .with_read_only(read_only)
            .with_env_overlay(loaded.overlay.clone()),
    );

    let bg = engine.clone();
    let bg_config = loaded.effective.clone();
    tokio::spawn(async move {
        let _ = bg.sync(None).await;
        if let Some(provider) = crate::engine::build_provider(&bg_config).await {
            bg.set_provider(provider);
            let _ = bg.embed_pending().await;
        }
    });

    // Prime the routing cache before serving so the very first `initialize`
    // renders complete instructions, never racing the background sync above.
    engine.refresh_routing_cache().await;

    let server = McpServer::new(engine);
    let running = rmcp::serve_server(server, (tokio::io::stdin(), tokio::io::stdout())).await?;
    let _ = running.waiting().await;
    drop(ownership);
    Ok(())
}

/// Run a tool by name: over the socket when a daemon is up, else in-process
/// against a directly opened store.
pub async fn run_tool(
    tool: &str,
    args: Value,
    db: Option<&Path>,
    config_path: Option<&Path>,
) -> anyhow::Result<Value> {
    if let Some(conn) = try_attach().await {
        let stream = conn.into_mcp().await?;
        return call_tool_over_stream(stream, tool, args).await;
    }
    let loaded = overlay::load(config_path)?;
    let db_path = resolve_db(db)?;
    let want_embeddings = matches!(tool, "search_engrams");
    let engine = open_standalone(loaded, &db_path, want_embeddings).await?;
    dispatch_engine(&engine, tool, args).await
}

/// Scaffold a virtual domain's MANIFEST from prebuilt markdown: over the daemon
/// when one owns the index, else against a directly opened store.
pub async fn scaffold_virtual_manifest(
    domain: &str,
    markdown: &str,
    db: Option<&Path>,
    config_path: Option<&Path>,
) -> anyhow::Result<Value> {
    use serde_json::json;
    if let Some(data) = ctl_if_running(json!({
        "v": 1, "cmd": "scaffold_manifest", "domain": domain, "markdown": markdown,
    }))
    .await?
    {
        return Ok(data);
    }
    let loaded = overlay::load(config_path)?;
    let db_path = resolve_db(db)?;
    let engine = open_standalone(loaded, &db_path, false).await?;
    Ok(engine.scaffold_virtual_manifest(domain, markdown).await?)
}

/// Import engram files into a virtual domain: over the daemon when one owns the
/// index, else against a directly opened store.
pub async fn domain_import(
    domain: &str,
    src: &Path,
    overwrite: bool,
    dry_run: bool,
    db: Option<&Path>,
    config_path: Option<&Path>,
) -> anyhow::Result<Value> {
    use serde_json::json;
    if let Some(data) = ctl_if_running(json!({
        "v": 1, "cmd": "domain_import", "domain": domain,
        "path": src.display().to_string(), "overwrite": overwrite, "dry_run": dry_run,
    }))
    .await?
    {
        return Ok(data);
    }
    let loaded = overlay::load(config_path)?;
    let db_path = resolve_db(db)?;
    let engine = open_standalone(loaded, &db_path, false).await?;
    Ok(engine
        .import_domain(domain, src, overwrite, dry_run)
        .await?)
}

/// Export a domain's engrams to a filesystem folder: over the daemon when one
/// owns the index, else against a directly opened store.
pub async fn domain_export(
    domain: &str,
    dest: &Path,
    force: bool,
    dry_run: bool,
    db: Option<&Path>,
    config_path: Option<&Path>,
) -> anyhow::Result<Value> {
    use serde_json::json;
    if let Some(data) = ctl_if_running(json!({
        "v": 1, "cmd": "domain_export", "domain": domain,
        "path": dest.display().to_string(), "force": force, "dry_run": dry_run,
    }))
    .await?
    {
        return Ok(data);
    }
    let loaded = overlay::load(config_path)?;
    let db_path = resolve_db(db)?;
    let engine = open_standalone(loaded, &db_path, false).await?;
    Ok(engine.export_domain(domain, dest, force, dry_run).await?)
}

/// Connect a new domain to a GitHub repository: over the daemon when one owns
/// the index, else against a directly opened store. `want_embeddings` is
/// `false` in the standalone fallback, matching `domain_import` and
/// `domain_export`: a one-shot command never triggers a surprise embedding
/// model download, and the domain is searchable via text immediately either
/// way; embedding follows whenever the daemon (or a later `sync --embed`)
/// gets to it.
pub async fn origin_add(
    repo: &str,
    domain: Option<&str>,
    path: Option<&str>,
    branch: Option<&str>,
    folder: Option<&str>,
    db: Option<&Path>,
    config_path: Option<&Path>,
) -> anyhow::Result<Value> {
    use serde_json::json;
    if let Some(data) = ctl_if_running(json!({
        "v": 1, "cmd": "origin_add", "repo": repo, "domain": domain,
        "path": path, "branch": branch, "folder": folder,
    }))
    .await?
    {
        return Ok(data);
    }
    let loaded = overlay::load(config_path)?;
    let db_path = resolve_db(db)?;
    let engine = open_standalone(loaded, &db_path, false).await?;
    Ok(engine
        .origin_add(repo, domain, path, branch, folder)
        .await?)
}

/// Bring one origin-connected domain (or every one) up to date: over the
/// daemon when one owns the index, else against a directly opened store.
pub async fn origin_update(
    domain: Option<&str>,
    db: Option<&Path>,
    config_path: Option<&Path>,
) -> anyhow::Result<Value> {
    use serde_json::json;
    if let Some(data) =
        ctl_if_running(json!({ "v": 1, "cmd": "origin_update", "domain": domain })).await?
    {
        return Ok(data);
    }
    let loaded = overlay::load(config_path)?;
    let db_path = resolve_db(db)?;
    let engine = open_standalone(loaded, &db_path, false).await?;
    Ok(engine.origin_update(domain).await?)
}

/// Report where one origin-connected domain (or every one) stands relative to
/// its origin, plus this machine's GitHub connection: over the daemon when
/// one owns the index, else against a directly opened store.
pub async fn origin_status(
    domain: Option<&str>,
    db: Option<&Path>,
    config_path: Option<&Path>,
) -> anyhow::Result<Value> {
    use serde_json::json;
    if let Some(data) =
        ctl_if_running(json!({ "v": 1, "cmd": "origin_status", "domain": domain })).await?
    {
        return Ok(data);
    }
    let loaded = overlay::load(config_path)?;
    let db_path = resolve_db(db)?;
    let engine = open_standalone(loaded, &db_path, false).await?;
    Ok(engine.origin_status(domain).await?)
}

/// Propose one team domain's local changes as a pull request against its
/// origin: over the daemon when one owns the index, else against a directly
/// opened store. `want_embeddings` is `false` in the standalone fallback: a
/// share never touches the working tree, so there is nothing new to embed.
pub async fn origin_share(
    domain: &str,
    title: Option<&str>,
    description: Option<&str>,
    db: Option<&Path>,
    config_path: Option<&Path>,
) -> anyhow::Result<Value> {
    use serde_json::json;
    if let Some(data) = ctl_if_running(json!({
        "v": 1, "cmd": "origin_share", "domain": domain,
        "title": title, "description": description,
    }))
    .await?
    {
        return Ok(data);
    }
    let loaded = overlay::load(config_path)?;
    let db_path = resolve_db(db)?;
    let engine = open_standalone(loaded, &db_path, false).await?;
    Ok(engine.origin_share(domain, title, description).await?)
}

/// Discard a declined, or still-open, share proposal for one team domain,
/// restoring local files that were not changed since sharing them: over the
/// daemon when one owns the index, else against a directly opened store.
pub async fn origin_discard(
    domain: &str,
    proposal: u64,
    db: Option<&Path>,
    config_path: Option<&Path>,
) -> anyhow::Result<Value> {
    use serde_json::json;
    if let Some(data) = ctl_if_running(json!({
        "v": 1, "cmd": "origin_discard", "domain": domain, "proposal": proposal,
    }))
    .await?
    {
        return Ok(data);
    }
    let loaded = overlay::load(config_path)?;
    let db_path = resolve_db(db)?;
    let engine = open_standalone(loaded, &db_path, false).await?;
    Ok(engine.origin_discard(domain, proposal).await?)
}

/// Resolve one recorded conflict for a team domain: over the daemon when one
/// owns the index, else against a directly opened store. `content` (a
/// caller-supplied merge) travels over the ctl socket base64-encoded, since
/// the JSON envelope carries text only and a resolved asset may be binary;
/// the in-process fallback passes the bytes straight through.
pub async fn origin_resolve(
    domain: &str,
    path: &str,
    keep: Option<&str>,
    content: Option<&[u8]>,
    db: Option<&Path>,
    config_path: Option<&Path>,
) -> anyhow::Result<Value> {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD as BASE64;
    use serde_json::json;
    let content_b64 = content.map(|c| BASE64.encode(c));
    if let Some(data) = ctl_if_running(json!({
        "v": 1, "cmd": "origin_resolve", "domain": domain, "path": path,
        "keep": keep, "content_b64": content_b64,
    }))
    .await?
    {
        return Ok(data);
    }
    let loaded = overlay::load(config_path)?;
    let db_path = resolve_db(db)?;
    let engine = open_standalone(loaded, &db_path, false).await?;
    Ok(engine.origin_resolve(domain, path, keep, content).await?)
}

/// Show, set or reset an agent-adjustable setting from the [`crate::settings`]
/// registry: over the daemon when one is running, else against the config
/// file directly (no index store is opened either way, unlike every data
/// command above). `action` is `show`, `set` or `unset`; `key` and `value`
/// are required for `set`, `key` alone for `unset`, and both are ignored for
/// `show`.
pub async fn configure(
    action: &str,
    key: Option<&str>,
    value: Option<&str>,
    config_path: Option<&Path>,
) -> anyhow::Result<Value> {
    use serde_json::json;
    if let Some(data) = ctl_if_running(json!({
        "v": 1, "cmd": "configure", "action": action, "key": key, "value": value,
    }))
    .await?
    {
        return Ok(data);
    }

    // The single load chokepoint resolves the config path and parses the
    // environment overlay: `show` reads the file config plus the overlay, and a
    // write mutates a clone of the file config and saves it to the resolved
    // path, so no environment value ever bakes into the saved file.
    let loaded = overlay::load(config_path)?;
    match action {
        "show" => Ok(json!({
            "settings": crate::settings::snapshot(&loaded.file, &loaded.overlay)
        })),
        "set" => {
            if loaded.effective.read_only() {
                anyhow::bail!("{}", crate::engine::EngineError::ReadOnly);
            }
            let key = key.ok_or_else(|| anyhow::anyhow!("configure set requires a key"))?;
            let value = value.ok_or_else(|| anyhow::anyhow!("configure set requires a value"))?;
            let mut file = loaded.file.clone();
            crate::settings::apply(&mut file, key, value)?;
            save_file(&loaded.path, &file)?;
            Ok(setting_view(&file, key, &loaded.overlay))
        }
        "unset" => {
            if loaded.effective.read_only() {
                anyhow::bail!("{}", crate::engine::EngineError::ReadOnly);
            }
            let key = key.ok_or_else(|| anyhow::anyhow!("configure unset requires a key"))?;
            let mut file = loaded.file.clone();
            crate::settings::unset(&mut file, key)?;
            save_file(&loaded.path, &file)?;
            Ok(setting_view(&file, key, &loaded.overlay))
        }
        other => anyhow::bail!("unknown configure action '{other}'; expected show, set or unset"),
    }
}

/// The just-applied setting's snapshot entry, as a JSON value, with a `note`
/// field attached when [`crate::settings::change_note`] has one (a
/// startup-effective reminder, an active env override, or both). `file` is the
/// freshly saved file config; the snapshot layers `overlay` on top, so an
/// env-overridden key reports its env value with `source: env`. `key` has
/// already been validated against the registry by `apply`/`unset`, so it is
/// always found.
fn setting_view(
    file: &crystalline_core::config::GlobalConfig,
    key: &str,
    overlay: &overlay::EnvOverlay,
) -> Value {
    crate::settings::snapshot(file, overlay)
        .into_iter()
        .find(|v| v.key == key)
        .map(|v| {
            let mut value = serde_json::to_value(v).unwrap_or(Value::Null);
            if let Some(note) = crate::settings::change_note(key, overlay)
                && let Value::Object(map) = &mut value
            {
                map.insert("note".to_string(), Value::String(note));
            }
            value
        })
        .unwrap_or(Value::Null)
}

/// Save a config to the path the load chokepoint already resolved.
fn save_file(path: &Path, config: &crystalline_core::config::GlobalConfig) -> anyhow::Result<()> {
    crystalline_core::config::save_yaml(path, config)
        .map_err(|e| anyhow::anyhow!("failed to save config {}: {e}", path.display()))
}

/// Resolve virtual-domain routing bullets for `prompt system`: over the daemon
/// when one is running (its warm state), else against a directly opened store.
/// Returns an empty map when the config has no virtual domains, so the common
/// all-file case never opens a store or a socket.
pub async fn virtual_routing_bullets(
    config: &crystalline_core::config::GlobalConfig,
    db: Option<&Path>,
) -> std::collections::BTreeMap<String, Vec<String>> {
    use serde_json::json;
    if !config.domains.values().any(|e| e.is_virtual()) {
        return std::collections::BTreeMap::new();
    }
    if let Ok(Some(data)) = ctl_if_running(json!({ "v": 1, "cmd": "routing_bullets" })).await
        && let Ok(map) = serde_json::from_value(data)
    {
        return map;
    }
    let db_path = match resolve_db(db) {
        Ok(p) => p,
        Err(_) => return std::collections::BTreeMap::new(),
    };
    // The caller already resolved the effective config (the overlay is applied
    // upstream in `run_prompt`), and this read-only path never persists, so a
    // no-op overlay over the given config is all `open_standalone` needs. The
    // path is only consulted on a post-startup domain re-read, which this
    // one-shot never performs.
    let loaded = overlay::LoadedConfig {
        path: crystalline_core::config::global_config_path().unwrap_or_default(),
        file: config.clone(),
        effective: config.clone(),
        overlay: overlay::EnvOverlay::default(),
    };
    match open_standalone(loaded, &db_path, false).await {
        Ok(engine) => engine.virtual_routing_bullets().await,
        Err(_) => std::collections::BTreeMap::new(),
    }
}

/// Call a tool over an MCP client on a socket stream and return its JSON value.
async fn call_tool_over_stream(
    stream: IpcStream,
    tool: &str,
    args: Value,
) -> anyhow::Result<Value> {
    let client = rmcp::serve_client((), stream).await?;
    let mut params = CallToolRequestParams::new(tool.to_string());
    if let Value::Object(map) = args {
        params = params.with_arguments(map);
    }
    let result = client.peer().call_tool(params).await;
    let out = match result {
        Ok(result) => extract_tool_value(&result),
        Err(e) => Err(anyhow::anyhow!("{e}")),
    };
    let _ = client.cancel().await;
    out
}

/// Pull the tool's JSON body out of the single text content block.
fn extract_tool_value(result: &CallToolResult) -> anyhow::Result<Value> {
    let v = serde_json::to_value(result)?;
    if let Some(text) = v.pointer("/content/0/text").and_then(Value::as_str) {
        return Ok(serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_string())));
    }
    Ok(v)
}

/// Dispatch a tool to the in-process engine.
async fn dispatch_engine(engine: &Engine, tool: &str, args: Value) -> anyhow::Result<Value> {
    let v = match tool {
        "write_engram" => engine.write_engram(&decode::<WriteParams>(args)?).await?,
        "read_engram" => engine.read_engram(&decode::<ReadParams>(args)?).await?,
        "edit_engram" => engine.edit_engram(&decode::<EditParams>(args)?).await?,
        "move_engram" => engine.move_engram(&decode::<MoveParams>(args)?).await?,
        "delete_engram" => engine.delete_engram(&decode::<DeleteParams>(args)?).await?,
        "search_engrams" => {
            engine
                .search_engrams(&decode::<SearchParams>(args)?)
                .await?
        }
        "build_context" => {
            engine
                .build_context(&decode::<ContextParams>(args)?)
                .await?
        }
        "recent_activity" => {
            engine
                .recent_activity(&decode::<RecentParams>(args)?)
                .await?
        }
        "list_domains" => {
            engine
                .list_domains(&decode::<ListDomainsParams>(args)?)
                .await?
        }
        "browse_domain" => engine.browse_domain(&decode::<BrowseParams>(args)?).await?,
        "validate_engrams" => {
            engine
                .validate_engrams(&decode::<ValidateParams>(args)?)
                .await?
        }
        "infer_schema" => engine.infer_schema(&decode::<InferParams>(args)?).await?,
        other => anyhow::bail!("unknown tool '{other}'"),
    };
    Ok(v)
}

fn decode<T: DeserializeOwned>(args: Value) -> anyhow::Result<T> {
    serde_json::from_value(args).map_err(|e| anyhow::anyhow!("invalid arguments: {e}"))
}

// --- ctl client --------------------------------------------------------------

/// Send a ctl command if a daemon is running, else `None`.
pub async fn ctl_if_running(cmd: Value) -> anyhow::Result<Option<Value>> {
    match try_attach().await {
        Some(conn) => Ok(Some(ctl_exchange(conn, cmd).await?)),
        None => Ok(None),
    }
}

/// Send a ctl command, erroring when no daemon is running.
pub async fn ctl_required(cmd: Value) -> anyhow::Result<Value> {
    match try_attach().await {
        Some(conn) => ctl_exchange(conn, cmd).await,
        None => {
            anyhow::bail!("no Crystalline daemon is running; start one with `crystalline serve`")
        }
    }
}

async fn ctl_exchange(conn: Connection, cmd: Value) -> anyhow::Result<Value> {
    let stream = conn.into_ctl().await?;
    let (read, mut write) = tokio::io::split(stream);
    let mut reader = BufReader::new(read);
    let mut line = serde_json::to_string(&cmd)?;
    line.push('\n');
    write.write_all(line.as_bytes()).await?;
    write.flush().await?;

    let mut response = String::new();
    reader.read_line(&mut response).await?;
    let value: Value = serde_json::from_str(response.trim())?;
    if value.get("ok").and_then(Value::as_bool) == Some(true) {
        Ok(value.get("data").cloned().unwrap_or(Value::Null))
    } else {
        anyhow::bail!(
            value
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("ctl error")
                .to_string()
        )
    }
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_max_level(tracing::Level::WARN)
        .try_init();
}
