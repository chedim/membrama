//! Shared by the tests that play the seat: it makes the control rings and its cell table first and sends its hello before the peer starts,
//! the way the seat's driver does.
#![allow(dead_code)]
use graphlink_core::frame::VERSION;
use graphlink_core::link::{Event, Link, Me};
use graphlink_core::transport::Transport;
use graphlink_shm::cells::{CellState, Cells, CellsReader, Reclaim};
use graphlink_shm::{remove_region, Shm};
use membrama::peer::{Numbers, Peer};
use serde_json::Value as J;
use std::collections::HashMap;

pub const NUMBERS: Numbers = Numbers { slots: 64, key_cap: 128, val_cap: 4096, epoch: 1, spins: 1_000_000, depth: 32 };
pub fn key(eid: &str, prop: &str) -> String { format!("{eid}\u{1}{prop}") }

pub struct FakeSeat { pub name: String, pub ctl: Shm, pub table: Cells, pub link: Link, revs: HashMap<String, f64>, rec: Reclaim }

impl FakeSeat {
    pub fn start(tag: &str) -> FakeSeat {
        let name = format!("mt{}{}", std::process::id() % 100000, tag);   // a region name is at most 31 bytes: /gl-<name>-ctl
        for r in ["ctl", "sc", "pc"] { let _ = remove_region(&format!("/gl-{name}-{r}")); }
        let mut ctl = Shm::create(format!("/gl-{name}-ctl"), 65536).expect("rings");
        let table = Cells::create(format!("/gl-{name}-sc"), NUMBERS.slots, NUMBERS.key_cap, NUMBERS.val_cap, 1).expect("seat table");
        let mut link = Link::new(Me { ns: "membrane".into(), build: "fake seat".into(), epoch: 1.0, support: vec![VERSION], initiator: true });
        ctl.send(&link.open()).unwrap();
        FakeSeat { name, ctl, table, link, revs: HashMap::new(), rec: Reclaim::new() }
    }
    pub fn peer(&self) -> Peer { self.peer_with(&NUMBERS) }
    /// the peer with its own table sized as the test says (the seat's `peers` fact would declare the same numbers)
    pub fn peer_with(&self, n: &Numbers) -> Peer { Peer::attach(&self.name, "membrama", n).expect("peer attaches") }
    /// the events the seat's link produced from what the peer sent, polling both sides a few rounds
    pub fn run(&mut self, peer: &mut Peer) -> Vec<Event> {
        let mut seen = Vec::new();
        for _ in 0..20 {
            peer.poll().unwrap();
            while let Some(f) = self.ctl.recv().unwrap() {
                let (events, reply) = self.link.receive(f);
                if let Some(r) = reply { self.ctl.send(&r).unwrap(); }
                seen.extend(events);
            }
        }
        seen
    }
}
impl FakeSeat {
    /// The seat publishes one cell (the next revision of it).
    pub fn put(&mut self, eid: &str, prop: &str, value: J) {
        let k = key(eid, prop);
        let rev = self.revs.get(&k).copied().unwrap_or(0.0) + 1.0;
        self.table.put(&k, rev, &value).unwrap();
        self.revs.insert(k, rev);
    }
    /// The seat's table could not store a value (it did not fit): the cell is Refused, and its value says why, as the seat's driver writes it.
    pub fn refuse(&mut self, eid: &str, prop: &str, why: &str) {
        let k = key(eid, prop);
        let rev = self.revs.get(&k).copied().unwrap_or(0.0) + 1.0;
        self.table.mark(&k, rev, CellState::Refused, why).unwrap();
        self.revs.insert(k, rev);
    }
    /// The seat removes a node (retires it): every cell it wrote for it, and its node cell, become Gone.
    pub fn retire(&mut self, eid: &str) {
        let prefix = format!("{eid}\u{1}");
        let keys: Vec<String> = self.revs.keys().filter(|k| k.starts_with(&prefix)).cloned().collect();
        for k in keys {
            let rev = self.revs[&k] + 1.0;
            self.table.mark(&k, rev, CellState::Gone, "").unwrap();
            self.revs.insert(k, rev);
        }
        let node = format!("{eid}\u{1}");
        let rev = self.revs.get(&node).copied().unwrap_or(0.0) + 1.0;
        self.table.mark(&node, rev, CellState::Gone, "").unwrap();
        self.revs.insert(node, rev);
    }
    /// The seat's turn as the reader of the peer's table, as its driver does it: read the count, scan, say it has been seen and take the peer's acknowledgement.
    pub fn acks(&mut self) {
        let r = self.reader();
        let seen = Reclaim::before(&r);
        let _ = r.scan("", 1_000_000, 32).unwrap();
        self.rec.after(seen, &r, &mut self.table, 1_000_000, 32).unwrap();
    }
    /// What the peer has published: a reader of the peer's table.
    pub fn reader(&self) -> CellsReader { CellsReader::attach(format!("/gl-{}-pc", self.name)).expect("the peer's table") }
}

/// The value of a live cell (a refused or gone cell is `None`).
pub fn cell(r: &CellsReader, eid: &str, prop: &str) -> Option<J> {
    r.get(&key(eid, prop), 1_000_000, 32).unwrap().filter(|c| c.state == graphlink_shm::cells::CellState::Live).map(|c| c.value)
}

impl Drop for FakeSeat { fn drop(&mut self) { for r in ["ctl", "sc", "pc"] { let _ = remove_region(&format!("/gl-{}-{r}", self.name)); } } }


/// The path of the model the model tests use: MEMBRAMA_TEST_MODEL names a Qwen3 chat gguf. Without it a test says so and passes, so CI never needs a model.
/// A process may start the backend once and the crate says contexts must not be built in parallel, and a model held in a static outlives the Metal
/// device's cleanup (the process aborts at exit), so a test file with model tests has ONE test that owns the model and runs its scenarios in turn.
pub fn model_path() -> Option<String> {
    let path = std::env::var("MEMBRAMA_TEST_MODEL").ok();
    if path.is_none() { eprintln!("SKIPPED: MEMBRAMA_TEST_MODEL is not set, so this was NOT tested against a model"); }
    path
}
