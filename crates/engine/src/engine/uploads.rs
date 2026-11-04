//! An agent's upload and download of one file: the shape an MCP verb sends a
//! file in (`content` for text, `base64` for bytes) and the shape it reads one
//! back in. The write itself is a person's upload: the same entry points the
//! REST routes call ([`Engine::attachment_write_joined`] for an engram
//! attachment, [`Engine::reflection_asset_create`] and
//! [`Engine::reflection_asset_write`] for a Reflection file), so the path
//! rules, the allowlist, the size cap and the rights are theirs.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;

use super::*;
use crate::scope::ReflectionCaller;

/// Whether a file of `mime` travels as text: the text mimes
/// [`crystalline_core::is_text_attachment_mime`] names, and SVG, which is
/// XML an agent writes as text. A file of any other mime travels as base64.
pub fn is_text_file_mime(mime: &str) -> bool {
    crystalline_core::is_text_attachment_mime(mime) || mime == "image/svg+xml"
}

/// The bytes an upload of `path` carries, from exactly one of `content` (the
/// text of a text file) and `base64` (the bytes of any file).
///
/// The path is checked first, with the sentence every attachment surface
/// refuses it with, so a bad path is never answered as a bad payload. Then
/// the payload: both or neither is refused, `content` for a file that is not
/// text is refused (its bytes would not survive a JSON string), and base64
/// that does not decode is refused naming the field. Whitespace inside the
/// base64 (a wrapped encoding) is ignored. Nothing is written by this.
pub fn upload_bytes(path: &str, content: Option<&str>, base64: Option<&str>) -> Result<Vec<u8>> {
    validate_attachment_path(path)?;
    match (content, base64) {
        (Some(_), Some(_)) => Err(EngineError::Invalid(format!(
            "send '{path}' as content (the text of a text file) or base64 (the bytes), not both"
        ))),
        (None, None) => Err(EngineError::Invalid(format!(
            "an upload of '{path}' needs content (the text of a text file) or base64 (the file's bytes)"
        ))),
        (Some(text), None) => {
            let name = path.rsplit('/').next().unwrap_or(path);
            match crystalline_core::attachment_mime(name) {
                Some(mime) if !is_text_file_mime(mime) => Err(EngineError::Invalid(format!(
                    "content carries text, and '{path}' is {mime}; send its bytes as base64 instead"
                ))),
                _ => Ok(text.as_bytes().to_vec()),
            }
        }
        (None, Some(encoded)) => {
            let compact: String = encoded
                .chars()
                .filter(|c| !c.is_ascii_whitespace())
                .collect();
            BASE64.decode(compact.as_bytes()).map_err(|e| {
                EngineError::Invalid(format!(
                    "base64 for '{path}' does not decode ({e}); send the file's bytes in standard base64"
                ))
            })
        }
    }
}

/// One file as a read answers it: where it is, what it is, and its contents,
/// as `text` for a text file that is valid UTF-8 and as `base64` otherwise
/// (a text file in some other encoding keeps every byte that way).
pub fn file_answer(domain: &str, bytes: Vec<u8>, row: &AttachmentRow) -> Value {
    let mut answer = json!({
        "domain": domain,
        "path": row.path,
        "mime": row.mime,
        "size": row.size,
        "sha256": row.sha256,
    });
    let bytes = if is_text_file_mime(&row.mime) {
        match String::from_utf8(bytes) {
            Ok(text) => {
                answer["text"] = Value::String(text);
                return answer;
            }
            Err(e) => e.into_bytes(),
        }
    } else {
        bytes
    };
    answer["base64"] = Value::String(BASE64.encode(bytes));
    answer
}

/// The `write_engram` fields that describe an engram and mean nothing for a
/// file, by name, for the ones `p` sets.
fn engram_fields_set(p: &WriteParams) -> Vec<&'static str> {
    let mut set = Vec::new();
    if !p.title.is_empty() {
        set.push("title");
    }
    if p.folder.is_some() {
        set.push("folder");
    }
    if p.engram_type.is_some() {
        set.push("type");
    }
    if !p.tags.is_empty() {
        set.push("tags");
    }
    if p.status.is_some() {
        set.push("status");
    }
    if p.metadata.as_ref().is_some_and(|m| !m.is_null()) {
        set.push("metadata");
    }
    set
}

