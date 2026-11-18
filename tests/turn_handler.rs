//! The turn handler (docs/design.md, "The turn, over GraphLink"): the seat writes a request node and its part nodes into its table; the handler runs
//! the complete request through a generator and publishes the reply and its chunks in the peer's table. A fake generator stands in for the model.
mod common;
use common::{cell, FakeSeat};
use graphlink_core::frame::{Op, Target};
use graphlink_core::link::Event;
use membrama::peer::Peer;
use membrama::turn::{Generator, Handler, Job, PartKind, Usage};
use serde_json::{json, Value as J};

#[derive(Default)]
struct FakeGen { pieces: Vec<String>, fail: Option<String>, seen: Vec<Vec<String>>, usage: Usage }
impl Generator for FakeGen {
    fn generate(&mut self, job: &Job, on_text: &mut dyn FnMut(&str) -> bool) -> Result<Usage, String> {
        self.seen.push(job.parts.iter().map(|p| p.text.clone()).collect());
        if let Some(f) = &self.fail { return Err(f.clone()); }
        for p in &self.pieces { if !on_text(p) { break; } }
        Ok(self.usage.clone())
    }
}

fn started(tag: &str) -> (FakeSeat, Peer, Handler) {
    let mut seat = FakeSeat::start(tag);
    let mut peer = seat.peer();
    seat.run(&mut peer);
    (seat, peer, Handler::new())
}
fn ticks(h: &mut Handler, peer: &mut Peer, g: &mut FakeGen) { for _ in 0..5 { h.tick(peer, g).unwrap(); } }

/// the seat writes a part node
fn part(seat: &mut FakeSeat, id: &str, n: usize, text: &str) {
    let eid = format!("membrane::LlmPart:{id}:{n}");
    seat.put(&eid, "kind", json!("text"));
    seat.put(&eid, "text", json!(text));
}
/// the seat writes the request node listing `n` parts
fn request(seat: &mut FakeSeat, id: &str, n: usize, settings: J) {
    let eid = format!("membrane::LlmRequest:{id}");
    seat.put(&eid, "server", json!("m"));
    seat.put(&eid, "parts", json!((0..n).map(|i| format!("membrane::LlmPart:{id}:{i}")).collect::<Vec<_>>()));
    seat.put(&eid, "settings", settings);
}

#[test]
fn a_complete_request_is_run_and_answered() {
    let (mut seat, mut peer, mut h) = started("t1");
    part(&mut seat, "1", 0, "Context: ");
    part(&mut seat, "1", 1, "Zorvex lives in Lima.");
    request(&mut seat, "1", 2, json!({"max_new": 12}));
    let mut g = FakeGen { pieces: vec!["Li".into(), "ma".into()], usage: Usage { read: 7, loaded: 0, generated: 2 }, ..Default::default() };
    ticks(&mut h, &mut peer, &mut g);
    assert_eq!(g.seen, vec![vec!["Context: ".to_string(), "Zorvex lives in Lima.".to_string()]], "the generator gets the parts as the seat wrote them, in order");
    let r = seat.reader();
    assert_eq!(cell(&r, "membrama::Reply:1", "state"), Some(json!("done")));
    assert_eq!(cell(&r, "membrama::Reply:1", "chunks"), Some(json!(1)));
    assert_eq!(cell(&r, "membrama::Chunk:1:0", "text"), Some(json!("Lima")));
    assert_eq!(cell(&r, "membrama::Reply:1", "usage"), Some(json!({"read": 7, "loaded": 0, "generated": 2})));
}

#[test]
fn a_request_missing_a_part_waits_for_it() {
    let (mut seat, mut peer, mut h) = started("t2");
    part(&mut seat, "1", 0, "Context: ");
    request(&mut seat, "1", 2, json!({"max_new": 12}));
    let mut g = FakeGen { pieces: vec!["ok".into()], ..Default::default() };
    ticks(&mut h, &mut peer, &mut g);
    assert!(g.seen.is_empty(), "not run while a part is missing");
    assert_eq!(cell(&seat.reader(), "membrama::Reply:1", "state"), None);
    part(&mut seat, "1", 1, "the rest");
    ticks(&mut h, &mut peer, &mut g);
    assert_eq!(g.seen.len(), 1);
    assert_eq!(cell(&seat.reader(), "membrama::Reply:1", "state"), Some(json!("done")));
}

#[test]
fn a_request_is_run_once() {
    let (mut seat, mut peer, mut h) = started("t3");
    part(&mut seat, "1", 0, "hi");
    request(&mut seat, "1", 1, json!({"max_new": 12}));
    let mut g = FakeGen { pieces: vec!["x".into()], ..Default::default() };
    ticks(&mut h, &mut peer, &mut g);
    ticks(&mut h, &mut peer, &mut g);
    assert_eq!(g.seen.len(), 1);
}

