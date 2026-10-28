//! The team repository rules (`M011`, `M012`, `M109`): what a repository a
//! team shares through GitHub must never hold, checked only by `crystalline
//! verify --team-repository` and the GitHub Action's `team-repository`
//! input.
//!
//! A shared domain's Reflection lives at one home, the team server: it fills
//! in `reflection_home` when Reflection is switched on there and backs the
//! folder up to the `crystalline/reflections` branch, and a share never
//! carries a file under `reflections/` while Reflection is on. A plain clone
//! of the repository skips both checks, so a person can switch Reflection
//! on in it and push the result with git. These rules are where the
//! repository itself says no, as a required status check on the pull
//! request or the push.
//!
//! Two things set them apart from every other rule:
//!
//! - **The domain's own settings cannot lower them.** A `.crystalline.yaml`
//!   in the same pull request could otherwise set them `off`, and a guard
//!   the guarded change can switch off guards nothing. `--strict` still
//!   promotes `M109`, since that only makes the check harder.
//! - **The message carries the whole story.** A team member reads the
//!   finding in a pull request comment or an annotation, which show the
//!   message alone, often without knowing how the Reflection works. So each
//!   message says what is wrong, why it is wrong in a team repository and
//!   what to do now, and the `fix` field stays empty rather than repeating
//!   the last part.
//!
//! With Reflection off, `reflections/` is ordinary content a team may share
//! like any other folder, so nothing here looks at it then.

use std::path::Path;

use walkdir::WalkDir;

use crate::manifest::{
    Manifest, REFLECTION_HOME_KEY, REFLECTION_KEY, ReflectionSwitch, TEAM_KEY, TeamForge,
    validate_reflection_home,
};
use crate::reflection::reflection_folders;

use super::scanner::Domain;
use super::{Severity, Sink};

/// How many paths the `M012` finding names before it counts the rest.
const LISTED_PATHS: usize = 20;

/// Why a Reflection kept in the repository is wrong, said the same way in
/// both Reflection findings.
const HOME_ONLY: &str =
    "In a team repository the Reflection lives only at its home, the team server";

/// Run the team repository rules over one scanned domain. A domain with no
/// MANIFEST, or one that does not parse, has nothing to check here: `M001`
/// and the format rules already report it.
pub(crate) fn check(domain: &Domain, sink: &mut Sink) {
    let Some(idx) = domain.manifest_index else {
        return;
    };
    let file = &domain.files[idx];
    let Ok(engram) = &file.parsed else { return };
    let manifest = Manifest::from_engram(engram, &file.source);
    let path = &file.path;

    // `M109`: the marker is missing. A warning: every repository from
    // before 0.30.0 lacks it until its next share from a connected
    // instance, and an unknown value is `M010`'s finding, not this one.
    if manifest.declared_team().is_none() {
        let github = TeamForge::GitHub.as_str();
        sink.emit(
            path,
            None,
            "M109",
            Severity::Warning,
            format!(
                "The MANIFEST has no `{TEAM_KEY}: {github}` line. The line marks this repository as a team domain, so Crystalline refuses to switch Reflection on in a clone that is not connected to the team. Share once from an instance connected to the domain; the share adds the line."
            ),
            None,
        );
    }

    if manifest.reflection() != ReflectionSwitch::On {
        return;
    }

    // `M011`: Reflection is on with no home, or with a home that is not a
    // server address and so is no home at all.
    if manifest.reflection_home().is_none() {
        let what = match manifest.declared_reflection_home() {
            None => format!(
                "The MANIFEST says `{REFLECTION_KEY}: on` and has no `{REFLECTION_HOME_KEY}`."
            ),
            Some(value) => {
                let why = validate_reflection_home(value)
                    .err()
                    .unwrap_or_else(|| format!("`{value}` is not a server address"));
                format!(
                    "The MANIFEST says `{REFLECTION_KEY}: on`, and its `{REFLECTION_HOME_KEY}` cannot be a home: {why}."
                )
            }
        };
        sink.emit(
            path,
            None,
            "M011",
            Severity::Error,
            format!(
                "{what} {HOME_ONLY}, which fills in `{REFLECTION_HOME_KEY}` when Reflection is switched on there. Without a home, any clone of this repository can keep its own Reflection and push its pages and threads to the shared branch. Set `{REFLECTION_KEY}` back to `off`, or switch Reflection on at the team server in the domain policies, so the home is filled in."
            ),
            None,
        );
    }

    // `M012`: files under `reflections/`, in any letter case, while
    // Reflection is on. One finding for the folder, not one per file: a
    // Reflection pushed by mistake holds pages, threads, baselines and
    // assets, and a comment listing each of them would bury the reason.
    let files = reflection_files(&domain.root);
    if files.is_empty() {
        return;
    }
    let folder = reflection_folders(&domain.root)
        .into_iter()
        .next()
        .unwrap_or_else(|| crate::reflection::REFLECTIONS_DIR.to_string());
    let mut listed = files
        .iter()
        .take(LISTED_PATHS)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if files.len() > LISTED_PATHS {
        listed.push_str(&format!(" and {} more", files.len() - LISTED_PATHS));
    }
    sink.emit(
        &domain.root.join(folder),
        None,
        "M012",
        Severity::Error,
        format!(
            "The MANIFEST says `{REFLECTION_KEY}: on`, and this branch holds {} under reflections/: {listed}. {HOME_ONLY}, and is backed up to the crystalline/reflections branch, never to the shared branch. Its pages and threads can hold comments from people who may not read the engrams. Remove these files from this branch, and switch Reflection on at the team server in the domain policies instead.",
            crate::text::plural(files.len(), "file", "files")
        ),
        None,
    );
}

/// Every file under the folders that spell `reflections/` in any letter
/// case, hidden ones included (a pushed Reflection carries its
/// `.reflected/` baselines too), as domain-relative forward-slash paths,
/// sorted. A folder that cannot be walked is skipped: the rule reports what
/// it can see.
fn reflection_files(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for folder in reflection_folders(root) {
        for entry in WalkDir::new(root.join(&folder)).into_iter().flatten() {
            if !entry.file_type().is_file() {
                continue;
            }
            if let Ok(rel) = entry.path().strip_prefix(root) {
                let parts: Vec<String> = rel
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect();
                out.push(parts.join("/"));
            }
        }
    }
    out.sort();
    out
}