impl Engine {
    /// `write_engram` with `path`: store one engram attachment as this caller
    /// sees the domain, exactly as the REST upload does.
    ///
    /// The fields that describe an engram are refused rather than ignored,
    /// so a caller who meant to write an engram learns it did not. `overwrite`
    /// false only creates: a path that already holds a file is refused with
    /// [`crate::engine::ASSET_EXISTS`]. The join is the one a share-link on
    /// the call opened, as the REST upload's `X-Crystalline-Join` is: the
    /// file then follows it into the draft owner's files.
    ///
    /// The receipt is `{domain, path, mime, size, sha256, created}` with
    /// `draft: true` when the file landed in an overlay.
    pub async fn upload_attachment(
        &self,
        p: &WriteParams,
        scope: &crate::scope::Scope,
        join: Option<&crate::join::Join>,
    ) -> Result<Value> {
        let Some(path) = p.path.as_deref() else {
            return Err(EngineError::Invalid(
                "an upload needs path, the file's path under assets/".into(),
            ));
        };
        // Refused rather than ignored, as a delete refuses it: a caller who
        // sent one would believe the replace was guarded when nothing
        // compared anything.
        if p.expected_checksum.is_some() {
            return Err(EngineError::Invalid(format!(
                "expected_checksum guards engram markdown only and has no meaning for the file '{path}'; upload it without one"
            )));
        }
        let set = engram_fields_set(p);
        if !set.is_empty() {
            return Err(EngineError::Invalid(format!(
                "{} {} not apply to a file; leave {} out when you upload '{path}'",
                set.join(", "),
                if set.len() == 1 { "does" } else { "do" },
                if set.len() == 1 { "it" } else { "them" },
            )));
        }
        let bytes = upload_bytes(path, p.content.as_deref(), p.base64.as_deref())?;
        let written = self
            .attachment_write_joined(&p.domain, path, bytes, scope, join, !p.overwrite)
            .await?;
        let mut receipt = json!({
            "domain": p.domain,
            "path": written.row.path,
            "mime": written.row.mime,
            "size": written.row.size,
            "sha256": written.row.sha256,
            "created": written.created,
        });
        if written.draft {
            receipt["draft"] = json!(true);
        }
        Ok(receipt)
    }

    /// `write_reflection` `write_asset`: store one Reflection file, exactly
    /// as the REST upload does. `overwrite` false only creates (a file at the
    /// path is refused, for a curator too); true replaces, which needs
    /// curate. The receipt is `{domain, path, mime, size, sha256, created}`,
    /// `path` inside `reflections/`.
    pub(super) async fn write_reflection_asset(
        &self,
        p: &WriteReflectionParams,
        caller: &ReflectionCaller,
    ) -> Result<Value> {
        let bytes = upload_bytes(&p.path, p.content.as_deref(), p.base64.as_deref())?;
        let written = if p.overwrite {
            self.reflection_asset_write(&p.domain, &p.path, bytes, caller)
                .await?
        } else {
            self.reflection_asset_create(&p.domain, &p.path, bytes, caller)
                .await?
        };
        Ok(json!({
            "domain": p.domain,
            "path": written.row.path,
            "mime": written.row.mime,
            "size": written.row.size,
            "sha256": written.row.sha256,
            "created": written.created,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_payload_is_exactly_one_field_that_fits_the_file() {
        assert_eq!(
            upload_bytes("assets/a.svg", Some("<svg/>"), None).unwrap(),
            b"<svg/>"
        );
        assert_eq!(
            upload_bytes("assets/a.png", None, Some("iVBO\nRw==")).unwrap(),
            BASE64.decode("iVBORw==").unwrap()
        );
        let err = |r: Result<Vec<u8>>| r.unwrap_err().to_string();
        assert!(err(upload_bytes("assets/a.png", Some("x"), None)).contains("base64"));
        assert!(err(upload_bytes("assets/a.png", None, Some("**"))).contains("base64"));
        assert!(err(upload_bytes("assets/a.txt", Some("x"), Some("eA=="))).contains("not both"));
        assert!(err(upload_bytes("assets/a.txt", None, None)).contains("content"));
        assert!(
            err(upload_bytes("ASSETS/a.png", None, Some("eA=="))).starts_with("attachment path")
        );
    }

    #[test]
    fn an_answer_is_text_for_readable_utf8_and_base64_otherwise() {
        let row = |path: &str, mime: &str| AttachmentRow {
            path: path.into(),
            sha256: "s".into(),
            mime: mime.into(),
            size: 1,
            modified: String::new(),
        };
        let svg = file_answer(
            "d",
            b"<svg/>".to_vec(),
            &row("assets/a.svg", "image/svg+xml"),
        );
        assert_eq!(svg["text"], "<svg/>");
        let latin = file_answer("d", b"\xf6".to_vec(), &row("assets/a.txt", "text/plain"));
        assert_eq!(latin["base64"], BASE64.encode(b"\xf6"));
        let png = file_answer("d", b"x".to_vec(), &row("assets/a.png", "image/png"));
        assert_eq!(png["base64"], "eA==");
        assert!(png.get("text").is_none());
    }
}
