//! membrama as a peer over the shared-memory regions (docs/design.md, "The turn, over GraphLink"). The test plays the seat (tests/common).
mod common;
use common::{key, FakeSeat};
use graphlink_core::frame::{Op, Target};
use graphlink_core::link::Event;
use graphlink_shm::cells::CellsReader;
use serde_json::json;

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

#[test]
fn scan_lists_the_live_cells_under_a_prefix() {
    let mut seat = FakeSeat::start("e");
    let mut peer = seat.peer();
    seat.run(&mut peer);
    seat.put("membrane::LlmRequest:1", "server", json!("m"));
    seat.put("membrane::LlmRequest:1", "parts", json!(["membrane::LlmPart:1:0"]));
    seat.put("membrane::LlmRequest:2", "server", json!("m"));
    seat.put("membrane::Other:1", "server", json!("x"));
    let mut got = peer.scan("membrane::LlmRequest:").unwrap();
    got.sort_by(|a, b| (a.0.clone(), a.1.clone()).cmp(&(b.0.clone(), b.1.clone())));
    assert_eq!(got, vec![
        ("membrane::LlmRequest:1".to_string(), "parts".to_string(), json!(["membrane::LlmPart:1:0"])),
        ("membrane::LlmRequest:1".to_string(), "server".to_string(), json!("m")),
        ("membrane::LlmRequest:2".to_string(), "server".to_string(), json!("m")),
    ]);
}

#[test]
fn the_peer_sees_the_seats_end_close() {
    use graphlink_shm::{remove_region, Shm};
    let name = format!("mt{}z", std::process::id() % 100000);
    for r in ["ctl", "sc", "pc"] { let _ = remove_region(&format!("/gl-{name}-{r}")); }
    let ctl = Shm::create(format!("/gl-{name}-ctl"), 65536).unwrap();
    let _table = graphlink_shm::cells::Cells::create(format!("/gl-{name}-sc"), 64, 128, 4096, 1).unwrap();
    let peer = membrama::peer::Peer::attach(&name, "membrama", &common::NUMBERS).unwrap();
    assert!(!peer.is_closed(), "open while the seat's end is there");
    drop(ctl);
    assert!(peer.is_closed(), "closed once the seat's end is dropped");
    for r in ["ctl", "sc", "pc"] { let _ = remove_region(&format!("/gl-{name}-{r}")); }
}
