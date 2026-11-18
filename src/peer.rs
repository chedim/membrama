//! The GraphLink side: membrama as a peer of the seat over the shared-memory regions the seat made (docs/design.md, "The turn, over GraphLink").
//! RED STUB: it attaches and makes its table, and does nothing else; the tests fail on their assertions.
use graphlink_core::frame::Target;
use graphlink_core::link::Event;
use graphlink_shm::cells::{Cells, CellsReader};
use graphlink_shm::Shm;
use serde_json::Value as J;

/// The owner's numbers for the regions (the `peers` fact declares the same ones to the seat).
#[derive(Clone, Debug)]
pub struct Numbers { pub slots: u64, pub key_cap: u32, pub val_cap: u32, pub epoch: u64, pub spins: u64, pub depth: usize }

pub struct Peer {
    _ctl: Shm,
    _own: Cells,
    _seat: CellsReader,
}

impl Peer {
    /// `name` is the peer's name in the seat's `peers` fact (the regions are `/gl-<name>-ctl`, `-sc`, `-pc`); `ns` is the namespace it owns.
    pub fn attach(name: &str, _ns: &str, n: &Numbers) -> Result<Peer, String> {
        let ctl = Shm::attach(format!("/gl-{name}-ctl"))?;
        let _ = graphlink_shm::remove_region(&format!("/gl-{name}-pc"));
        let own = Cells::create(format!("/gl-{name}-pc"), n.slots, n.key_cap, n.val_cap, n.epoch)?;
        let seat = CellsReader::attach(format!("/gl-{name}-sc"))?;
        Ok(Peer { _ctl: ctl, _own: own, _seat: seat })
    }
    pub fn poll(&mut self) -> Result<Vec<Event>, String> { Ok(Vec::new()) }
    pub fn is_open(&self) -> bool { false }
    pub fn publish(&mut self, _eid: &str, _prop: &str, _value: &J) -> Result<(), String> { Ok(()) }
    pub fn subscribe(&mut self, _target: Target, _props: Option<Vec<String>>) -> Result<(), String> { Ok(()) }
    pub fn read(&self, _eid: &str, _prop: &str) -> Result<Option<J>, String> { Ok(None) }
}
