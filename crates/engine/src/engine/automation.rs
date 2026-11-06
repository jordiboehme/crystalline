//! The reflect and evolve loops, run headless: whether a loop is due on a
//! domain, the lease a runner takes while it works, the run it records and
//! the pull request it shares its changes in.
//!
//! Status is computed from state alone (R1): what the loop would work now,
//! less what a finished run left for a person at the same version (R3), and
//! nothing while a fresh lease is held. A run shares only the files it
//! changed (R4): the lease keeps the domain's unshared files with their
//! checksums at take time.

use super::*;
use crystalline_core::automation::{
    LEASE_RENEW_SECS, LEASE_STALE_SECS, Loop, RUNS_KEPT, RunOutcome, RunReport, SUMMARY_MAX_BYTES,
    backoff_reason, backoff_secs, cap_summary,
};
use crystalline_index::{
    AutomationLease, AutomationRun, DueKey, LeaseClaim, LeaseHolder, PathChecksum,
};

use crate::scope::{ReflectionCaller, ReflectionRight, Scope};

/// The MANIFEST's path in a domain, as the share and the run's writes name it.
const MANIFEST_FILE: &str = "MANIFEST.md";

/// Why a domain whose loop can run nowhere here is not due.
const NOT_HOSTED_HERE: &str = "not hosted here";

/// The refusal of a take while another run holds a fresh lease.
const ALREADY_RUNNING: &str = "already running";

/// The error a webhook URL without a secret shows: nothing is sent.
pub const WEBHOOK_NO_SECRET: &str = "no webhook secret is set on this instance, so nothing is sent";

/// The longest runner label a lease keeps.
const RUNNER_MAX_BYTES: usize = 128;

/// The summary an expired run is recorded with.
const EXPIRED_SUMMARY: &str = "The runner stopped renewing its lease.";

/// Where a loop on a domain may run, seen from this instance.
enum Runs {
    /// Here.
    Here,
    /// On another instance, by its name.
    On(String),
    /// Nowhere right now, for this reason (a Reflection that is off, a
    /// read-only domain).
    Nowhere(EngineError),
}

/// What a loop would work on a domain now: every item with its key and
/// version, before the items a run left are taken out.
#[derive(Default)]
struct Work {
    /// Each item, and for reflect the page it is on.
    items: Vec<(DueKey, String)>,
    /// Judgment findings (evolve), never due.
    judgment: u64,
}

/// [`Work`] less the items a finished run left for a person.
struct Due {
    items: Vec<(DueKey, String)>,
    left: usize,
    judgment: u64,
}

/// The reflection caller a loop's rights are decided for.
fn loop_caller(scope: &Scope) -> ReflectionCaller {
    match scope {
        Scope::Unrestricted => ReflectionCaller::owner(),
        Scope::User { account, .. } => ReflectionCaller::person(scope.clone(), account),
        Scope::Anonymous | Scope::Agent { .. } => {
            ReflectionCaller::person(Scope::Anonymous, "anonymous")
        }
    }
}

/// Who a run shares as.
fn share_actor(scope: &Scope) -> ShareActor {
    match scope {
        Scope::Unrestricted => ShareActor::Machine,
        Scope::User { account, .. } => ShareActor::Account(account.clone()),
        Scope::Anonymous | Scope::Agent { .. } => ShareActor::HttpAgent,
    }
}

/// The account a runner works as, when it has one.
fn scope_account(scope: &Scope) -> Option<String> {
    match scope {
        Scope::User { account, .. } => Some(account.clone()),
        _ => None,
    }
}

/// How many runs in a row failed, newest first, and when the newest of them
/// ended. A run that expired neither counts nor breaks the row; any other
/// outcome ends it.
fn failed_in_a_row(runs: &[AutomationRun]) -> (u32, Option<&str>) {
    let mut failed = 0u32;
    let mut last = None;
    for run in runs {
        if run.outcome == RunOutcome::Failed.as_str() {
            failed += 1;
            last.get_or_insert(run.ended_at.as_str());
        } else if run.outcome != RunOutcome::Expired.as_str() {
            break;
        }
    }
    (failed, last)
}

fn same_item(a: &DueKey, b: &DueKey) -> bool {
    a.kind == b.kind && a.key == b.key && a.version == b.version
}

/// A sha256 over `parts`, each closed by a NUL.
fn version_of<'a>(parts: impl IntoIterator<Item = &'a str>) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update([0u8]);
    }
    crystalline_index::hex_lower(&hasher.finalize())
}

/// A run as the answers show it: everything but the items it left, which
/// are bookkeeping keyed by page paths and folder names (`left` counts
/// them).
fn run_json(run: &AutomationRun) -> Value {
    let mut value = json!(run);
    if let Some(obj) = value.as_object_mut() {
        obj.remove("left_items");
    }
    value
}

/// What a status answer shows of a lease that is still fresh.
fn running_json(lease: &AutomationLease) -> Value {
    json!({
        "runner": lease.holder.runner,
        "account": lease.holder.account,
        "taken_at": lease.taken_at,
        "renewed_at": lease.renewed_at,
    })
}

impl Engine {
    /// Whether `lp` is due on each of `domains`, or on every domain `scope`
    /// may run it on when none are named: `{loop, domains: [{domain, loop,
    /// due, counts, left_for_person, running, runs_on, reason, last_run,
    /// delivery}]}`; `delivery` ([`Delivery`]) is null unless the MANIFEST
    /// turns the loop's webhook trigger on.
    ///
    /// Reflect counts the queue a headless run works (threads waiting for
    /// the agent, feedback, page updates; no update of a page open in a live
    /// editor), evolve counts its findings and only mechanical ones are due.
    /// An item a finished run left for a person is not due again until it
    /// changes, and is counted in `left_for_person` instead. A held lease
    /// reports `running` and nothing is due; a domain whose loop runs on
    /// another instance names it in `runs_on` and is never due here. A named
    /// domain the caller may not run the loop on is refused; the unnamed
    /// scope leaves it out.
    ///
    /// After failed runs the loop backs off on that domain: it is not due,
    /// and `reason` says "backing off after N failed runs", until 5 minutes
    /// after the last failure, doubled for each further failed run in a row
    /// and at most 6 hours. A run that did not fail ends the row.
    pub async fn automation_status(
        &self,
        lp: Loop,
        domains: &[String],
        scope: &Scope,
    ) -> Result<Value> {
        self.automation_status_forced(lp, domains, scope, false)
            .await
    }

    /// [`Engine::automation_status`], and with `force` a loop that backs off
    /// after failed runs is due anyway when it has work: what a person who
    /// starts a run by hand (`<loop> run --force`) asks. Nothing else is
    /// skipped.
    pub async fn automation_status_forced(
        &self,
        lp: Loop,
        domains: &[String],
        scope: &Scope,
        force: bool,
    ) -> Result<Value> {
        let named: Vec<String> = if domains.is_empty() {
            self.loop_domains(lp, scope).await?
        } else {
            let mut once: Vec<String> = Vec::new();
            for name in domains {
                self.require_loop_right(lp, name, scope).await?;
                if !once.contains(name) {
                    once.push(name.clone());
                }
            }
            once
        };
        let mut out = Vec::with_capacity(named.len());
        for domain in &named {
            out.push(self.loop_status(lp, domain, scope, true, force).await?);
        }
        Ok(json!({ "loop": lp, "domains": out }))
    }

