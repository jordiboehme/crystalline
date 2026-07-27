//! Chunking and fingerprints.
//!
//! An engram body is split into embedding chunks on blank-line paragraph
//! boundaries, then consecutive paragraphs are greedily packed up to a token
//! budget (bge accepts 512 tokens; the default budget of 450 leaves room for the
//! special tokens and the title prepended to the first chunk). A fenced code
//! block is kept intact as a single unit even when it contains blank lines, so a
//! code sample is never cut in half. A paragraph that exceeds the budget on its
//! own is the one exception: it is hard-split into budget-sized pieces (a fence
//! included, kept whole only up to the budget), because a body with no blank
//! line would otherwise become a single chunk of unbounded size and every
//! consumer downstream, the tokenizer first, would pay for it. The first chunk
//! gets the engram title and description prepended so a short engram still
//! carries its heading into the vector space.
//!
//! Each chunk carries a fingerprint `sha256(model_id + ":" + text)`. The sync
//! engine hands the fingerprints to [`crate::Store::replace_chunks`], which
//! carries over an existing embedding whenever a fingerprint is unchanged, so an
//! edit only re-embeds the paragraphs that actually changed and a model swap
//! (folded into the fingerprint) re-embeds everything.

use sha2::{Digest, Sha256};

use crate::store::NewChunk;

/// The default token budget per chunk. Below bge's 512-token input limit, with
/// headroom for the special tokens and the first chunk's title prepend.
pub const DEFAULT_MAX_TOKENS: usize = 450;

/// The default local model id, used as the chunk-fingerprint namespace when no
/// embeddings provider is configured. Kept in step with the local provider's
/// reported model id so fingerprints computed at sync time match the model that
/// later embeds them.
pub const DEFAULT_MODEL_ID: &str = "bge-small-en-v1.5";

/// Parameters for chunking one engram.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkParams {
    /// The model id folded into every chunk fingerprint.
    pub model_id: String,
    /// The per-chunk token budget.
    pub max_tokens: usize,
}

impl Default for ChunkParams {
    fn default() -> ChunkParams {
        ChunkParams {
            model_id: DEFAULT_MODEL_ID.to_string(),
            max_tokens: DEFAULT_MAX_TOKENS,
        }
    }
}

impl ChunkParams {
    /// Parameters for a specific model with the default token budget.
    pub fn for_model(model_id: impl Into<String>) -> ChunkParams {
        ChunkParams {
            model_id: model_id.into(),
            max_tokens: DEFAULT_MAX_TOKENS,
        }
    }
}

/// The character-based token estimate used when a real tokenizer is not on hand
/// (the sync path does not load the model): about four characters per token.
pub fn estimate_tokens(text: &str) -> usize {
    let chars = text.chars().count();
    if chars == 0 { 0 } else { chars.div_ceil(4) }
}

/// Chunk an engram using the built-in character-based token estimate.
pub fn chunk_engram(
    title: &str,
    description: Option<&str>,
    body: &str,
    params: &ChunkParams,
) -> Vec<NewChunk> {
    chunk_engram_with(title, description, body, params, &estimate_tokens)
}

/// Chunk an engram, measuring token counts with the supplied counter (the
/// provider's tokenizer when available, else [`estimate_tokens`]).
pub fn chunk_engram_with(
    title: &str,
    description: Option<&str>,
    body: &str,
    params: &ChunkParams,
    count: &dyn Fn(&str) -> usize,
) -> Vec<NewChunk> {
    let paragraphs = split_paragraphs(body);
    let packed = pack(&paragraphs, params.max_tokens, count);

    let header = build_header(title, description);
    let mut finals: Vec<String> = Vec::new();
    for (i, body_text) in packed.iter().enumerate() {
        let text = if i == 0 && !header.is_empty() {
            if body_text.is_empty() {
                header.clone()
            } else {
                format!("{header}\n\n{body_text}")
            }
        } else {
            body_text.clone()
        };
        let text = text.trim().to_string();
        if !text.is_empty() {
            finals.push(text);
        }
    }
    // An engram with only a title and description (no body) still yields one
    // chunk so it can be found semantically.
    if finals.is_empty() && !header.is_empty() {
        finals.push(header);
    }

    finals
        .into_iter()
        .enumerate()
        .map(|(seq, text)| NewChunk {
            seq: seq as i64,
            text_hash: fingerprint(&params.model_id, &text),
            text,
        })
        .collect()
}

/// The fingerprint of a chunk: `sha256(model_id + ":" + text)`, lowercase hex.
pub fn fingerprint(model_id: &str, text: &str) -> String {
    let mut h = Sha256::new();
    h.update(model_id.as_bytes());
    h.update(b":");
    h.update(text.as_bytes());
    crate::hex_lower(&h.finalize())
}

fn build_header(title: &str, description: Option<&str>) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !title.trim().is_empty() {
        parts.push(title.trim().to_string());
    }
    if let Some(d) = description
        && !d.trim().is_empty()
    {
        parts.push(d.trim().to_string());
    }
    parts.join("\n\n")
}

