//! The Reflection's engine side: the switch, the home check, the one gate
//! every reflection verb passes, and the verbs themselves.

use std::path::PathBuf;

use crystalline_core::reflection as r;
use crystalline_core::reflection::{ReflectionHome, ReflectionState};
use crystalline_core::relink::BodyGate;

use super::fence::{DiskUndo, KeepText, OnRefusal, RowOp};
use super::sources::SourceFacts;
use super::*;
use crate::changes::ReflectionChanged;
use crate::collab::session::{MovedTo, RoomDoc};
use crate::params::WriteReflectionParams;
use crate::reflection_backup::BackupCause;
use crate::scope::{ReflectionCaller, ReflectionRight, Scope, reflection_right_word};

// The words of the gate's state refusals and of the anonymous comment
// refusal. Public so a surface that answers them in its own terms (REST:
// 409 for a state, 401 for the anonymous visitor) matches these and never
// a copy that could drift from them.

/// The start of the refusal for a Reflection that is switched off; the
/// domain's name follows in quotes.
pub const REFUSED_OFF_START: &str = "Reflection is off for ";
/// The pending refusal's words for a caller who reads engrams; the engram
/// paths follow.
pub const REFUSED_PENDING_NAMED: &str = "waits until these engrams leave reflections/";
/// The pending refusal's words for a caller who does not read engrams: no
/// engram is named.
pub const REFUSED_PENDING: &str = "waits until the engrams in reflections/ are moved out";
/// The pending refusal's words for files under `reflections/` that hold the
/// switch although they do not read as engrams now (frontmatter that does
/// not parse, another encoding); the paths follow.
pub const REFUSED_PENDING_UNREADABLE: &str =
    "waits on files in reflections/ that do not read as engrams now";
/// The pending refusal's words for files or folders under `reflections/`
/// that cannot be opened; the paths follow.
pub const REFUSED_PENDING_UNOPENED: &str =
    "waits on files or folders in reflections/ that cannot be opened";
/// The pending refusal's words for a Reflection folder spelled in more than
/// one letter case; the spellings follow.
pub const REFUSED_PENDING_SPELLINGS: &str =
    "waits until only one folder spells reflections/, in any letter case";
/// The refusal for a GitHub-connected domain whose Reflection has no home.
pub const REFUSED_HOMELESS: &str = "this domain is connected to GitHub and its Reflection has no home yet: switch Reflection on in the domain policies at the team server";
/// The words every refusal for a team domain that is not connected here
/// carries: a MANIFEST with the [`crystalline_core::TEAM_KEY`] marker and no
/// origin on this instance, typically a plain clone of the team repository
/// registered as a local domain. The switch-on and the gate both refuse it
/// with one sentence that names the domain before these words.
pub const REFUSED_UNCONNECTED_TEAM: &str = "is a team domain and is not connected here";
/// The start of the refusal for a Reflection held by another instance; its
/// address follows.
pub const REFUSED_ELSEWHERE_START: &str = "this domain's Reflection lives at ";
/// The start of the refusal for a Reflection another instance on this
/// shared database serves, because it holds the domain's host lock; the
/// host's label and the way to move it follow.
pub const REFUSED_HOSTED_START: &str = "this domain's Reflection is served by ";

/// The gate's refusal in a one-shot command on a shared
/// database, when no instance of this machine holds the domain's lock: a
/// command never claims a domain to serve its Reflection.
pub const REFUSED_NOT_BY_A_COMMAND: &str =
    "this domain's Reflection is not served by a command; start the daemon (crystalline serve)";

/// The gate's refusal for a Reflection the instance labelled `label` serves.
/// What a caller who is not signed in reads for the host's name.
pub const ANOTHER_INSTANCE: &str = "another instance";

pub(super) fn hosted_refusal(label: &str) -> EngineError {
    EngineError::Refused(format!(
        "{REFUSED_HOSTED_START}{label}; move it with crystalline serve --take-over"
    ))
}

/// The refusal of a thread write by the anonymous visitor of a public domain.
pub const ANONYMOUS_COMMENT: &str =
    "anonymous visitors read this Reflection and cannot comment; sign in to comment";

/// A write's receipt when the open room raised its edit conflict over it.
/// "Saved" rather than "on disk": an engram edit that went past a room may
/// have landed in a draft or a virtual domain's row.
pub const ROOM_CONFLICT_NOTE: &str = "your write is saved, and it collides with text somebody in the open room has not saved yet: the people in the room are choosing between their text and yours";
/// A read's receipt when somebody has the engram open in a room that cannot
/// save right now: the read answered the saved version, which is where an
/// edit goes too.
pub const ROOM_READ_NOTE: &str = "a person has unsaved text in an editor that cannot save right now; you read the saved version, and your edits go to it";
/// A write's receipt when the open room could not take it in yet.
pub const ROOM_WAITING_NOTE: &str = "your write is saved; the open room has an unresolved change of its own, and the people in it meet yours when they resolve it";

/// Say on a write's receipt what an open room did with the write behind it
/// ([`crate::collab::session::CollabSessions::absorb_doc_change`]): `live`
/// when it merged it in, `file` with `room` and `room_note` when it raised
/// its edit conflict over it or waits behind one of its own. `present` is who
/// is in the room. A reflection's write and an engram edit that went past a
/// room share it, so both say it in the same words.
pub(crate) fn note_room_outcome(
    receipt: &mut Value,
    absorbed: crate::collab::session::Absorbed,
    present: Vec<String>,
) {
    use crate::collab::session::Absorbed;
    receipt["present"] = json!(present);
    match absorbed {
        Absorbed::Merged => receipt["landed"] = json!("live"),
        Absorbed::Conflict => {
            receipt["landed"] = json!("file");
            receipt["room"] = json!("conflict");
            receipt["room_note"] = json!(ROOM_CONFLICT_NOTE);
        }
        Absorbed::Deferred => {
            receipt["landed"] = json!("file");
            receipt["room"] = json!("waiting");
            receipt["room_note"] = json!(ROOM_WAITING_NOTE);
        }
    }
}

/// What the gate hands a reflection verb.
#[derive(Debug, Clone)]
pub(crate) struct ReflectionAccess {
    /// The domain root.
    pub root: PathBuf,
    /// The Reflection's folder, in its spelling on disk
    /// ([`reflection_dir`]), resolved once per verb.
    pub folder: PathBuf,
    /// The domain's index id.
    pub domain_id: DomainId,
    /// The caller's reflection right.
    pub right: ReflectionRight,
    /// Whether the caller may read engrams here; source engrams are stripped
    /// from every answer when not.
    pub engrams_visible: bool,
}

impl ReflectionAccess {
    /// The absolute path of a file inside the Reflection's folder.
    pub(crate) fn abs(&self, inner: &str) -> PathBuf {
        join_rel(&self.folder, inner)
    }
}

/// The facts the switch is screened against, gathered before any write lock.
#[derive(Debug, Clone, Default)]
pub(crate) struct SwitchFacts {
    /// The domain's name, for the refusals that tell a person what to run.
    pub domain: String,
    pub virtual_domain: bool,
    /// The folders in the domain root that spell `reflections` in any letter
    /// case, sorted. More than one cannot be one Reflection: their files
    /// would land on the same rows.
    pub folders: Vec<String>,
    /// The engrams under `reflections/` that wait to move out, sorted: the
    /// files [`crystalline_core::is_pending_engram`] calls pending that read
    /// as engrams.
    pub engrams: Vec<String>,
    /// Files that open and do not read as engrams now, sorted.
    pub broken: Vec<String>,
    /// Files and folders that cannot be opened, sorted.
    pub unopened: Vec<String>,
    pub origin: bool,
    /// This instance's `service.public_url`, canonical.
    pub own_url: Option<String>,
    /// Paths under `reflections/` the team's copy still holds (the origin
    /// base snapshot), sorted. Empty for a domain with no origin.
    pub team_copy: Vec<String>,
    /// The origin base snapshot that exists but cannot be read, with why:
    /// it may hold files under `reflections/`, so the switch-on is refused
    /// (fail closed, as the pending check fails).
    pub team_copy_unreadable: Option<String>,
}

/// How a page move onto a taken target ends its refusal, so a surface can
/// answer it as the conflict it is (409).
pub const TARGET_TAKEN: &str = "; remove it or pick another path";

/// Whether a MANIFEST's text declares `reflection: on`.
pub(crate) fn declares_on(source: &str) -> bool {
    crystalline_core::parse_engram(source)
        .map(|e| crystalline_core::Manifest::from_engram(&e, source).reflection())
        .unwrap_or_default()
        == crystalline_core::ReflectionSwitch::On
}

fn declared_home(source: &str) -> Option<String> {
    let engram = crystalline_core::parse_engram(source).ok()?;
    crystalline_core::Manifest::from_engram(&engram, source)
        .reflection_home()
        .map(str::to_string)
}

/// The whole sentence for a team domain that is not connected here, as the
/// switch-on and the gate both answer it: what is wrong, why, and the two
/// steps that fix it. A team domain's Reflection lives only at its home,
/// the team server, which fills in `reflection_home` and backs it up to its
/// own branch; a copy that is not connected has neither check, so whatever
/// Reflection it kept would travel with a plain git push instead.
pub(crate) fn unconnected_team_refusal(domain: &str) -> String {
    format!(
        "'{domain}' {REFUSED_UNCONNECTED_TEAM}. A team domain's Reflection lives only at its home, the team server. Connect the domain first (crystalline domain add {domain} --origin <owner>/<repo>, or add_domain with repo; configure turns collaboration on), then switch Reflection on in the domain policies at the team server"
    )
}

/// Whether a team domain that is not connected here is about to keep a
/// Reflection: one of `texts` carries the team marker, and the facts say the
/// domain has no origin on this instance. A virtual domain is refused for
/// its own reason first.
fn unconnected_team(facts: &SwitchFacts, texts: &[&str]) -> bool {
    !facts.origin
        && !facts.virtual_domain
        && texts
            .iter()
            .any(|text| crystalline_core::team_of_source(text).is_some())
}

/// The refusal of a switch-on for a virtual domain.
pub(crate) fn virtual_refusal() -> EngineError {
    EngineError::Invalid(
        "Reflection needs a domain kept as files; this domain is virtual".to_string(),
    )
}

/// Screen a MANIFEST change that is about to be written. Only a change that
/// switches Reflection ON is screened; switching it off is always allowed
/// (the files stay where they are).
pub(crate) fn screen_switch(before: &str, after: &str, facts: &SwitchFacts) -> Result<()> {
    if declares_on(before) || !declares_on(after) {
        return Ok(());
    }
    if facts.virtual_domain {
        return Err(virtual_refusal());
    }
    // Before every other check: a team domain that is not connected here
    // never keeps a Reflection, so moving engrams out first would be work
    // for nothing. The text it replaces counts as much as the text it
    // writes, so one edit that drops the marker and switches on is refused
    // too.
    if unconnected_team(facts, &[before, after]) {
        return Err(EngineError::Invalid(unconnected_team_refusal(
            &facts.domain,
        )));
    }
    if facts.folders.len() > 1 {
        let named: Vec<String> = facts.folders.iter().map(|f| format!("{f}/")).collect();
        return Err(EngineError::Invalid(format!(
            "this domain has {} folders that differ only in letter case: {}. Reflection would read them as one folder and their files would collide. Keep one (move the files of the others out), then switch Reflection on",
            facts.folders.len(),
            named.join(", ")
        )));
    }
    let mut held: Vec<String> = Vec::new();
    if !facts.engrams.is_empty() {
        held.push(format!(
            "These engrams live in reflections/, which Reflection would take over: {}. Move them to another folder first (move_engram)",
            facts.engrams.join(", ")
        ));
    }
    if !facts.broken.is_empty() {
        held.push(format!(
            "These files in reflections/ do not read as engrams now: {}. {BROKEN_ADVICE}",
            facts.broken.join(", ")
        ));
    }
    if !facts.unopened.is_empty() {
        held.push(format!(
            "These files or folders in reflections/ cannot be opened: {}. {UNOPENED_ADVICE}",
            facts.unopened.join(", ")
        ));
    }
    if !held.is_empty() {
        return Err(EngineError::Invalid(format!(
            "{}. Then switch Reflection on",
            held.join(". ")
        )));
    }
    if let Some(snapshot) = &facts.team_copy_unreadable {
        return Err(EngineError::Invalid(format!(
            "the record of the team's copy of this domain cannot be read ({snapshot}), so it is unknown whether the team's copy still holds files in reflections/. Repair or remove that record (the next pull writes it again), then switch Reflection on"
        )));
    }
    if !facts.team_copy.is_empty() {
        return Err(EngineError::Invalid(format!(
            "the team's copy of this domain still holds these files in reflections/: {}. Share the change that moves them out first (share_changes), then switch Reflection on",
            facts.team_copy.join(", ")
        )));
    }
    screen_home(after, facts)
}

/// Refuse a MANIFEST edit that switches Reflection on although the facts
/// were not gathered for it, because the switch was on when the edit began
/// and went off before it landed. Sending the edit again screens it.
pub(crate) fn refuse_unscreened_switch_on(before: &str, after: &str) -> Result<()> {
    if declares_on(before) || !declares_on(after) {
        return Ok(());
    }
    Err(EngineError::Conflict(
        "the MANIFEST's reflection switch changed while this edit ran; read it again and send the edit again".to_string(),
    ))
}

/// The home half of [`screen_switch`]: a domain connected to GitHub needs
/// this instance's `service.public_url` as its `reflection_home`. Also the
/// whole screen of the home refill, where the switch is already on.
pub(crate) fn screen_home(after: &str, facts: &SwitchFacts) -> Result<()> {
    // A homeless domain is also a team domain that is not connected here
    // and already says `on`: switching it on again must not pass as a
    // refill, which has no home to fill in for a domain with no origin.
    if unconnected_team(facts, &[after]) {
        return Err(EngineError::Invalid(unconnected_team_refusal(
            &facts.domain,
        )));
    }
    if facts.origin {
        let Some(own) = facts.own_url.as_deref() else {
            return Err(EngineError::Invalid(
                "switch Reflection on at the team server: this instance has no service.public_url, so it cannot be the home of a shared domain's Reflection".to_string(),
            ));
        };
        // The same rule the stored key meets, so a home this instance writes
        // is always one it reads back as its own.
        if let Err(why) = crystalline_core::validate_reflection_home(own) {
            return Err(EngineError::Invalid(format!(
                "this instance's service.public_url cannot be a Reflection's home: {why}"
            )));
        }
        if declared_home(after).as_deref() != Some(own) {
            return Err(EngineError::Invalid(format!(
                "switch Reflection on in the domain policies at the team server, so its home is filled in (reflection_home: {own})"
            )));
        }
    }
    Ok(())
}

/// The guidance sentence of a receipt that names `unsourced_sections`.
const UNSOURCED_GUIDANCE: &str = "These sections name no source engrams: pass sources for each section you built from engrams, so the queue can follow them.";

/// The PACT a freshly switched-on Reflection starts with: the five things
/// the pact agrees on, as headings for people and the agent to fill in. Each
/// heading carries its section id line, the page marker that keeps an
/// unedited pact the Reflection's: read as an engram it would hold the
/// switch pending after a rebuild or a switch-off and on again (R48).
pub(crate) fn pact_starter(domain: &str) -> String {
    let text = format!(
        "---\ntitle: How we write about {domain}\n---\n\n# How we write about {domain}\n\nThis is the agreement between the people and the agents who work on this domain's Reflection. Change it like any page; the agent reads every change.\n\n## Audience\n\n- Who reads these pages, and what they already know.\n\n## Level of detail\n\n- How deep the pages go, and what they leave to the engrams.\n\n## Presentation\n\n- Which diagrams, callouts and tables help these readers, and what to avoid.\n\n## Structure\n\nOne glob per bullet, for example `- guides/*.md`. Write the reasons in a sentence above the list.\n\n- *.md\n\n## Pages\n\n- One bullet per page or space a person asked for: its path or glob, what they asked for in their words, who asked and when.\n"
    );
    let (with_ids, _) = crystalline_core::reflection::assign_ids(
        &text,
        &BTreeSet::new(),
        &mut crystalline_core::reflection::random_section_id,
    );
    with_ids
}

/// What holds a pending switch, sorted by why, so each kind is named in
/// its own words.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct PendingNames {
    /// Engrams to move out.
    pub engrams: Vec<String>,
    /// Files that open but do not read as engrams now: frontmatter that does
    /// not parse, or bytes that are not UTF-8.
    pub broken: Vec<String>,
    /// Files and folders that cannot be opened.
    pub unopened: Vec<String>,
    /// The folder's spellings, when there is more than one.
    pub spellings: Vec<String>,
}

