//! The GraphLink side: membrama as a peer of the seat over the shared-memory regions the seat made (docs/design.md, "The turn, over GraphLink").
//! The seat makes the control rings `/gl-<name>-ctl` and its cell table `/gl-<name>-sc` and sends its hello first; the peer attaches both, answers,
//! makes its own cell table `/gl-<name>-pc` (it is the only writer; the seat maps it read-only) and reads the seat's cells it subscribed to.
//! Sans-I/O like the library: `poll` is called by whoever owns the thread; nothing here blocks or starts a thread.
use graphlink_core::frame::{Op, QueryBy, Target, VERSION};
use graphlink_core::link::{Event, Link, Me};
use graphlink_core::transport::Transport;
use graphlink_shm::cells::{CellState, Cells, CellsReader, Reclaim};
use graphlink_shm::Shm;
use serde_json::Value as J;
use std::collections::{HashMap, HashSet};

/// What a cell of the seat's table says: its value, that it is not there (yet, or gone), or that the seat's table could not store the value and why (a `Refused` cell).
#[derive(Clone, Debug, PartialEq)]
pub enum Seen { Live(J), Absent, Refused(String) }

/// The owner's numbers for the regions (the `peers` fact declares the same ones to the seat).
#[derive(Clone, Debug, PartialEq)]
pub struct Numbers { pub slots: u64, pub key_cap: u32, pub val_cap: u32, pub epoch: u64, pub spins: u64, pub depth: usize }

pub struct Peer {
    ctl: Shm,
    link: Link,
    own: Cells,
    seat: CellsReader,
    revs: HashMap<String, f64>,
    /// the nodes this peer has made a cell for (the node's own cell, which tells the seat's driver the node exists)
    nodes: HashSet<String>,
    reclaim: Reclaim,
    hello_sent: bool,
    spins: u64,
    depth: usize,
    val_cap: u32,
    asked: u64,
}

/// A cell's key: the node id, U+0001, the prop (`docs/graph-link.md`).
fn cell_key(eid: &str, prop: &str) -> String { format!("{eid}\u{1}{prop}") }

impl Peer {
    /// The token the seat made for this peer; it goes in the hello (the seat refuses a hello without one).
    pub fn with_token(mut self, token: String) -> Peer { self.link = self.link.with_token(token); self }