/// Greedily pack consecutive paragraphs up to the token budget. A paragraph
/// that fits is never split, so a fenced block stays whole; one larger than the
/// budget on its own is cut into budget-sized pieces first, so no chunk can
/// exceed the budget however the body is written.
fn pack(paragraphs: &[String], max_tokens: usize, count: &dyn Fn(&str) -> usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_tokens = 0usize;

    for p in paragraphs {
        let pt = count(p);
        // A paragraph within the budget takes the same path it always did; only
        // an oversized one is cut, and only then is anything measured twice.
        if pt <= max_tokens || max_tokens == 0 {
            add_piece(
                p,
                pt,
                max_tokens,
                &mut out,
                &mut current,
                &mut current_tokens,
            );
            continue;
        }
        for piece in split_to_budget(p, max_tokens, count) {
            let piece_tokens = count(piece);
            add_piece(
                piece,
                piece_tokens,
                max_tokens,
                &mut out,
                &mut current,
                &mut current_tokens,
            );
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// Add one piece to the chunk being packed, flushing before it when it would
/// overflow the budget and after it once the budget is reached.
fn add_piece(
    piece: &str,
    piece_tokens: usize,
    max_tokens: usize,
    out: &mut Vec<String>,
    current: &mut String,
    current_tokens: &mut usize,
) {
    if !current.is_empty() && *current_tokens + piece_tokens > max_tokens {
        out.push(std::mem::take(current));
        *current_tokens = 0;
    }
    if !current.is_empty() {
        current.push_str("\n\n");
    }
    current.push_str(piece);
    *current_tokens += piece_tokens;
    if *current_tokens >= max_tokens {
        out.push(std::mem::take(current));
        *current_tokens = 0;
    }
}

/// Cut one paragraph into pieces that each fit the token budget. A paragraph
/// within the budget is returned whole, so an ordinary body packs exactly as it
/// did before the cap existed. Cuts land on char boundaries and prefer the last
/// newline or space in the tail of a piece so words stay intact, and the pieces
/// concatenate back to the input.
fn split_to_budget<'a>(
    text: &'a str,
    max_tokens: usize,
    count: &dyn Fn(&str) -> usize,
) -> Vec<&'a str> {
    if max_tokens == 0 || count(text) <= max_tokens {
        return vec![text];
    }
    // Characters per token measured on this text, so both the built-in estimate
    // and a real tokenizer land near the budget on the first cut. Only the
    // candidate piece is measured after that: measuring the whole remainder on
    // every cut would make splitting a huge paragraph quadratic.
    let per_token = (text.chars().count() / count(text).max(1)).max(1);
    let target_chars = max_tokens.saturating_mul(per_token).max(1);

    let mut pieces: Vec<&str> = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let mut take = target_chars;
        let mut end = loop {
            let end = char_boundary_at(rest, take);
            if take <= 1 || count(&rest[..end]) <= max_tokens {
                break end;
            }
            take = (take * 3 / 4).max(1);
        };
        if end == rest.len() {
            pieces.push(rest);
            break;
        }
        if let Some(cut) = whitespace_cut(&rest[..end]) {
            end = cut;
        }
        let (head, tail) = rest.split_at(end);
        pieces.push(head);
        rest = tail;
    }
    pieces
}

/// The byte offset of the `chars`-th char boundary, or the end of the string.
fn char_boundary_at(text: &str, chars: usize) -> usize {
    text.char_indices()
        .nth(chars)
        .map(|(i, _)| i)
        .unwrap_or(text.len())
}

/// A cut point just past the last newline (else the last space) in the final
/// fifth of `head`, so a piece ends on a word rather than mid-word. `None` when
/// the tail holds no whitespace to cut on. ASCII whitespace bytes never occur
/// inside a multi-byte sequence, so the offset is always a char boundary.
fn whitespace_cut(head: &str) -> Option<usize> {
    let window = (head.len() / 5).max(1);
    let bytes = head.as_bytes();
    let mut newline: Option<usize> = None;
    let mut space: Option<usize> = None;
    for i in (head.len() - window..head.len()).rev() {
        let b = bytes[i];
        if b == b'\n' {
            newline = Some(i + 1);
            break;
        }
        if space.is_none() && b.is_ascii_whitespace() {
            space = Some(i + 1);
        }
    }
    newline.or(space).filter(|&cut| cut < head.len())
}

/// Split a body into paragraphs on blank lines, keeping fenced code blocks whole.
fn split_paragraphs(body: &str) -> Vec<String> {
    let mut paras: Vec<String> = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    let mut in_fence = false;
    let mut marker = "";

    let flush = |current: &mut Vec<&str>, paras: &mut Vec<String>| {
        if !current.is_empty() {
            paras.push(current.join("\n"));
            current.clear();
        }
    };

    for line in body.lines() {
        let trimmed = line.trim_start();
        let is_fence = trimmed.starts_with("```") || trimmed.starts_with("~~~");

        if in_fence {
            current.push(line);
            if is_fence && trimmed.starts_with(marker) {
                in_fence = false;
                flush(&mut current, &mut paras);
            }
            continue;
        }

        if is_fence {
            // A fence opens its own unit: flush any prose paragraph first.
            flush(&mut current, &mut paras);
            marker = if trimmed.starts_with("```") {
                "```"
            } else {
                "~~~"
            };
            current.push(line);
            in_fence = true;
            continue;
        }

        if line.trim().is_empty() {
            flush(&mut current, &mut paras);
        } else {
            current.push(line);
        }
    }
    flush(&mut current, &mut paras);
    paras
}