impl PendingNames {
    /// The gate's refusal: one sentence per kind, each in the words a
    /// surface matches it by.
    fn refusal(&self, domain: &str) -> String {
        let mut sentences = Vec::new();
        if !self.engrams.is_empty() {
            sentences.push(format!(
                "Reflection for '{domain}' {REFUSED_PENDING_NAMED}: {}",
                self.engrams.join(", ")
            ));
        }
        if !self.broken.is_empty() {
            sentences.push(format!(
                "Reflection for '{domain}' {REFUSED_PENDING_UNREADABLE}: {}. {BROKEN_ADVICE}",
                self.broken.join(", ")
            ));
        }
        if !self.unopened.is_empty() {
            sentences.push(format!(
                "Reflection for '{domain}' {REFUSED_PENDING_UNOPENED}: {}. {UNOPENED_ADVICE}",
                self.unopened.join(", ")
            ));
        }
        if !self.spellings.is_empty() {
            sentences.push(format!(
                "Reflection for '{domain}' {REFUSED_PENDING_SPELLINGS}: {}. Keep one and move the files of the others out",
                self.spellings.join(", ")
            ));
        }
        if sentences.is_empty() {
            return format!("Reflection for '{domain}' {REFUSED_PENDING}");
        }
        sentences.join(". ")
    }
}

/// What to do with a file that opens and does not read as an engram.
const BROKEN_ADVICE: &str =
    "Fix them so they read again (frontmatter that parses, UTF-8 text), then move them out";
/// What to do with a file or folder that cannot be opened.
const UNOPENED_ADVICE: &str = "Make them readable for Crystalline, then move them out";

/// Sort the paths the pending check names (domain-relative, a folder with a
/// trailing `/`) by why they hold the switch: a folder that is one of two or
/// more spellings of `reflections/`, a folder or file that cannot be opened,
/// a file that opens and does not read as an engram now, or an engram.
/// Blocking file reads: callers on the runtime use [`sort_pending_off_runtime`].
pub(crate) fn sort_pending(root: &Path, names: &[String]) -> PendingNames {
    let spellings = r::reflection_folders(root);
    let mut out = PendingNames::default();
    for name in names {
        if let Some(folder) = name.strip_suffix('/') {
            if spellings.len() > 1 && spellings.iter().any(|s| s == folder) {
                out.spellings.push(name.clone());
            } else {
                out.unopened.push(name.clone());
            }
            continue;
        }
        let Ok(bytes) = std::fs::read(join_rel(root, name)) else {
            out.unopened.push(name.clone());
            continue;
        };
        let reads = String::from_utf8(bytes).is_ok_and(|text| {
            crystalline_core::parse_engram(&crystalline_core::to_lf(&text)).is_ok()
        });
        if reads {
            out.engrams.push(name.clone());
        } else {
            out.broken.push(name.clone());
        }
    }
    out
}

/// [`sort_pending`] on the blocking pool, as [`Engine::pending_engrams`]
/// reads the same files.
async fn sort_pending_off_runtime(root: PathBuf, names: Vec<String>) -> Result<PendingNames> {
    tokio::task::spawn_blocking(move || sort_pending(&root, &names))
        .await
        .map_err(|e| {
            EngineError::Internal(format!(
                "the files in the Reflection folder could not be read: {e}"
            ))
        })
}

impl Engine {
    /// This instance's `service.public_url` in its canonical spelling (the
    /// origin in ascii, default port dropped, the path with no trailing
    /// slash), the spelling [`crystalline_core::validate_reflection_home`]
    /// holds the stored key to, so a home this instance writes compares
    /// equal to the one it reads back. `None` when unset or not a valid base.
    pub(super) fn own_public_url(&self) -> Option<String> {
        crate::settings::public_base(&self.config.read().unwrap()).map(|base| base.to_string())
    }

    /// The engrams under `reflections/` that wait to move out, sorted: the
    /// one predicate [`crystalline_core::is_pending_engram`] asked of the
    /// files the sync asks it of ([`crystalline_index::pending_engrams`]), so
    /// what this names and what keeps the switch pending never differ. The
    /// file reads run on the blocking pool, like [`Engine::reflection_mode`].
    async fn pending_engrams(&self, domain: &str) -> Result<Vec<String>> {
        let ContentSource::File { root } = self.read_source(domain) else {
            return Ok(Vec::new());
        };
        // Every gate asks this, so it asks the index two questions rather
        // than loading every recorded path (R82). With rows recorded and none
        // under the folder (a Reflection in effect keeps its pages out of the
        // engram rows), nothing can be pending and no file is read, unless
        // the folder is spelled twice, which holds the switch whatever the
        // files are. The spelling check lists the root: blocking, like the
        // predicate's file reads. The rows are remembered per domain until
        // a change of them is announced (R83); the files are read fresh.
        let recorded = self.recorded_paths(domain, &root).await?;
        tokio::task::spawn_blocking(move || {
            if recorded.nothing_pending(&root) {
                Vec::new()
            } else {
                crystalline_index::pending_engrams(&root, recorded.under_reflections.iter())
            }
        })
        .await
        .map_err(|e| {
            EngineError::Internal(format!(
                "the engrams in the Reflection folder of '{domain}' could not be read: {e}"
            ))
        })
    }

    /// The facts [`screen_switch`] reads, for one domain. Only a switch reads
    /// them: the team copy comes from the origin's base snapshot on disk,
    /// which the gate of every reflection verb has no need for.
    pub(crate) async fn switch_facts(&self, domain: &str) -> Result<SwitchFacts> {
        let entry = self.domain_entry(domain)?;
        let own_url = self.own_public_url();
        let origin = entry.origin.is_some();
        let ContentSource::File { root } = self.read_source(domain) else {
            return Ok(SwitchFacts {
                domain: domain.to_string(),
                virtual_domain: true,
                origin,
                own_url,
                ..Default::default()
            });
        };
        let folders = r::reflection_folders(&root);
        let named = sort_pending_off_runtime(root, self.pending_engrams(domain).await?).await?;
        // The base snapshot is read for a domain with an origin only, and a
        // missing or unreadable one holds nothing.
        let mut team_copy: Vec<String> = Vec::new();
        let mut team_copy_unreadable = None;
        if origin && let Ok(dir) = self.origin_state_dir(domain) {
            match crystalline_remote::state::OriginState::load(&dir) {
                Ok(Some(state)) => {
                    team_copy = state
                        .files
                        .keys()
                        .filter(|p| crystalline_core::reflection::is_under_reflections(p))
                        .cloned()
                        .collect();
                    team_copy.sort();
                }
                // No snapshot yet: the team's copy was never pulled here.
                Ok(None) => {}
                Err(e) => team_copy_unreadable = Some(format!("{}: {e}", dir.display())),
            }
        }
        Ok(SwitchFacts {
            domain: domain.to_string(),
            virtual_domain: false,
            folders,
            engrams: named.engrams,
            broken: named.broken,
            unopened: named.unopened,
            origin,
            own_url,
            team_copy,
            team_copy_unreadable,
        })
    }

    /// Screen a MANIFEST text about to be written to `domain` by a raw
    /// writer (the MANIFEST editor, a save, a restore, a move onto the
    /// MANIFEST) with [`screen_switch`]. `before` is the text it replaces,
    /// read from the domain when `None`. The facts are gathered only for a
    /// text that switches Reflection on, so every other MANIFEST write costs
    /// one parse.
    pub(crate) async fn screen_manifest_write(
        &self,
        domain: &str,
        before: Option<&str>,
        after: &str,
    ) -> Result<()> {
        if !declares_on(after) {
            return Ok(());
        }
        let before = match before {
            Some(text) => text.to_string(),
            None => self.manifest_source(domain).await?.markdown,
        };
        if declares_on(&before) {
            return Ok(());
        }
        let facts = self.switch_facts(domain).await?;
        screen_switch(&before, after, &facts)
    }

    /// Write `reflections/PACT.md` from the starter when the domain has none.
    /// A folder spelled in another letter case is the Reflection's folder
    /// (the classifier's rule), so the pact goes into it rather than beside
    /// it, and a pact spelled in another case counts as the pact.
    pub(crate) async fn seed_pact(&self, domain: &str) -> Result<()> {
        let ContentSource::File { root } = self.read_source(domain) else {
            return Ok(());
        };
        let folder = reflection_dir(&root);
        let has_pact = std::fs::read_dir(&folder).is_ok_and(|entries| {
            entries.flatten().any(|entry| {
                entry.file_name().to_str().is_some_and(|n| {
                    n.eq_ignore_ascii_case(crystalline_core::reflection::PACT_FILE)
                })
            })
        });
        if has_pact {
            return Ok(());
        }
        write_file(
            &folder.join(crystalline_core::reflection::PACT_FILE),
            &pact_starter(domain),
        )
    }

    /// What this domain's Reflection is, seen from this instance, for a
    /// read: a host lock whose holder went stale is named, not taken over.
    pub async fn reflection_state(&self, domain: &str) -> Result<ReflectionState> {
        self.reflection_state_as(domain, false).await
    }

    /// [`Engine::reflection_state`] for a write when `takes_over`: a host
    /// lock whose holder sent no heartbeat for the stale threshold is taken
    /// over here, as a sync takes it. Only a write by a caller who may make
    /// it asks this; a status read, a listing and a sweep never do.
    pub(crate) async fn reflection_state_as(
        &self,
        domain: &str,
        takes_over: bool,
    ) -> Result<ReflectionState> {
        if self.reflection_switch(domain) == crystalline_core::ReflectionSwitch::Off {
            return Ok(ReflectionState::Off);
        }
        // Asked on every gate, so it reads no snapshot from disk: only the
        // index rows, the files they name, the MANIFEST and the configuration.
        let origin = self.domain_entry(domain)?.origin.is_some();
        let ContentSource::File { root } = self.read_source(domain) else {
            return Ok(ReflectionState::Off);
        };
        // The host lock first (R92): on a shared database only its holder
        // serves the Reflection, whatever the rows say.
        if let Some(label) = self
            .reflection_host_elsewhere(domain, &root, takes_over)
            .await?
        {
            return Ok(ReflectionState::HostedElsewhere { label });
        }
        let engrams = self.pending_engrams(domain).await?;
        if !engrams.is_empty() {
            return Ok(ReflectionState::Pending { engrams });
        }
        // A team domain that is not connected here (its MANIFEST carries
        // the marker) has no home here, whatever its MANIFEST names: its
        // Reflection lives at the team server, which only a connected copy
        // can be. Asked before the home, so a plain clone on the team
        // server's own machine, whose MANIFEST names this very instance, is
        // refused like every other stray copy.
        if !origin && crystalline_core::team_at(&root).is_some() {
            return Ok(ReflectionState::Homeless);
        }
        let declared = crystalline_core::reflection_home_at(&root);
        match (declared, origin) {
            (Some(home), _) if self.own_public_url().as_deref() == Some(home.as_str()) => {
                Ok(ReflectionState::On {
                    home: ReflectionHome::Here,
                })
            }
            (Some(home), _) => Ok(ReflectionState::Elsewhere { url: home }),
            (None, true) => Ok(ReflectionState::Homeless),
            (None, false) => Ok(ReflectionState::On {
                home: ReflectionHome::Local,
            }),
        }
    }

    /// The label of the instance that serves `domain`'s Reflection, when
    /// that is another instance on this shared database (R92): the one whose
    /// id the domain's host lock records, or `Some(None)` while a rename
    /// pauses a domain whose lock nobody holds. `None` when this instance
    /// serves it.
    ///
    /// Read fresh on every call, never through the gate's remembered rows:
    /// the instance's own hosts are asked first, in memory, so the host (and
    /// every engine without an instance id) answers without the store lock;
    /// only a non-host on a shared database locks the store, to learn who
    /// holds the lock. A database that is not shared (Turso) is one
    /// instance's own, so its Reflection is always served here, and saying
    /// so takes no store lock ([`Engine::store_is_shared`]).
    ///
    /// Another instance's live holder keeps the lock, and the refusal names
    /// it and the way out (`--take-over`). A lock nobody holds (none yet, or
    /// the host released it on a graceful shutdown), one recorded under this
    /// instance's own id (a restart before its first claim) and, for a write
    /// (`takes_over`), one whose holder sent no heartbeat for the stale
    /// threshold (a host that crashed, or slept) are claimed here and now,
    /// under the store lock,
    /// the way a plain sync claims them: two instances that ask at once
    /// resolve to one holder, since the claim is one conditional upsert
    /// whose condition the store checks again against the row the winner
    /// wrote. A claim from another instance marks a host move as any
    /// take-over does. An acquired domain is handed to the daemon's
    /// watcher, which arms it and syncs it like any domain this instance
    /// hosts. The claim touches the store, the hosts map and the event bus,
    /// never a room or a file lock, so a room's save may ask it.
    ///
    /// A host that took the lock from this instance is seen here once the
    /// gate ([`Engine::confirm_reflection_host`]) or the heartbeat
    /// ([`Engine::renew_hosts`]) drops the domain from this instance's hosts.
    async fn reflection_host_elsewhere(
        &self,
        domain: &str,
        root: &std::path::Path,
        takes_over: bool,
    ) -> Result<Option<Option<String>>> {
        if self.instance_id.is_empty() || self.hosted.read().unwrap().contains_key(domain) {
            return Ok(None);
        }
        if !self.store_is_shared().await {
            return Ok(None);
        }
        let store = self.store.lock().await;
        let host = match store.domain_id(domain).await? {
            Some(id) => store.domain_host(id).await?,
            None => None,
        };
        // Another instance's live claim stands. One that sent no heartbeat
        // for the stale threshold is taken over below by a write, with the
        // claim a plain sync makes, which decides in the store whether it is
        // stale; a read names it. A command never claims for a Reflection,
        // and a rename claims when it ends, so both keep naming the old
        // holder.
        if let Some(host) = host.as_ref().filter(|host| self.foreign_holder(host))
            && (!takes_over
                || self.is_one_shot()
                || self.is_renaming(domain)
                || self.held_elsewhere(
                    Some(&host.instance_id),
                    Some(&host.heartbeat_at),
                    chrono::Utc::now(),
                ))
        {
            return Ok(Some(Some(host.label.clone())));
        }
        // A one-shot command never claims a domain to serve its Reflection:
        // it holds no lock past a walk and sends no heartbeat, and a read
        // must change no durable state (no lock row, no host-move mark). It
        // serves the Reflection only while this machine holds the lock live:
        // its daemon, an `mcp --embedded` session or another command's walk,
        // all the same disk under the same id, and no other instance can
        // claim the lock from a live holder without `--take-over`. Which of
        // them holds it does not matter and the row cannot tell. Otherwise
        // nobody here serves it, and the gate says to start the daemon.
        if self.is_one_shot() {
            let stale_before = (now_offset() - Duration::seconds(self.stale_secs)).to_rfc3339();
            let daemon_holds = host.is_some_and(|host| {
                host.instance_id == self.instance_id && host.heartbeat_at >= stale_before
            });
            return Ok(if daemon_holds { None } else { Some(None) });
        }
        // A domain a rename has paused is claimed by the sync that ends the
        // rename, under the name it ends with (as `Engine::claim_host`). Until
        // then nobody holds the lock, so nobody serves the Reflection.
        if self.is_renaming(domain) {
            return Ok(Some(None));
        }
        match self.claim_file_host(&*store, domain, root, false).await? {
            HostClaim::Acquired { .. } => {
                drop(store);
                // The claim handed the domain to the watcher, whose
                // catch-up sync walks this disk; a page only the previous
                // host's disk has keeps its row and is reported, it is never
                // written here (Task 50).
                Ok(None)
            }
            HostClaim::HeldByOther(host) => Ok(Some(Some(host.label))),
        }
    }

    /// The refusal naming the host to `caller`: a signed-in caller reads its
    /// label, and how long ago it was last seen when its heartbeat is stale
    /// (a read never takes such a lock over); anybody else reads "another
    /// instance".
    async fn hosted_refusal_to(
        &self,
        domain: &str,
        label: &str,
        caller: &ReflectionCaller,
    ) -> EngineError {
        if !caller.may_comment() {
            return hosted_refusal(ANOTHER_INSTANCE);
        }
        match self.stale_minutes(domain).await {
            Some(minutes) => hosted_refusal(&format!(
                "{label} (last seen {minutes} minute{} ago)",
                if minutes == 1 { "" } else { "s" }
            )),
            None => hosted_refusal(label),
        }
    }

    /// How many minutes ago the holder of `domain`'s host lock sent its
    /// last heartbeat, when that is past the stale threshold. One query,
    /// asked only on a refusal.
    async fn stale_minutes(&self, domain: &str) -> Option<i64> {
        let store = self.store.lock().await;
        let id = store.domain_id(domain).await.ok().flatten()?;
        let host = store.domain_host(id).await.ok().flatten()?;
        drop(store);
        let beat = DateTime::parse_from_rfc3339(&host.heartbeat_at).ok()?;
        let since = chrono::Utc::now().signed_duration_since(beat.with_timezone(&chrono::Utc));
        (since > Duration::seconds(self.stale_secs)).then(|| since.num_minutes().max(1))
    }