    /// What a person may see of the loops on `domain`: `{reflect?, evolve?}`
    /// (just `only` when it names one), each the entry
    /// [`Engine::automation_status`] gives, cut to the caller's share. The
    /// right to see any of it is the right to read the domain: a domain the
    /// caller may not see is refused as an unknown one.
    ///
    /// - Everyone who reads: `last_run` without its `account`, `summary` and
    ///   `pr_url`, and `running` without its `account`.
    /// - A caller who may run the loop (curate the Reflection, write the
    ///   knowledge) also gets `summary`, `pr_url` and `account`, and, when
    ///   `counts` is true, `counts` and `left_for_person`. Both are `null`
    ///   for everyone else and with `counts` false, which skips the due
    ///   work (a sweep of the domain) altogether.
    /// - Only a domain manager or an instance admin gets `delivery`, which
    ///   can name the receiver's host and how this instance is set up.
    pub async fn automation_overview(
        &self,
        domain: &str,
        scope: &Scope,
        only: Option<Loop>,
        counts: bool,
    ) -> Result<Value> {
        self.require_domain(domain, scope).await?;
        let manager = !matches!(scope, Scope::Anonymous)
            && self.domain_right(scope, domain).await? >= crate::scope::DomainRight::Manage;
        let mut out = serde_json::Map::new();
        for lp in [Loop::Reflect, Loop::Evolve] {
            if only.is_some_and(|only| only != lp) {
                continue;
            }
            let may_run = match self.require_loop_right(lp, domain, scope).await {
                Ok(()) => true,
                Err(EngineError::Forbidden(_) | EngineError::DomainReadOnly(_)) => false,
                Err(e) => return Err(e),
            };
            let counted = may_run && counts;
            let mut entry = self.loop_status(lp, domain, scope, counted, false).await?;
            if let Some(obj) = entry.as_object_mut() {
                if !may_run {
                    for key in ["last_run", "running"] {
                        if let Some(inner) = obj.get_mut(key).and_then(Value::as_object_mut) {
                            for secret in ["account", "summary", "pr_url"] {
                                inner.remove(secret);
                            }
                        }
                    }
                }
                if !counted {
                    obj.insert("left_for_person".to_string(), Value::Null);
                    obj.insert("counts".to_string(), Value::Null);
                }
                if !manager {
                    obj.insert("delivery".to_string(), Value::Null);
                }
            }
            out.insert(lp.as_str().to_string(), entry);
        }
        Ok(Value::Object(out))
    }

    /// Take the lease of `lp` on `domain` for `runner`: `{token,
    /// renew_secs}`. Refused with "already running" while another run holds
    /// a fresh lease, and with "runs on" and its name where another instance
    /// runs the loop. A stale lease is taken over, and the run that held it
    /// is recorded as expired, once: the store names the lease the claim
    /// replaced in the claim itself.
    ///
    /// An evolve take on a team domain whose MANIFEST names no
    /// `evolve_home` claims the home for this instance when it has a
    /// `service.public_url` (see `claim_evolve_home`): the answer then
    /// carries `evolve_home` and a `note`, and without an address only the
    /// `note` that every clone may still run evolve.
    pub async fn automation_lease_take(
        &self,
        lp: Loop,
        domain: &str,
        runner: &str,
        scope: &Scope,
    ) -> Result<Value> {
        let runner = runner.trim();
        if runner.is_empty() || runner.len() > RUNNER_MAX_BYTES {
            return Err(EngineError::Invalid(format!(
                "runner must be a label of 1 to {RUNNER_MAX_BYTES} bytes"
            )));
        }
        self.require_loop_right(lp, domain, scope).await?;
        match self.loop_runs(lp, domain, scope).await {
            Runs::Here => {}
            Runs::On(name) => return Err(EngineError::Refused(format!("runs on {name}"))),
            Runs::Nowhere(e) => return Err(e),
        }
        let id = self.loop_domain_id(domain).await?;
        let due = self.loop_due(lp, domain, id, scope).await?;
        let mut unshared_at_start = self.unshared_now(domain, scope).await?;
        // A claim of the evolve home an earlier run wrote and never shared
        // (it failed, was stopped or ran with --no-share) rides with this
        // run: a MANIFEST unshared at the take would be held back as a
        // person's work, and no later run would carry it.
        // A claim the team declined is withdrawn first, so this run neither
        // carries it nor acts as the home.
        let withdrawn = match lp {
            Loop::Evolve => self.withdraw_declined_evolve_claim(domain).await,
            Loop::Reflect => None,
        };
        let pending_claim = match lp {
            Loop::Evolve if withdrawn.is_none() => self.pending_evolve_claim(domain),
            _ => None,
        };
        if pending_claim.is_some() {
            unshared_at_start.retain(|file| file.path != MANIFEST_FILE);
        }
        let (now, stale_before) = self.automation_window();
        let holder = LeaseHolder {
            instance_id: self.instance_id.clone(),
            account: scope_account(scope),
            runner: runner.to_string(),
            unshared_at_start,
            due_at_start: due.items.into_iter().map(|(key, _)| key).collect(),
        };
        let mut taken = self
            .take_lease(lp, id, &holder, &now, &stale_before)
            .await?;
        // After the lease is held, so of two takes racing only the one that
        // won writes the home.
        if withdrawn.is_some() {
            taken["note"] = json!(format!(
                "The team declined this instance's evolve_home claim for '{domain}', so it is withdrawn here and this instance is not the home: every clone of it may run evolve. To claim it again, unset evolve_home here (configure) and run evolve."
            ));
        } else if let Some(own) = pending_claim {
            self.note_claim_written(domain, scope).await;
            taken["note"] = json!(format!(
                "The claim of the evolve home of '{domain}' for this instance (evolve_home: {own}) is not in the team's copy yet, so this run's pull request carries it too."
            ));
        } else if lp == Loop::Evolve {
            let (home, note) = self.claim_evolve_home(domain, scope).await;
            if let Some(home) = home {
                taken["evolve_home"] = json!(home);
            }
            if let Some(note) = note {
                taken["note"] = json!(note);
            }
        }
        Ok(taken)
    }

    /// The store half of [`Engine::automation_lease_take`]: claim the lease
    /// and record a stale lease it replaced as expired.
    async fn take_lease(
        &self,
        lp: Loop,
        id: DomainId,
        holder: &LeaseHolder,
        now: &str,
        stale_before: &str,
    ) -> Result<Value> {
        let store = self.store.lock().await;
        match store
            .take_automation_lease(id, lp.as_str(), holder, now, stale_before)
            .await?
        {
            LeaseClaim::Held(_) => Err(EngineError::Refused(ALREADY_RUNNING.to_string())),
            LeaseClaim::Acquired { token, replaced } => {
                // A take that lands over a lease means that lease was stale:
                // its runner stopped, and the history says so.
                if let Some(old) = replaced {
                    let run = AutomationRun {
                        started_at: old.taken_at.clone(),
                        ended_at: old.renewed_at.clone(),
                        runner: old.holder.runner.clone(),
                        account: old.holder.account.clone(),
                        outcome: RunOutcome::Expired.as_str().to_string(),
                        done: 0,
                        left: 0,
                        judgment_waiting: 0,
                        summary: EXPIRED_SUMMARY.to_string(),
                        pr_url: old.pr_url.clone(),
                        left_items: Vec::new(),
                    };
                    store
                        .record_automation_run(id, lp.as_str(), &run, RUNS_KEPT)
                        .await?;
                }
                Ok(json!({ "token": token, "renew_secs": LEASE_RENEW_SECS }))
            }
        }
    }

    /// Renew the lease `token` names: `{renewed: true, renew_secs}`. Refused
    /// when the lease is gone or another run took it.
    pub async fn automation_lease_renew(
        &self,
        lp: Loop,
        domain: &str,
        token: &str,
        scope: &Scope,
    ) -> Result<Value> {
        self.require_loop_right(lp, domain, scope).await?;
        let id = self.loop_domain_id(domain).await?;
        let (now, _) = self.automation_window();
        let renewed = {
            let store = self.store.lock().await;
            store
                .renew_automation_lease(id, lp.as_str(), token, &now)
                .await?
        };
        if !renewed {
            return Err(lease_lost(lp, domain));
        }
        Ok(json!({ "renewed": true, "renew_secs": LEASE_RENEW_SECS }))
    }

