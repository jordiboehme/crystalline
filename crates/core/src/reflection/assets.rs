//! Files in a Reflection: what a reflection's links point at.
//!
//! The rules for what may be attached (the extension allowlist, the size cap,
//! the path rules) are [`crate::attachment`]'s, unchanged: a Reflection's file
//! is `reflections/assets/<rel>`, and its path inside `reflections/`,
//! `assets/<rel>`, is exactly what [`crate::validate_asset_path`] accepts.
//! Only the way a page points at a file differs: an engram names an
//! attachment from the domain root, a reflection with a relative link from
//! its own folder. A reflection may not embed an engram attachment:
//! the file is copied into `reflections/assets/` first.

use crate::address::{CrystallineUrl, SCHEME};
use crate::attachment::{ASSETS_PREFIX, canonical_asset_path, is_under_assets, link_targets};
use crate::kind::ContentKind;
use crate::reflection::layout::{domain_rel, is_under_reflections};
use crate::reflection::links::{relative_to, resolve};

/// Where one link target of a reflection points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssetTarget {
    /// A file under `reflections/assets/`, by its canonical path inside
    /// `reflections/` (`assets/<rel>`).
    Reflection(String),
    /// An engram attachment: the domain root's `assets/`, reached by a link
    /// that climbs out of `reflections/` or starts with `/`, or any
    /// `crystalline://<domain>/assets/...` address.
    Engram,
    /// Anything else: another reflection, a web page, a fragment.
    Other,
}

/// Classify one link target of the reflection at `reflection_path` (a path
/// inside `reflections/`). `target` is what the shared link target scan
/// (`crate::attachment`'s `link_targets`) hands out: percent-decoded, without
/// a leading `./`, a title or a `#fragment`. A backslash is a separator, as a
/// browser reads it.
pub fn classify_target(target: &str, reflection_path: &str) -> AssetTarget {
    if target.starts_with(SCHEME) {
        return match CrystallineUrl::parse(target) {
            Some(url) if matches!(url.asset_ref(), Some((ContentKind::Engram, _))) => {
                AssetTarget::Engram
            }
            _ => AssetTarget::Other,
        };
    }
    // Any other URL scheme (RFC 3986, case-insensitive) is a URL, never a
    // file: `javascript:x//../assets/a.png` and a drive letter `C:/a.png`
    // included.
    if target.is_empty() || has_url_scheme(target) {
        return AssetTarget::Other;
    }
    // A browser resolving a relative link reads `\` as `/`, so a backslash
    // spelling reaches the same file and is classified like the `/` form.
    let target = target.replace('\\', "/");
    let target = target.as_str();
    if let Some(rooted) = target.strip_prefix('/') {
        return if is_under_assets(rooted) {
            AssetTarget::Engram
        } else {
            AssetTarget::Other
        };
    }
    // Resolved from the reflection's folder inside the domain, so a link that
    // climbs out of reflections/ is followed to where it lands. Above the
    // domain root it is nobody's file.
    let Some(landed) = resolve(&domain_rel(reflection_path), target) else {
        return AssetTarget::Other;
    };
    if is_under_reflections(&landed) {
        let inner = landed.split_once('/').map(|(_, rest)| rest).unwrap_or("");
        return match canonical_asset_path(inner) {
            Some(path) if path.len() > ASSETS_PREFIX.len() => AssetTarget::Reflection(path),
            _ => AssetTarget::Other,
        };
    }
    if is_under_assets(&landed) {
        AssetTarget::Engram
    } else {
        AssetTarget::Other
    }
}

/// Whether `target` starts with an RFC 3986 scheme: a letter, then letters,
/// digits, `+`, `-` or `.`, then `:`, before any `/`, `?` or `#`.
fn has_url_scheme(target: &str) -> bool {
    let mut chars = target.chars();
    if !chars.next().is_some_and(|c| c.is_ascii_alphabetic()) {
        return false;
    }
    for c in chars {
        match c {
            ':' => return true,
            c if c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.') => {}
            _ => return false,
        }
    }
    false
}

/// The distinct files under `reflections/assets/` a reflection (or any part
/// of one, a section's text for example) references, by path inside
/// `reflections/`, in order of first appearance. Fenced code never counts.
pub fn reflection_asset_refs(source: &str, reflection_path: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for target in link_targets(source) {
        if let AssetTarget::Reflection(path) = classify_target(&target, reflection_path)
            && !out.contains(&path)
        {
            out.push(path);
        }
    }
    out
}

/// The distinct link targets of a reflection that reach an engram
/// attachment, as written (percent-decoded), in order.
pub fn engram_attachment_embeds(source: &str, reflection_path: &str) -> Vec<String> {
    link_targets(source)
        .into_iter()
        .filter(|target| classify_target(target, reflection_path) == AssetTarget::Engram)
        .collect()
}