    /// Whether `host` records another instance than this one as the holder.
    fn foreign_holder(&self, host: &crystalline_index::DomainHost) -> bool {
        !host.instance_id.is_empty() && host.instance_id != self.instance_id
    }

    /// The host check of a gated verb, a room's save and a backup pass on a
    /// domain this instance remembers hosting: on a shared database the lock
    /// is read again (one query, by the id the hosts map keeps), so a
    /// takeover by another instance takes effect here at once rather than at
    /// the next heartbeat. A lost lock drops the domain from this instance's
    /// hosts, announces the move, and answers the new host's label. Turso
    /// and an engine without an instance id never ask the store.
    pub(crate) async fn confirm_reflection_host(&self, domain: &str) -> Result<Option<String>> {
        if self.instance_id.is_empty() {
            return Ok(None);
        }
        let Some(id) = self.hosted.read().unwrap().get(domain).copied() else {
            return Ok(None);
        };
        // A database of this instance's own has no other host to lose the
        // lock to, and saying so needs no store lock.
        if !self.store_is_shared().await {
            return Ok(None);
        }
        let store = self.store.lock().await;
        let host = store.domain_host(id).await?;
        drop(store);
        let Some(host) = host.filter(|host| self.foreign_holder(host)) else {
            return Ok(None);
        };
        tracing::warn!("lost the host lock for domain '{domain}'; another instance took over");
        if self.hosted.write().unwrap().remove(domain).is_some() {
            self.announce_host_moved(domain);
        }
        Ok(Some(host.label))
    }

    /// The one gate of every reflection verb that reads: right, then
    /// state, then the right this verb needs, in that order. A host lock
    /// whose holder went stale is named, never taken over, by a read.
    pub(crate) async fn require_reflection(
        &self,
        domain: &str,
        caller: &ReflectionCaller,
        need: ReflectionRight,
    ) -> Result<ReflectionAccess> {
        self.require_reflection_as(domain, caller, need, false)
            .await
    }

    /// [`Engine::require_reflection`] for a verb that writes (a page, a
    /// thread, a file, a move, a delete, a settle): a host lock whose holder
    /// went stale is taken over, but only for a caller who has the right
    /// the verb needs and is signed in.
    pub(crate) async fn require_reflection_to_write(
        &self,
        domain: &str,
        caller: &ReflectionCaller,
        need: ReflectionRight,
    ) -> Result<ReflectionAccess> {
        self.require_reflection_as(domain, caller, need, true).await
    }

    async fn require_reflection_as(
        &self,
        domain: &str,
        caller: &ReflectionCaller,
        need: ReflectionRight,
        writes: bool,
    ) -> Result<ReflectionAccess> {
        let (right, engrams_visible) = self.caller_reflection_right(domain, caller).await?;
        let takes_over = writes && right >= need && caller.may_comment();
        match self.reflection_state_as(domain, takes_over).await? {
            ReflectionState::On { .. } => {}
            ReflectionState::Off => {
                return Err(EngineError::Refused(format!(
                    "{REFUSED_OFF_START}'{domain}'; its owner switches it on in the domain policies"
                )));
            }
            ReflectionState::Pending { engrams } if engrams_visible => {
                let root = match self.read_source(domain) {
                    ContentSource::File { root } => Some(root),
                    _ => None,
                };
                let named = match root {
                    Some(root) => sort_pending_off_runtime(root, engrams).await?,
                    None => PendingNames {
                        engrams,
                        ..Default::default()
                    },
                };
                return Err(EngineError::Refused(named.refusal(domain)));
            }
            ReflectionState::Pending { .. } => {
                return Err(EngineError::Refused(format!(
                    "Reflection for '{domain}' {REFUSED_PENDING}"
                )));
            }
            ReflectionState::Homeless if self.domain_has_origin(domain)? => {
                return Err(EngineError::Refused(REFUSED_HOMELESS.to_string()));
            }
            ReflectionState::Homeless => {
                return Err(EngineError::Refused(unconnected_team_refusal(domain)));
            }
            ReflectionState::Elsewhere { url } => {
                return Err(EngineError::Refused(format!(
                    "{REFUSED_ELSEWHERE_START}{url}"
                )));
            }
            ReflectionState::HostedElsewhere { label: Some(label) } => {
                return Err(self.hosted_refusal_to(domain, &label, caller).await);
            }
            ReflectionState::HostedElsewhere { label: None }
                if self.is_one_shot() && !self.is_renaming(domain) =>
            {
                return Err(EngineError::Refused(REFUSED_NOT_BY_A_COMMAND.to_string()));
            }
            ReflectionState::HostedElsewhere { label: None } => {
                return Err(EngineError::Refused(format!(
                    "{REFUSED_HOSTED_START}no instance while '{domain}' is being renamed; try again when the rename is done"
                )));
            }
        };
        if let Some(label) = self.confirm_reflection_host(domain).await? {
            return Err(self.hosted_refusal_to(domain, &label, caller).await);
        }
        if right < need {
            // When the write list is why the right fell short, say so and
            // name the fix.
            if let Some(account) = self
                .reflection_list_refusal(&caller.scope, domain, need)
                .await?
            {
                return Err(EngineError::Forbidden(
                    WriteBlock::WriteList { account }.sentence(domain),
                ));
            }
            return Err(EngineError::Forbidden(format!(
                "your access to the Reflection of '{domain}' is {}, and {} is needed",
                reflection_right_word(right),
                reflection_right_word(need)
            )));
        }
        let ContentSource::File { root } = self.read_source(domain) else {
            return Err(EngineError::Refused(format!(
                "'{domain}' keeps no files for a Reflection"
            )));
        };
        // The state above filled the gate's remembered entry, which holds
        // the row id too: a poll of a domain nothing changed asks the store
        // nothing here.
        let domain_id = match self.remembered_domain_id(domain, &root) {
            Some(id) => id,
            None => self.domain_source(domain).await?.0,
        };
        Ok(ReflectionAccess {
            folder: reflection_dir(&root),
            root,
            domain_id,
            right,
            engrams_visible,
        })
    }

    /// The Reflection's status for this caller: never a refusal on the
    /// state, only on the right (a domain the caller has no business with
    /// answers as unknown).
    pub async fn reflection_status(
        &self,
        domain: &str,
        caller: &ReflectionCaller,
    ) -> Result<Value> {
        let (right, engrams_visible) = self.caller_reflection_right(domain, caller).await?;
        let state = self.reflection_state(domain).await?;
        // The host's name is a machine's hostname or address: only a
        // signed-in caller reads it.
        let shown = |label: &Option<String>| match label {
            Some(_) if !caller.may_comment() => Some(ANOTHER_INSTANCE.to_string()),
            other => other.clone(),
        };
        let (home, home_url, host_label) = match &state {
            ReflectionState::On { home } => (Some(home.as_str()), None, None),
            ReflectionState::Elsewhere { url } => (Some("elsewhere"), Some(url.clone()), None),
            ReflectionState::HostedElsewhere { label } => (None, None, shown(label)),
            ReflectionState::Off | ReflectionState::Pending { .. } | ReflectionState::Homeless => {
                (None, None, None)
            }
        };
        let pending: Vec<String> = match &state {
            ReflectionState::Pending { engrams } if engrams_visible => engrams.clone(),
            ReflectionState::Pending { .. }
            | ReflectionState::Off
            | ReflectionState::Homeless
            | ReflectionState::Elsewhere { .. }
            | ReflectionState::HostedElsewhere { .. }
            | ReflectionState::On { .. } => Vec::new(),
        };
        Ok(json!({
            "domain": domain,
            "enabled": !matches!(state, ReflectionState::Off),
            "state": state.word(),
            "home": home,
            "home_url": home_url,
            "host_label": host_label,
            "right": reflection_right_word(right),
            "engrams_visible": engrams_visible,
            "pending_engrams": pending,
        }))
    }

    /// The caller's reflection right on `domain` and whether it reads the
    /// domain's engrams: the one preamble of the gate and of the status. An
    /// unregistered name answers as one, in the caller's scope, never
    /// "Reflection is off"; a domain the caller may neither read nor reflect
    /// on answers the same way, so a private domain's existence is not
    /// revealed.
    pub(super) async fn caller_reflection_right(
        &self,
        domain: &str,
        caller: &ReflectionCaller,
    ) -> Result<(ReflectionRight, bool)> {
        if self.domain_entry(domain).is_err() {
            self.require_domain(domain, &caller.scope).await?;
        }
        let engrams_visible = self.engrams_visible(&caller.scope, domain).await?;
        // The MCP open tier curates every domain it can see; a private
        // domain stays as invisible to it as to every anonymous caller.
        let right = if caller.open_tier {
            if engrams_visible {
                ReflectionRight::Curate
            } else {
                ReflectionRight::None
            }
        } else {
            self.reflection_right(&caller.scope, domain).await?
        };
        if right == ReflectionRight::None && !engrams_visible {
            // The domain became visible between the two reads (a race): the
            // answer is still the one a hidden domain gets, with the
            // registered list this caller may see.
            self.require_domain(domain, &caller.scope).await?;
            let mut hidden = self.hidden_for(&caller.scope).await?;
            hidden.insert(domain.to_string());
            return Err(self.unknown_domain(domain, &hidden));
        }
        Ok((right, engrams_visible))
    }

    /// Whether `scope` may read engrams in `domain` (`DomainRight::Read` or
    /// more), without an error for a hidden domain.
    ///
    /// The same answer as asking [`Engine::hidden_for`] whether it holds
    /// `domain`, without its store read when this engine knows the name: the
    /// unregistered half only ever names domains the engine does not know,
    /// so for a known one it cannot change the answer. Every Reflection
    /// poll and gate asks this, so a known domain's never waits on the
    /// store lock here.
    pub(crate) async fn engrams_visible(&self, scope: &Scope, domain: &str) -> Result<bool> {
        if self.knows_domain(domain) {
            return Ok(!self.hidden_screens(scope).await?.contains(domain));
        }
        Ok(!self.hidden_for(scope).await?.contains(domain))
    }
}

/// One engram's facts a section's sources are held against.
#[derive(Debug, Clone)]
pub(crate) struct EngramFact {
    pub permalink: String,
    pub title: String,
    pub status: String,
    pub checksum: String,
    /// Seconds since the epoch of the file's last change.
    pub mtime: i64,
}

/// The absolute path of a file inside a domain's `reflections/`, in the
/// folder's spelling on disk (see [`reflection_dir`]). A verb that holds a
/// [`ReflectionAccess`] uses [`ReflectionAccess::abs`], which resolves the
/// folder once; a room, which holds none, uses this.
pub(super) fn reflection_abs(root: &Path, inner: &str) -> PathBuf {
    join_rel(&reflection_dir(root), inner)
}

/// The pages among `pages` (paths inside the Reflection's folder) whose
/// text on disk `rewrite` would change (it answers how many links it
/// rewrites), as the rooms a move screens before anything moves. The disk
/// decides, not the index, since the relink reads the disk too. A page that
/// is gone has nothing to rewrite, and one that cannot be read is left to
/// the relink, which names it in the move's receipt.
pub(super) fn rooms_to_rewrite<'a>(
    access: &ReflectionAccess,
    pages: impl Iterator<Item = &'a str>,
    rewrite: &dyn Fn(&str, &str) -> usize,
) -> Vec<String> {
    pages
        .filter(|inner| {
            matches!(read_opt(&access.abs(inner)), Ok(Some(text)) if rewrite(&text, inner) > 0)
        })
        .map(str::to_string)
        .collect()
}

/// The Reflection's folder under `root`: the one on disk when its name is
/// `reflections` in another letter case (the classifier's rule, so it is the
/// Reflection's folder), else `reflections`. A write never puts a second
/// spelling beside the first.
pub(super) fn reflection_dir(root: &Path) -> PathBuf {
    let name = r::reflection_folders(root)
        .into_iter()
        .next()
        .unwrap_or_else(|| r::REFLECTIONS_DIR.to_string());
    root.join(name)
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// A page without its `reflection:` frontmatter block, which names the
/// source engrams: what a reader who cannot read engrams is answered, what
/// the search looks at and what a live room holds. The rest of the
/// frontmatter stays. A frontmatter that cannot be cut key by key goes
/// whole: such a reader loses a title line, never an engram name. (A write
/// cuts the incoming block with `write_provenance` instead and refuses a
/// frontmatter it cannot cut, because dropping it would lose the writer's
/// other keys: the two cuts differ on purpose.)
pub(crate) fn without_provenance(text: &str) -> String {
    r::write_provenance(text, &r::Provenance::new()).unwrap_or_else(|_| {
        crystalline_core::frontmatter::split_frontmatter(text)
            .map(|parts| parts.body.trim_start_matches(['\n', '\r']).to_string())
            .unwrap_or_else(|| text.to_string())
    })
}

/// A source's state and its engram's title: `current`, `changed` (the
/// engram's checksum moved), `retired` (its status is in the retirement
/// set), `missing` (no such engram any more) or `unavailable` (its domain
/// cannot be checked here: never read as missing, so nobody drops a source
/// because a domain is offline). The one place a source's state is decided:
/// the read answers it and the reflect queue reads it.
pub(crate) fn source_state(src: &r::SourceRef, facts: &SourceFacts) -> (&'static str, String) {
    if facts.unavailable(src) {
        return ("unavailable", String::new());
    }
    match facts.fact(src) {
        None => ("missing", String::new()),
        Some(f) if crystalline_index::is_retired_status(&f.status.to_lowercase()) => {
            ("retired", f.title.clone())
        }
        Some(f) if f.checksum != src.checksum => ("changed", f.title.clone()),
        Some(f) => ("current", f.title.clone()),
    }
}

/// A source as the answers show it; `domain` only for a source of another
/// domain than the page's, by the local name a reader reaches it by here
/// ([`SourceFacts::shown_domain`]).
pub(crate) fn source_view(src: &r::SourceRef, facts: &SourceFacts) -> Value {
    let (state, title) = source_state(src, facts);
    let mut view = json!({ "permalink": src.engram, "title": title, "state": state });
    if let Some(domain) = facts.shown_domain(src) {
        view["domain"] = json!(domain);
    }
    view
}

/// Who a reflection write records as having changed a section.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SectionAuthors {
    /// The names every changed section records.
    pub changed: Vec<String>,
    /// The names a room carries for a heading typed without an id yet,
    /// recorded by the section that gets its id under that heading: the
    /// room's typists have usually left by the close that mints it. Keyed by
    /// the heading text and the section's place, from 0, among the sections
    /// under that heading text that have no id yet, so two new sections
    /// with one heading each name their own typist.
    pub new_by_heading: BTreeMap<(String, usize), BTreeSet<String>>,
    /// Record "someone" for a changed or newly id'd section whose list
    /// comes out empty. A room sets it when nobody is connected.
    pub someone_if_empty: bool,
}

/// The name a section records when it changed and nobody can be named.
pub(crate) const SOMEONE: &str = "someone";

impl SectionAuthors {
    /// `names` for every changed section, nothing carried by heading, and
    /// an empty list recording nobody.
    pub fn plain(names: Vec<String>) -> SectionAuthors {
        SectionAuthors {
            changed: names,
            ..SectionAuthors::default()
        }
    }
}

/// The sentence a write receipt carries when the writer made up an id line.
const WRITTEN_ID_GUIDANCE: &str =
    "Crystalline gives a new heading its id line; never write one yourself.";

/// The sentence a write receipt carries when the writer made up the
/// bookkeeping block.
const WRITTEN_BLOCK_GUIDANCE: &str = "The reflection: frontmatter key is Crystalline's bookkeeping; the one written here was replaced. Never write it: pass sources instead.";

