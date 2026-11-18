//! The command line of `membrama serve`, the program the seat starts from its `peers` fact. Every number is the owner's and has no default
//! (the regions' sizes must equal what the seat declared; the model's context and the bank limits are the owner's choices).
//! RED STUB.
use crate::peer::Numbers;

#[derive(Debug, PartialEq)]
pub struct Config {
    pub peer: String,
    pub ns: String,
    pub model: String,
    pub n_ctx: u32,
    pub numbers: Numbers,
    pub bank_cap: usize,
    pub max_banks: usize,
    pub die_with_parent: bool,
}

pub fn parse(_args: &[String]) -> Result<Config, String> { Err("not built".into()) }