    /// Release the lease `token` names: `{released}`, `false` when the lease
    /// is gone or another run holds it.
    pub async fn automation_lease_release(
        &self,
        lp: Loop,
        domain: &str,
        token: &str,
        scope: &Scope,
    ) -> Result<Value> {
        self.require_loop_right(lp, domain, scope).await?;
        let id = self.loop_domain_id(domain).await?;
        let released = {
            let store = self.store.lock().await;
            store
                .release_automation_lease(id, lp.as_str(), token)
                .await?
        };
        Ok(json!({ "released": released.is_some() }))
    }

    /// Record the run that holds `token`, then release its lease: `{run}`.
    ///
    /// The release is the authority: it is a compare-and-delete on the
    /// token, and the run is recorded from the lease it returns, in the same
    /// store lock. A token that no longer holds the lease (it expired and
    /// another run took it, or it was never this lease's) is refused and
    /// records nothing.
    ///
    /// Only a `done` run leaves items for a person: every item due when the
    /// lease was taken that is still due at the same version (R3). A run that
    /// stopped at its cap, failed or expired leaves none, so what it did not
    /// reach stays due. An evolve run also counts the judgment findings
    /// waiting. The summary keeps its first 2 KiB, and the pull request the
    /// run shared in rides along from the lease.
    pub async fn automation_run_record(
        &self,
        lp: Loop,
        domain: &str,
        token: &str,
        report: &RunReport,
        scope: &Scope,
    ) -> Result<Value> {
        let started_at = report.started_at.trim();
        if !started_at.is_empty() && DateTime::parse_from_rfc3339(started_at).is_err() {
            return Err(EngineError::Invalid(format!(
                "started_at '{started_at}' is not an RFC 3339 time"
            )));
        }
        self.require_loop_right(lp, domain, scope).await?;
        let id = self.loop_domain_id(domain).await?;
        // A wrong token is refused before the work is counted; the release
        // below decides for good.
        self.held_lease(lp, domain, id, token).await?;
        // Counted before the lock, so a long evolve sweep holds no store
        // lock; it may be a moment old by the time the run is recorded.
        let needs_work = report.outcome == RunOutcome::Done || lp == Loop::Evolve;
        let work = if needs_work {
            self.loop_work(lp, domain, scope).await?
        } else {
            Work::default()
        };
        let (now, _) = self.automation_window();
        let store = self.store.lock().await;
        let Some(lease) = store
            .release_automation_lease(id, lp.as_str(), token)
            .await?
        else {
            return Err(lease_lost(lp, domain));
        };
        let left_items: Vec<DueKey> = if report.outcome == RunOutcome::Done {
            lease
                .holder
                .due_at_start
                .iter()
                .filter(|start| work.items.iter().any(|(now, _)| same_item(start, now)))
                .cloned()
                .collect()
        } else {
            Vec::new()
        };
        let run = AutomationRun {
            started_at: if started_at.is_empty() {
                lease.taken_at.clone()
            } else {
                started_at.to_string()
            },
            ended_at: now,
            runner: lease.holder.runner.clone(),
            account: lease.holder.account.clone(),
            outcome: report.outcome.as_str().to_string(),
            done: report.done,
            left: left_items.len() as u64,
            judgment_waiting: work.judgment,
            summary: cap_summary(&report.summary, SUMMARY_MAX_BYTES).to_string(),
            pr_url: lease.pr_url.clone(),
            left_items,
        };
        store
            .record_automation_run(id, lp.as_str(), &run, RUNS_KEPT)
            .await?;
        drop(store);
        Ok(json!({ "domain": domain, "loop": lp, "run": run_json(&run) }))
    }

    /// Note that `account` wrote `paths` in `domain`, for the run whose
    /// lease that account holds there (R4, ruling B). The MCP tool dispatch
    /// calls this after every write-shaped tool that succeeded; nothing else
    /// does, so a write over REST, Fluid, the files or a sync is never noted
    /// and never shared by a run.
    ///
    /// The token is the rule, as for renew, share and record: a lease that
    /// went stale and that nobody took over still notes, so a runner that
    /// lapsed loses none of its writes. A domain where the account holds no
    /// lease notes nothing.
    ///
    /// The engine cannot tell which run made an MCP write, so the note lands
    /// in every lease the account holds on the domain, the reflect and the
    /// evolve lease alike. A reflect runner and an evolve runner that work
    /// one domain at the same time take separate accounts, or each pull
    /// request carries the other run's writes.
    pub async fn note_written(&self, domain: &str, account: &str, paths: &[String]) -> Result<()> {
        if paths.is_empty() || account.is_empty() {
            return Ok(());
        }
        let store = self.store.lock().await;
        let Some(id) = store.domain_id(domain).await? else {
            return Ok(());
        };
        store.note_automation_written(id, account, paths).await?;
        Ok(())
    }

    /// Share what the run that holds `token` changed, as a pull request
    /// whatever the domain's `sharing` says (R4): `{outcome: "shared" |
    /// "nothing_to_share" | "no_origin", url?, files, held_back, attributed,
    /// recorded?}`. `recorded` (on `shared`) is `false` when the lease was
    /// taken over during the share, so the run record will not carry the url.
    ///
    /// Only the runner's own MCP writes are shared: the paths the lease's
    /// account wrote over MCP while it held the lease
    /// ([`Engine::note_written`]) that are unshared now. A lease with no
    /// account cannot attribute a write; it shares the files that are new
    /// or changed since the take instead, and says `attributed: false`.
    /// Either way a path a person had unshared work on when the lease was
    /// taken is held back, even when the run changed it too: it is listed
    /// in `held_back` and named in the pull request description, so a
    /// person shares it. The title names the loop, the domain and the date;
    /// the description is the summary, capped like the run record's.
    pub async fn automation_share(
        &self,
        lp: Loop,
        domain: &str,
        token: &str,
        summary: &str,
        scope: &Scope,
    ) -> Result<Value> {
        self.require_loop_right(lp, domain, scope).await?;
        let id = self.loop_domain_id(domain).await?;
        let lease = self.held_lease(lp, domain, id, token).await?;
        let attributed = lease.holder.account.is_some();
        if !self.config.read().unwrap().github_enabled() || !self.domain_has_origin(domain)? {
            return Ok(json!({
                "outcome": "no_origin",
                "files": [],
                "held_back": [],
                "attributed": attributed,
            }));
        }
        let start = &lease.holder.unshared_at_start;
        let candidates: Vec<String> = self
            .unshared_now(domain, scope)
            .await?
            .into_iter()
            .filter(|now| {
                if attributed {
                    lease.written.contains(&now.path)
                } else {
                    !start
                        .iter()
                        .any(|s| s.path == now.path && s.checksum == now.checksum)
                }
            })
            .map(|now| now.path)
            .collect();
        let (mut held_back, files): (Vec<String>, Vec<String>) = candidates
            .into_iter()
            .partition(|path| start.iter().any(|s| &s.path == path));
        held_back.sort();
        if files.is_empty() {
            return Ok(json!({
                "outcome": "nothing_to_share",
                "files": [],
                "held_back": held_back,
                "attributed": attributed,
            }));
        }
        let title = format!(
            "{} run on {domain}, {}",
            lp.title_word(),
            self.automation_clock().format("%Y-%m-%d")
        );
        let mut description = cap_summary(summary, SUMMARY_MAX_BYTES).to_string();
        if !held_back.is_empty() {
            let n = held_back.len();
            description.push_str(&format!(
                "\n\n{n} {} held back because a person had unshared work on {}.",
                if n == 1 { "file was" } else { "files were" },
                if n == 1 { "it" } else { "them" },
            ));
        }
        let receipt = self
            .origin_share_with(
                domain,
                Some(&title),
                Some(&description),
                None,
                Some(&files),
                Some(true),
                share_actor(scope),
            )
            .await?;
        let report = match receipt["outcome"].as_str() {
            Some("proposed") => &receipt,
            Some("updated") => &receipt["proposal"],
            Some("nothing_to_share") => {
                return Ok(json!({
                    "outcome": "nothing_to_share",
                    "files": [],
                    "held_back": held_back,
                    "attributed": attributed,
                }));
            }
            _ => &Value::Null,
        };
        let Some(url) = report["url"].as_str().map(str::to_string) else {
            // Anything else (conflicts waiting, a diverged proposal) is the
            // share's own answer, passed on as it came.
            let mut answer = receipt.clone();
            answer["files"] = json!(files);
            answer["held_back"] = json!(held_back);
            answer["attributed"] = json!(attributed);
            return Ok(answer);
        };
        // A takeover during the share leaves the pull request open with no
        // lease to keep its address: the answer still names it, and says it
        // was not recorded.
        let recorded = {
            let store = self.store.lock().await;
            store
                .set_automation_pr_url(id, lp.as_str(), token, &url)
                .await?
        };
        if !recorded {
            tracing::warn!(
                "the {lp} run on '{domain}' shared {url}, but its lease was taken over meanwhile"
            );
        }
        // What the pull request carries, as the share reports it.
        let carried: Vec<&Value> = ["added", "updated", "deleted"]
            .iter()
            .filter_map(|k| report[*k].as_array())
            .flatten()
            .collect();
        let mut answer = json!({
            "outcome": "shared",
            "url": url,
            "files": carried,
            "held_back": held_back,
            "attributed": attributed,
            "recorded": recorded,
        });
        // The team marker the share added, with its reason, so a run's
        // report says why the MANIFEST is among its files.
        if let Some(marker) = receipt.get("team_marker") {
            answer["team_marker"] = marker.clone();
        }
        Ok(answer)
    }