/// Rules 1 to 4 of a reflection write: the engine's bookkeeping block
/// replaces the incoming one, a heading that lost its id line gets it back
/// by heading, headings get new ids when `assign_ids`, changed sections
/// record each of `authors.changed` (one entry each, the on-disk list) and a
/// section that just got its id also the names carried for its heading,
/// records of vanished sections go. Answers a [`Composed`]. A live room
/// composes through it too.
pub(super) fn compose_reflection(
    current: Option<&str>,
    baseline: &str,
    incoming: &str,
    authors: &SectionAuthors,
    assign_ids: bool,
    now: &str,
) -> Result<Composed> {
    let stored = current.map(r::read_provenance).unwrap_or_default();
    let stripped =
        r::write_provenance(incoming, &r::Provenance::new()).map_err(EngineError::Invalid)?;
    let stripped = match current {
        Some(previous) => r::reattach_ids(&stripped, previous),
        None => stripped,
    };
    let (text, minted) = if assign_ids {
        // Ids the page must not mint again: those the baseline and the stored
        // records hold, minus the ones still on the page.
        let on_page: BTreeSet<String> = r::split_sections(&stripped)
            .into_iter()
            .filter_map(|s| s.id)
            .collect();
        let taken: BTreeSet<String> = r::baseline_ids(baseline)
            .into_iter()
            .chain(stored.keys().cloned())
            .filter(|id| !on_page.contains(id))
            .collect();
        r::assign_ids(&stripped, &taken, &mut r::random_section_id)
    } else {
        (stripped, Vec::new())
    };
    // The stored page is cut like the incoming one before the two are
    // compared: a page whose frontmatter held only the block loses it and the
    // blank lines after it, which would read as a change of the top.
    let before: HashMap<String, String> = current
        .map(|c| {
            let bare =
                r::write_provenance(c, &r::Provenance::new()).unwrap_or_else(|_| c.to_string());
            r::split_sections(&bare)
                .into_iter()
                .filter_map(|s| Some((s.id?, r::section_hash(&s.text))))
                .collect()
        })
        .unwrap_or_default();
    let mut prov = r::Provenance::new();
    // How many sections under each heading text got their id in this
    // compose so far, for the key into `new_by_heading`. Only the minted
    // ones count, in page order: they are the sections the room saw without
    // an id, which is what its keys count. A section that arrived with an id
    // of its own (pasted from another page) takes no place.
    let minted_ids: BTreeSet<&str> = minted.iter().map(String::as_str).collect();
    let mut new_seen: HashMap<String, usize> = HashMap::new();
    let mut changed = Vec::new();
    for section in r::split_sections(&text) {
        let Some(id) = section.id.clone() else {
            continue;
        };
        let place = minted_ids.contains(id.as_str()).then(|| {
            let seen = new_seen.entry(section.heading.clone()).or_default();
            *seen += 1;
            *seen - 1
        });
        let mut record = stored.get(&id).cloned().unwrap_or_default();
        if before.get(&id) != Some(&r::section_hash(&section.text)) {
            changed.push(id.clone());
            record.changed_at = Some(now.to_string());
            let carried = place
                .and_then(|place| {
                    authors
                        .new_by_heading
                        .get(&(section.heading.clone(), place))
                })
                .into_iter()
                .flatten();
            let mut names: Vec<&String> = Vec::new();
            for name in authors.changed.iter().chain(carried) {
                if !names.contains(&name) {
                    names.push(name);
                }
            }
            let someone = SOMEONE.to_string();
            if names.is_empty() && authors.someone_if_empty {
                names.push(&someone);
            }
            for name in names {
                if !record.changed_by.contains(name) {
                    record.changed_by.push(name.clone());
                }
            }
        }
        prov.insert(id, record);
    }
    Ok(Composed {
        text,
        minted,
        prov,
        changed,
    })
}

/// What [`compose_reflection`] answers.
pub(super) struct Composed {
    /// The text to write, without the bookkeeping block re-emitted yet.
    pub text: String,
    /// The ids minted, in page order.
    pub minted: Vec<String>,
    /// The new records.
    pub prov: r::Provenance,
    /// The ids of the sections this text changed against the stored page
    /// (the top included), in page order: new ones too.
    pub changed: Vec<String>,
}

