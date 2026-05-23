//! Word splitter + prompt builder for GLiNER v2.1 inputs.
//!
//! At training time, GLiNER v2.1 builds inputs as a flat list of
//! whitespace-separated "words" and hands them to flair, which calls
//! the HuggingFace fast tokenizer with `is_split_into_words=True`:
//!
//! ```text
//! ["<<ENT>>", "label_1", "<<ENT>>", "label_2", "<<SEP>>",
//!  "word_1", "word_2", ..., "word_N"]
//! ```
//!
//! The DeBERTa-v3-small fast tokenizer then prepends `[CLS]`, expands
//! each word into one-or-more SentencePiece subtokens (`▁word`,
//! `▁word_piece`, ...) and appends a trailing `[SEP]`. `<<ENT>>` and
//! `<<SEP>>` are *regular* added tokens (not "special"), so the
//! Metaspace pre-tokenizer skips them and each one occupies exactly
//! one slot in `input_ids`.
//!
//! Pooling at inference (matches `compute_score_eval` in upstream
//! v2.1 `gliner/model.py`):
//!
//! - Label embeddings: hidden states at the positions of the
//!   `<<ENT>>` markers in the prompt section (one per label).
//! - Word embeddings: hidden state at the *first* subtoken of every
//!   input word in the words section.
//!
//! The structural `[CLS]` (id 1) and trailing `[SEP]` (id 2) are
//! DeBERTa's actual sequence-boundary tokens. They are *not* the
//! same as `<<ENT>>` (128001) / `<<SEP>>` (128002), which carry the
//! GLiNER-specific marker semantics.

use std::sync::OnceLock;

use regex::Regex;

use super::GlinerError;

/// One word identified by [`split_words`], with its byte-exact
/// character offsets in the original UTF-8 input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WordOffset {
    /// The word's text, slice-equal to `&text[char_start..char_end]`.
    pub text: String,
    /// Inclusive char offset (byte index) of the word in the source.
    pub char_start: usize,
    /// Exclusive char offset (byte index) of the word end.
    pub char_end: usize,
}

/// Tokenised input ready to be fed to the backbone.
#[derive(Debug, Clone)]
pub struct TokenizedInput {
    /// Full token sequence: `[CLS] <<ENT>> label_1_subtokens <<ENT>>
    /// label_2_subtokens ... <<SEP>> word_1_subtokens word_2_subtokens
    /// ... [SEP]`.
    pub input_ids: Vec<u32>,
    /// `1` everywhere — GLiNER does not use padding at inference
    /// (single-sequence batches).
    pub attention_mask: Vec<u32>,
    /// Position of each `<<ENT>>` marker in `input_ids`. Length equals
    /// the number of labels; preserves label order.
    pub ent_positions: Vec<u32>,
    /// For every word in the original split, the absolute index in
    /// `input_ids` of its first subtoken (post-`<<SEP>>`).
    pub word_first_subtoken: Vec<u32>,
    /// Per-word character offsets in the source string, parallel
    /// to `word_first_subtoken`.
    pub word_offsets: Vec<(usize, usize)>,
}

/// Compile the upstream gline-rs splitter regex on first use.
/// `\w+` matches contiguous word characters (Unicode letters /
/// digits / underscore); `[^\s\w]` matches any single non-whitespace
/// non-word character (every punctuation mark is its own token).
fn splitter() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\w+|[^\s\w]").expect("invariant: literal regex is valid"))
}

/// Split `text` into words, preserving the original byte offsets.
pub fn split_words(text: &str) -> Vec<WordOffset> {
    splitter()
        .find_iter(text)
        .map(|m| WordOffset {
            text: m.as_str().to_owned(),
            char_start: m.start(),
            char_end: m.end(),
        })
        .collect()
}

/// IDs the tokenizer must resolve at load time; pass-through here so
/// the builder stays a pure function of inputs.
#[derive(Debug, Clone, Copy)]
pub struct TokenizerIds {
    /// DeBERTa-v3 `[CLS]` (id 1 on the standard tokenizer).
    pub cls: u32,
    /// DeBERTa-v3 `[SEP]` (id 2 on the standard tokenizer).
    pub sep: u32,
    /// GLiNER `<<ENT>>` marker (id 128001 in the v2.1 trained vocab).
    pub ent: u32,
    /// GLiNER `<<SEP>>` prompt-terminator (id 128002).
    pub prompt_sep: u32,
}

/// Build the full token sequence for one input.
pub fn tokenize(
    tokenizer: &tokenizers::Tokenizer,
    text: &str,
    labels: &[&str],
    ids: &TokenizerIds,
    max_len: usize,
) -> Result<TokenizedInput, GlinerError> {
    let mut input_ids: Vec<u32> = Vec::with_capacity(max_len);
    let mut ent_positions: Vec<u32> = Vec::with_capacity(labels.len());

    // [CLS] — DeBERTa's structural sentence-start token.
    input_ids.push(ids.cls);

    // <<ENT>> + label subtokens for every label, in order.
    for label in labels {
        ent_positions.push(input_ids.len() as u32);
        input_ids.push(ids.ent);
        let enc = tokenizer
            .encode(*label, false)
            .map_err(|e| GlinerError::Tokenizer(e.to_string()))?;
        input_ids.extend_from_slice(enc.get_ids());
    }

    // <<SEP>> — GLiNER's prompt-section terminator (NOT [SEP]). This
    // is the boundary between the label prompt and the words section.
    input_ids.push(ids.prompt_sep);

    // Word subtokens. Split first so we can drive per-word encoding
    // and track the first-subtoken position of each word.
    let words = split_words(text);
    let mut word_first_subtoken: Vec<u32> = Vec::with_capacity(words.len());
    let mut word_offsets: Vec<(usize, usize)> = Vec::with_capacity(words.len());

    for word in &words {
        let enc = tokenizer
            .encode(word.text.as_str(), false)
            .map_err(|e| GlinerError::Tokenizer(e.to_string()))?;
        let subtokens = enc.get_ids();
        if subtokens.is_empty() {
            // Tokenizer dropped the word (whitespace-only / unknown
            // surrogate). Skip rather than fabricate a position.
            continue;
        }
        word_first_subtoken.push(input_ids.len() as u32);
        word_offsets.push((word.char_start, word.char_end));
        input_ids.extend_from_slice(subtokens);
    }

    // Trailing [SEP] — DeBERTa's structural sentence-end token.
    input_ids.push(ids.sep);

    if input_ids.len() > max_len {
        return Err(GlinerError::InputTooLong {
            got: input_ids.len(),
            max: max_len,
        });
    }

    let attention_mask = vec![1u32; input_ids.len()];

    Ok(TokenizedInput {
        input_ids,
        attention_mask,
        ent_positions,
        word_first_subtoken,
        word_offsets,
    })
}