    /// The newest `limit` runs of `lp` on `domain`, newest first:
    /// `{domain, loop, runs}`. The items a run left are its own
    /// bookkeeping and stay out; `left` counts them.
    pub async fn automation_runs(
        &self,
        lp: Loop,
        domain: &str,
        limit: usize,
        scope: &Scope,
    ) -> Result<Value> {
        self.require_loop_right(lp, domain, scope).await?;
        let id = self.loop_domain_id(domain).await?;
        let runs = {
            let store = self.store.lock().await;
            store
                .automation_runs(id, lp.as_str(), limit.clamp(1, RUNS_KEPT))
                .await?
        };
        let runs: Vec<Value> = runs.iter().map(run_json).collect();
        Ok(json!({ "domain": domain, "loop": lp, "runs": runs }))
    }

    /// Read `at` as now in the automation verbs, or the real clock again
    /// with `None`.
    #[cfg(any(test, feature = "testing"))]
    pub fn set_automation_now(&self, at: Option<DateTime<FixedOffset>>) {
        *self.automation_now.lock().unwrap() = at;
    }

    /// Now, for the automation verbs: the test clock when one is set.
    fn automation_clock(&self) -> DateTime<FixedOffset> {
        #[cfg(any(test, feature = "testing"))]
        if let Some(at) = *self.automation_now.lock().unwrap() {
            return at;
        }
        now_offset()
    }

    /// `(now, stale_before)` in the one RFC 3339 spelling every lease time
    /// is written in, so the store compares them as strings.
    fn automation_window(&self) -> (String, String) {
        let now = self.automation_clock();
        let stale_before = now - Duration::seconds(LEASE_STALE_SECS);
        (now.to_rfc3339(), stale_before.to_rfc3339())
    }

    /// Refuse a caller who may not run `lp` on `domain`: reflect needs the
    /// right to curate its Reflection, evolve to write its knowledge. The
    /// right alone: where the loop runs is the status's to say.
    async fn require_loop_right(&self, lp: Loop, domain: &str, scope: &Scope) -> Result<()> {
        match lp {
            Loop::Reflect => {
                let (right, _) = self
                    .caller_reflection_right(domain, &loop_caller(scope))
                    .await?;
                if right < ReflectionRight::Curate {
                    return Err(EngineError::Forbidden(format!(
                        "your access to the Reflection of '{domain}' is {}, and curate is needed to run its reflect loop",
                        crate::scope::reflection_right_word(right)
                    )));
                }
            }
            Loop::Evolve => {
                self.require_domain(domain, scope).await?;
                if let Some(e) = self.loop_write_refusal(domain).await {
                    return Err(e);
                }
                if !self
                    .writable_domains(scope)
                    .await?
                    .iter()
                    .any(|d| d == domain)
                {
                    return Err(EngineError::Forbidden(format!(
                        "you may not write to '{domain}', so you may not run its evolve loop"
                    )));
                }
            }
        }
        Ok(())
    }

    /// Every domain `scope` may run `lp` on: reflect, the domains with
    /// Reflection switched on that it curates (wherever their home is), not
    /// read-only and not a local copy a connected server replaces; evolve,
    /// the domains it may write.
    async fn loop_domains(&self, lp: Loop, scope: &Scope) -> Result<Vec<String>> {
        match lp {
            Loop::Evolve => self.writable_domains(scope).await,
            Loop::Reflect => {
                let hidden = self.hidden_for(scope).await?;
                let caller = loop_caller(scope);
                // Like the unnamed reflect queue: no local copy a connected
                // server replaces, and no read-only domain.
                let shadowed = self.shadowed_domains();
                let mut names = self.known_domain_names();
                names.retain(|name| !hidden.contains(name) && !shadowed.contains(name));
                names.sort();
                names.dedup();
                let mut out = Vec::new();
                for name in names {
                    if self.reflection_switch(&name) == crystalline_core::ReflectionSwitch::Off
                        || self.writes_refused(&name).await
                    {
                        continue;
                    }
                    if matches!(
                        self.caller_reflection_right(&name, &caller).await,
                        Ok((right, _)) if right >= ReflectionRight::Curate
                    ) {
                        out.push(name);
                    }
                }
                Ok(out)
            }
        }
    }

    /// Where `lp` on `domain` runs. Reflect runs on the Reflection's home
    /// ([`Engine::reflection_home_here`]); evolve on the instance that holds
    /// the domain's host lock, which off a shared database is this one.
    /// Why a loop may not run on `domain` because nothing may be written
    /// there ([`Engine::writes_refused`]), or `None`. A locked domain is
    /// answered as before; an archived repository with its own sentence.
    async fn loop_write_refusal(&self, domain: &str) -> Option<EngineError> {
        if self.domain_is_read_only(domain).await {
            return Some(EngineError::DomainReadOnly(domain.to_string()));
        }
        self.github_archived_repo(domain)
            .map(|repo| EngineError::DomainReadOnly(super::archived_sentence(domain, &repo)))
    }