#[test]
fn a_long_reply_is_cut_into_chunks_on_character_boundaries() {
    let (mut seat, mut peer, mut h) = started("t4");
    part(&mut seat, "1", 0, "hi");
    request(&mut seat, "1", 1, json!({"max_new": 4000}));
    let mut g = FakeGen { pieces: vec!["é".to_string(); 2500], ..Default::default() };   // 5000 bytes, two bytes a character
    ticks(&mut h, &mut peer, &mut g);
    let r = seat.reader();
    let c0 = cell(&r, "membrama::Chunk:1:0", "text").unwrap(); let c1 = cell(&r, "membrama::Chunk:1:1", "text").unwrap();
    // a cell's value cap holds the whole CBOR item: a string this long has a 3-byte header, so a chunk is at most cap - 3 bytes, cut on a character
    let full = (common::NUMBERS.val_cap as usize - 3) / 2 * 2;
    assert_eq!(c0.as_str().unwrap().len(), full);
    assert_eq!(c1.as_str().unwrap().len(), 5000 - full);
    assert_eq!(cell(&r, "membrama::Reply:1", "chunks"), Some(json!(2)));
    assert_eq!(format!("{}{}", c0.as_str().unwrap(), c1.as_str().unwrap()), "é".repeat(2500));
}

#[test]
fn a_failed_generation_is_an_error_reply() {
    let (mut seat, mut peer, mut h) = started("t5");
    part(&mut seat, "1", 0, "hi");
    request(&mut seat, "1", 1, json!({"max_new": 12}));
    let mut g = FakeGen { fail: Some("boom".into()), ..Default::default() };
    ticks(&mut h, &mut peer, &mut g);
    let r = seat.reader();
    assert_eq!(cell(&r, "membrama::Reply:1", "state"), Some(json!("error")));
    assert!(cell(&r, "membrama::Reply:1", "error").unwrap().as_str().unwrap().contains("boom"));
}

#[test]
fn a_request_aborted_before_it_starts_is_not_run() {
    let (mut seat, mut peer, mut h) = started("t6");
    part(&mut seat, "1", 0, "hi");
    request(&mut seat, "1", 1, json!({"max_new": 12}));
    seat.put("membrane::LlmRequest:1", "abort", json!(true));
    let mut g = FakeGen { pieces: vec!["x".into()], ..Default::default() };
    ticks(&mut h, &mut peer, &mut g);
    assert!(g.seen.is_empty());
    assert_eq!(cell(&seat.reader(), "membrama::Reply:1", "state"), Some(json!("aborted")));
}

#[test]
fn a_request_without_max_new_is_an_error_not_a_guess() {
    let (mut seat, mut peer, mut h) = started("t7");
    part(&mut seat, "1", 0, "hi");
    request(&mut seat, "1", 1, json!({}));
    let mut g = FakeGen { pieces: vec!["x".into()], ..Default::default() };
    ticks(&mut h, &mut peer, &mut g);
    assert!(g.seen.is_empty());
    let r = seat.reader();
    assert_eq!(cell(&r, "membrama::Reply:1", "state"), Some(json!("error")));
    assert!(cell(&r, "membrama::Reply:1", "error").unwrap().as_str().unwrap().contains("max_new"));
}

#[test]
fn an_entity_part_carries_its_id_and_version() {
    let (mut seat, mut peer, mut h) = started("t8");
    let eid = "membrane::LlmPart:1:0";
    seat.put(eid, "kind", json!("entity")); seat.put(eid, "eid", json!("Zorvex")); seat.put(eid, "version", json!("v3")); seat.put(eid, "text", json!("Zorvex lives in Lima."));
    request(&mut seat, "1", 1, json!({"max_new": 12}));
    struct Probe(Option<PartKind>);
    impl Generator for Probe { fn generate(&mut self, job: &Job, _: &mut dyn FnMut(&str) -> bool) -> Result<Usage, String> { self.0 = Some(job.parts[0].kind.clone()); Ok(Usage::default()) } }
    let mut p = Probe(None);
    for _ in 0..5 { h.tick(&mut peer, &mut p).unwrap(); }
    assert_eq!(p.0, Some(PartKind::Entity { eid: "Zorvex".into(), version: "v3".into() }));
}

#[test]
fn the_handler_subscribes_to_requests_and_their_parts() {
    let (mut seat, mut peer, mut h) = started("t9");
    let mut g = FakeGen::default();
    h.tick(&mut peer, &mut g).unwrap();
    let events = seat.run(&mut peer);
    let prefixes: Vec<String> = events.iter().filter_map(|e| match e { Event::Op(Op::Sub { target: Target::Prefix(p), .. }) => Some(p.clone()), _ => None }).collect();
    assert!(prefixes.contains(&"membrane::LlmRequest:".to_string()) && prefixes.contains(&"membrane::LlmPart:".to_string()), "{prefixes:?}");
}