/// The refusal for a `sources` key that names no single section, worded so
/// the writer gets it right on the next call: what a key may be, and the
/// sections this page has.
fn sources_key_miss(
    miss: &r::SectionKeyMiss,
    key: &str,
    path: &str,
    sections: &[r::Section],
) -> EngineError {
    EngineError::Invalid(match miss {
        r::SectionKeyMiss::Ambiguous(headings) => format!(
            "the key '{key}' names {} sections in '{path}' ({}), so it cannot \
             carry sources: write the page without sources, then write it again \
             with sources keyed by the ids from ids_assigned or read_reflection",
            headings.len(),
            headings
                .iter()
                .map(|h| format!("'{h}'"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        r::SectionKeyMiss::None => {
            let headings: Vec<String> = sections
                .iter()
                .filter(|s| !s.heading.is_empty())
                .map(|s| format!("'{}'", s.heading))
                .collect();
            let has = if headings.is_empty() {
                "this page has no ## section yet, only _top for the text above them".to_string()
            } else {
                format!("this page has {}", headings.join(", "))
            };
            format!(
                "no section '{key}' in '{path}' to set sources on: a key is a ## \
                 section's id or its heading text, or _top for the text before the \
                 first ##; the # title and ### headings are not sections, and {has}"
            )
        }
    })
}

/// The headings of the sections of `stored` that owed feedback against
/// `baseline` (new or changed, not the top) and that `written` no longer
/// holds, by id or, for a section without one, by heading. A write that
/// leaves such a section out says so on its receipt, so a person's change
/// never leaves the page without a word.
fn removed_with_feedback(
    stored: Option<&str>,
    baseline: &str,
    written: &[r::Section],
) -> Vec<String> {
    let Some(stored) = stored else {
        return Vec::new();
    };
    r::feedback_changes(stored, baseline)
        .into_iter()
        .filter(|c| c.kind != r::ChangeKind::Removed && c.id.as_deref() != Some(r::TOP_SECTION))
        .filter(|c| match &c.id {
            Some(id) => !written.iter().any(|s| s.id.as_deref() == Some(id.as_str())),
            None => !written
                .iter()
                .any(|s| s.heading.to_lowercase() == c.heading.to_lowercase()),
        })
        .map(|c| c.heading)
        .collect()
}

/// Move every thread of a page that is anchored by heading onto the id its
/// heading carries in `page`, the text just written, so a later rename of
/// the heading keeps the thread on its section. Runs under the page lock,
/// after the page is on disk: a failure here leaves the thread anchored by
/// heading, which still resolves, so it is logged and the write stands. A
/// heading the page no longer holds answers no id, and its thread stays as
/// it is.
fn anchor_threads_on_ids(threads_abs: &Path, page: &str) {
    let file = match read_opt(threads_abs) {
        Ok(Some(file)) => file,
        Ok(None) => return,
        Err(e) => {
            tracing::warn!("could not read {}: {e}", threads_abs.display());
            return;
        }
    };
    let sections = r::split_sections(page);
    let id_of = |heading: &str| {
        r::find_section(&sections, heading)
            .filter(|s| !s.heading.is_empty())
            .and_then(|s| s.id.clone())
    };
    let Some(moved) = crystalline_core::threads::anchor_headings_on_ids(&file, &id_of) else {
        return;
    };
    if let Err(e) = write_file(threads_abs, &moved) {
        tracing::warn!(
            "could not anchor the threads in {} on section ids: {e}",
            threads_abs.display()
        );
    }
}

/// `text` with `prov` as its bookkeeping block, as LF, and each of `ids`
/// still on the page stamped `reflected` with the hash of its section in
/// that stored text. The top's text starts after the frontmatter the block
/// brings, which `text` may not have had yet; only the block is rewritten,
/// so a second pass leaves every section as it is. The write and the settle
/// both end here.
fn stamp_reflected(text: &str, prov: &mut r::Provenance, ids: &[String]) -> Result<String> {
    let write =
        |text: &str, prov: &r::Provenance| -> Result<String> {
            Ok(crystalline_core::to_lf(
                &r::write_provenance(text, prov).map_err(EngineError::Invalid)?,
            )
            .into_owned())
        };
    let stored = write(text, prov)?;
    let sections = r::split_sections(&stored);
    let mut rehashed = false;
    for id in ids {
        let Some(section) = sections
            .iter()
            .find(|s| s.id.as_deref() == Some(id.as_str()))
        else {
            continue;
        };
        let hash = r::section_hash(&section.text);
        let record = prov.entry(id.clone()).or_default();
        if record.reflected.as_deref() != Some(hash.as_str()) {
            record.reflected = Some(hash);
            rehashed = true;
        }
    }
    if rehashed {
        write(&stored, prov)
    } else {
        Ok(stored)
    }
}

impl Engine {
    /// Every engram of a domain by permalink, with the checksum a section's
    /// sources are compared against.
    pub(crate) async fn engram_facts(
        &self,
        domain_id: DomainId,
        domain: &str,
    ) -> Result<HashMap<String, EngramFact>> {
        let store = self.store.lock().await;
        #[cfg(any(test, feature = "testing"))]
        self.recorded_path_loads
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let stamps = store.file_stamps(domain_id).await?;
        let engrams = store.list_engrams(domain, None, None).await?;
        Ok(engrams
            .into_iter()
            .filter_map(|d| {
                let stamp = stamps.get(&d.path)?;
                Some((
                    d.permalink.clone(),
                    EngramFact {
                        permalink: d.permalink,
                        title: d.title,
                        status: d.status,
                        checksum: stamp.sha256.clone(),
                        mtime: stamp.mtime,
                    },
                ))
            })
            .collect())
    }

    /// The facts of the engrams `wanted` names, by permalink: what a
    /// page's own sources are held against. One index seek per distinct
    /// permalink under the store lock ([`Store::cited_engrams`] asks a
    /// repeated permalink once), so a page citing a few engrams never pays
    /// for every engram of the domain the way [`Engine::engram_facts`]
    /// does. A permalink no engram holds has no entry, which a source reads
    /// as missing.
    ///
    /// [`Store::cited_engrams`]: crystalline_index::Store::cited_engrams
    pub(crate) async fn cited_facts(
        &self,
        domain_id: DomainId,
        wanted: Vec<String>,
    ) -> Result<HashMap<String, EngramFact>> {
        if wanted.is_empty() {
            return Ok(HashMap::new());
        }
        let cited = self
            .store
            .lock()
            .await
            .cited_engrams(domain_id, &wanted)
            .await?;
        Ok(cited
            .into_iter()
            .map(|c| {
                (
                    c.permalink.clone(),
                    EngramFact {
                        permalink: c.permalink,
                        title: c.title,
                        status: c.status,
                        checksum: c.sha256,
                        mtime: c.mtime,
                    },
                )
            })
            .collect())
    }

    /// Put one reflection's row and its chunks in the index (R53), from the
    /// text just written to `abs`. The row carries the file's own
    /// modification instant, in the spelling the sync's prefilter compares
    /// (`asset_modified`), so the next walk does not read the file again.
    /// A committed write nudges the embed worker (R56), after the store lock
    /// is released.
    pub(super) async fn index_reflection(
        &self,
        domain_id: DomainId,
        inner: &str,
        text: &str,
        abs: &Path,
        on_refusal: super::fence::OnRefusal<'_>,
    ) -> Result<()> {
        // The row and its chunks commit together, as an engram row and its
        // chunks do (`commit_overlay_row`), behind the host-lock fence: a
        // row left stamped over missing chunks would be skipped by the
        // sync's prefilter until a full reindex, and a row this instance
        // writes after losing the lock would land behind the new host's
        // back. No caller holds a transaction here: it would hold the store
        // lock this takes.
        self.commit_reflection_rows(
            domain_id,
            &[super::fence::RowOp::Page { inner, text, abs }],
            on_refusal,
        )
        .await
    }

    /// Arm a failure of the next reflection index write, after its row and
    /// chunks were written and before the commit. Disarmed by the write it
    /// hits.
    #[cfg(any(test, feature = "testing"))]
    pub fn fail_next_reflection_index(&self) {
        self.reflection_index_fault
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

impl Engine {
    /// `write_reflection`: dispatch on the action.
    pub async fn write_reflection(
        &self,
        p: &WriteReflectionParams,
        caller: &ReflectionCaller,
    ) -> Result<Value> {
        match p.action.as_deref().unwrap_or("write") {
            "write" => self.write_reflection_text(p, caller).await,
            "move" => {
                let to = p.to.as_deref().ok_or_else(|| {
                    EngineError::Invalid("move needs `to`, the new path inside reflections/".into())
                })?;
                self.move_reflection(&p.domain, &p.path, to, caller).await
            }
            "delete" => {
                self.delete_reflection(&p.domain, &p.path, caller).await?;
                Ok(json!({ "domain": p.domain, "path": p.path, "deleted": true }))
            }
            "settle" => {
                let outcome = p.outcome.as_deref().unwrap_or("kept");
                self.settle_reflection(&p.domain, &p.path, &p.sections, outcome, caller)
                    .await
            }
            "move_asset" => {
                let to = p.to.as_deref().ok_or_else(|| {
                    EngineError::Invalid(
                        "move_asset needs `to`, the file's new path inside reflections/ (assets/...)"
                            .into(),
                    )
                })?;
                self.move_reflection_asset(&p.domain, &p.path, to, caller)
                    .await
            }
            "delete_asset" => {
                self.reflection_asset_delete(&p.domain, &p.path, caller)
                    .await?;
                Ok(json!({ "domain": p.domain, "path": p.path, "deleted": true }))
            }
            other => Err(EngineError::Invalid(format!(
                "`{other}` is not a write_reflection action; use write, move, delete, settle, move_asset or delete_asset"
            ))),
        }
    }

    async fn write_reflection_text(
        &self,
        p: &WriteReflectionParams,
        caller: &ReflectionCaller,
    ) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        // PACT.md is written by curators; everyone else proposes a
        // change through a thread on it.
        let need = if p.sources.is_empty() && p.path != r::PACT_FILE {
            ReflectionRight::Edit
        } else {
            ReflectionRight::Curate
        };
        let access = self
            .require_reflection_to_write(&p.domain, caller, need)
            .await?;
        self.refuse_read_only_domain(&p.domain, &crate::scope::Scope::Unrestricted)
            .await?;
        r::validate_reflection_path(&p.path).map_err(EngineError::Invalid)?;
        // Rule 7: the whole page, one section of it, or (sources alone) the
        // stored page as it stands, curated by the sources named.
        let op = match (p.markdown.as_deref(), p.operation.as_deref()) {
            (Some(_), Some(_)) => {
                return Err(EngineError::Invalid("write takes markdown (the whole reflection) or operation (one section), not both".into()));
            }
            (None, None) if p.sources.is_empty() => {
                return Err(EngineError::Invalid("write needs markdown, the whole reflection, or operation with section and content, or sources alone to curate sections as they stand".into()));
            }
            (None, None) => None,
            (Some(_), None) => None,
            (None, Some(name)) => Some(crystalline_core::emit::SectionOp::parse(name, p.include_subsections).ok_or_else(|| {
                EngineError::Invalid(format!("`{name}` is not a section operation; use replace_section, insert_before_section or insert_after_section"))
            })?),
        };
        // Each source by permalink (this domain) or address (any domain),
        // resolved in the writer's scope, and their facts read one seek
        // each before the page lock.
        // A person's confirmation lets a source through that the reach rule
        // alone refuses (`super::disclosure`); nobody behind the caller, no
        // confirmation.
        let by = super::disclosure::confirmer(caller).filter(|_| p.disclosed.is_some());
        let named = self
            .resolve_source_names(
                &p.domain,
                &caller.scope,
                p.sources.values().flatten(),
                by.is_some(),
            )
            .await?;
        let wanted: Vec<r::SourceRef> = named.values().map(|n| n.source_ref("")).collect();
        let facts = self
            .source_facts(&p.domain, access.domain_id, None, wanted.iter())
            .await?;
        let abs = access.abs(&p.path);
        let baseline_abs = access.abs(&r::baseline_path(&p.path));
        let _lock = self.lock_file(&abs).await;
        // The person said yes to the readers the question named: still them?
        if let (Some(_), Some(basis)) = (&by, p.disclosed.as_deref()) {
            self.hold_disclosure_to_its_question(&p.domain, &p.path, &named, basis)
                .await?;
        }

        let current = read_opt(&abs)?;
        // Rule 0: a create never replaces a page, and a stale checksum is
        // refused; both are decided under the lock, so two racing writes
        // cannot both land. A checksum for a page that is not there has
        // nothing to compare: the write creates it.
        let swap = match (p.create_only, p.expected_checksum.as_deref()) {
            (true, _) => Swap::Absent,
            (false, Some(expected)) => Swap::Expect(expected),
            (false, None) => Swap::Any,
        };
        match check_swap(current.as_deref(), &swap) {
            Ok(()) | Err(SwapRefused::Missing) => {}
            Err(SwapRefused::Exists) => {
                return Err(EngineError::Conflict(format!(
                    "a reflection already lives at '{}'",
                    p.path
                )));
            }
            Err(SwapRefused::Stale { .. }) => {
                return Err(EngineError::Conflict(format!(
                    "stale edit: '{}' changed since it was read; read it again and repeat the change",
                    p.path
                )));
            }
        }
        if current.is_none() {
            r::validate_new_reflection_path(&p.path).map_err(EngineError::Invalid)?;
            self.refuse_only_elsewhere(&p.domain, access.domain_id, ElsewhereRow::Page(&p.path))
                .await?;
        }
        let (incoming, heading_stripped, placed_after_section) = match op {
            None => match (&p.markdown, current.as_deref()) {
                (Some(markdown), _) => (markdown.clone(), None, false),
                (None, Some(stored)) => (stored.to_string(), None, false),
                (None, None) => {
                    return Err(EngineError::NotFound(format!(
                        "no reflection '{}' in domain '{}' to curate by sources; write it with markdown first",
                        p.path, p.domain
                    )));
                }
            },
            Some(op) => {
                let stored = current.as_deref().ok_or_else(|| {
                    EngineError::NotFound(format!(
                        "no reflection '{}' in domain '{}'",
                        p.path, p.domain
                    ))
                })?;
                let section = p.section.as_deref().ok_or_else(|| {
                    EngineError::Invalid(
                        "a section operation needs `section`, the heading path".into(),
                    )
                })?;
                let content = p.content.as_deref().ok_or_else(|| {
                    EngineError::Invalid("a section operation needs `content`".into())
                })?;
                let edit = r::edit_section(stored, op, section, content).map_err(section_err)?;
                (edit.text, edit.heading_stripped, edit.placed_after_section)
            }
        };
        // An engram attachment is copied into reflections/assets/
        // first; a write that adds an embed of one is refused, nothing
        // written. An embed the stored page already holds (written by hand
        // or in a room) is the reflect queue's to name, so a change to
        // another section is not held up by it.
        let stored_embeds = current
            .as_deref()
            .map(|stored| r::engram_attachment_embeds(stored, &p.path))
            .unwrap_or_default();
        if r::engram_attachment_embeds(&incoming, &p.path)
            .iter()
            .any(|target| !stored_embeds.contains(target))
        {
            r::refuse_engram_attachments(&incoming, &p.path).map_err(EngineError::Invalid)?;
        }
        // Rule 8: the canonical spellings, the funnel every engram write's text
        // passes through, over the body after the frontmatter. The stored
        // page and its baseline go through it too, as a link rewrite that is
        // no person's change (`relink`): a section the writer left alone then
        // matches the stored one, so a respelled alias in it records nobody
        // and owes no feedback.
        let stored_baseline = read_opt(&baseline_abs)?.unwrap_or_default();
        let funnel = self
            .spelling_funnel(
                &caller.scope,
                &[
                    &incoming,
                    current.as_deref().unwrap_or(""),
                    &stored_baseline,
                ],
                BodyGate::Split,
            )
            .await?;
        let respell = |text: &str| funnel.respell(text);
        let (incoming, normalized) = respell(&incoming);
        let (stored, baseline) = match current.as_deref() {
            Some(page) => {
                let out =
                    r::relink(page, &stored_baseline, &respell).map_err(EngineError::Invalid)?;
                (Some(out.page), out.baseline)
            }
            None => (None, stored_baseline.clone()),
        };
        // The bookkeeping is Crystalline's (rule 1 replaces the block): a
        // block the writer made up, and an id line no heading can carry, are
        // replaced and said, so an agent corrected in silence does not keep
        // writing them. The page sent back as it was read is told nothing.
        // A room save does not come here: it keeps a pasted section's id and
        // takes no stray id line out, which is cosmetic, since Fluid never
        // writes a made-up one.
        // A writer is held against the block as it was shown to them: a
        // reader who may not see a source of another domain read the block
        // without it, and sends it back that way.
        let stored_foreign: BTreeSet<String> = current
            .as_deref()
            .map(r::read_provenance)
            .unwrap_or_default()
            .into_values()
            .flat_map(|record| record.sources.into_iter().filter_map(|src| src.domain))
            .collect();
        let sight = if stored_foreign.is_empty() {
            None
        } else {
            Some(self.source_sight(&caller.scope).await?)
        };
        let shown_block = match (&sight, current.as_deref()) {
            (Some(sight), Some(page)) => r::block_as_written(&sight.shown_text(page)),
            _ => None,
        };
        let made_up_block = r::block_as_written(&incoming).is_some_and(|block| {
            current.as_deref().and_then(r::block_as_written).as_ref() != Some(&block)
                && shown_block.as_ref() != Some(&block)
        });

        // An id is per page, so one only another page carries was copied or
        // made up: it is replaced too. Asked only for ids this page, its
        // baseline and its records never had, so a page sent back as it was
        // read reads no other page. The store lock is taken inside the file
        // lock, the order `index_reflection` uses.
        let mut known = r::baseline_ids(&baseline);
        if let Some(page) = stored.as_deref() {
            known.extend(r::split_sections(page).into_iter().filter_map(|s| s.id));
            known.extend(r::read_provenance(page).into_keys());
        }
        let unknown: BTreeSet<String> = r::split_sections(&incoming)
            .into_iter()
            .skip(1)
            .filter_map(|s| s.id)
            .filter(|id| !known.contains(id))
            .collect();
        let elsewhere: BTreeSet<String> = if unknown.is_empty() {
            BTreeSet::new()
        } else {
            let rows = self
                .store
                .lock()
                .await
                .list_reflections(access.domain_id)
                .await?;
            rows.iter()
                .filter(|row| row.path != p.path)
                .flat_map(|row| r::split_sections(&row.content))
                .filter_map(|s| s.id)
                .filter(|id| unknown.contains(id))
                .collect()
        };
        let written = r::drop_written_ids(&incoming, &|id| elsewhere.contains(id));
        let Composed {
            text,
            minted,
            mut prov,
            changed,
        } = compose_reflection(
            stored.as_deref(),
            &baseline,
            &written.text,
            &SectionAuthors::plain(vec![caller.author.name.clone()]),
            true,
            &now(),
        )?;
        let sections = r::split_sections(&text);
        // Each made-up id the heading it sat under no longer carries, with
        // the id it carries now; sources keyed by the made-up id follow it.
        let replaced: Vec<(String, String)> = written
            .dropped
            .iter()
            .filter_map(|(k, made_up)| {
                let id = sections.get(k + 1)?.id.clone()?;
                (&id != made_up).then(|| (made_up.clone(), id))
            })
            .collect();
        let changed_ids = changed.clone();
        let mut advanced: Vec<String> = Vec::new();
        // Each section and source domain this write carries on a person's
        // confirmation.
        let mut confirmed: Vec<(String, String)> = Vec::new();
        for (key, engrams) in &p.sources {
            let key = replaced
                .iter()
                .find(|(made_up, _)| made_up == key)
                .map_or(key, |(_, id)| id);
            let section = r::resolve_section_key(&sections, key)
                .map_err(|miss| sources_key_miss(&miss, key, &p.path, &sections))?;
            let id = section
                .id
                .clone()
                .expect("every heading has an id after assign_ids");
            let mut refs: Vec<r::SourceRef> = Vec::new();
            for name in engrams {
                let source = &named[name];
                let fact = facts.fact(&source.source_ref("")).ok_or_else(|| {
                    EngineError::Invalid(match &source.domain {
                        None => format!(
                            "no engram '{}' in domain '{}' to name as a source",
                            source.permalink, p.domain
                        ),
                        Some(_) => format!("no engram '{}' to name as a source", source.said()),
                    })
                })?;
                let src = source.source_ref(&fact.checksum);
                if let (true, Some(domain)) = (source.disclose, src.domain.as_ref())
                    && !confirmed.iter().any(|(s, d)| *s == id && d == domain)
                {
                    confirmed.push((id.clone(), domain.clone()));
                }
                // One engram named twice (by permalink and by address) is
                // one source.
                if !refs
                    .iter()
                    .any(|r| r.domain == src.domain && r.engram == src.engram)
                {
                    refs.push(src);
                }
            }
            let record = prov.entry(id.clone()).or_default();
            // A write never drops what the writer could not see: the
            // section's sources in a domain hidden from them stay as they
            // were. A source the writer sees is theirs to keep or leave out,
            // one whose domain cannot be checked here included: leaving it
            // out is how a source of a domain that is gone for good goes.
            // Each kept source goes back to its stored place, so a writer
            // who cannot see it does not move it on every write.
            for (at, src) in record.sources.iter().enumerate() {
                if sight.as_ref().is_some_and(|sight| !sight.sees(src)) {
                    refs.insert(at.min(refs.len()), src.clone());
                }
            }
            record.sources = refs;
            record.reflected = Some(r::section_hash(&section.text));
            record.changed_by.clear();
            record.changed_at = None;
            record.answered_by = None;
            advanced.push(id);
        }
        // Whether sources named a section: what the receipt calls curated.
        let curated = !advanced.is_empty();
        // An agent's write is reflected, with or without sources: every
        // section it creates, changes or removes moves to what it wrote, so
        // its own text never comes back to it as feedback (D5). Feedback is
        // for people's edits, and those stay owed through the agent's write:
        // a section that owed a change before it (a person's edit, a heading
        // typed in a plain editor that gets its id here, a section sent back
        // in another case without its id), and a stale copy of a section the
        // page no longer held, are settled only by sources, a settle or a
        // thread.
        if caller.author.agent {
            let stored_page = stored.as_deref().unwrap_or("");
            let owed = r::feedback_changes(stored_page, &baseline);
            let owed_ids: BTreeSet<&str> = owed.iter().filter_map(|c| c.id.as_deref()).collect();
            // The headings of owed sections this write may have sent back
            // without their id: one that never had an id, and one whose id
            // is no longer on the page. A person's section still on the page
            // under its id names no other section.
            let on_page: BTreeSet<&str> = sections.iter().filter_map(|s| s.id.as_deref()).collect();
            let owed_headings: Vec<String> = owed
                .iter()
                .filter(|c| c.kind != r::ChangeKind::Removed)
                .filter(|c| c.id.as_deref().is_none_or(|id| !on_page.contains(id)))
                .map(|c| c.heading.to_lowercase())
                .collect();
            let on_stored: BTreeSet<String> = r::split_sections(stored_page)
                .into_iter()
                .filter_map(|s| s.id)
                .collect();
            let mut known_elsewhere = r::baseline_ids(&baseline);
            if let Some(page) = stored.as_deref() {
                known_elsewhere.extend(r::read_provenance(page).into_keys());
            }
            for id in changed {
                if advanced.contains(&id) || owed_ids.contains(id.as_str()) {
                    continue;
                }
                let Some(section) = sections
                    .iter()
                    .find(|s| s.id.as_deref() == Some(id.as_str()))
                else {
                    continue;
                };
                if id != r::TOP_SECTION
                    && !on_stored.contains(&id)
                    && (known_elsewhere.contains(&id)
                        || owed_headings.contains(&section.heading.to_lowercase()))
                {
                    continue;
                }
                let record = prov.entry(id.clone()).or_default();
                record.changed_by.clear();
                record.changed_at = None;
                record.answered_by = None;
                advanced.push(id);
            }
            // A reflected section the agent left out is its removal.
            let reflected = r::baseline_ids(&baseline);
            for id in &on_stored {
                if !on_page.contains(id.as_str())
                    && reflected.contains(id)
                    && !owed_ids.contains(id.as_str())
                {
                    advanced.push(id.clone());
                }
            }
        }
        let final_text = stamp_reflected(&text, &mut prov, &advanced)?;
        // The `##` sections this write created or changed that leave it with
        // no source in their record: provenance alone, whatever else counts
        // a section as reflected. The pact is no page.
        let unsourced_sections: Vec<String> = if p.path == r::PACT_FILE {
            Vec::new()
        } else {
            sections
                .iter()
                .skip(1)
                .filter(|s| {
                    s.id.as_ref().is_some_and(|id| {
                        // A made-up id the write replaced still names the
                        // section it sat under.
                        (changed_ids.contains(id)
                            || replaced
                                .iter()
                                .any(|(old, new)| new == id && changed_ids.contains(old)))
                            && prov.get(id).is_none_or(|rec| rec.sources.is_empty())
                    })
                })
                .map(|s| s.heading.clone())
                .collect()
        };
        let removed_with_feedback = removed_with_feedback(stored.as_deref(), &baseline, &sections);
        let threads_abs = access.abs(&r::threads_path(&p.path));
        let mut undo = DiskUndo::default();
        for file in [&abs, &baseline_abs, &threads_abs] {
            undo.record(file)?;
        }
        write_file(&abs, &final_text)?;
        if !advanced.is_empty() || baseline != stored_baseline {
            write_file(
                &baseline_abs,
                &r::advance_baseline(&baseline, &final_text, &advanced),
            )?;
        }
        anchor_threads_on_ids(&threads_abs, &final_text);
        self.index_reflection(
            access.domain_id,
            &p.path,
            &final_text,
            &abs,
            OnRefusal {
                domain: &p.domain,
                undo,
                keep: Some(KeepText {
                    kind: crystalline_core::kept::KeptKind::Reflection,
                    path: &p.path,
                    text: &final_text,
                    caller,
                    holds: &abs,
                }),
                read: false,
            },
        )
        .await?;
        self.record_disclosures(
            &p.domain,
            access.domain_id,
            &p.path,
            &final_text,
            &confirmed,
            by.as_deref(),
        )
        .await?;
        self.mark_reflection_dirty(&p.domain, BackupCause::Settled);
        self.announce_reflection(
            &p.domain,
            &p.path,
            ReflectionChangeKind::Changed,
            None,
            change_label(&caller.scope),
        );
        // The page lock goes before the room is touched: a room's save holds
        // the room's state and then takes this lock, so the other order
        // here would deadlock against a person typing. A room save that slips
        // in between finds the file moved, merges it and lands the same text.
        drop(_lock);
        let mut receipt = json!({
            "domain": p.domain,
            "path": p.path,
            "checksum": sha256_hex(final_text.as_bytes()),
            "created": current.is_none(),
            "ids_assigned": minted,
            "curated": curated,
        });
        note_domain_names_normalized(&mut receipt, normalized);
        note_markdown_findings(
            &mut receipt,
            crystalline_core::lint_markdown(&final_text, 1, false),
            self.content_source(&p.domain)
                .ok()
                .and_then(|source| domain_verify_config(&source))
                .as_ref(),
        );
        note_heading_stripped(&mut receipt, heading_stripped);
        note_placed_after_section(&mut receipt, placed_after_section);
        if p.path == r::PACT_FILE {
            // Checked against the pages as the index holds them now, this
            // write included.
            let pages: Vec<String> = self
                .store
                .lock()
                .await
                .list_reflections(access.domain_id)
                .await?
                .into_iter()
                .map(|row| row.path)
                .filter(|path| path != r::PACT_FILE)
                .collect();
            let findings = r::structure_findings(&final_text, &pages);
            if !findings.is_empty() {
                receipt["structure_findings"] = json!(
                    findings
                        .iter()
                        .map(|f| json!({ "bullet": f.bullet, "reason": f.reason }))
                        .collect::<Vec<_>>()
                );
            }
        }
        if !unsourced_sections.is_empty() {
            receipt["unsourced_sections"] = json!(unsourced_sections);
            add_guidance(&mut receipt, UNSOURCED_GUIDANCE);
        }
        if !replaced.is_empty() {
            let list: Vec<String> = replaced
                .iter()
                .map(|(made_up, id)| format!("{made_up} (now {id})"))
                .collect();
            add_guidance(
                &mut receipt,
                &format!(
                    "{WRITTEN_ID_GUIDANCE} The id lines written here were replaced: {}.",
                    list.join(", ")
                ),
            );
        }
        if made_up_block {
            add_guidance(&mut receipt, WRITTEN_BLOCK_GUIDANCE);
        }
        if !removed_with_feedback.is_empty() {
            add_guidance(
                &mut receipt,
                &format!(
                    "This write removed sections a person changed since the last reflection: {}. \
                     Their change left the page with them; if that was not meant, write them \
                     back (read_reflection first, and pass expected_checksum on a whole-page \
                     rewrite).",
                    removed_with_feedback
                        .iter()
                        .map(|h| format!("'{h}'"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            );
        }
        let doc = RoomDoc::Reflection(p.path.clone());
        if let Some(rooms) = self.collab_rooms()
            && rooms.has_live_doc(&p.domain, &doc, None).await
        {
            // The room takes the file in by a three-way merge into its own
            // live text, so a person's unsaved typing stays; it reads the
            // page without the block (contract "Co-editing a reflection"),
            // and its save re-attaches the stored block. The write has landed
            // either way: a room that closed meanwhile is no refusal.
            match rooms.absorb_doc_change(&p.domain, &doc, None).await {
                Ok(outcome) => {
                    note_room_outcome(&mut receipt, outcome.absorbed, outcome.participants);
                }
                Err(e) => tracing::debug!(
                    domain = %p.domain,
                    path = %p.path,
                    "the open room did not take the write in: {e}"
                ),
            }
        }
        Ok(receipt)
    }
}

impl Engine {
    /// Tell every tab that may read this domain's Reflection that something
    /// in it changed. The event route delivers it by reflection right, not by
    /// engram right.
    pub(crate) fn announce_reflection(
        &self,
        domain: &str,
        path: &str,
        kind: ReflectionChangeKind,
        from: Option<&str>,
        actor: Option<String>,
    ) {
        self.announce(Change::Reflection(ReflectionChanged {
            domain: domain.to_string(),
            path: path.to_string(),
            kind,
            from: from.map(str::to_string),
            actor,
            checksum: None,
            audience: None,
        }));
    }

    /// Announce that the threads of the page at `path` changed, with the
    /// checksum of the threads file as this write left it on disk (the
    /// write normalizes line endings to LF): the watcher finds the same
    /// file a moment later, and the bus drops that echo by it; the next
    /// complete walk compares with it too.
    pub(crate) fn announce_threads(
        &self,
        domain: &str,
        path: &str,
        threads: &str,
        actor: Option<String>,
    ) {
        let checksum = super::sha256_hex(crystalline_core::to_lf(threads).as_bytes());
        self.remember_threads(domain, path, Some(&checksum));
        self.announce(Change::Reflection(ReflectionChanged {
            domain: domain.to_string(),
            path: path.to_string(),
            kind: ReflectionChangeKind::Threads,
            from: None,
            actor,
            checksum: Some(checksum),
            audience: None,
        }));
    }

    /// Announce `switched` when the domain's switch is no longer `before`.
    /// Every Crystalline writer of a MANIFEST calls it after its write lands.
    pub(crate) fn announce_switch_if_moved(
        &self,
        domain: &str,
        before: crystalline_core::ReflectionSwitch,
        actor: Option<String>,
    ) {
        if self.reflection_switch(domain) != before {
            self.announce_reflection(domain, "", ReflectionChangeKind::Switched, None, actor);
        }
    }

    /// Save and close the domain's reflection rooms, telling each socket
    /// `switched_off`, when landing `after` as the domain's MANIFEST in its
    /// folder switches the Reflection off (`None`: the MANIFEST goes away).
    ///
    /// Every Crystalline writer of a MANIFEST calls it after its last
    /// refusal and before its write, for the folder only, never for a draft:
    /// the state is still on, so each room's last save lands in its page.
    /// It takes the co-editing registry, so a caller holds no room's guard
    /// and no file's write lock (lock order: registry, then session).
    pub(crate) async fn close_reflection_rooms_if_switching_off(
        &self,
        domain: &str,
        after: Option<&str>,
    ) {
        if self.switches_reflection_off(domain, after) {
            self.close_reflection_rooms(domain).await;
        }
    }

    /// Whether landing `after` as `domain`'s MANIFEST in its folder switches
    /// its Reflection off: it is on now, and `after` does not say on (`None`:
    /// the MANIFEST goes away). Takes no lock.
    pub(crate) fn switches_reflection_off(&self, domain: &str, after: Option<&str>) -> bool {
        self.reflection_switch(domain) == crystalline_core::ReflectionSwitch::On
            && !after.is_some_and(declares_on)
    }

    /// Save and close every reflection room of `domain` with `switched_off`:
    /// the switch-off is about to land. See
    /// [`Engine::close_reflection_rooms_if_switching_off`].
    pub(crate) async fn close_reflection_rooms(&self, domain: &str) {
        if let Some(rooms) = self.collab_rooms() {
            // Boxed because the recursion is in the types only: `close_docs`
            // runs room saves, and a MANIFEST room's save can reach this
            // function. At run time it never recurses: `close_docs` closes
            // reflection rooms, whose saves never write the MANIFEST.
            Box::pin(rooms.close_docs(
                domain,
                crystalline_core::ContentKind::Reflection,
                "switched_off",
            ))
            .await;
        }
    }

    /// A reflection as a room reads it: the page without the block that
    /// names its source engrams, for everyone in the room; the checksum is
    /// the file's, so every stale check compares file against file.
    pub async fn reflection_room_text(&self, domain: &str, inner: &str) -> Result<EngramText> {
        let not_found =
            || EngineError::NotFound(format!("no reflection '{inner}' in domain '{domain}'"));
        let ContentSource::File { root } = self.read_source(domain) else {
            return Err(not_found());
        };
        let file = read_opt(&reflection_abs(&root, inner))?.ok_or_else(not_found)?;
        Ok(EngramText {
            domain: domain.to_string(),
            permalink: inner.to_string(),
            path: r::domain_rel(inner),
            checksum: sha256_hex(file.as_bytes()),
            content: without_provenance(&file),
        })
    }

    /// A room's save. `authors` are the names each changed section records,
    /// as the room decided them (the signed-in accounts that edited since
    /// its last save, and those that typed a heading still without an id);
    /// an empty list records nobody, or "someone" when the room says so.
    /// The stored bookkeeping wins over whatever the room text carries.
    /// Section ids are assigned on the `final_save` only. Answers the
    /// receipt a room reads (`checksum`, `permalink`, `path`); `permalink`
    /// is `inner`, so a reflection room is never re-keyed.
    ///
    /// A page that is gone answers `NotFound` (the room's deletion conflict)
    /// unless `expected_checksum` is empty, which only the room's restore
    /// passes, for a page it puts back. A domain whose Reflection is not on
    /// here any more is refused, and nothing is written.
    pub async fn save_reflection_room(
        &self,
        domain: &str,
        inner: &str,
        text: &str,
        expected_checksum: &str,
        authors: &SectionAuthors,
        final_save: bool,
    ) -> Result<Value> {
        // A room can outlive the state it was opened in: switched off, a
        // pending switch, a home elsewhere or a host lock another instance
        // took make the folder engram content again, or another instance's. The save waits (a refusal the room
        // shows and retries) instead of writing ids and bookkeeping into it.
        // Before the domain's own write check, whose host gate would answer
        // a moved lock in the words of an engram write.
        if !matches!(
            self.reflection_state_as(domain, true).await?,
            ReflectionState::On { .. }
        ) || self.confirm_reflection_host(domain).await?.is_some()
        {
            return Err(EngineError::Refused(format!(
                "the Reflection of '{domain}' is not on here any more, so this room's text is not saved; this tab keeps a copy, and if the page closes the server keeps one too"
            )));
        }
        self.refuse_read_only_domain(domain, &crate::scope::Scope::Unrestricted)
            .await?;
        let not_found =
            || EngineError::NotFound(format!("no reflection '{inner}' in domain '{domain}'"));
        let ContentSource::File { root } = self.read_source(domain) else {
            return Err(not_found());
        };
        let abs = reflection_abs(&root, inner);
        let baseline_abs = reflection_abs(&root, &r::baseline_path(inner));
        let _lock = self.lock_file(&abs).await;
        let current = read_opt(&abs)?;
        let receipt = |file: &str| {
            json!({
                "checksum": sha256_hex(file.as_bytes()),
                "permalink": inner,
                "path": r::domain_rel(inner),
            })
        };
        match current.as_deref() {
            None if !expected_checksum.is_empty() => return Err(not_found()),
            Some(file) if sha256_hex(file.as_bytes()) != expected_checksum => {
                return Err(EngineError::Conflict(format!(
                    "stale edit: '{inner}' changed since the room last saved it"
                )));
            }
            _ => {}
        }
        // Rule 8 the way a write takes it: the room's text arrives respelled
        // already (the session's `normalize_doc`), and the stored page and
        // its baseline get the same respelling as a link rewrite that is no
        // person's change, so a section nobody touched records nobody.
        let stored_baseline = read_opt(&baseline_abs)?.unwrap_or_default();
        let funnel = self
            .spelling_funnel(
                &crate::scope::Scope::Unrestricted,
                &[text, current.as_deref().unwrap_or(""), &stored_baseline],
                BodyGate::Split,
            )
            .await?;
        let respell = |source: &str| funnel.respell(source);
        let (incoming, _) = respell(text);
        let (stored, baseline) = match current.as_deref() {
            Some(page) => {
                let out =
                    r::relink(page, &stored_baseline, &respell).map_err(EngineError::Invalid)?;
                (Some(out.page), out.baseline)
            }
            None => (None, stored_baseline.clone()),
        };
        let Composed {
            text: composed,
            prov,
            ..
        } = compose_reflection(
            stored.as_deref(),
            &baseline,
            &incoming,
            authors,
            final_save,
            &now(),
        )?;
        let final_text = crystalline_core::to_lf(
            &r::write_provenance(&composed, &prov).map_err(EngineError::Invalid)?,
        )
        .into_owned();
        // A refused save puts the files back and keeps nothing itself: the
        // room holds the text, shows the refusal, and keeps the text when
        // it closes unsaved.
        let threads_abs = reflection_abs(&root, &r::threads_path(inner));
        let mut undo = DiskUndo::default();
        for file in [&baseline_abs, &abs, &threads_abs] {
            undo.record(file)?;
        }
        if baseline != stored_baseline {
            write_file(&baseline_abs, &baseline)?;
        }
        // A close with nothing new to land writes nothing.
        let wrote = current.as_deref() != Some(final_text.as_str());
        if wrote {
            write_file(&abs, &final_text)?;
            anchor_threads_on_ids(&threads_abs, &final_text);
            let (domain_id, _) = self.domain_source(domain).await?;
            self.index_reflection(
                domain_id,
                inner,
                &final_text,
                &abs,
                OnRefusal::put_back(domain, undo),
            )
            .await?;
            // The typist when the save holds one person's words alone; a
            // save of several, or of somebody the room cannot name, is by
            // nobody in particular.
            let actor = match authors.changed.as_slice() {
                [one] => Some(one.clone()),
                _ => None,
            };
            self.announce_reflection(domain, inner, ReflectionChangeKind::Changed, None, actor);
        }
        // A room's saves settle a minute after the typing stops; its close
        // settles them at once, whatever the close itself wrote.
        if final_save {
            self.mark_reflection_dirty(domain, BackupCause::Settled);
        } else if wrote || baseline != stored_baseline {
            self.mark_reflection_dirty(domain, BackupCause::Edit);
        }
        Ok(receipt(&final_text))
    }

    /// The right to be in a reflection's room: edit, curate for `PACT.md`
    /// (as its `PUT` takes). Public because `require_reflection` is
    /// engine-private. A room is opened to write, so this is the write gate:
    /// a host lock whose holder went stale is taken over for a signed-in
    /// caller with the right, as a page write takes it.
    pub async fn require_reflection_edit(
        &self,
        domain: &str,
        path: &str,
        caller: &ReflectionCaller,
    ) -> Result<()> {
        let need = if path == r::PACT_FILE {
            ReflectionRight::Curate
        } else {
            ReflectionRight::Edit
        };
        self.require_reflection_to_write(domain, caller, need)
            .await
            .map(|_| ())
    }
}

impl Engine {
    /// `read_reflection` without a path.
    pub async fn read_reflection_tree(
        &self,
        domain: &str,
        caller: &ReflectionCaller,
    ) -> Result<Value> {
        let access = self
            .require_reflection(domain, caller, ReflectionRight::Read)
            .await?;
        let rows = self
            .store
            .lock()
            .await
            .list_reflections(access.domain_id)
            .await?;
        let open_threads = |inner: &str| -> usize {
            read_opt(&access.abs(&r::threads_path(inner)))
                .ok()
                .flatten()
                .map(|t| {
                    crystalline_core::threads::parse_threads(&t)
                        .iter()
                        .filter(|t| t.status == crystalline_core::threads::ThreadStatus::Open)
                        .count()
                })
                .unwrap_or(0)
        };
        let mut pact =
            json!({ "path": r::PACT_FILE, "title": "", "exists": false, "open_threads": 0 });
        let mut root = TreeNode::default();
        for row in &rows {
            if row.path == r::PACT_FILE {
                pact = json!({ "path": r::PACT_FILE, "title": row.title, "exists": true, "open_threads": open_threads(r::PACT_FILE) });
                continue;
            }
            let sections = r::split_sections(&row.content).len().saturating_sub(1);
            root.insert(
                &row.path,
                json!({
                    "kind": "reflection",
                    "name": row.path.rsplit('/').next().unwrap_or(&row.path),
                    "path": row.path,
                    "title": row.title,
                    "open_threads": open_threads(&row.path),
                    "sections": sections,
                }),
            );
        }
        Ok(json!({ "domain": domain, "pact": pact, "nodes": root.render("") }))
    }
}

/// The folder tree the tree answer is built from.
#[derive(Default)]
struct TreeNode {
    folders: BTreeMap<String, TreeNode>,
    files: Vec<Value>,
}

impl TreeNode {
    fn insert(&mut self, path: &str, leaf: Value) {
        match path.split_once('/') {
            Some((folder, rest)) => self
                .folders
                .entry(folder.to_string())
                .or_default()
                .insert(rest, leaf),
            None => self.files.push(leaf),
        }
    }

    fn render(&self, prefix: &str) -> Vec<Value> {
        let mut out: Vec<Value> = self
            .folders
            .iter()
            .map(|(name, node)| {
                let path = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
                json!({ "kind": "folder", "name": name, "path": path, "children": node.render(&path) })
            })
            .collect();
        let mut files = self.files.clone();
        files.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
        out.extend(files);
        out
    }
}

impl Engine {
    /// `read_reflection` with a path.
    pub async fn read_reflection(
        &self,
        domain: &str,
        inner: &str,
        caller: &ReflectionCaller,
    ) -> Result<Value> {
        let access = self
            .require_reflection(domain, caller, ReflectionRight::Read)
            .await?;
        r::validate_reflection_path(inner).map_err(EngineError::Invalid)?;
        let text = read_opt(&access.abs(inner))?.ok_or_else(|| {
            EngineError::NotFound(format!("no reflection '{inner}' in domain '{domain}'"))
        })?;
        let prov = r::read_provenance(&text);
        // Only the engrams the page cites are read, one seek each (in their
        // own domain for a source of another): a page without sources asks
        // the index nothing. A source in a domain the reader may not open is
        // neither read nor shown; its section counts it.
        let sight = if access.engrams_visible
            && prov
                .values()
                .any(|record| record.sources.iter().any(|src| src.domain.is_some()))
        {
            Some(self.source_sight(&caller.scope).await?)
        } else {
            None
        };
        let sees = |src: &r::SourceRef| sight.as_ref().is_none_or(|sight| sight.sees(src));
        let facts = if access.engrams_visible {
            self.source_facts(
                domain,
                access.domain_id,
                None,
                prov.values()
                    .flat_map(|record| record.sources.iter())
                    .filter(|src| sees(src)),
            )
            .await?
        } else {
            SourceFacts::default()
        };
        // A reader who cannot read engrams gets the page without the block
        // that names them, and one who may not open a source's domain gets
        // it without that source. The sections, `line` included, are counted
        // on the markdown answered, so a line always points at its heading
        // there.
        let markdown = match &sight {
            _ if !access.engrams_visible => without_provenance(&text),
            Some(sight) => sight.shown_text(&text),
            None => text.clone(),
        };
        // A section whose only difference from what was last reflected is a
        // task box is not changed: ticking a box is not something to weigh.
        let reflected = r::parse_baseline(
            &read_opt(&access.abs(&r::baseline_path(inner)))?.unwrap_or_default(),
        );
        // Who shared a section's text on purpose, and when: for curators,
        // who are the ones to settle the page (`super::disclosure`).
        let disclosures = if access.engrams_visible && access.right >= ReflectionRight::Curate {
            self.reflection_disclosures(access.domain_id).await?
        } else {
            Default::default()
        };
        let sections: Vec<Value> = r::split_sections(&markdown)
            .iter()
            .filter(|s| s.id.as_deref() != Some(r::TOP_SECTION))
            .map(|s| {
                let record = s.id.as_deref().and_then(|id| prov.get(id));
                let only_boxes = reflected
                    .iter()
                    .find(|(id, _)| Some(id.as_str()) == s.id.as_deref())
                    .is_some_and(|(_, before)| r::same_but_task_boxes(before, &s.text));
                let mut row = json!({
                    "id": s.id,
                    "heading": s.heading,
                    "line": s.line,
                    "changed": r::text_changed(record, s) && !only_boxes,
                });
                if access.engrams_visible {
                    let sources = record.map(|rec| rec.sources.as_slice()).unwrap_or_default();
                    row["sources"] = json!(
                        sources
                            .iter()
                            .filter(|src| sees(src))
                            .map(|src| source_view(src, &facts))
                            .collect::<Vec<_>>()
                    );
                    let hidden = sources.iter().filter(|src| !sees(src)).count();
                    if hidden > 0 {
                        row["hidden_sources"] = json!(hidden);
                    }
                }
                if let Some(disclosure) =
                    s.id.as_deref().and_then(|id| disclosures.newest(inner, id))
                {
                    row["disclosed"] = json!({ "by": disclosure.by, "at": disclosure.at });
                }
                row
            })
            .collect();
        let threads = read_opt(&access.abs(&r::threads_path(inner)))?
            .map(|t| crystalline_core::threads::parse_threads(&t))
            .unwrap_or_default();
        let open: Vec<Value> = threads
            .iter()
            .filter(|t| t.status == crystalline_core::threads::ThreadStatus::Open)
            .map(|t| thread_view(inner, t, &text))
            .collect();
        let mut answer = json!({
            "domain": domain,
            "path": inner,
            "title": r::title_of(&text, inner),
            "markdown": markdown,
            // The file as stored, whatever was answered: a PUT's
            // `expected_checksum` is compared against the file.
            "checksum": sha256_hex(text.as_bytes()),
            "sections": sections,
            "threads": open,
        });
        // Who has the page open in a co-editing room (R77): nested, as the
        // contract and Fluid read it; absent while no room is open.
        if let Some(rooms) = self.collab_rooms() {
            let doc = RoomDoc::Reflection(inner.to_string());
            if rooms.has_live_doc(domain, &doc, None).await {
                answer["live"] =
                    json!({ "present": rooms.participants(domain, &doc, None, None).await });
            }
        }
        Ok(answer)
    }
}

/// A thread as the answers show it (`ThreadView` in the contract). The
/// Reflection is the target: the anchor's state comes from
/// `anchor_state` over the page, and `anchor.section` is what the anchor
/// names (`SectionRef::key`): the section's id, or its heading for a thread
/// opened on a heading that had no id yet.
pub(crate) fn thread_view(inner: &str, t: &crystalline_core::threads::Thread, text: &str) -> Value {
    use crystalline_core::threads as th;
    let last = chrono::DateTime::parse_from_rfc3339(t.last_at())
        .map(|d| d.with_timezone(&chrono::Utc))
        .ok();
    let age = last
        .map(|d| (chrono::Utc::now() - d).num_seconds().max(0))
        .unwrap_or(0);
    json!({
        "id": t.id,
        "path": inner,
        "anchor": {
            "section": t.anchor.section.key(),
            "quote": t.anchor.quote,
            "occurrence": t.anchor.occurrence,
            "state": r::anchor_state(text, &t.anchor).as_str(),
        },
        "status": t.status.as_str(),
        "waiting_on": th::waiting_on(t).as_str(),
        "opened_at": t.opened_at(),
        "age_secs": age,
        "entries": t.entries.iter().map(|e| json!({
            "at": e.at,
            "kind": e.kind.as_str(),
            "author": { "name": e.author.name, "agent": e.author.agent },
            "text": e.text,
        })).collect::<Vec<_>>(),
    })
}

/// One reflection search (R57).
pub struct ReflectionSearch<'a> {
    /// The words to find.
    pub query: &'a str,
    /// `hybrid` (the default), `text` or `semantic`.
    pub search_type: Option<&'a str>,
    /// Hits per page, clamped to 1..=[`MAX_PAGE_LIMIT`].
    pub limit: usize,
    /// One-based page.
    pub page: usize,
}

/// A reflection search's mode. `title` and `permalink` are engram modes and
/// are refused with the three a Reflection takes named.
fn parse_reflection_mode(s: Option<&str>) -> Result<SearchMode> {
    Ok(match s.unwrap_or("hybrid") {
        "hybrid" => SearchMode::Hybrid,
        "text" => SearchMode::Text,
        "semantic" => SearchMode::Semantic,
        other => {
            return Err(EngineError::Invalid(format!(
                "unknown search_type '{other}' for a Reflection; expected hybrid, text or semantic"
            )));
        }
    })
}

impl Engine {
    /// Search a domain's Reflection (R57): text, semantic or hybrid over its
    /// sections. A section never holds the frontmatter (`split_sections`
    /// starts at the body) and neither do the chunks (R52), so no hit or
    /// snippet comes out of a page's `reflection:` block, for any caller.
    /// Hybrid and semantic answer as text when there is no provider, nothing
    /// of this domain's pages embedded by the active model (another domain's
    /// vectors do not count), or stale vectors in this domain; `mode` says
    /// which ran. No hit carries a score.
    pub async fn search_reflection(
        &self,
        domain: &str,
        s: &ReflectionSearch<'_>,
        caller: &ReflectionCaller,
    ) -> Result<Value> {
        let access = self
            .require_reflection(domain, caller, ReflectionRight::Read)
            .await?;
        let requested = parse_reflection_mode(s.search_type)?;
        let query = s.query.trim();
        if query.is_empty() {
            return Err(EngineError::Invalid(
                "a search needs `query`, the words to find".into(),
            ));
        }
        let limit = s.limit.clamp(1, MAX_PAGE_LIMIT);
        let page = s.page.max(1);
        let provider = self.provider();
        // The store lock is held to read the rows and the coverage, dropped
        // across the embed call, and taken again for the vector statement,
        // as in `search_engrams_under`.
        let (rows, mut mode) = {
            let store = self.store.lock().await;
            let rows = store.list_reflections(access.domain_id).await?;
            let ready = requested != SearchMode::Text
                && provider.is_some()
                && store
                    .reflection_coverage(access.domain_id)
                    .await?
                    .has_active_embeddings(&self.model_id);
            (rows, if ready { requested } else { SearchMode::Text })
        };
        let mut vectors = Vec::new();
        if mode != SearchMode::Text
            && let Some(provider) = &provider
        {
            let qvec = provider
                .embed_queries(&[query.to_string()])
                .await
                .map_err(|e| EngineError::Internal(e.to_string()))?
                .into_iter()
                .next()
                .unwrap_or_default();
            let store = self.store.lock().await;
            match store
                .reflection_vector_hits(access.domain_id, &qvec, &self.model_id)
                .await
            {
                Ok(v) => vectors = v,
                // Vectors of another model or width: text until the worker
                // has re-embedded the pages (R57).
                Err(crystalline_index::IndexError::StaleEmbeddings { .. }) => {
                    mode = SearchMode::Text
                }
                Err(e) => return Err(e.into()),
            }
        }
        let floor = f64::from(crystalline_index::rank::DEFAULT_MIN_SIMILARITY);
        let hits = crystalline_index::reflection::rank_sections(
            &rows, query, mode, &vectors, floor, limit, page,
        );
        Ok(json!({
            "domain": domain,
            "query": query,
            "mode": mode_str(mode),
            "total": hits.total,
            "page": hits.page,
            "limit": hits.limit,
            "count": hits.items.len(),
            "hits": hits.items,
        }))
    }
}

impl Engine {
    pub async fn move_reflection(
        &self,
        domain: &str,
        from: &str,
        to: &str,
        caller: &ReflectionCaller,
    ) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let access = self
            .require_reflection_to_write(domain, caller, ReflectionRight::Edit)
            .await?;
        self.refuse_read_only_domain(domain, &crate::scope::Scope::Unrestricted)
            .await?;
        r::validate_reflection_path(from).map_err(EngineError::Invalid)?;
        r::validate_new_reflection_path(to).map_err(EngineError::Invalid)?;
        if from == r::PACT_FILE || to == r::PACT_FILE {
            return Err(EngineError::Invalid(
                "PACT.md is the domain's agreement; edit it instead of moving it".into(),
            ));
        }
        // Every other page whose links the move rewrites waits: a rewrite
        // under an open room would race its next save. Screened from the
        // text on disk before anything moves, and relinked from this list
        // below. The moved page's own room is not a refusal: it is closed
        // below, once every screen has passed.
        let rows = self
            .store
            .lock()
            .await
            .list_reflection_stamps(access.domain_id)
            .await?;
        let others = rows
            .iter()
            .map(|row| row.path.as_str())
            .filter(|path| *path != from && *path != to);
        let linking = rooms_to_rewrite(&access, others, &|text, inner| {
            r::rewrite_links_to(text, inner, from, to).1
        });
        self.refuse_under_live_pages(domain, &linking).await?;
        let src = access.abs(from);
        let dst = access.abs(to);
        // Every check that can refuse the move runs here first, as a screen,
        // so a refused move leaves an open room over the page alone; they
        // run again under the locks below, where they decide.
        if !src.exists() {
            return Err(EngineError::NotFound(format!(
                "no reflection '{from}' in domain '{domain}'"
            )));
        }
        self.refuse_taken_page_target(domain, &access, from, to)
            .await?;
        // The room over the page saves its text and ends; no room opens over
        // the old path until the move is done (the fence). Its sockets are
        // told where the page went once the move has committed, and
        // `internal` if it fails from here. After the gate, the path rules
        // and the screens, so a caller without the right never learns that a
        // page is open and a refused move leaves the room open; before the
        // page lock, which a room's save takes.
        let closing = self
            .close_room_for_move(
                domain,
                RoomDoc::Reflection(from.to_string()),
                None,
                MovedTo {
                    domain: None,
                    path: to.to_string(),
                    permalink: None,
                },
            )
            .await;
        // The test seam sits between the close and the file lock because that
        // window is the one R81 named: the fence keeps a room from opening in
        // it.
        self.take_room_screen_hold().await;
        // Both pages' locks, in lock-key order, so two crossing moves cannot
        // wait on each other; two spellings of one file take its lock once.
        // Held until the moved page is indexed.
        let _locks = self.lock_files(&[&src, &dst]).await;
        let text = read_opt(&src)?.ok_or_else(|| {
            EngineError::NotFound(format!("no reflection '{from}' in domain '{domain}'"))
        })?;
        self.refuse_taken_page_target(domain, &access, from, to)
            .await?;
        // The page's own relative links follow the move, and its
        // baseline gets the same rewrite with the `reflected` hashes carried,
        // so a move owes no feedback.
        let old_baseline = read_opt(&access.abs(&r::baseline_path(from)))?;
        let moved = r::relink(&text, old_baseline.as_deref().unwrap_or(""), &|s| {
            r::rebase_links(s, from, to)
        })
        .map_err(EngineError::Invalid)?;
        // The row is built from the text as written, so it matches the file.
        let page = crystalline_core::to_lf(&moved.page).into_owned();
        let mut undo = DiskUndo::default();
        for (old_rel, new_rel) in [
            (from.to_string(), to.to_string()),
            (r::baseline_path(from), r::baseline_path(to)),
            (r::threads_path(from), r::threads_path(to)),
        ] {
            let (old, new) = (access.abs(&old_rel), access.abs(&new_rel));
            if is_case_only_rename(&old, &new, &old_rel, &new_rel) {
                undo.record_case_rename(&old, &new)?;
            } else {
                undo.record(&old)?;
                undo.record(&new)?;
            }
        }
        let replace = |old_rel: &str, new_rel: &str, text: &str| {
            let (old, new) = (access.abs(old_rel), access.abs(new_rel));
            let case_only = is_case_only_rename(&old, &new, old_rel, new_rel);
            replace_file_with(&old, &new, case_only, |p| write_file(p, text))
        };
        replace(from, to, &page)?;
        if old_baseline.is_some() {
            replace(
                &r::baseline_path(from),
                &r::baseline_path(to),
                &moved.baseline,
            )?;
        }
        let mut threads_moved = false;
        if let Some(body) = read_opt(&access.abs(&r::threads_path(from)))? {
            replace(&r::threads_path(from), &r::threads_path(to), &body)?;
            threads_moved = true;
        }
        // The old row goes and the new one comes in one fenced commit.
        self.commit_reflection_rows(
            access.domain_id,
            &[
                RowOp::DeletePage(from),
                RowOp::Page {
                    inner: to,
                    text: &page,
                    abs: &dst,
                },
            ],
            OnRefusal::put_back(domain, undo),
        )
        .await?;
        self.move_disclosures(access.domain_id, from, to).await?;
        drop(_locks);
        // The page has moved: the room's sockets learn its new path now, and
        // the fence lifts.
        if let Some(closing) = closing {
            closing.moved().await;
        }
        self.announce_reflection(
            domain,
            to,
            ReflectionChangeKind::Moved,
            Some(from),
            change_label(&caller.scope),
        );
        // Every other page that links the moved one, PACT.md included: a link
        // that follows a move is no change of the agreement, and a move needs
        // only edit access. Each page under its own lock. The move has
        // landed by now, so a page that cannot be relinked is named in the
        // receipt (and the log) instead of failing a move that happened.
        let mut failed: Vec<Value> = Vec::new();
        let mut note_failure = |path: &str, e: EngineError| {
            tracing::warn!(
                domain,
                from,
                to,
                page = path,
                "a link to the moved reflection could not be rewritten: {e}"
            );
            super::move_::note_link_failure(&mut failed, domain, path, &e);
        };
        let mut links = 0;
        for row in rows.iter().filter(|row| row.path != from && row.path != to) {
            let inner = row.path.as_str();
            match self
                .relink_reflection(domain, &access, inner, &|s| {
                    r::rewrite_links_to(s, inner, from, to)
                })
                .await
            {
                Ok(n) => {
                    links += n;
                    if n > 0 {
                        // A link that followed the move: nobody's change.
                        self.announce_reflection(
                            domain,
                            inner,
                            ReflectionChangeKind::Changed,
                            None,
                            None,
                        );
                    }
                }
                Err(e) => note_failure(inner, e),
            }
        }
        // The move and every page it relinked.
        self.mark_reflection_dirty(domain, BackupCause::Settled);
        let mut receipt = json!({ "from": from, "to": to, "links_rewritten": links, "threads_moved": threads_moved });
        if !failed.is_empty() {
            receipt["links_failed"] = json!(failed);
        }
        Ok(receipt)
    }

    /// Refuse a page move whose target is taken (the error ends with
    /// [`TARGET_TAKEN`]): a page, a threads file or a
    /// baseline left at `to` would be adopted by the moved page, and a page
    /// only the previous host of a host move has is the only shared copy.
    /// Asked once as the move's screen and again under its page locks.
    async fn refuse_taken_page_target(
        &self,
        domain: &str,
        access: &ReflectionAccess,
        from: &str,
        to: &str,
    ) -> Result<()> {
        for (old, leftover, what) in [
            (from.to_string(), to.to_string(), "a reflection"),
            (r::threads_path(from), r::threads_path(to), "a threads file"),
            (r::baseline_path(from), r::baseline_path(to), "a baseline"),
        ] {
            // The page itself under another letter case is the same file.
            let itself =
                is_case_only_rename(&access.abs(&old), &access.abs(&leftover), &old, &leftover);
            if access.abs(&leftover).exists() && !itself {
                return Err(EngineError::Conflict(format!(
                    "'{leftover}' already holds {what}{TARGET_TAKEN}"
                )));
            }
        }
        if !access.abs(to).exists() {
            self.refuse_only_elsewhere(domain, access.domain_id, ElsewhereRow::Page(to))
                .await?;
        }
        Ok(())
    }

    /// Rewrite the links of one reflection and give its baseline the same
    /// rewrite (`crystalline_core::reflection::relink`): a link that
    /// follows a moved page (or a moved file) is not a person's
    /// change, so it owes no feedback item. Under the page's write lock; the
    /// index row follows. Answers how many links changed; a page that is gone
    /// or holds no such link is left alone.
    pub(super) async fn relink_reflection(
        &self,
        domain: &str,
        access: &ReflectionAccess,
        inner: &str,
        rewrite: &(dyn Fn(&str) -> (String, usize) + Sync),
    ) -> Result<usize> {
        self.relink_page(domain, access.domain_id, &access.folder, inner, rewrite)
            .await
    }

    /// [`Engine::relink_reflection`] for a page of the Reflection folder
    /// `folder` of the domain `domain_id`, for a caller that holds no
    /// [`ReflectionAccess`] (the domain rename's respell).
    pub(super) async fn relink_page(
        &self,
        domain: &str,
        domain_id: DomainId,
        folder: &Path,
        inner: &str,
        rewrite: &(dyn Fn(&str) -> (String, usize) + Sync),
    ) -> Result<usize> {
        let abs = join_rel(folder, inner);
        let baseline_abs = join_rel(folder, &r::baseline_path(inner));
        let _lock = self.lock_file(&abs).await;
        let Some(page) = read_opt(&abs)? else {
            return Ok(0);
        };
        let baseline = read_opt(&baseline_abs)?;
        let out = r::relink(&page, baseline.as_deref().unwrap_or(""), rewrite)
            .map_err(EngineError::Invalid)?;
        if out.links == 0 {
            return Ok(0);
        }
        let page = crystalline_core::to_lf(&out.page).into_owned();
        let mut undo = DiskUndo::default();
        undo.record(&abs)?;
        undo.record(&baseline_abs)?;
        write_file(&abs, &page)?;
        if baseline.is_some() {
            write_file(&baseline_abs, &out.baseline)?;
        }
        // A rewrite of links is nobody's text, so a refusal keeps nothing.
        self.index_reflection(
            domain_id,
            inner,
            &page,
            &abs,
            OnRefusal::put_back(domain, undo),
        )
        .await?;
        Ok(out.links)
    }

    pub async fn delete_reflection(
        &self,
        domain: &str,
        inner: &str,
        caller: &ReflectionCaller,
    ) -> Result<()> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let access = self
            .require_reflection_to_write(domain, caller, ReflectionRight::Edit)
            .await?;
        self.refuse_read_only_domain(domain, &crate::scope::Scope::Unrestricted)
            .await?;
        // After the gate, as in `move_reflection`.
        self.refuse_under_live_pages(domain, &[inner.to_string()])
            .await?;
        r::validate_reflection_path(inner).map_err(EngineError::Invalid)?;
        if inner == r::PACT_FILE {
            return Err(EngineError::Invalid(
                "PACT.md is the domain's agreement; edit it instead of deleting it".into(),
            ));
        }
        let abs = access.abs(inner);
        // The test seam sits between the screen and the file lock because that
        // window is the one R81 pins: a room may open in it.
        self.take_room_screen_hold().await;
        let _lock = self.lock_file(&abs).await;
        let threads_abs = access.abs(&r::threads_path(inner));
        let baseline_abs = access.abs(&r::baseline_path(inner));
        let mut undo = DiskUndo::default();
        for file in [&abs, &threads_abs, &baseline_abs] {
            undo.record(file)?;
        }
        if !remove_if_there(&abs)? {
            return Err(EngineError::NotFound(format!(
                "no reflection '{inner}' in domain '{domain}'"
            )));
        }
        remove_if_there(&threads_abs)?;
        remove_if_there(&baseline_abs)?;
        self.commit_reflection_rows(
            access.domain_id,
            &[RowOp::DeletePage(inner)],
            OnRefusal::put_back(domain, undo),
        )
        .await?;
        self.drop_disclosures(access.domain_id, inner).await?;
        self.mark_reflection_dirty(domain, BackupCause::Settled);
        self.announce_reflection(
            domain,
            inner,
            ReflectionChangeKind::Deleted,
            None,
            change_label(&caller.scope),
        );
        Ok(())
    }

    pub async fn settle_reflection(
        &self,
        domain: &str,
        inner: &str,
        ids: &[String],
        outcome: &str,
        caller: &ReflectionCaller,
    ) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        if !matches!(outcome, "kept" | "taken_in") {
            return Err(EngineError::Invalid(format!(
                "`{outcome}` is not a settle outcome; use kept or taken_in"
            )));
        }
        let access = self
            .require_reflection_to_write(domain, caller, ReflectionRight::Curate)
            .await?;
        self.refuse_read_only_domain(domain, &crate::scope::Scope::Unrestricted)
            .await?;
        // After the gate, so a caller without the right learns nothing about
        // the path; before any file is touched.
        r::validate_reflection_path(inner).map_err(EngineError::Invalid)?;
        let abs = access.abs(inner);
        let baseline_abs = access.abs(&r::baseline_path(inner));
        let _lock = self.lock_file(&abs).await;
        let text = read_opt(&abs)?.ok_or_else(|| {
            EngineError::NotFound(format!("no reflection '{inner}' in domain '{domain}'"))
        })?;
        let baseline = read_opt(&baseline_abs)?.unwrap_or_default();
        let sections = r::split_sections(&text);
        // No ids settles the whole page: every section on it, the top
        // included, and every section its baseline holds that the page no
        // longer does.
        let ids: Vec<String> = if ids.is_empty() {
            let mut every: Vec<String> = sections.iter().filter_map(|s| s.id.clone()).collect();
            for id in r::baseline_ids(&baseline) {
                if !every.contains(&id) {
                    every.push(id);
                }
            }
            every
        } else {
            ids.to_vec()
        };
        let ids = ids.as_slice();
        let mut prov = r::read_provenance(&text);
        // A take-in reads the facts of the settled sections' sources only,
        // from the page as it stands under the file lock. The store lock is
        // taken inside the file lock, the order `index_reflection` below
        // already uses.
        // A source in a domain the settler may not open is not refreshed:
        // they cannot have taken in what they could not read.
        let cited: Vec<r::SourceRef> = if outcome == "taken_in" {
            ids.iter()
                .filter_map(|id| prov.get(id))
                .flat_map(|record| record.sources.iter().cloned())
                .collect()
        } else {
            Vec::new()
        };
        let sight = if cited.iter().any(|src| src.domain.is_some()) {
            Some(self.source_sight(&caller.scope).await?)
        } else {
            None
        };
        let sees = |src: &r::SourceRef| sight.as_ref().is_none_or(|sight| sight.sees(src));
        let facts = self
            .source_facts(
                domain,
                access.domain_id,
                None,
                cited.iter().filter(|src| sees(src)),
            )
            .await?;
        let known: BTreeSet<String> = sections
            .iter()
            .filter_map(|s| s.id.clone())
            .chain(r::baseline_ids(&baseline))
            .collect();
        for id in ids {
            if !known.contains(id) {
                return Err(EngineError::Invalid(format!(
                    "no section '{id}' in '{inner}' or its baseline"
                )));
            }
            if let Some(section) = sections
                .iter()
                .find(|s| s.id.as_deref() == Some(id.as_str()))
            {
                let record = prov.entry(id.clone()).or_default();
                record.reflected = Some(r::section_hash(&section.text));
                record.changed_by.clear();
                record.changed_at = None;
                record.answered_by = None;
                if outcome == "taken_in" {
                    for src in &mut record.sources {
                        if !sees(src) {
                            continue;
                        }
                        if let Some(fact) = facts.fact(src) {
                            src.checksum = fact.checksum.clone();
                        }
                    }
                }
            }
        }
        let final_text = stamp_reflected(&text, &mut prov, ids)?;
        let mut undo = DiskUndo::default();
        undo.record(&abs)?;
        undo.record(&baseline_abs)?;
        write_file(&abs, &final_text)?;
        write_file(
            &baseline_abs,
            &r::advance_baseline(&baseline, &final_text, ids),
        )?;
        // A settle sends no text, so a refusal keeps nothing.
        self.index_reflection(
            access.domain_id,
            inner,
            &final_text,
            &abs,
            OnRefusal::put_back(domain, undo),
        )
        .await?;
        self.mark_reflection_dirty(domain, BackupCause::Settled);
        self.announce_reflection(
            domain,
            inner,
            ReflectionChangeKind::Changed,
            None,
            change_label(&caller.scope),
        );
        Ok(json!({ "domain": domain, "path": inner, "settled": ids, "outcome": outcome }))
    }
}

/// Write `text` at `new` and remove `old`. For a case-only rename (see
/// `is_case_only_rename`) the file is renamed first, which changes its
/// spelling atomically and keeps it on disk under one name or the other if the
/// write then fails; `write` is the write, a seam for the failing-write test.
fn replace_file_with(
    old: &Path,
    new: &Path,
    case_only: bool,
    write: impl FnOnce(&Path) -> Result<()>,
) -> Result<()> {
    if case_only {
        std::fs::rename(old, new).map_err(|source| EngineError::Io {
            path: new.display().to_string(),
            source,
        })?;
        write(new)
    } else {
        write(new)?;
        remove_if_there(old).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A write that fails after a case-only rename leaves the file on disk,
    /// under the new name, with its old text.
    #[test]
    fn a_failed_write_after_a_case_only_rename_loses_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let (old, new) = (tmp.path().join("a.md"), tmp.path().join("A.md"));
        std::fs::write(&old, "the only copy").unwrap();
        let failed = replace_file_with(&old, &new, true, |_| {
            Err(EngineError::Internal("disk full".into()))
        });
        assert!(failed.is_err());
        let names: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["A.md"]);
        assert_eq!(std::fs::read_to_string(&new).unwrap(), "the only copy");
    }

    const OFF: &str = "---\ntype: manifest\ntitle: eng\nstatus: stable\n---\n\n# eng\n";
    const ON: &str =
        "---\ntype: manifest\ntitle: eng\nstatus: stable\nreflection: on\n---\n\n# eng\n";

    /// Every kind that holds a switch-on is named in its own sentence, each
    /// starting with a capital letter; the encoding advice goes only with
    /// files that open and do not read as engrams.
    #[test]
    fn the_switch_on_refusal_names_each_kind_in_its_own_sentence() {
        let facts = SwitchFacts {
            engrams: vec!["reflections/a.md".into()],
            broken: vec!["reflections/b.md".into()],
            unopened: vec!["reflections/c.md".into()],
            ..Default::default()
        };
        let err = screen_switch(OFF, ON, &facts).unwrap_err().to_string();
        assert_eq!(
            err,
            "These engrams live in reflections/, which Reflection would take over: reflections/a.md. Move them to another folder first (move_engram). These files in reflections/ do not read as engrams now: reflections/b.md. Fix them so they read again (frontmatter that parses, UTF-8 text), then move them out. These files or folders in reflections/ cannot be opened: reflections/c.md. Make them readable for Crystalline, then move them out. Then switch Reflection on"
        );
        let unopened = SwitchFacts {
            unopened: vec!["reflections/c.md".into()],
            ..Default::default()
        };
        let err = screen_switch(OFF, ON, &unopened).unwrap_err().to_string();
        assert!(!err.contains("UTF-8"), "{err}");
    }

    /// A file that opens and does not read as an engram is broken; one that
    /// cannot be opened, and a folder that cannot be read, are unopened.
    #[cfg(unix)]
    #[test]
    fn pending_names_tell_a_broken_file_from_one_that_cannot_be_opened() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("reflections")).unwrap();
        std::fs::write(root.join("reflections/ok.md"), "# Ok\n").unwrap();
        std::fs::write(root.join("reflections/broken.md"), "---\n: [\n---\n").unwrap();
        std::fs::write(root.join("reflections/latin.md"), b"# Caf\xe9\n").unwrap();
        let shut = root.join("reflections/shut.md");
        std::fs::write(&shut, "# Shut\n").unwrap();
        std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o000)).unwrap();
        let blocked = std::fs::read(&shut).is_err();
        let names: Vec<String> = [
            "reflections/ok.md",
            "reflections/broken.md",
            "reflections/latin.md",
            "reflections/shut.md",
            "reflections/sub/",
        ]
        .map(String::from)
        .to_vec();
        let named = sort_pending(root, &names);
        std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(named.engrams, ["reflections/ok.md"]);
        assert_eq!(
            named.broken,
            ["reflections/broken.md", "reflections/latin.md"]
        );
        if blocked {
            assert_eq!(named.unopened, ["reflections/shut.md", "reflections/sub/"]);
        }
        assert!(named.spellings.is_empty());
    }

    /// Two spellings of the folder collide on the Reflection's rows, so the
    /// switch-on names both; a filesystem that folds case (the macOS and
    /// Windows defaults) cannot hold both, so the screen is pinned here.
    #[test]
    fn two_spellings_of_the_folder_refuse_the_switch_and_name_both() {
        let facts = SwitchFacts {
            folders: vec!["Reflections".into(), "reflections".into()],
            ..Default::default()
        };
        let err = screen_switch(OFF, ON, &facts).unwrap_err().to_string();
        assert!(
            err.contains("Reflections/") && err.contains("reflections/"),
            "{err}"
        );
        // One folder in any spelling is the Reflection's folder.
        let one = SwitchFacts {
            folders: vec!["Reflections".into()],
            ..Default::default()
        };
        screen_switch(OFF, ON, &one).unwrap();
        // A switch-off is never screened.
        screen_switch(ON, OFF, &facts).unwrap();
    }

    /// The gate's fast path settles "nothing pending" only over rows that
    /// lie outside the folder: no row at all is an empty index, and a row
    /// under the folder is the predicate's to judge.
    #[test]
    fn only_rows_outside_the_folder_settle_nothing_pending() {
        let tmp = tempfile::tempdir().unwrap();
        let empty = crate::engine::RecordedPaths::default();
        assert!(!empty.nothing_pending(tmp.path()), "an empty index");
        let outside = crate::engine::RecordedPaths {
            any: true,
            under_reflections: Vec::new(),
        };
        assert!(outside.nothing_pending(tmp.path()));
        let under = crate::engine::RecordedPaths {
            any: true,
            under_reflections: vec!["Reflections/a.md".into()],
        };
        assert!(!under.nothing_pending(tmp.path()));
    }

    /// A write goes into the Reflection's folder as it is spelled on disk;
    /// with no folder yet it is `reflections`.
    #[test]
    fn a_reflection_path_takes_the_folder_spelling_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(reflection_abs(tmp.path(), "guides/a.md").ends_with("reflections/guides/a.md"));
        std::fs::create_dir_all(tmp.path().join("Reflections")).unwrap();
        assert!(reflection_abs(tmp.path(), "guides/a.md").ends_with("Reflections/guides/a.md"));
    }

    /// A section that gets its id in this compose records the names the room
    /// carried for its heading, even when nobody is named as having changed
    /// anything in this save; a section nobody touched names nobody.
    #[test]
    fn a_section_that_gets_its_id_records_the_names_carried_for_its_heading() {
        let current = "# A\n\n## One\n<!-- r:aaaa1111 -->\n\nFirst.\n\n## Two\n<!-- r:bbbb2222 -->\n\nSecond.\n";
        let incoming = format!("{current}\n## Three\n\nThird.\n");
        let authors = SectionAuthors {
            changed: vec![],
            new_by_heading: BTreeMap::from([(
                ("Three".to_string(), 0),
                BTreeSet::from(["ada".to_string()]),
            )]),
            someone_if_empty: false,
        };
        let Composed {
            text, minted, prov, ..
        } = compose_reflection(
            Some(current),
            "",
            &incoming,
            &authors,
            true,
            "2026-10-15T00:00:00Z",
        )
        .unwrap();
        assert_eq!(minted.len(), 1, "{text}");
        let three = r::split_sections(&text)
            .into_iter()
            .find(|s| s.heading == "Three")
            .and_then(|s| s.id)
            .expect("Three got an id");
        assert_eq!(prov[&three].changed_by, vec!["ada".to_string()], "{text}");
        assert!(prov["aaaa1111"].changed_by.is_empty(), "{prov:?}");
        assert!(prov["bbbb2222"].changed_by.is_empty(), "{prov:?}");
    }

    /// A section that arrives with an id of its own (pasted from another
    /// page) is not one the room counted as id-less, so it takes no place
    /// among the carried names: the new section after it under the same
    /// heading still names its typist, and the pasted one names nobody.
    #[test]
    fn a_pasted_section_with_an_id_takes_no_place_among_the_carried_names() {
        let current = "# A\n\n## One\n<!-- r:aaaa1111 -->\n\nFirst.\n";
        let incoming =
            format!("{current}\n## Three\n<!-- r:cccc3333 -->\n\nPasted.\n\n## Three\n\nThird.\n");
        let authors = SectionAuthors {
            changed: vec![],
            new_by_heading: BTreeMap::from([(
                ("Three".to_string(), 0),
                BTreeSet::from(["ada".to_string()]),
            )]),
            someone_if_empty: false,
        };
        let Composed {
            text, minted, prov, ..
        } = compose_reflection(
            Some(current),
            "",
            &incoming,
            &authors,
            true,
            "2026-10-15T00:00:00Z",
        )
        .unwrap();
        assert_eq!(minted.len(), 1, "{text}");
        assert_eq!(
            prov[&minted[0]].changed_by,
            vec!["ada".to_string()],
            "{text}"
        );
        assert!(prov["cccc3333"].changed_by.is_empty(), "{prov:?}");
    }

    #[test]
    fn the_starter_pact_says_how_structure_is_read() {
        let pact = pact_starter("eng");
        let structure = crystalline_core::reflection::split_sections(&pact)
            .into_iter()
            .find(|s| s.heading == "Structure")
            .unwrap();
        assert!(
            structure.text.contains(
                "One glob per bullet, for example `- guides/*.md`. Write the reasons in a sentence above the list."
            ),
            "{pact}"
        );
    }

    #[test]
    fn the_starter_pact_has_a_pages_part() {
        let pact = pact_starter("eng");
        let sections = crystalline_core::reflection::split_sections(&pact);
        let last = sections.last().unwrap();
        assert_eq!(last.heading, "Pages", "{pact}");
        assert!(
            last.text.contains(
                "- One bullet per page or space a person asked for: its path or glob, what they asked for in their words, who asked and when."
            ),
            "{pact}"
        );
        // The bullet is guidance, never a structure glob.
        assert_eq!(
            crystalline_core::reflection::structure_globs(&pact),
            vec!["*.md".to_string()]
        );
    }

    /// The seeded pact carries a section id line under every heading, so an
    /// unedited pact is a marked page and never holds the switch pending. Its
    /// five sections come in a fixed order, the globs read from the
    /// Structure section by its heading.
    #[test]
    fn the_seeded_pact_is_a_marked_page() {
        let tmp = tempfile::tempdir().unwrap();
        let folder = tmp.path().join("reflections");
        std::fs::create_dir_all(&folder).unwrap();
        let pact = pact_starter("eng");
        std::fs::write(folder.join("PACT.md"), &pact).unwrap();
        assert!(
            !crystalline_core::is_pending_engram(
                tmp.path(),
                "reflections/PACT.md",
                crystalline_core::EngramRow::Unknown
            ),
            "{pact}"
        );
        let sections = crystalline_core::reflection::split_sections(&pact);
        let headings: Vec<&str> = sections
            .iter()
            .skip(1)
            .map(|s| s.heading.as_str())
            .collect();
        assert_eq!(
            headings,
            [
                "Audience",
                "Level of detail",
                "Presentation",
                "Structure",
                "Pages"
            ],
            "{pact}"
        );
        let ids = sections.iter().skip(1).filter(|s| s.id.is_some()).count();
        assert_eq!(ids, 5, "{pact}");
        assert_eq!(
            crystalline_core::reflection::structure_globs(&pact),
            vec!["*.md".to_string()]
        );
    }
}
