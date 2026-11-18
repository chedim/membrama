//! The command line of `membrama serve`, the program the seat starts from its `peers` fact. Every number is the owner's and has no default
//! (the regions' sizes must equal what the seat declared; the model's context and the bank limits are the owner's choices).
use crate::peer::Numbers;
use std::collections::HashMap;

#[derive(Debug, PartialEq)]
pub struct Config {
    /// The peer's name in the seat's `peers` fact (the regions are `/gl-<name>-ctl`, `-sc`, `-pc`).
    pub peer: String,
    /// The namespace membrama owns.
    pub ns: String,
    /// The model file (a gguf); its path is also the model's identity for the banks, which live in this process only.
    pub model: String,
    pub n_ctx: u32,
    pub numbers: Numbers,
    /// The most bytes of banks kept.
    pub bank_cap: usize,
    /// The most entity parts of one request served from banks.
    pub max_banks: usize,
    /// End when the process that started this one is gone.
    pub die_with_parent: bool,
    /// The file holding the token the seat made for this peer (`<data dir>/peers/<name>.token`): the seat refuses a hello without one.
    pub token_file: Option<String>,
}

const VALUED: [&str; 13] = ["--token-file", "--peer", "--ns", "--model", "--n-ctx", "--slots", "--key-cap", "--val-cap", "--epoch", "--spins", "--depth", "--bank-cap", "--max-banks"];

pub fn parse(args: &[String]) -> Result<Config, String> {
    let mut got: HashMap<&str, &str> = HashMap::new();
    let mut die_with_parent = false;
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if a == "--die-with-parent" { die_with_parent = true; i += 1; continue; }
        let Some(opt) = VALUED.iter().find(|o| **o == a) else { return Err(format!("unknown option {a}")) };
        let Some(v) = args.get(i + 1) else { return Err(format!("{opt} needs a value")) };
        got.insert(opt, v);
        i += 2;
    }
    let text = |o: &str| got.get(o).map(|v| v.to_string()).ok_or_else(|| format!("missing {o} (no default: it is the owner's number)"));
    fn num<T: std::str::FromStr>(got: &HashMap<&str, &str>, o: &str) -> Result<T, String> {
        let v = got.get(o).ok_or_else(|| format!("missing {o} (no default: it is the owner's number)"))?;
        v.parse::<T>().map_err(|_| format!("{o}: {v:?} is not a number"))
    }
    Ok(Config {
        peer: text("--peer")?, ns: text("--ns")?, model: text("--model")?, n_ctx: num(&got, "--n-ctx")?,
        numbers: Numbers { slots: num(&got, "--slots")?, key_cap: num(&got, "--key-cap")?, val_cap: num(&got, "--val-cap")?, epoch: num(&got, "--epoch")?, spins: num(&got, "--spins")?, depth: num(&got, "--depth")? },
        bank_cap: num(&got, "--bank-cap")?, max_banks: num(&got, "--max-banks")?, die_with_parent,
        token_file: None,   // RED STUB: the option is accepted and ignored
    })
}