    async fn loop_runs(&self, lp: Loop, domain: &str, scope: &Scope) -> Runs {
        // An instance started read-only writes nothing, so a run there
        // could do nothing.
        if self.read_only {
            return Runs::Nowhere(EngineError::ReadOnly);
        }
        match lp {
            Loop::Reflect => {
                if self.reflection_home_here(domain).await {
                    return Runs::Here;
                }
                match self.reflection_state(domain).await {
                    Ok(crystalline_core::reflection::ReflectionState::Elsewhere { url }) => {
                        Runs::On(url)
                    }
                    Ok(crystalline_core::reflection::ReflectionState::HostedElsewhere {
                        label: Some(label),
                    }) => Runs::On(label),
                    _ => Runs::Nowhere(
                        match self
                            .require_reflection(
                                domain,
                                &loop_caller(scope),
                                ReflectionRight::Curate,
                            )
                            .await
                        {
                            Err(e) => e,
                            Ok(_) => match self.loop_write_refusal(domain).await {
                                Some(e) => e,
                                None => EngineError::Refused(format!(
                                    "the Reflection of '{domain}' cannot run here right now; try again later"
                                )),
                            },
                        },
                    ),
                }
            }
            Loop::Evolve => {
                if let Some(e) = self.loop_write_refusal(domain).await {
                    return Runs::Nowhere(e);
                }
                // The home first, then the host lock among the instances that
                // are the home: instances of one shared database usually
                // serve one address, so they all are, and the lock picks one.
                // Where the home names another address than the lock holder
                // has, nobody in that group runs, and each names the other.
                if let Some(home) = self.evolve_home_elsewhere(domain) {
                    return Runs::On(home);
                }
                if !self.store_is_shared().await {
                    return Runs::Here;
                }
                let store = self.store.lock().await;
                let host = match store.domain_id(domain).await {
                    Ok(Some(id)) => store.domain_host(id).await,
                    Ok(None) => Ok(None),
                    Err(e) => Err(e),
                };
                drop(store);
                match host {
                    Ok(Some(host))
                        if !host.instance_id.is_empty() && host.instance_id != self.instance_id =>
                    {
                        Runs::On(if host.label.is_empty() {
                            host.instance_id
                        } else {
                            host.label
                        })
                    }
                    Ok(_) => Runs::Here,
                    Err(e) => Runs::Nowhere(e.into()),
                }
            }
        }
    }

    /// The `evolve_home` the MANIFEST of `domain` names, when the domain is
    /// a team domain and the home is not this instance: the loop runs there,
    /// and this clone refuses a scheduled run. A team domain is one connected
    /// to GitHub here, or a plain clone that carries the team marker, which
    /// could carry a run's changes to the team with a git push. A local or a
    /// virtual domain ignores the key, and so does a MANIFEST whose value is
    /// not an address (verify's `M013`).
    fn evolve_home_elsewhere(&self, domain: &str) -> Option<String> {
        let ContentSource::File { root } = self.read_source(domain) else {
            return None;
        };
        let home = crystalline_core::evolve_home_at(&root)?;
        let team = self
            .domain_entry(domain)
            .is_ok_and(|entry| entry.origin.is_some())
            || crystalline_core::team_at(&root).is_some();
        (team && self.own_public_url().as_deref() != Some(home.as_str())).then_some(home)
    }

    /// Claim the evolve home of `domain` for this instance, for the evolve
    /// run that just took its lease: `(home, note)`.
    ///
    /// Crystalline never starts a program, so it has no switch that says
    /// "evolve is scheduled here". The lease take is the one step every
    /// scheduled runner passes through (`evolve run`, `watch`, a container
    /// job, n8n, a run a webhook started), which makes it the moment an
    /// instance learns that it runs evolve. Only a domain connected to
    /// GitHub here with no `evolve_home` is claimed, and only by an instance
    /// with a `service.public_url`: without one it cannot be named, and the
    /// note says every clone may still run evolve.
    ///
    /// The line is written as the runner, through the edit path (a draft on
    /// a domain that reviews changes), and noted as the run's write, so the
    /// run's own pull request carries it and the merge makes it every
    /// clone's. A MANIFEST whose `evolve_home` is not an address is left to
    /// its owner (`M013`), as the team marker leaves an unknown `team`.
    /// A failed write never fails the take: the run goes on, and the note
    /// says why there is no home.
    async fn claim_evolve_home(
        &self,
        domain: &str,
        scope: &Scope,
    ) -> (Option<String>, Option<String>) {
        let connected = self.config.read().unwrap().github_enabled()
            && self
                .domain_entry(domain)
                .is_ok_and(|entry| entry.origin.is_some());
        if !connected {
            return (None, None);
        }
        let ContentSource::File { root } = self.read_source(domain) else {
            return (None, None);
        };
        let declared = std::fs::read_to_string(root.join("MANIFEST.md"))
            .ok()
            .and_then(|source| {
                let engram = parse_engram(&source).ok()?;
                Manifest::from_engram(&engram, &source)
                    .declared_evolve_home()
                    .map(str::to_string)
            });
        if let Some(declared) = declared {
            if crystalline_core::validate_evolve_home(&declared).is_ok() {
                // The loop runs here, so the home is this instance already.
                return (None, None);
            }
            return (
                None,
                Some(format!(
                    "'{domain}' names an evolve_home that is not a server address (`{declared}`, verify reports it as M013), so every clone of it may run evolve. Remove the line, and the next scheduled evolve run at an instance with service.public_url claims the home."
                )),
            );
        }
        if self.evolve_claim_declined(domain) {
            return (
                None,
                Some(format!(
                    "'{domain}' has no evolve_home yet, so every clone of it may run evolve. This instance does not claim it again on its own (see crystalline doctor)."
                )),
            );
        }
        let Some(own) = self
            .own_public_url()
            .filter(|own| crystalline_core::validate_evolve_home(own).is_ok())
        else {
            return (
                None,
                Some(format!(
                    "'{domain}' has no evolve_home yet, so every clone of it may run evolve. This instance has no service.public_url, so it cannot become the home: the first scheduled evolve run at an instance with one claims it."
                )),
            );
        };
        let edit = vec![super::domain_settings::PolicyEdit::Set {
            key: crystalline_core::EVOLVE_HOME_KEY.to_string(),
            value: own.clone(),
        }];
        if let Err(e) = self.write_engine_policies(domain, edit, scope).await {
            tracing::warn!("claiming the evolve home of '{domain}': {e}");
            return (
                None,
                Some(format!(
                    "'{domain}' has no evolve_home yet, so every clone of it may run evolve. Writing this instance as its home failed: {e}"
                )),
            );
        }
        self.note_claim_written(domain, scope).await;
        (
            Some(own.clone()),
            Some(format!(
                "This instance is now the evolve home of '{domain}' (evolve_home: {own}). The run's pull request carries the MANIFEST change, or the next run's when this run shares nothing; once it is merged, other clones of the domain refuse a scheduled evolve run."
            )),
        )
    }

    /// Note the MANIFEST as the run's own write, for a runner with an
    /// account, so its attributed share carries the evolve home's line.
    async fn note_claim_written(&self, domain: &str, scope: &Scope) {
        if let Some(account) = scope_account(scope)
            && let Err(e) = self
                .note_written(domain, &account, &[MANIFEST_FILE.to_string()])
                .await
        {
            tracing::warn!("noting the evolve home of '{domain}' for its run: {e}");
        }
    }

    /// The record that the team declined this instance's claim of the
    /// evolve home of `domain`, in its origin state folder.
    pub(crate) fn evolve_claim_declined_path(&self, domain: &str) -> Option<PathBuf> {
        self.origin_state_dir(domain)
            .ok()
            .map(|dir| dir.join(crystalline_remote::state::EVOLVE_HOME_DECLINED_FILE))
    }

    /// Whether the team declined this instance's claim of the evolve home of
    /// `domain` and nobody took the record away since.
    fn evolve_claim_declined(&self, domain: &str) -> bool {
        self.evolve_claim_declined_path(domain)
            .is_some_and(|path| path.is_file())
    }

