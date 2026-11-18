//! Shared by the tests that play the seat: it makes the control rings and its cell table first and sends its hello before the peer starts,
//! the way the seat's driver does.
#![allow(dead_code)]
use graphlink_core::frame::{Op, Target, VERSION};
use graphlink_core::link::{Event, Link, Me};
use graphlink_core::transport::Transport;
use graphlink_shm::cells::{Cells, CellsReader};
use graphlink_shm::{remove_region, Shm};
use membrama::peer::{Numbers, Peer};
use serde_json::Value as J;
use std::collections::HashMap;

pub const NUMBERS: Numbers = Numbers { slots: 64, key_cap: 128, val_cap: 4096, epoch: 1, spins: 1_000_000, depth: 32 };
pub fn key(eid: &str, prop: &str) -> String { format!("{eid}\u{1}{prop}") }

pub struct FakeSeat { pub name: String, pub ctl: Shm, pub table: Cells, pub link: Link, revs: HashMap<String, f64> }

impl FakeSeat {
    pub fn start(tag: &str) -> FakeSeat {
        let name = format!("mt{}{}", std::process::id() % 100000, tag);   // a region name is at most 31 bytes: /gl-<name>-ctl
        for r in ["ctl", "sc", "pc"] { let _ = remove_region(&format!("/gl-{name}-{r}")); }
        let mut ctl = Shm::create(format!("/gl-{name}-ctl"), 65536).expect("rings");
        let table = Cells::create(format!("/gl-{name}-sc"), NUMBERS.slots, NUMBERS.key_cap, NUMBERS.val_cap, 1).expect("seat table");
        let mut link = Link::new(Me { ns: "membrane".into(), build: "fake seat".into(), epoch: 1.0, support: vec![VERSION], initiator: true });
        ctl.send(&link.open()).unwrap();
        FakeSeat { name, ctl, table, link, revs: HashMap::new() }
    }
    pub fn peer(&self) -> Peer { Peer::attach(&self.name, "membrama", &NUMBERS).expect("peer attaches") }
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
    /// What the peer has published: a reader of the peer's table.
    pub fn reader(&self) -> CellsReader { CellsReader::attach(format!("/gl-{}-pc", self.name)).expect("the peer's table") }
}

pub fn cell(r: &CellsReader, eid: &str, prop: &str) -> Option<J> { r.get(&key(eid, prop), 1_000_000, 32).unwrap().map(|c| c.value) }

impl Drop for FakeSeat { fn drop(&mut self) { for r in ["ctl", "sc", "pc"] { let _ = remove_region(&format!("/gl-{}-{r}", self.name)); } } }

