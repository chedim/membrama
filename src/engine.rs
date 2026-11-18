//! The llama.cpp side: read text into a sequence, take a bank of it, splice a bank in, generate greedily (docs/design.md, "The bank").
//! A bank is taken from the text computed alone from position 0 and loaded after whatever precedes it, its positions shifted
//! to where it lands (`llama_state_seq_set_data` into a spare sequence, `seq_add`, `seq_cp` into the live one, `seq_rm`).
use crate::bank::Bank;
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::context::session::LlamaStateSeqFlags;
use llama_cpp_2::context::LlamaContext;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::LlamaModel;
use llama_cpp_2::sampling::LlamaSampler;
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
}

fn e<T: std::fmt::Display>(what: &str) -> impl FnOnce(T) -> String + '_ { move |err| format!("{what}: {err}") }

impl<'m> Runner<'m> {
    pub fn new(model: &'m LlamaModel, backend: &LlamaBackend, n_ctx: u32) -> Result<Self, String> {
        let params = LlamaContextParams::default().with_n_ctx(NonZeroU32::new(n_ctx)).with_n_batch(n_ctx);
        let ctx = model.new_context(backend, params).map_err(e("context"))?;
        Ok(Runner { model, ctx, n_batch: n_ctx as usize, pos: 0, decoded: 0 })
    }

    /// Forgets the live sequence.
    pub fn reset(&mut self) {
        self.ctx.clear_kv_cache();
        self.pos = 0;
    }

    /// Reads `text` (special tokens such as `<|im_start|>` are parsed) after what the live sequence holds.
    pub fn feed_text(&mut self, text: &str) -> Result<(), String> {
        let tokens = self.model.vocab().tokenize(text.as_bytes(), false, true);
        for chunk in tokens.chunks(self.n_batch) {
            let mut batch = LlamaBatch::new(chunk.len(), 1);
            let last = chunk.len() - 1;
            for (i, t) in chunk.iter().enumerate() {
                batch.add(*t, self.pos + i as i32, &[LIVE], i == last).map_err(e("batch"))?;
            }
            self.ctx.decode(&mut batch).map_err(e("decode"))?;
            self.pos += chunk.len() as i32;
            self.decoded += chunk.len();
        }
        Ok(())
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
        Ok(())
    }

    /// Greedy decoding of up to `max_new` tokens from where the live sequence stands.
    pub fn generate(&mut self, max_new: usize) -> Result<String, String> {
        let vocab = self.model.vocab();
        let mut sampler = LlamaSampler::greedy();
        let mut out = Vec::new();
        for _ in 0..max_new {
            let t = sampler.sample(&self.ctx, -1);
            if vocab.is_eog(t) { break; }
            out.extend(vocab.token_to_piece(t, false, None));
            let mut batch = LlamaBatch::new(1, 1);
            batch.add(t, self.pos, &[LIVE], true).map_err(e("batch"))?;
            self.ctx.decode(&mut batch).map_err(e("decode"))?;
            self.pos += 1;
            self.decoded += 1;
        }
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    /// Tokens decoded since `new` (what the model had to read and write).
    pub fn decoded(&self) -> usize { self.decoded }
}

/// The generator over a real model.
/// RED STUB.
pub struct EngineGenerator<'m> { _runner: Runner<'m> }

impl<'m> EngineGenerator<'m> {
    pub fn new(runner: Runner<'m>) -> Self { EngineGenerator { _runner: runner } }
}

impl crate::turn::Generator for EngineGenerator<'_> {
    fn generate(&mut self, _job: &crate::turn::Job, _on_text: &mut dyn FnMut(&str) -> bool) -> Result<crate::turn::Usage, String> { Ok(crate::turn::Usage::default()) }
}