    /// `name` is the peer's name in the seat's `peers` fact (the regions are `/gl-<name>-ctl`, `-sc`, `-pc`); `ns` is the namespace it owns.
    pub fn attach(name: &str, ns: &str, n: &Numbers) -> Result<Peer, String> {
        let ctl = Shm::attach(format!("/gl-{name}-ctl"))?;
        let _ = graphlink_shm::remove_region(&format!("/gl-{name}-pc"));   // an old table of a peer that died
        let own = Cells::create(format!("/gl-{name}-pc"), n.slots, n.key_cap, n.val_cap, n.epoch)?;
        let seat = CellsReader::attach(format!("/gl-{name}-sc"))?;
        let link = Link::new(Me { ns: ns.into(), build: format!("membrama {}", env!("CARGO_PKG_VERSION")), epoch: n.epoch as f64, support: vec![VERSION], initiator: false });
        Ok(Peer { ctl, link, own, seat, revs: HashMap::new(), nodes: HashSet::new(), reclaim: Reclaim::new(), hello_sent: false, spins: n.spins, depth: n.depth, val_cap: n.val_cap, asked: 0 })
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

    /// Writes one cell of this peer's table (the next revision of it); the seat reads it as a replica of a node this namespace owns. The first cell of a node also makes the node's own
    /// cell (the node's id and an empty prop), which is what the seat's driver reads first.
    pub fn publish(&mut self, eid: &str, prop: &str, value: &J) -> Result<(), String> {
        if self.nodes.insert(eid.to_string()) {
            if let Err(e) = self.put_cell(&cell_key(eid, ""), &J::Bool(true)) { self.nodes.remove(eid); return Err(e); }
        }
        self.put_cell(&cell_key(eid, prop), value)
    }

    fn put_cell(&mut self, key: &str, value: &J) -> Result<(), String> {
        let rev = self.revs.get(key).copied().unwrap_or(0.0) + 1.0;
        self.own.put(key, rev, value)?;
        self.revs.insert(key.to_string(), rev);
        Ok(())
    }

    /// A node this peer owns goes away (the seat retired the request it answered): the node's own cell and every cell of it become Gone, and their slots can be reused once the seat has acknowledged.
    pub fn retire(&mut self, eid: &str) -> Result<(), String> {
        if !self.nodes.remove(eid) { return Ok(()); }
        let prefix = cell_key(eid, "");
        let mut keys: Vec<String> = self.revs.keys().filter(|k| k.starts_with(&prefix)).cloned().collect();
        keys.sort();
        for k in keys {
            let rev = self.revs[&k] + 1.0;
            self.own.mark(&k, rev, CellState::Gone, "")?;
            self.revs.insert(k, rev);
        }
        Ok(())
    }

    /// Retires every node this peer owns whose id starts with `prefix` (the chunk nodes of one reply).
    pub fn retire_prefix(&mut self, prefix: &str) -> Result<(), String> {
        let mut ids: Vec<String> = self.nodes.iter().filter(|e| e.starts_with(prefix)).cloned().collect();
        ids.sort();
        for e in ids { self.retire(&e)?; }
        Ok(())
    }

    /// Before reading the seat's table in a turn: the count of the seat's removals, to acknowledge after the pass.
    pub fn before(&self) -> u64 { Reclaim::before(&self.seat) }

    /// After the pass: say that the seat's removals up to `seen` have been seen, and reuse the slots of this table the seat has acknowledged. An acknowledgement that cannot be written
    /// (this table has no slot left for it) is an error and is tried again next turn.
    pub fn after(&mut self, seen: u64) -> Result<(), String> { self.reclaim.after(seen, &self.seat, &mut self.own, self.spins, self.depth) }

    /// Asks the seat to publish a node (or a prefix) of its own to `/gl-<name>-sc`.
    pub fn subscribe(&mut self, target: Target, props: Option<Vec<String>>) -> Result<(), String> {
        let frame = self.link.send(vec![Op::Sub { target, props, limit: None }])?;
        self.ctl.send(&frame).map_err(|e| format!("the control rings: {e:?}"))
    }

    /// Tells the seat to stop publishing a node it was asked to.
    pub fn unsubscribe(&mut self, target: Target) -> Result<(), String> { self.send_ops(vec![Op::Unsub { target }]) }

    /// The latest value of a cell the seat published, or `None` when it is not there (or is gone or refused).
    pub fn read(&self, eid: &str, prop: &str) -> Result<Option<J>, String> {
        Ok(self.seat.get(&cell_key(eid, prop), self.spins, self.depth)?.filter(|c| c.state == CellState::Live).map(|c| c.value))
    }

    /// What the seat's cell says, with a refusal told apart from absence: a value that did not fit the seat's table is a `Refused` cell, and waiting for it would be waiting for ever.
    pub fn seen(&self, eid: &str, prop: &str) -> Result<Seen, String> {
        Ok(match self.seat.get(&cell_key(eid, prop), self.spins, self.depth)? {
            Some(c) if c.state == CellState::Live => Seen::Live(c.value),
            Some(c) if c.state == CellState::Refused => Seen::Refused(c.value.as_str().unwrap_or("refused").to_string()),
            _ => Seen::Absent,
        })
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

    /// The next id for an op that gets an answer (`wid`, `rid`, `qid`).
    fn next_id(&mut self, kind: &str) -> String { self.asked += 1; format!("{kind}{}", self.asked) }

    fn send_ops(&mut self, ops: Vec<Op>) -> Result<(), String> {
        let frame = self.link.send(ops)?;
        self.ctl.send(&frame).map_err(|e| format!("the control rings: {e:?}"))
    }

    /// Writes a node the seat owns (`wr`); the id of the write, whose answer comes back as an `Op::Res` event.
    pub fn write(&mut self, id: &str, prop: &str, value: &J) -> Result<String, String> {
        let wid = self.next_id("w");
        self.send_ops(vec![Op::Wr { wid: wid.clone(), id: id.into(), prop: prop.into(), value: value.clone() }])?;
        Ok(wid)
    }

    /// Asks the seat for the props of a node (`read`); the id of the ask, whose answer comes back as an `Op::Val` event with that `rid`.
    pub fn read_req(&mut self, id: &str, props: &[String]) -> Result<String, String> {
        let rid = self.next_id("r");
        self.send_ops(vec![Op::Read { id: id.into(), props: props.to_vec(), rid: rid.clone() }])?;
        Ok(rid)
    }

    /// Asks the seat for a page of nodes (`q`); the id of the query, whose answer comes back as an `Op::Page` event.
    pub fn query(&mut self, by: QueryBy, after: Option<String>, limit: Option<u64>) -> Result<String, String> {
        let qid = self.next_id("q");
        self.send_ops(vec![Op::Query { qid: qid.clone(), by, after, limit }])?;
        Ok(qid)
    }

    /// Whether the seat says a node it published exists: `None` while it has said nothing about it.
    pub fn exists(&self, eid: &str) -> Result<Option<bool>, String> {
        Ok(self.seat.get(&cell_key(eid, ""), self.spins, self.depth)?.map(|c| c.state == CellState::Live))
    }
}
