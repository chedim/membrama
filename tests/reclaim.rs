//! Reclaiming slots (graphlink docs/shared-memory.md, 4b), membrama's side: a cell table never reuses a slot, so when the seat retires a request membrama marks the cells of its reply and chunks Gone,
//! acknowledges the seat's removals, and gives its own acknowledged slots to the next replies. The test plays the seat with a table of membrama's that is too small to hold every reply.
mod common;
use common::{cell, FakeSeat, NUMBERS};
use graphlink_shm::cells::CellState;
use membrama::peer::{Numbers, Peer};
use membrama::turn::{Generator, Handler, Job, Usage};
use serde_json::json;

const SPINS: u64 = 1_000_000;
const DEPTH: usize = 32;

struct Gen;
impl Generator for Gen {
    fn generate(&mut self, _job: &Job, on_text: &mut dyn FnMut(&str) -> bool) -> Result<Usage, String> { on_text("answer"); Ok(Usage::default()) }
}
fn started(tag: &str, own_slots: u64) -> (FakeSeat, Peer, Handler) {
    let mut seat = FakeSeat::start(tag);
    let mut peer = seat.peer_with(&Numbers { slots: own_slots, ..NUMBERS });
    seat.run(&mut peer);
    (seat, peer, Handler::new())
}
fn ticks(h: &mut Handler, p: &mut Peer) { for _ in 0..4 { h.tick(p, &mut Gen).unwrap(); } }
/// the seat writes a request of one text part, under the id `id`
fn ask(seat: &mut FakeSeat, id: &str) {
    seat.put(&format!("membrane::LlmPart:{id}:0"), "kind", json!("text")); seat.put(&format!("membrane::LlmPart:{id}:0"), "text", json!("hi"));
    let req = format!("membrane::LlmRequest:{id}");
    seat.put(&req, "server", json!("m")); seat.put(&req, "parts", json!([format!("membrane::LlmPart:{id}:0")])); seat.put(&req, "settings", json!({"max_new": 8}));
}
fn retire(seat: &mut FakeSeat, id: &str) { seat.retire(&format!("membrane::LlmRequest:{id}")); seat.retire(&format!("membrane::LlmPart:{id}:0")); }
fn state(seat: &FakeSeat, eid: &str, prop: &str) -> Option<CellState> {
    seat.reader().get(&format!("{eid}\u{1}{prop}"), SPINS, DEPTH).unwrap().map(|c| c.state)
}

#[test]
fn a_reply_is_a_node_with_a_cell_of_its_own_that_says_it_exists() {
    let (mut seat, mut peer, mut h) = started("q1", 64);
    ask(&mut seat, "1"); ticks(&mut h, &mut peer);
    assert_eq!(state(&seat, "membrama::Reply:1", ""), Some(CellState::Live), "the seat's driver reads a node's own cell first");
    assert_eq!(cell(&seat.reader(), "membrama::Reply:1", "state"), Some(json!("done")));
}

#[test]
fn when_the_seat_retires_a_request_the_cells_of_its_reply_and_chunks_become_gone() {
    let (mut seat, mut peer, mut h) = started("q2", 64);
    ask(&mut seat, "1"); ticks(&mut h, &mut peer);
    assert_eq!(state(&seat, "membrama::Chunk:1:0", "text"), Some(CellState::Live));
    retire(&mut seat, "1");
    ticks(&mut h, &mut peer);
    for (eid, prop) in [("membrama::Reply:1", ""), ("membrama::Reply:1", "state"), ("membrama::Chunk:1:0", ""), ("membrama::Chunk:1:0", "text")] {
        assert_eq!(state(&seat, eid, prop), Some(CellState::Gone), "{eid} {prop:?}");
    }
}

#[test]
fn the_peer_says_which_removals_of_the_seats_table_it_has_seen() {
    let (mut seat, mut peer, mut h) = started("q3", 64);
    ask(&mut seat, "1"); ticks(&mut h, &mut peer);
    retire(&mut seat, "1");
    ticks(&mut h, &mut peer);
    assert!(seat.table.gone_seq() > 0);
    assert_eq!(seat.reader().peer_ack(SPINS, DEPTH).unwrap(), seat.table.gone_seq());
}

#[test]
fn replies_pass_through_a_table_too_small_for_them_all() {
    // 12 slots: one request takes several cells (its reply node and props, its chunk node and text) and the acknowledgement one, so the third would not fit without reuse
    let (mut seat, mut peer, mut h) = started("q4", 12);
    for round in 0..6 {
        let id = format!("r{round}");
        ask(&mut seat, &id); ticks(&mut h, &mut peer);
        assert_eq!(cell(&seat.reader(), &format!("membrama::Reply:{id}"), "state"), Some(json!("done")), "round {round}");
        assert_eq!(cell(&seat.reader(), &format!("membrama::Chunk:{id}:0"), "text"), Some(json!("answer")), "round {round}");
        seat.acks();                 // the seat has read the reply
        retire(&mut seat, &id);      // and retires the request
        ticks(&mut h, &mut peer);    // membrama marks the reply Gone and acknowledges
        seat.acks();                 // the seat has seen those removals and says so
        ticks(&mut h, &mut peer);    // membrama frees the slots the seat acknowledged
    }
}
