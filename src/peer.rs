//! The GraphLink side: membrama as a peer of the seat over the shared-memory regions the seat made (docs/design.md, "The turn, over GraphLink").
//! The seat makes the control rings `/gl-<name>-ctl` and its cell table `/gl-<name>-sc` and sends its hello first; the peer attaches both, answers,
//! makes its own cell table `/gl-<name>-pc` (it is the only writer; the seat maps it read-only) and reads the seat's cells it subscribed to.
//! Sans-I/O like the library: `poll` is called by whoever owns the thread; nothing here blocks or starts a thread.
use graphlink_core::frame::{Op, Target, VERSION};
use graphlink_core::link::{Event, Link, Me};
use graphlink_core::transport::Transport;
use graphlink_shm::cells::{CellState, Cells, CellsReader};
use graphlink_shm::Shm;
use serde_json::Value as J;
use std::collections::HashMap;

/// The owner's numbers for the regions (the `peers` fact declares the same ones to the seat).
#[derive(Clone, Debug, PartialEq)]
pub struct Numbers { pub slots: u64, pub key_cap: u32, pub val_cap: u32, pub epoch: u64, pub spins: u64, pub depth: usize }

pub struct Peer {
    ctl: Shm,
    link: Link,
    own: Cells,
    seat: CellsReader,
    revs: HashMap<String, f64>,
    hello_sent: bool,
    spins: u64,
    depth: usize,
    val_cap: u32,
}

/// A cell's key: the node id, U+0001, the prop (`docs/graph-link.md`).
fn cell_key(eid: &str, prop: &str) -> String { format!("{eid}\u{1}{prop}") }

impl Peer {
    /// The token the seat made for this peer; it goes in the hello (the seat refuses a hello without one).
    /// RED STUB: ignored.
    pub fn with_token(self, _token: String) -> Peer { self }

    /// `name` is the peer's name in the seat's `peers` fact (the regions are `/gl-<name>-ctl`, `-sc`, `-pc`); `ns` is the namespace it owns.
    pub fn attach(name: &str, ns: &str, n: &Numbers) -> Result<Peer, String> {
        let ctl = Shm::attach(format!("/gl-{name}-ctl"))?;
        let _ = graphlink_shm::remove_region(&format!("/gl-{name}-pc"));   // an old table of a peer that died
        let own = Cells::create(format!("/gl-{name}-pc"), n.slots, n.key_cap, n.val_cap, n.epoch)?;
        let seat = CellsReader::attach(format!("/gl-{name}-sc"))?;
        let link = Link::new(Me { ns: ns.into(), build: format!("membrama {}", env!("CARGO_PKG_VERSION")), epoch: n.epoch as f64, support: vec![VERSION], initiator: false });
        Ok(Peer { ctl, link, own, seat, revs: HashMap::new(), hello_sent: false, spins: n.spins, depth: n.depth, val_cap: n.val_cap })
    }

    /// Takes the frames that arrived, answers what the link answers, and returns the events (ops from the seat among them).
    pub fn poll(&mut self) -> Result<Vec<Event>, String> {
        let mut events = Vec::new();
        while let Some(frame) = self.ctl.recv().map_err(|e| format!("the control rings: {e:?}"))? {
            let (evs, reply) = self.link.receive(frame);
            if let Some(r) = reply { self.ctl.send(&r).map_err(|e| format!("the control rings: {e:?}"))?; }
            events.extend(evs);
        }
        // the seat spoke first; the link does not answer a hello by itself, so the peer sends its own once it has seen the seat's
        if !self.hello_sent && events.iter().any(|e| matches!(e, Event::Opened { .. })) {
            let hello = self.link.open();
            self.ctl.send(&hello).map_err(|e| format!("the control rings: {e:?}"))?;
            self.hello_sent = true;
        }
        Ok(events)
    }

    pub fn is_open(&self) -> bool { self.link.is_open() }

    /// Whether the seat's end of the control rings has been dropped (the seat stopped): the peer should end.
    pub fn is_closed(&self) -> bool { self.ctl.is_closed() }

    /// Writes one cell of this peer's table (the next revision of it); the seat reads it as a replica of a node this namespace owns.
    pub fn publish(&mut self, eid: &str, prop: &str, value: &J) -> Result<(), String> {
        let key = cell_key(eid, prop);
        let rev = self.revs.get(&key).copied().unwrap_or(0.0) + 1.0;
        self.own.put(&key, rev, value)?;
        self.revs.insert(key, rev);
        Ok(())
    }

    /// Asks the seat to publish a node (or a prefix) of its own to `/gl-<name>-sc`.
    pub fn subscribe(&mut self, target: Target, props: Option<Vec<String>>) -> Result<(), String> {
        let frame = self.link.send(vec![Op::Sub { target, props, limit: None }])?;
        self.ctl.send(&frame).map_err(|e| format!("the control rings: {e:?}"))
    }

    /// The latest value of a cell the seat published, or `None` when it is not there (or is gone or refused).
    pub fn read(&self, eid: &str, prop: &str) -> Result<Option<J>, String> {
        Ok(self.seat.get(&cell_key(eid, prop), self.spins, self.depth)?.filter(|c| c.state == CellState::Live).map(|c| c.value))
    }

    /// The most bytes a cell's value can take (the CBOR item as a whole, its header included).
    pub fn val_cap(&self) -> u32 { self.val_cap }

    /// The live cells of the seat's table whose node id starts with `prefix`: `(node id, prop, value)`.
    pub fn scan(&self, prefix: &str) -> Result<Vec<(String, String, J)>, String> {
        let mut out = Vec::new();
        for (key, cell) in self.seat.scan(prefix, self.spins, self.depth)? {
            if cell.state != CellState::Live { continue; }
            if let Some((eid, prop)) = key.split_once('\u{1}') { out.push((eid.to_string(), prop.to_string(), cell.value)); }
        }
        Ok(out)
    }
}
