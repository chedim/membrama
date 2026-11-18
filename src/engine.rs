//! The llama.cpp side: read text into a sequence, take a bank of it, splice a bank in, generate greedily (docs/design.md, "The bank").
//! A bank is taken from the text computed alone from position 0 and loaded after whatever precedes it, its positions shifted
//! to where it lands (`llama_state_seq_set_data` into a spare sequence, `seq_add`, `seq_cp` into the live one, `seq_rm`).
use crate::bank::{hash_text, Bank, BankKey, BankStore};
use crate::turn::{bank_plan, Job, PartKind, Usage};
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::context::session::LlamaStateSeqFlags;
use llama_cpp_2::context::LlamaContext;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::LlamaModel;
use llama_cpp_2::sampling::LlamaSampler;
use llama_cpp_2::token::LlamaToken;
use std::num::NonZeroU32;

/// The live sequence.
const LIVE: i32 = 0;
/// The spare sequence a bank is loaded into before it is copied into the live one.
const SPARE: i32 = 1;

pub struct Runner<'m> {
    model: &'m LlamaModel,
    ctx: LlamaContext<'m>,
    n_batch: usize,
    pos: i32,
    decoded: usize,
    /// The tokens the live sequence holds, in order (prompt, then what was generated); `None` once a bank was spliced in, because a bank's tokens are not known here.
    live: Option<Vec<LlamaToken>>,
}

fn e<T: std::fmt::Display>(what: &str) -> impl FnOnce(T) -> String + '_ { move |err| format!("{what}: {err}") }

impl<'m> Runner<'m> {
    pub fn new(model: &'m LlamaModel, backend: &LlamaBackend, n_ctx: u32) -> Result<Self, String> {
        let params = LlamaContextParams::default().with_n_ctx(NonZeroU32::new(n_ctx)).with_n_batch(n_ctx);
        let ctx = model.new_context(backend, params).map_err(e("context"))?;
        Ok(Runner { model, ctx, n_batch: n_ctx as usize, pos: 0, decoded: 0, live: Some(Vec::new()) })
    }

    /// Forgets the live sequence.
    pub fn reset(&mut self) {
        self.ctx.clear_kv_cache();
        self.pos = 0;
        self.live = Some(Vec::new());
    }

    /// The tokens of `text`, special tokens parsed.
    pub fn tokenize(&self, text: &str) -> Vec<LlamaToken> { self.model.vocab().tokenize(text.as_bytes(), false, true) }

    /// Reads `text` (special tokens such as `<|im_start|>` are parsed) after what the live sequence holds.
    pub fn feed_text(&mut self, text: &str) -> Result<(), String> {
        let tokens = self.tokenize(text);
        self.read_tokens(&tokens)
    }

    /// Reads `tokens` after what the live sequence holds.
    fn read_tokens(&mut self, tokens: &[LlamaToken]) -> Result<(), String> {
        for chunk in tokens.chunks(self.n_batch) {
            let mut batch = LlamaBatch::new(chunk.len(), 1);
            let last = chunk.len() - 1;
            for (i, t) in chunk.iter().enumerate() {
                batch.add(*t, self.pos + i as i32, &[LIVE], i == last).map_err(e("batch"))?;
            }
            self.ctx.decode(&mut batch).map_err(e("decode"))?;
            self.pos += chunk.len() as i32;
            self.decoded += chunk.len();
            if let Some(live) = &mut self.live { live.extend_from_slice(chunk); }
        }
        Ok(())
    }

    /// Makes the live sequence hold exactly `tokens`: what it already holds from the start is kept (the cache after identical tokens is identical, so the
    /// answer does not change), the rest is dropped and only the tokens that differ are read. At least the last token is always read, to have its logits.
    /// A sequence with a spliced bank in it is not reused: it is reset.
    pub fn set_prompt(&mut self, tokens: &[LlamaToken]) -> Result<(), String> {
        let keep = match &self.live {
            Some(live) => common_prefix_len(live, tokens).min(tokens.len().saturating_sub(1)),
            None => 0,
        };
        let cut = keep as u32;
        if keep == 0 || self.ctx.kv_cache_seq_rm(LIVE, Some(cut), None).is_err() { self.reset(); self.read_tokens(tokens) }
        else {
            self.pos = keep as i32;
            if let Some(live) = &mut self.live { live.truncate(keep); }
            self.read_tokens(&tokens[keep..])
        }
    }

    /// The bank of `text`: read alone from position 0, then its sequence state. Leaves that text in the live sequence; `reset` after.
    pub fn make_bank(&mut self, text: &str) -> Result<Bank, String> {
        self.reset();
        self.feed_text(text)?;
        let flags = LlamaStateSeqFlags::empty();
        let size = self.ctx.state_seq_get_size_ext(LIVE, flags);
        let mut state = vec![0u8; size];
        // SAFETY: `state` holds exactly the size the context just reported for this sequence.
        let written = unsafe { self.ctx.state_seq_get_data_ext(state.as_mut_ptr(), LIVE, flags) };
        if written != size { return Err(format!("bank: wrote {written} of {size} bytes")); }
        Ok(Bank { tokens: self.pos as u32, state })
    }

    /// Loads `bank` after what the live sequence holds, as if its text had been read there. Fails (and the caller reads the
    /// text instead) when the state cannot be loaded or the model's cache cannot shift positions.
    pub fn splice(&mut self, bank: &Bank) -> Result<(), String> {
        // SAFETY: the bytes are a state this crate took from a sequence of the same model.
        let ok = unsafe { self.ctx.state_seq_set_data_ext(&bank.state, SPARE, LlamaStateSeqFlags::empty()) };
        if !ok { return Err("bank: the state was refused".into()); }
        self.ctx.kv_cache_seq_add(SPARE, None, None, self.pos).map_err(e("shift"))?;
        self.ctx.kv_cache_seq_cp(SPARE, LIVE, None, None).map_err(e("copy"))?;
        self.ctx.kv_cache_seq_rm(SPARE, None, None).map_err(e("clear"))?;
        self.pos += bank.tokens as i32;
        self.live = None;
        Ok(())
    }

