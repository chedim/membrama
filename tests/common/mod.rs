//! Shared by the tests that play the seat: it makes the control rings and its cell table first and sends its hello before the peer starts,
//! the way the seat's driver does.
#![allow(dead_code)]
use graphlink_core::frame::VERSION;
use graphlink_core::link::{Event, Link, Me};
use graphlink_core::transport::Transport;
use graphlink_shm::cells::{Cells, CellsReader};
use graphlink_shm::{remove_region, Shm};
use membrama::peer::{Numbers, Peer};
use serde_json::Value as J;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::LlamaModel;

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

/// The value of a live cell (a refused or gone cell is `None`).
pub fn cell(r: &CellsReader, eid: &str, prop: &str) -> Option<J> {
    r.get(&key(eid, prop), 1_000_000, 32).unwrap().filter(|c| c.state == graphlink_shm::cells::CellState::Live).map(|c| c.value)
}

impl Drop for FakeSeat { fn drop(&mut self) { for r in ["ctl", "sc", "pc"] { let _ = remove_region(&format!("/gl-{}-{r}", self.name)); } } }


struct Shared { backend: LlamaBackend, model: LlamaModel }
static SHARED: OnceLock<Shared> = OnceLock::new();
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// Runs `f` with the real model: MEMBRAMA_TEST_MODEL names a Qwen3 chat gguf. Without it the test says so and passes, so CI never needs a model.
/// A process may start the backend once and the crate says contexts must not be built in parallel, so the model is loaded once and the tests take turns.
pub fn with_model(f: impl FnOnce(&LlamaBackend, &LlamaModel)) {
    let Ok(path) = std::env::var("MEMBRAMA_TEST_MODEL") else { eprintln!("SKIPPED: MEMBRAMA_TEST_MODEL is not set, so this was NOT tested against a model"); return; };
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let s = SHARED.get_or_init(|| {
        let backend = LlamaBackend::init().expect("backend");
        let model = LlamaModel::load_from_file(&backend, &path, &LlamaModelParams::default().with_n_gpu_layers(99)).expect("model");
        Shared { backend, model }
    });
    f(&s.backend, &s.model)
}
