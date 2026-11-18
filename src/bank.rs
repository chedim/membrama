//! The bank store: the model's cache state for an entity's rendered text, kept per model under a byte cap (docs/design.md).
//! A bank is a pure cache: dropping any of it only means the text is read again.
use sha2::{Digest, Sha256};
use std::collections::HashMap;

/// What a bank is valid for: one model, one entity at one version, one rendering of its text.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BankKey {
    /// Identity of the model file (its hash); a bank is valid for one model only.
    pub model: String,
    pub eid: String,
    /// The entity's graph version stamp when the text was rendered; opaque here, equal means unchanged.
    pub version: String,
    pub text_hash: [u8; 32],
}

/// The sequence state of the text computed alone from position 0 (`llama_state_seq_get_data`), and its length in tokens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bank {
    pub tokens: u32,
    pub state: Vec<u8>,
}

/// A bank that cannot be stored because it is larger than the whole cap.
#[derive(Debug, PartialEq, Eq)]
pub struct TooBig {
    pub size: usize,
    pub cap: usize,
}

struct Entry {
    bank: Bank,
    used: u64,
}

pub struct BankStore {
    cap: usize,
    bytes: usize,
    clock: u64,
    entries: HashMap<BankKey, Entry>,
}

pub fn hash_text(text: &str) -> [u8; 32] {
    Sha256::digest(text.as_bytes()).into()
}

impl BankStore {
    /// `cap` is the most bytes of bank state the store keeps.
    pub fn new(cap: usize) -> Self {
        BankStore { cap, bytes: 0, clock: 0, entries: HashMap::new() }
    }

    pub fn bytes(&self) -> usize { self.bytes }
    pub fn len(&self) -> usize { self.entries.len() }
    pub fn is_empty(&self) -> bool { self.entries.is_empty() }

    /// Whether a bank for `key` is kept; this is not a use, so it does not move the bank in the eviction order.
    pub fn contains(&self, key: &BankKey) -> bool { self.entries.contains_key(key) }

    /// The bank for `key`, counted as used now.
    pub fn get(&mut self, key: &BankKey) -> Option<&Bank> {
        self.clock += 1;
        let now = self.clock;
        self.entries.get_mut(key).map(|e| { e.used = now; &e.bank })
    }

    /// Banks `bank` under `key`. Any other version of the same entity for the same model is dropped (the latest put is the
    /// current version), then the least recently used banks go until it fits. Returns every key dropped to make it so;
    /// a bank larger than the whole cap is refused and nothing is dropped.
    pub fn put(&mut self, key: BankKey, bank: Bank) -> Result<Vec<BankKey>, TooBig> {
        let size = bank.state.len();
        if size > self.cap { return Err(TooBig { size, cap: self.cap }); }
        let mut dropped = Vec::new();
        if let Some(old) = self.entries.remove(&key) { self.bytes -= old.bank.state.len(); }
        let stale: Vec<BankKey> = self.entries.keys().filter(|k| k.model == key.model && k.eid == key.eid && k.version != key.version).cloned().collect();
        for k in stale { self.remove(&k); dropped.push(k); }
        while self.bytes + size > self.cap {
            let oldest = self.entries.iter().min_by_key(|(_, e)| e.used).map(|(k, _)| k.clone()).expect("over the cap with nothing stored");
            self.remove(&oldest);
            dropped.push(oldest);
        }
        self.clock += 1;
        self.bytes += size;
        self.entries.insert(key, Entry { bank, used: self.clock });
        Ok(dropped)
    }

    /// Drops every bank of `model`; the number dropped.
    pub fn drop_model(&mut self, model: &str) -> usize {
        let keys: Vec<BankKey> = self.entries.keys().filter(|k| k.model == model).cloned().collect();
        for k in &keys { self.remove(k); }
        keys.len()
    }

    fn remove(&mut self, key: &BankKey) {
        if let Some(e) = self.entries.remove(key) { self.bytes -= e.bank.state.len(); }
    }
}