/// Why a reflection's text cannot be written: it embeds an engram attachment.
/// The sentence names the links and the relative link the file would have
/// once it is copied into `reflections/assets/`.
pub fn refuse_engram_attachments(source: &str, reflection_path: &str) -> Result<(), String> {
    let found = engram_attachment_embeds(source, reflection_path);
    let Some(first) = found.first() else {
        return Ok(());
    };
    let name = first.rsplit(['/', '\\']).next().unwrap_or(first);
    let suggestion = relative_to(reflection_path, &format!("{ASSETS_PREFIX}{name}"));
    Err(format!(
        "a reflection cannot embed an engram attachment ({}); copy the file into reflections/assets/ and link it from there, for example {suggestion} (read it with read_engram, then write it with write_reflection action write_asset; to copy a picture, read it with include_images: false, then write_asset with that base64)",
        found.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_resolve_from_the_reflections_own_folder() {
        let page = "# A\n\n![flow](../assets/flow.png#right,w=50%) and [deck](../assets/deep/deck.pdf \"Q3\")\n![again](./../assets/flow.png)\n![size](../assets/gr%C3%B6%C3%9Fe.png)\n\n```\n![fenced](../assets/fenced.png)\n```\n\n[peer](peer.md) [web](https://x.test/assets/a.png) [top](#top)\n";
        assert_eq!(
            reflection_asset_refs(page, "guides/a.md"),
            [
                "assets/flow.png",
                "assets/deep/deck.pdf",
                "assets/größe.png"
            ]
        );
        assert_eq!(
            reflection_asset_refs("![flow](assets/flow.png)\n", "a.md"),
            ["assets/flow.png"],
            "from the folder root, assets/ is the Reflection's own folder"
        );
        assert!(
            reflection_asset_refs("![flow](../assets/flow.png)\n", "a.md").is_empty(),
            "that link climbs out to the engram attachments"
        );
    }

    /// The discriminating pair: one link, two places, two answers.
    #[test]
    fn an_engram_attachment_is_told_apart_from_a_reflection_file() {
        assert_eq!(
            classify_target("../assets/flow.png", "a.md"),
            AssetTarget::Engram
        );
        assert_eq!(
            classify_target("../assets/flow.png", "guides/a.md"),
            AssetTarget::Reflection("assets/flow.png".into())
        );
        assert_eq!(
            classify_target("../../assets/flow.png", "guides/a.md"),
            AssetTarget::Engram
        );
        assert_eq!(
            classify_target("crystalline://eng/assets/flow.png", "a.md"),
            AssetTarget::Engram
        );
        assert_eq!(
            classify_target("crystalline://other/assets/flow.png", "a.md"),
            AssetTarget::Engram,
            "any domain's attachment"
        );
        assert_eq!(
            classify_target("/assets/flow.png", "a.md"),
            AssetTarget::Engram
        );
        assert_eq!(
            classify_target("crystalline://eng/runbooks/deploy", "a.md"),
            AssetTarget::Other
        );
        assert_eq!(
            classify_target("crystalline://eng/reflections/assets/flow.png", "a.md"),
            AssetTarget::Other,
            "an address is no reference"
        );
        assert_eq!(
            classify_target("../../../assets/x.png", "guides/a.md"),
            AssetTarget::Other,
            "above the domain root is nobody's file"
        );
        assert_eq!(classify_target("peer.md", "a.md"), AssetTarget::Other);
        assert_eq!(
            classify_target("https://x.test/assets/a.png", "a.md"),
            AssetTarget::Other
        );
    }

    #[test]
    fn the_refusal_names_the_link_and_where_to_copy_it() {
        assert!(refuse_engram_attachments("![ok](../assets/flow.png)\n", "guides/a.md").is_ok());
        let err =
            refuse_engram_attachments("# A\n\n![shot](../assets/shot.png)\n", "a.md").unwrap_err();
        assert!(err.contains("../assets/shot.png"), "names the link: {err}");
        assert!(
            err.contains("reflections/assets/") && err.contains("for example assets/shot.png"),
            "says where to copy it and the link it would have: {err}"
        );
        assert!(
            err.contains("read_engram") && err.contains("write_asset"),
            "tells the agent how to copy the file: {err}"
        );
        assert!(
            refuse_engram_attachments("```\n![x](../assets/shot.png)\n```\n", "a.md").is_ok(),
            "a fenced example is no embed"
        );
        assert_eq!(
            engram_attachment_embeds(
                "![a](crystalline://eng/assets/a.png) ![b](/assets/b.png)\n",
                "a.md"
            ),
            ["crystalline://eng/assets/a.png", "/assets/b.png"]
        );
    }

    /// A browser resolving a relative link turns `\` into `/`, so a
    /// backslash spelling reaches the same file and is read like the `/` form.
    #[test]
    fn a_backslash_is_a_separator_as_a_browser_reads_it() {
        assert_eq!(
            classify_target("..\\assets\\shot.png", "a.md"),
            AssetTarget::Engram
        );
        assert_eq!(
            classify_target("..\\assets\\flow.png", "guides/a.md"),
            AssetTarget::Reflection("assets/flow.png".into())
        );
        let err = refuse_engram_attachments("![shot](..\\assets\\shot.png)\n", "a.md").unwrap_err();
        assert!(
            err.contains("for example assets/shot.png"),
            "the suggestion names the file, not the whole link: {err}"
        );
    }
}