    /// Withdraw this instance's unshared claim of the evolve home of
    /// `domain` when the team closed the pull request that carried it
    /// without merging: the MANIFEST goes back to the team's copy, and the
    /// decline is recorded, so the instance stops acting as the home and
    /// does not claim it again on its own. Answers the withdrawn address.
    ///
    /// The pull request carried the claim when it lists the MANIFEST with
    /// exactly the bytes the folder still holds; a MANIFEST changed since is
    /// a person's, and stays.
    async fn withdraw_declined_evolve_claim(&self, domain: &str) -> Option<String> {
        let own = self.pending_evolve_claim(domain)?;
        let dir = self.origin_state_dir(domain).ok()?;
        let state = crystalline_remote::state::OriginState::load(&dir).ok()??;
        let ContentSource::File { root } = self.read_source(domain) else {
            return None;
        };
        let current = std::fs::read(root.join(MANIFEST_FILE)).ok()?;
        let sha = sha256_hex(&current);
        let declined = state
            .proposals
            .iter()
            .chain(state.history.iter())
            .filter(|p| p.status == crystalline_remote::state::ProposalStatus::Declined)
            .flat_map(|p| p.files.iter())
            .any(|file| file.path == MANIFEST_FILE && file.sha256.as_deref() == Some(&sha));
        if !declined {
            return None;
        }
        let base = std::fs::read(dir.join("base").join(MANIFEST_FILE)).ok()?;
        if let Err(e) = std::fs::write(root.join(MANIFEST_FILE), &base).and_then(|()| {
            std::fs::write(
                dir.join(crystalline_remote::state::EVOLVE_HOME_DECLINED_FILE),
                &own,
            )
        }) {
            tracing::warn!("withdrawing the declined evolve home claim of '{domain}': {e}");
            return None;
        }
        if let Err(e) = self.sync(Some(domain)).await {
            tracing::warn!("syncing '{domain}' after withdrawing its evolve home claim: {e}");
        }
        Some(own)
    }

    /// This instance's address, when the MANIFEST of `domain` claims the
    /// evolve home for it and the claim is not in the team's copy yet: the
    /// folder's MANIFEST names this instance, the base snapshot's does not,
    /// and the two differ in nothing else but the `generated` stamp the
    /// edit path writes. A MANIFEST with a person's own unshared edit is not
    /// a pending claim, so the run holds it back as their work. A domain
    /// that reviews changes keeps the claim in the runner's draft, which the
    /// next take writes again (the folder has no home), so it needs no help.
    fn pending_evolve_claim(&self, domain: &str) -> Option<String> {
        let connected = self.config.read().unwrap().github_enabled()
            && self
                .domain_entry(domain)
                .is_ok_and(|entry| entry.origin.is_some());
        if !connected {
            return None;
        }
        let own = self.own_public_url()?;
        let ContentSource::File { root } = self.read_source(domain) else {
            return None;
        };
        let current = std::fs::read_to_string(root.join(MANIFEST_FILE)).ok()?;
        let current = crystalline_core::to_lf(&current).into_owned();
        if crystalline_core::evolve_home_of_source(&current).as_deref() != Some(own.as_str()) {
            return None;
        }
        let base_dir = self.origin_state_dir(domain).ok()?.join("base");
        let base = std::fs::read_to_string(base_dir.join(MANIFEST_FILE)).ok()?;
        let base = crystalline_core::to_lf(&base).into_owned();
        if crystalline_core::evolve_home_of_source(&base).as_deref() == Some(own.as_str()) {
            return None;
        }
        let bare = |text: &str| {
            let text = remove_frontmatter_field(text, crystalline_core::EVOLVE_HOME_KEY);
            remove_frontmatter_field(&text, "generated")
        };
        crystalline_core::frontmatter::same_content(&bare(&current), &bare(&base)).then_some(own)
    }

    /// The index row of `domain`, which the lease and the runs hang on.
    async fn loop_domain_id(&self, domain: &str) -> Result<DomainId> {
        let store = self.store.lock().await;
        store.domain_id(domain).await?.ok_or_else(|| {
            EngineError::Invalid(format!(
                "domain '{domain}' is not indexed yet; try again after its first sync"
            ))
        })
    }

    /// The lease of `lp` on `domain` when `token` holds it, a stale one
    /// included; refused otherwise.
    async fn held_lease(
        &self,
        lp: Loop,
        domain: &str,
        id: DomainId,
        token: &str,
    ) -> Result<AutomationLease> {
        let lease = {
            let store = self.store.lock().await;
            store.automation_lease(id, lp.as_str()).await?
        };
        lease
            .filter(|lease| !token.is_empty() && lease.token == token)
            .ok_or_else(|| lease_lost(lp, domain))
    }

    /// What `lp` would work on `domain` now, every item with its version.
    async fn loop_work(&self, lp: Loop, domain: &str, scope: &Scope) -> Result<Work> {
        match lp {
            Loop::Reflect => {
                let items = self.reflect_due_items(domain, &loop_caller(scope)).await?;
                Ok(Work {
                    items: items.into_iter().map(|due| (due.key, due.page)).collect(),
                    judgment: 0,
                })
            }
            Loop::Evolve => {
                let today = self.automation_clock().date_naive().to_string();
                let swept = self
                    .evolve_findings(
                        &EvolveParams {
                            domains: vec![domain.to_string()],
                            today: Some(today),
                            ..EvolveParams::default()
                        },
                        scope,
                    )
                    .await?;
                // One item per rule, engram and (for a pair rule) pair; the
                // version is the scope an acknowledgment records, which moves
                // only when the evidence does, never with the date. A rule
                // whose scope is empty keeps one constant version, so once a
                // run left it, it stays left until its key changes, as an
                // acknowledgment of it would.
                let mut grouped: BTreeMap<String, Vec<String>> = BTreeMap::new();
                let mut judgment = 0u64;
                for f in &swept.findings {
                    if f.class != Class::Mechanical {
                        judgment += 1;
                        continue;
                    }
                    let mut key = format!("{}:{}:{}", f.rule, f.domain, f.permalink);
                    if crystalline_index::is_pair_scoped(f.rule) && !f.scope.is_empty() {
                        key.push(':');
                        key.push_str(&f.scope);
                    }
                    grouped.entry(key).or_default().push(f.scope.clone());
                }
                let items = grouped
                    .into_iter()
                    .map(|(key, mut scopes)| {
                        scopes.sort();
                        let version = version_of(scopes.iter().map(String::as_str));
                        (
                            DueKey {
                                kind: "finding".to_string(),
                                key,
                                version,
                            },
                            String::new(),
                        )
                    })
                    .collect();
                Ok(Work { items, judgment })
            }
        }
    }

    /// [`Engine::loop_work`] less what a finished run among the last
    /// [`RUNS_KEPT`] left for a person at the same version.
    async fn loop_due(&self, lp: Loop, domain: &str, id: DomainId, scope: &Scope) -> Result<Due> {
        let work = self.loop_work(lp, domain, scope).await?;
        let runs = {
            let store = self.store.lock().await;
            store.automation_runs(id, lp.as_str(), RUNS_KEPT).await?
        };
        let left: Vec<&DueKey> = runs.iter().flat_map(|run| run.left_items.iter()).collect();
        let total = work.items.len();
        let items: Vec<(DueKey, String)> = work
            .items
            .into_iter()
            .filter(|(item, _)| !left.iter().any(|l| same_item(l, item)))
            .collect();
        Ok(Due {
            left: total - items.len(),
            items,
            judgment: work.judgment,
        })
    }

