//! The llama.cpp side: read text into a sequence, take a bank of it, splice a bank in, generate greedily (docs/design.md, "The bank").
//! RED STUB: every method is a placeholder that does nothing useful; the model test fails on its assertions.
use crate::bank::Bank;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::model::LlamaModel;

pub struct Runner<'m> {
    _model: &'m LlamaModel,
}

impl<'m> Runner<'m> {
    pub fn new(model: &'m LlamaModel, _backend: &LlamaBackend, _n_ctx: u32) -> Result<Self, String> { Ok(Runner { _model: model }) }
    pub fn reset(&mut self) {}
    pub fn feed_text(&mut self, _text: &str) -> Result<(), String> { Ok(()) }
    pub fn make_bank(&mut self, _text: &str) -> Result<Bank, String> { Ok(Bank { tokens: 0, state: Vec::new() }) }
    pub fn splice(&mut self, _bank: &Bank) -> Result<(), String> { Ok(()) }
    pub fn generate(&mut self, _max_new: usize) -> Result<String, String> { Ok(String::new()) }
    /// Tokens decoded since `new` (what the model had to read).
    pub fn decoded(&self) -> usize { 0 }
}