    /// Greedy decoding of up to `max_new` tokens from where the live sequence stands; the text as one string.
    pub fn generate(&mut self, max_new: usize) -> Result<String, String> {
        let mut out = String::new();
        self.generate_with(max_new, &mut |t| { out.push_str(t); true })?;
        Ok(out)
    }

    /// The same, handing the text to `on_text` as it is produced, whole characters only (a character the model spreads over several
    /// tokens is held until it is complete); `on_text` answers `false` to stop.
    pub fn generate_with(&mut self, max_new: usize, on_text: &mut dyn FnMut(&str) -> bool) -> Result<(), String> {
        let vocab = self.model.vocab();
        let mut sampler = LlamaSampler::greedy();
        let mut pending: Vec<u8> = Vec::new();
        for _ in 0..max_new {
            let t = sampler.sample(&self.ctx, -1);
            if vocab.is_eog(t) { break; }
            pending.extend(vocab.token_to_piece(t, false, None));
            let ok = match std::str::from_utf8(&pending) {
                Ok(_) => pending.len(),
                Err(err) => err.valid_up_to(),
            };
            if ok > 0 {
                let text = String::from_utf8_lossy(&pending[..ok]).into_owned();
                pending.drain(..ok);
                if !on_text(&text) { return Ok(()); }
            }
            let mut batch = LlamaBatch::new(1, 1);
            batch.add(t, self.pos, &[LIVE], true).map_err(e("batch"))?;
            self.ctx.decode(&mut batch).map_err(e("decode"))?;
            self.pos += 1;
            self.decoded += 1;
            if let Some(live) = &mut self.live { live.push(t); }
        }
        if !pending.is_empty() { on_text(&String::from_utf8_lossy(&pending)); }
        Ok(())
    }

    /// Tokens decoded since `new` (what the model had to read and write).
    pub fn decoded(&self) -> usize { self.decoded }
}

/// The generator over a real model: it reads the parts as the seat wrote them, in order, then generates. The entity parts `bank_plan` picks are
/// served from banks (taken the first time, kept per model, entity, version and text) and spliced in where they stand; everything else is read as
/// text, and so is a bank that cannot be kept or loaded. The tokens the model sees are the same either way.
pub struct EngineGenerator<'m> {
    runner: Runner<'m>,
    store: BankStore,
    model_id: String,
    max_banks: usize,
}

impl<'m> EngineGenerator<'m> {
    /// `model_id` identifies the model file (a bank is valid for one model only); `cap` is the most bytes of banks kept; `max_banks` is the most
    /// entity parts of one request served from banks (the measured limit is two).
    pub fn new(runner: Runner<'m>, model_id: &str, cap: usize, max_banks: usize) -> Self {
        EngineGenerator { runner, store: BankStore::new(cap), model_id: model_id.to_string(), max_banks }
    }

    /// How many banks are kept.
    pub fn bank_count(&self) -> usize { self.store.len() }

    /// Whether the bank of this entity at this version and text is kept.
    pub fn has_bank(&self, eid: &str, version: &str, text: &str) -> bool { self.store.contains(&self.key(eid, version, text)) }

    fn key(&self, eid: &str, version: &str, text: &str) -> BankKey {
        BankKey { model: self.model_id.clone(), eid: eid.to_string(), version: version.to_string(), text_hash: hash_text(text) }
    }
}

impl crate::turn::Generator for EngineGenerator<'_> {
    fn generate(&mut self, job: &Job, on_text: &mut dyn FnMut(&str) -> bool) -> Result<Usage, String> {
        let before = self.runner.decoded();
        let plan = bank_plan(&job.parts, self.max_banks);
        let keys: Vec<Option<BankKey>> = job.parts.iter().enumerate().map(|(i, p)| match &p.kind {
            PartKind::Entity { eid, version } if plan.contains(&i) => Some(self.key(eid, version, &p.text)),
            _ => None,
        }).collect();
        // the banks first: taking one reads the entity's text alone, which clears the live sequence
        for (part, key) in job.parts.iter().zip(&keys) {
            let Some(key) = key else { continue };
            if self.store.get(key).is_none() {
                let bank = self.runner.make_bank(&part.text)?;
                let _ = self.store.put(key.clone(), bank);   // a bank bigger than the whole cap is not kept: its text is read below
            }
        }
        let mut loaded = 0usize;
        if plan.is_empty() {
            // no bank in this request: the live sequence keeps whatever the previous request shares with this one
            let tokens: Vec<_> = job.parts.iter().flat_map(|p| self.runner.tokenize(&p.text)).collect();
            self.runner.set_prompt(&tokens)?;
        } else {
            self.runner.reset();
            for (part, key) in job.parts.iter().zip(&keys) {
                if let Some(bank) = key.as_ref().and_then(|k| self.store.get(k)) {
                    if self.runner.splice(bank).is_ok() { loaded += bank.tokens as usize; continue; }
                }
                self.runner.feed_text(&part.text)?;
            }
        }
        let read = self.runner.decoded() - before;
        self.runner.generate_with(job.max_new, on_text)?;
        Ok(Usage { read, loaded, generated: self.runner.decoded() - before - read })
    }
}

/// How many tokens two sequences share from the start.
pub fn common_prefix_len<T: PartialEq>(a: &[T], b: &[T]) -> usize { a.iter().zip(b).take_while(|(x, y)| x == y).count() }
