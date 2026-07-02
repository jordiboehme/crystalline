//! E-family rules: frontmatter format and encoding.
//!
//! `E001`-`E006` are unconditional errors: a well-formed Engram must parse,
//! carry the four required fields and use a lowercase slug permalink with
//! clean UTF-8. `E007` (tag format) and the outside-the-recommended-set half
//! of `E003` are softer: Crystalline never enforces a closed `type` or
//! `status` vocabulary, so those two only ever inform.

use crate::address::slugify;
use crate::engram::RECOMMENDED_TYPES;
use crate::parse::ParseError;

use super::scanner::ScannedFile;
use super::{Severity, Sink};

pub(crate) fn check(file: &ScannedFile, sink: &mut Sink) {
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

    for tag in &fm.tags {
        if !is_lower_hyphen(tag) {
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

fn is_lower_hyphen(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && !s.ends_with('-')
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}