    /// One domain's entry in [`Engine::automation_status`].
    ///
    /// `counted` is whether the caller may run the loop: only then are the
    /// due items worked out (they need the right to curate the Reflection or
    /// to write the knowledge). Without it the entry still shows the lease,
    /// the last run and the delivery, and counts nothing. `force` skips the
    /// backoff after failed runs, and nothing else.
    async fn loop_status(
        &self,
        lp: Loop,
        domain: &str,
        scope: &Scope,
        counted: bool,
        force: bool,
    ) -> Result<Value> {
        let runs_on = self.loop_runs(lp, domain, scope).await;
        let id = {
            let store = self.store.lock().await;
            store.domain_id(domain).await?
        };
        let (lease, runs) = match id {
            Some(id) => {
                let store = self.store.lock().await;
                (
                    store.automation_lease(id, lp.as_str()).await?,
                    store.automation_runs(id, lp.as_str(), RUNS_KEPT).await?,
                )
            }
            None => (None, Vec::new()),
        };
        let last_run = runs.first();
        // The wait after failed runs in a row, while it lasts.
        let backing_off = match failed_in_a_row(&runs) {
            (failed, Some(ended)) if failed > 0 => {
                let now = self.automation_clock();
                // A failure "later" than now (the clock went back) starts no
                // wait, so a wait never outlasts its own length.
                DateTime::parse_from_rfc3339(ended)
                    .ok()
                    .filter(|ended| *ended <= now)
                    .filter(|ended| now < *ended + Duration::seconds(backoff_secs(failed)))
                    .map(|_| failed)
            }
            _ => None,
        };
        let (_, stale_before) = self.automation_window();
        let running = lease.filter(|lease| lease.renewed_at.as_str() >= stale_before.as_str());
        let due = match (&runs_on, id) {
            (Runs::Here, Some(id)) if counted => Some(self.loop_due(lp, domain, id, scope).await?),
            _ => None,
        };
        let counts = match lp {
            Loop::Reflect => {
                let items = due.as_ref().map(|d| d.items.as_slice()).unwrap_or_default();
                let of = |kind: &str| items.iter().filter(|(k, _)| k.kind == kind).count();
                let pages = items
                    .iter()
                    .filter(|(k, page)| k.kind == "update" && !page.is_empty())
                    .map(|(_, page)| page.as_str())
                    .collect::<BTreeSet<_>>()
                    .len();
                json!({
                    "waiting_for_the_agent": of("review"),
                    "feedback": of("feedback"),
                    "updates": of("update"),
                    "pages_to_update": pages,
                })
            }
            Loop::Evolve => json!({
                "mechanical": due.as_ref().map_or(0, |d| d.items.len()),
                "judgment": due.as_ref().map_or(0, |d| d.judgment),
            }),
        };
        let is_due = running.is_none()
            && (force || backing_off.is_none())
            && due.as_ref().is_some_and(|d| !d.items.is_empty());
        // The trigger's delivery only where the MANIFEST turns it on.
        let delivery = if self.loop_trigger(domain, lp).await == crystalline_core::Trigger::Webhook
        {
            json!(self.automation_delivery(domain, lp))
        } else {
            Value::Null
        };
        Ok(json!({
            "domain": domain,
            "loop": lp,
            "due": is_due,
            "counts": counts,
            "left_for_person": due.as_ref().map_or(0, |d| d.left),
            "running": running.as_ref().map(running_json),
            "runs_on": match &runs_on {
                Runs::On(name) => Some(name.as_str()),
                _ => None,
            },
            "reason": match runs_on {
                Runs::Nowhere(_) => Some(NOT_HOSTED_HERE.to_string()),
                _ => backing_off.map(backoff_reason),
            },
            "last_run": last_run.map(run_json),
            "delivery": delivery,
        }))
    }

    /// The unshared files of `domain` with their checksums now, the way a
    /// share sees them: a reviewing domain's drafts of the caller, else the
    /// working tree against the base. Empty for a domain with no origin.
    /// A deleted file has an empty checksum.
    async fn unshared_now(&self, domain: &str, scope: &Scope) -> Result<Vec<PathChecksum>> {
        if !self.config.read().unwrap().github_enabled() || !self.domain_has_origin(domain)? {
            return Ok(Vec::new());
        }
        if self.reviews_changes(domain) {
            let Some(actor) = crate::scope::overlay_actor(scope) else {
                return Ok(Vec::new());
            };
            let (_, root, _) = self.origin_spec_for_domain(domain)?;
            return Ok(self
                .overlay_local_changes(domain, &root, &actor)
                .await?
                .into_iter()
                .map(|(path, _, bytes)| PathChecksum {
                    path,
                    checksum: bytes.map(|b| sha256_hex(&b)).unwrap_or_default(),
                })
                .collect());
        }
        let (_, _, _, local) =
            self.team_local_changes(domain, self.reflection_mode(domain).await?)?;
        Ok(local
            .substantive()
            .map(|change| PathChecksum {
                path: change.path().to_string(),
                checksum: match change {
                    crystalline_remote::changes::LocalChange::Added { sha256, .. }
                    | crystalline_remote::changes::LocalChange::Modified { sha256, .. } => {
                        sha256.clone()
                    }
                    crystalline_remote::changes::LocalChange::Deleted { .. } => String::new(),
                },
            })
            .collect())
    }
}

/// What one loop's trigger webhook last did on one domain, for the status
/// answer and the policies card.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct Delivery {
    /// When the last webhook was taken by its receiver, RFC 3339.
    pub last_sent: Option<String>,
    /// Why the last delivery failed; cleared by the next one that works.
    pub last_error: Option<String>,
    /// Whether this instance has a webhook URL and a secret to sign with.
    pub configured: bool,
}

/// What the engine keeps of a [`Delivery`]: everything but `configured`,
/// which is read from the config each time.
#[derive(Debug, Clone, Default)]
pub(super) struct DeliveryRecord {
    last_sent: Option<String>,
    last_error: Option<String>,
}

/// The instance's trigger webhook as the settings give it:
/// `automation.webhook.url` and `automation.webhook.secret`, the
/// environment overlay applied.
#[derive(Clone)]
pub struct WebhookTarget {
    /// The address, as set; the sender parses it before it posts.
    pub url: String,
    /// The signing secret, when one is set.
    pub secret: Option<String>,
}

/// Hand-written so the secret cannot reach a log line, nor the URL's
/// userinfo, path or query (a webhook path often works as a bearer): the
/// URL shows only its scheme and host.
impl std::fmt::Debug for WebhookTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let origin = match url::Url::parse(&self.url) {
            Ok(url) => format!("{}://{}", url.scheme(), url.host_str().unwrap_or("?")),
            Err(_) => "(not a URL)".to_string(),
        };
        f.debug_struct("WebhookTarget")
            .field("url", &origin)
            .field("secret", &self.secret.as_ref().map(|_| "(set)"))
            .finish()
    }
}

