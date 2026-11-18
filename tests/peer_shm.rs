//! membrama as a peer over the shared-memory regions (docs/design.md, "The turn, over GraphLink"). The test plays the seat the way the seat's
//! driver does: it makes the control rings and its cell table first, and sends its hello before the peer is started.
use graphlink_core::frame::{Frame, Op, Target, VERSION};
use graphlink_core::link::{Event, Link, Me};
use graphlink_core::transport::Transport;
use graphlink_shm::cells::{Cells, CellsReader};
use graphlink_shm::{remove_region, Shm};
use membrama::peer::{Numbers, Peer};
use serde_json::json;

const NUMBERS: Numbers = Numbers { slots: 64, key_cap: 128, val_cap: 4096, epoch: 1, spins: 1_000_000, depth: 32 };
fn key(eid: &str, prop: &str) -> String { format!("{eid}\u{1}{prop}") }

struct FakeSeat { name: String, ctl: Shm, table: Cells, link: Link }

impl FakeSeat {
    fn start(tag: &str) -> FakeSeat {
        let name = format!("mt{}{}", std::process::id() % 100000, tag);   // a region name is at most 31 bytes: /gl-<name>-ctl
        for r in ["ctl", "sc", "pc"] { let _ = remove_region(&format!("/gl-{name}-{r}")); }
        let mut ctl = Shm::create(format!("/gl-{name}-ctl"), 65536).expect("rings");
        let table = Cells::create(format!("/gl-{name}-sc"), NUMBERS.slots, NUMBERS.key_cap, NUMBERS.val_cap, 1).expect("seat table");
        let mut link = Link::new(Me { ns: "membrane".into(), build: "fake seat".into(), epoch: 1.0, support: vec![VERSION], initiator: true });
        ctl.send(&link.open()).unwrap();
        FakeSeat { name, ctl, table, link }
    }
    fn peer(&self) -> Peer { Peer::attach(&self.name, "membrama", &NUMBERS).expect("peer attaches") }
    /// the events the seat's link produced from what the peer sent, polling both sides a few rounds
    fn run(&mut self, peer: &mut Peer) -> Vec<Event> {
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
impl Drop for FakeSeat { fn drop(&mut self) { for r in ["ctl", "sc", "pc"] { let _ = remove_region(&format!("/gl-{}-{r}", self.name)); } } }

#[test]
fn the_peer_answers_the_seats_hello_with_its_namespace() {
    let mut seat = FakeSeat::start("a");
    let mut peer = seat.peer();
    let events = seat.run(&mut peer);
    assert!(events.iter().any(|e| matches!(e, Event::Opened { peer_ns, .. } if peer_ns == "membrama")), "the seat should see the peer open: {events:?}");
    assert!(peer.is_open());
}

#[test]
fn a_node_the_peer_publishes_is_read_by_the_seat_from_the_peers_table() {
    let mut seat = FakeSeat::start("b");
    let mut peer = seat.peer();
    seat.run(&mut peer);
    peer.publish("membrama::Reply:7", "state", &json!("running")).unwrap();
    peer.publish("membrama::Reply:7", "state", &json!("done")).unwrap();
    let reader = CellsReader::attach(format!("/gl-{}-pc", seat.name)).unwrap();
    let cell = reader.get(&key("membrama::Reply:7", "state"), 1_000_000, 32).unwrap().expect("the cell is there");
    assert_eq!(cell.value, json!("done"));
    assert_eq!(cell.rev, 2.0, "each write of the same cell is the next revision");
}

#[test]
fn a_subscription_reaches_the_seat_as_a_sub_op() {
    let mut seat = FakeSeat::start("c");
    let mut peer = seat.peer();
    seat.run(&mut peer);
    peer.subscribe(Target::Id("membrane::LlmRequest:7".into()), None).unwrap();
    let events = seat.run(&mut peer);
    assert!(events.iter().any(|e| matches!(e, Event::Op(Op::Sub { target: Target::Id(id), .. }) if id == "membrane::LlmRequest:7")), "the seat should get the sub: {events:?}");
}

#[test]
fn a_cell_the_seat_published_is_read_by_the_peer() {
    let mut seat = FakeSeat::start("d");
    let mut peer = seat.peer();
    seat.run(&mut peer);
    seat.table.put(&key("membrane::LlmRequest:7", "server"), 1.0, &json!("qwen3")).unwrap();
    assert_eq!(peer.read("membrane::LlmRequest:7", "server").unwrap(), Some(json!("qwen3")));
    assert_eq!(peer.read("membrane::LlmRequest:7", "nothing").unwrap(), None);
}
