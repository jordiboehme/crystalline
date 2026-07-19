//! E-family rules: frontmatter format and encoding.
//!
//! `E001`-`E006` are unconditional errors: a well-formed Engram must parse,
//! carry the four required fields and use a lowercase slug permalink with
//! clean UTF-8. `E007` (tag format) and the outside-the-recommended-set half
//! of `E003` are softer: Crystalline never enforces a closed `type` or
//! `status` vocabulary, so those two only ever inform. `E008` warns when a
//! permalink starts with the domain's own name: a permalink is
//! domain-relative (the OKF Concept ID made explicit) and the domain name is
//! per-user configuration, so persisting it into a file misleads as soon as
//! the domain is registered under another name.

use crate::address::slugify;
use crate::engram::RECOMMENDED_TYPES;
use crate::parse::ParseError;

use super::scanner::ScannedFile;
use super::{Severity, Sink};

pub(crate) fn check(file: &ScannedFile, domain_name: &str, sink: &mut Sink) {
    let engram = match &file.parsed {
        Ok(e) => e,
        Err(ParseError::Bom) => {
            sink.emit(
                &file.path,
                None,
                "E006",
                Severity::Error,
                "file starts with a UTF-8 byte order mark",
                Some("save the file as UTF-8 without a BOM".into()),
            );
            return;
        }
        Err(ParseError::NullByte { line, .. }) => {
            sink.emit(
                &file.path,
                Some(*line),
                "E006",
                Severity::Error,
                "file contains a null byte",
                Some("remove the null byte and re-save as plain UTF-8 text".into()),
            );
            return;
        }
        Err(ParseError::Yaml { message }) => {
            sink.emit(
                &file.path,
                None,
                "E001",
                Severity::Error,
                format!("frontmatter YAML is invalid: {message}"),
                None,
            );
            return;
        }
        Err(ParseError::FrontmatterNotMapping) => {
            sink.emit(
                &file.path,
                None,
                "E001",
                Severity::Error,
                "frontmatter is not a mapping",
                None,
            );
            return;
        }
    };

    let fm = &engram.frontmatter;

    if fm.title.trim().is_empty() {
        sink.emit(
            &file.path,
            None,
            "E002",
            Severity::Error,
            "required field `title` is missing",
            None,
        );
    }
    if fm
        .permalink
        .as_deref()
        .map(str::trim)
        .unwrap_or("")
        .is_empty()
    {
        sink.emit(
            &file.path,
            None,
            "E002",
            Severity::Error,
            "required field `permalink` is missing",
            None,
        );
    }

    if fm.engram_type.trim().is_empty() {
        sink.emit(
            &file.path,
            None,
            "E003",
            Severity::Error,
            "required field `type` is missing",
            None,
        );
    } else if !RECOMMENDED_TYPES.contains(&fm.engram_type.as_str()) {
        sink.emit(
            &file.path,
            None,
            "E003",
            Severity::Info,
            format!("type `{}` is outside the recommended set", fm.engram_type),
            None,
        );
    }

    if fm.tags.is_empty() {
        sink.emit(
            &file.path,
            None,
            "E004",
            Severity::Error,
            "required field `tags` is missing or empty",
            None,
        );
    }

    if let Some(p) = &fm.permalink
        && !p.is_empty()
        && slugify(p) != *p
    {
        sink.emit(
            &file.path,
            None,
            "E005",
            Severity::Error,
            format!("permalink `{p}` is not a lowercase slug path"),
            Some(format!("use `{}`", slugify(p))),
        );
    }

    // E008: a permalink that opens with the domain's own name persists
    // per-user configuration into content. Exempt when the whole permalink
    // is just the file's own path slug: then the leading segment is a real
    // subfolder that happens to share the name, correct by path.
    if let Some(p) = &fm.permalink
        && let Some(rest) = p.strip_prefix(&format!("{}/", slugify(domain_name)))
        && !rest.is_empty()
        && *p != slugify(&file.rel_path.to_string_lossy())
    {
        sink.emit(
            &file.path,
            None,
            "E008",
            Severity::Warning,
            format!(
                "permalink `{p}` starts with the domain name; permalinks are domain-relative and the domain name is per-user configuration"
            ),
            Some(format!("use `{rest}`")),
        );
    }

    for tag in &fm.tags {
        if !crate::tags::is_lower_hyphen(tag) {
            sink.emit(
                &file.path,
                None,
                "E007",
                Severity::Warning,
                format!("tag `{tag}` is not lowercase-with-hyphens"),
                None,
            );
        }
    }
}