impl Engine {
    /// The instance's trigger webhook, or `None` when no URL is set.
    pub fn webhook_target(&self) -> Option<WebhookTarget> {
        let config = self.config.read().unwrap();
        let webhook = config.automation.as_ref()?.webhook.as_ref()?;
        let url = webhook
            .url
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())?;
        Some(WebhookTarget {
            url: url.to_string(),
            secret: webhook
                .secret
                .as_deref()
                .filter(|s| !s.is_empty())
                .map(str::to_string),
        })
    }

    /// How many mechanical findings make the evolve trigger send
    /// (`automation.evolve_threshold`).
    pub fn evolve_threshold(&self) -> u64 {
        self.config.read().unwrap().automation_evolve_threshold()
    }

    /// The trigger the MANIFEST of `domain` sets for `lp`; off when it sets
    /// none or the MANIFEST cannot be read. A virtual domain's MANIFEST is
    /// read from the database.
    pub async fn loop_trigger(&self, domain: &str, lp: Loop) -> crystalline_core::Trigger {
        let Ok(source) = self.manifest_markdown(domain).await else {
            return crystalline_core::Trigger::Off;
        };
        let Ok(engram) = parse_engram(&source) else {
            return crystalline_core::Trigger::Off;
        };
        Manifest::from_engram(&engram, &source).trigger(lp)
    }

    /// Every registered domain whose MANIFEST sets `<lp>_trigger: webhook`,
    /// sorted.
    pub async fn webhook_domains(&self, lp: Loop) -> Vec<String> {
        let mut names = self.known_domain_names();
        names.sort();
        names.dedup();
        let mut out = Vec::new();
        for name in names {
            if self.loop_trigger(&name, lp).await == crystalline_core::Trigger::Webhook {
                out.push(name);
            }
        }
        out
    }

    /// Whether a fresh lease of `lp` is held on `domain`: one read of the
    /// lease row, without the work a status computes.
    pub async fn automation_lease_fresh(&self, domain: &str, lp: Loop) -> Result<bool> {
        let store = self.store.lock().await;
        let Some(id) = store.domain_id(domain).await? else {
            return Ok(false);
        };
        let lease = store.automation_lease(id, lp.as_str()).await?;
        drop(store);
        let (_, stale_before) = self.automation_window();
        Ok(lease.is_some_and(|lease| lease.renewed_at.as_str() >= stale_before.as_str()))
    }

    /// Whether `scope` may be a loop run at work right now: it holds a live
    /// lease of either loop on any domain. A run's account holds its lease;
    /// a run as the machine owner holds one with no account, and on a local
    /// session the owner and its run look alike, so while such a lease is
    /// live every owner session counts as the run. Asked before a person is
    /// asked to share text on purpose (`super::disclosure`): a run never
    /// confirms, and an account that runs loops confirms through Fluid while
    /// its run is going. The anonymous caller has nobody behind it and counts
    /// as a run.
    pub async fn acts_for_a_run(&self, scope: &Scope) -> Result<bool> {
        if matches!(scope, Scope::Anonymous) {
            return Ok(true);
        }
        Ok(!self.run_domains(scope).await?.is_empty())
    }

    /// The domains `scope` holds a live lease of either loop on: what a run
    /// as `scope` works on now. For the machine owner, the leases held with
    /// no account. Sorted.
    pub async fn run_domains(&self, scope: &Scope) -> Result<Vec<String>> {
        let mut out: Vec<String> = self
            .run_leases(scope)
            .await?
            .into_iter()
            .map(|(_, domain)| domain)
            .collect();
        out.dedup();
        Ok(out)
    }

    /// The live leases `scope` holds, as (loop, domain), sorted by domain:
    /// [`Engine::run_domains`] with the loop each one runs.
    pub async fn run_leases(&self, scope: &Scope) -> Result<Vec<(Loop, String)>> {
        let (_, stale_before) = self.automation_window();
        let mut domains: Vec<String> = self.registered_domain_entries().keys().cloned().collect();
        domains.sort();
        let store = self.store.lock().await;
        let mut out = Vec::new();
        for domain in domains {
            let Some(id) = store.domain_id(&domain).await? else {
                continue;
            };
            for lp in [Loop::Reflect, Loop::Evolve] {
                let Some(lease) = store.automation_lease(id, lp.as_str()).await? else {
                    continue;
                };
                if lease.renewed_at.as_str() < stale_before.as_str() {
                    continue;
                }
                let held = match scope {
                    Scope::User { account, .. } => {
                        lease.holder.account.as_deref() == Some(account.as_str())
                    }
                    Scope::Unrestricted => lease.holder.account.is_none(),
                    Scope::Anonymous | Scope::Agent { .. } => false,
                };
                if held {
                    out.push((lp, domain.clone()));
                }
            }
        }
        Ok(out)
    }

    /// The domains a request by `scope` may not read because it is
    /// unattended (`crate::read_limit`), or `None` when it is attended.
    ///
    /// A request is unattended when its session says so (`marked`: started
    /// with `crystalline mcp --unattended`, which every headless loop run's
    /// MCP config carries, or named a run domain) or when `scope` holds a
    /// live loop lease ([`Engine::run_domains`]; for the machine owner any
    /// lease held with no account, since the owner and the owner's own run
    /// look alike on a local session). It may read a domain X only when
    /// every reader of the domain D it works on may read X
    /// (`readers_cover(D, X)`, the GitHub rule included). D is:
    ///
    /// * `run_domain`, the domain the run started this session for, while
    ///   `scope` holds a live lease on it (a name without that lease counts
    ///   for nothing, so it can only ever narrow);
    /// * otherwise each domain `scope` holds a live lease on, and X must be
    ///   covered for all of them (the intersection);
    /// * otherwise every registered domain: a session marked unattended with
    ///   no lease may write anywhere, so it reads only what every domain's
    ///   readers may read (in practice the domains everyone signed in may
    ///   read, and no local domain while any domain is backed up to GitHub).
    ///
    /// It writes only into the domains D it works on (the named one, or each
    /// leased one); with no lease it may write wherever its rights allow,
    /// since every reader of every domain may read what it reads. Fails
    /// safe: an error hides every domain and allows no write.
    pub async fn read_limit_for(
        &self,
        scope: &Scope,
        marked: bool,
        run_domain: Option<&str>,
    ) -> Option<crate::read_limit::Limit> {
        let all: Vec<String> = self.registered_domain_entries().keys().cloned().collect();
        let works_on = match self.run_domains(scope).await {
            Ok(domains) => domains,
            Err(e) => {
                tracing::warn!("the loop leases were not read; reading nothing: {e}");
                return Some(crate::read_limit::Limit {
                    hidden: all.into_iter().collect(),
                    writes: Some(Vec::new()),
                });
            }
        };
        if works_on.is_empty() && !marked && run_domain.is_none() {
            return None;
        }
        // What the run works on, and so may write into. Without a lease it
        // may write wherever its rights allow: what it reads every reader of
        // every domain may read, so nothing it writes reaches anyone new.
        let (targets, writes) = match run_domain {
            Some(domain) if works_on.iter().any(|d| d == domain) => {
                (vec![domain.to_string()], Some(vec![domain.to_string()]))
            }
            _ if !works_on.is_empty() => (works_on.clone(), Some(works_on)),
            _ => (all.clone(), None),
        };
        let mut hidden = HashSet::new();
        for source in &all {
            for target in &targets {
                if target == source {
                    continue;
                }
                match self.readers_cover(target, source).await {
                    Ok(true) => {}
                    Ok(false) | Err(_) => {
                        hidden.insert(source.clone());
                        break;
                    }
                }
            }
        }
        Some(crate::read_limit::Limit { hidden, writes })
    }

    /// The name this instance shows people: the label its host locks carry,
    /// else the one it was given, else its id.
    pub fn instance_name(&self) -> String {
        if let Some(label) = self.resolved_label.lock().unwrap().clone()
            && !label.is_empty()
        {
            return label;
        }
        if !self.label.is_empty() {
            return self.label.clone();
        }
        self.instance_id.clone()
    }

    /// What the trigger webhook of `lp` last did on `domain`.
    pub fn automation_delivery(&self, domain: &str, lp: Loop) -> Delivery {
        let target = self.webhook_target();
        let configured = target.as_ref().is_some_and(|t| t.secret.is_some());
        // A URL without a secret never sends, and says so from the settings,
        // so the state shows after a restart and before the trigger looks.
        let no_secret = target.as_ref().is_some_and(|t| t.secret.is_none());
        let record = self
            .automation_deliveries
            .lock()
            .unwrap()
            .get(&(domain.to_string(), lp))
            .cloned()
            .unwrap_or_default();
        Delivery {
            last_sent: record.last_sent,
            last_error: record
                .last_error
                .or_else(|| no_secret.then(|| WEBHOOK_NO_SECRET.to_string())),
            configured,
        }
    }

    /// Record how a delivery of the trigger webhook of `lp` on `domain`
    /// ended: `Ok` with the time its receiver took it, which clears the last
    /// error, or `Err` with why it failed, which keeps the last time sent.
    pub fn record_delivery(
        &self,
        domain: &str,
        lp: Loop,
        outcome: std::result::Result<String, String>,
    ) {
        let mut deliveries = self.automation_deliveries.lock().unwrap();
        let record = deliveries.entry((domain.to_string(), lp)).or_default();
        match outcome {
            Ok(sent_at) => {
                record.last_sent = Some(sent_at);
                record.last_error = None;
            }
            Err(error) => record.last_error = Some(error),
        }
    }
}

/// The refusal of a renew, a record or a share whose token no longer holds
/// the lease.
fn lease_lost(lp: Loop, domain: &str) -> EngineError {
    EngineError::Refused(format!(
        "lease is not held: this run's {lp} lease of '{domain}' expired or another run took it"
    ))
}
