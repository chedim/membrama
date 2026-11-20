//! The loop (docs/design.md, "The loop"): membrama as an always-on co-processor over the graph link. The test plays the seat (`Sim`: the seat's end of the
//! link, its LogView, the writes it takes or refuses) and uses a STUB MODEL (a scripted function). That proves the loop's own logic: what wakes a step, what
//! the step is told, what the tools do on the link, what is visible when something fails. It proves nothing about what a real model writes in answer, about the
//! seat's real LogView or write gate (cliffvm), or about the packs running in a real engine.
mod common;
use common::{cell, FakeSeat};
use graphlink_core::frame::{Op, Target};
use graphlink_core::link::Event;
use graphlink_core::transport::Transport;
use graphlink_shm::cells::CellsReader;
use membrama::brain::*;
use membrama::peer::Peer;
use membrama::toolcall::ToolCall;
use serde_json::{json, Value as J};
use std::collections::{HashMap, VecDeque};

const LOGVIEW: &str = "membrane::LogView:membrama";

struct Sim {
    seat: FakeSeat,
    nodes: HashMap<String, HashMap<String, J>>,
    journal: Vec<J>,
    writes: Vec<(String, String, J)>,
    subs: Vec<String>,
    prefix_subs: Vec<String>,
    refuse: Option<String>,
    /// refuse only the writes to nodes whose id holds this text
    refuse_for: Option<String>,
    reader: Option<CellsReader>,
    served_n: f64,
    /// the LogView's page does not fit the peer's table: the cell is Refused, as the seat's driver writes it
    rows_too_big: bool,
}

impl Sim {
    fn new(tag: &str, doctrine: Option<&str>) -> (Sim, Peer) {
        let seat = FakeSeat::start(tag);
        let mut peer = seat.peer();
        let mut sim = Sim { seat, nodes: HashMap::new(), journal: vec![], writes: vec![], subs: vec![], prefix_subs: vec![], refuse: None, refuse_for: None, reader: None, served_n: -1.0, rows_too_big: false };
        if let Some(d) = doctrine { sim.nodes.entry(DOCTRINE_ID.into()).or_default().insert("text".into(), json!(d)); }
        sim.pump(&mut peer);
        (sim, peer)
    }
    /// something outside wrote a node's property (a person, a driver): one line of the boundary log
    fn ext(&mut self, eid: &str, prop: &str, value: &str) {
        self.nodes.entry(eid.into()).or_default().insert(prop.into(), json!(value));
        self.log(eid, prop, "person:1", value.len());
    }
    fn log(&mut self, eid: &str, prop: &str, writer: &str, size: usize) {
        let n = self.journal.len();
        self.journal.push(json!({"n": n, "eid": eid, "prop": prop, "op": "set", "writer": writer, "ts": 0, "size": size}));
    }
    fn set_node(&mut self, eid: &str, prop: &str, v: J) {
        self.nodes.entry(eid.into()).or_default().insert(prop.into(), v);
        if self.subs.iter().any(|s| s == eid) || self.prefix_subs.iter().any(|p| eid.starts_with(p.as_str())) { self.publish_node(eid); }
    }
    fn publish_node(&mut self, eid: &str) {
        let props: Vec<(String, J)> = self.nodes.get(eid).map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect()).unwrap_or_default();
        if props.is_empty() && !self.nodes.contains_key(eid) { self.seat.retire(eid); return; }
        self.seat.put(eid, "", json!(true));
        for (k, v) in props { self.seat.put(eid, &k, v); }
    }
    fn on_op(&mut self, op: Op, out: &mut Vec<Op>) {
        match op {
            Op::Sub { target: Target::Id(id), .. } => { self.subs.push(id.clone()); self.publish_node(&id); }
            Op::Sub { target: Target::Prefix(p), .. } => {
                self.prefix_subs.push(p.clone());
                let ids: Vec<String> = self.nodes.keys().filter(|k| k.starts_with(&p)).cloned().collect();
                for id in ids { self.publish_node(&id); }
            }
            Op::Wr { wid, id, prop, value } => {
                if let (Some(m), true) = (self.refuse_for.clone(), true) { if id.contains(&m) { out.push(Op::Res { wid, refused: Some("no tool rows for you".into()) }); return; } }
                if let Some(why) = self.refuse.clone() { out.push(Op::Res { wid, refused: Some(why) }); return; }
                self.writes.push((id.clone(), prop.clone(), value.clone()));
                self.nodes.entry(id.clone()).or_default().insert(prop.clone(), value.clone());
                self.log(&id, &prop, "link:membrama", value.to_string().len());
                if self.subs.contains(&id) { self.publish_node(&id); }
                out.push(Op::Res { wid, refused: None });
            }
            // the real seat answers `wr` and publishes what is subscribed; it does not answer `read` or `query` ops (cliffvm peers.rs)
            _ => {}
        }
    }
    /// the LogView: answers an ask (a new `n` in the peer's own `LogAsk:main`) with the page from `from`
    fn serve_log(&mut self) {
        if self.reader.is_none() { self.reader = CellsReader::attach(format!("/gl-{}-pc", self.seat.name)).ok(); }
        let Some(r) = &self.reader else { return };
        let Some(n) = cell(r, "membrama::LogAsk:main", "n").and_then(|v| v.as_f64()) else { return };
        if n == self.served_n { return; }
        self.served_n = n;
        let from = cell(r, "membrama::LogAsk:main", "from").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let rows: Vec<J> = self.journal.iter().skip(from).take(200).cloned().collect();
        let next = from + rows.len();
        let head = if self.journal.is_empty() { J::Null } else { json!(self.journal.len() - 1) };
        if self.rows_too_big { self.seat.refuse(LOGVIEW, "rows", "value is 31000 bytes, the table's value size is 4096"); } else { self.seat.put(LOGVIEW, "rows", json!(rows)); }
        self.seat.put(LOGVIEW, "next", json!(next));
        self.seat.put(LOGVIEW, "head", head);
    }
    /// one turn of the seat, then the peer's turn: the events the peer got
    fn pump(&mut self, peer: &mut Peer) -> Vec<Event> {
        let mut out = Vec::new();
        while let Some(f) = self.seat.ctl.recv().unwrap() {
            let (events, reply) = self.seat.link.receive(f);
            if let Some(r) = reply { self.seat.ctl.send(&r).unwrap(); }
            for e in events { if let Event::Op(op) = e { self.on_op(op, &mut out); } }
        }
        self.serve_log();
        if !out.is_empty() { let fr = self.seat.link.send(out).unwrap(); self.seat.ctl.send(&fr).unwrap(); }
        peer.poll().unwrap()
    }
    fn written(&self, eid: &str, prop: &str) -> Vec<J> { self.writes.iter().filter(|w| w.0 == eid && w.1 == prop).map(|w| w.2.clone()).collect() }
}

#[derive(Default)]
struct Stub { seen: Vec<(Vec<J>, Vec<J>)>, script: VecDeque<Result<Reply, String>> }
impl Model for Stub {
    fn step(&mut self, messages: &[J], tools: &[J]) -> Result<Reply, String> {
        self.seen.push((messages.to_vec(), tools.to_vec()));
        self.script.pop_front().unwrap_or(Ok(Reply { text: "ok".into(), tool_calls: vec![] }))
    }
}
fn calls(cs: Vec<(&str, J)>) -> Result<Reply, String> { Ok(Reply { text: String::new(), tool_calls: cs.into_iter().map(|(n, a)| ToolCall { name: n.into(), arguments: a }).collect() }) }
fn user(m: &(Vec<J>, Vec<J>)) -> String { m.0.iter().find(|x| x["role"] == "user").map(|x| x["content"].as_str().unwrap_or("").to_string()).unwrap_or_default() }

fn cfg() -> BrainConfig { BrainConfig { delta_cap: 50, max_new: 256, max_rounds: 4, ask_every_ms: 10, start: Start::Head } }

struct Rig { sim: Sim, peer: Peer, brain: Brain, model: Stub, clock: u64 }
impl Rig {
    fn new(tag: &str, doctrine: Option<&str>, cfg: BrainConfig) -> Rig {
        let (sim, peer) = Sim::new(tag, doctrine);
        Rig { sim, peer, brain: Brain::new(cfg, "membrama"), model: Stub::default(), clock: 1_000 }
    }
    fn drive(&mut self, rounds: usize) {
        for _ in 0..rounds {
            self.clock += 10;
            let ev = self.sim.pump(&mut self.peer);
            self.brain.tick(&mut self.peer, &mut self.model, &ev, self.clock).unwrap();
        }
    }
    fn own(&self, eid: &str, prop: &str) -> Option<J> { cell(&self.sim.seat.reader(), eid, prop) }
    fn error(&self) -> Option<String> { self.own(LOOP_ID, "error").and_then(|v| v.as_str().map(String::from)) }
}

#[test]
fn on_connect_the_loop_writes_the_log_viewer_and_the_guard_to_the_seat_as_packs() {
    let mut r = Rig::new("l1", Some("d"), cfg());
    r.drive(8);
    assert_eq!(r.sim.written(&format!("Pack:{VIEWER_PACK}"), "text"), vec![json!(viewer_pack("membrama"))]);
    assert_eq!(r.sim.written(&format!("Pack:{GUARD_PACK}"), "text"), vec![json!(guard_pack("membrama"))]);
    assert!(r.error().is_none(), "{:?}", r.error());
}

#[test]
fn the_viewer_is_a_tab_that_reads_the_logview_and_the_guard_removes_both_when_the_link_is_not_up() {
    let v = viewer_pack("membrama");
    for want in ["PACK membrama_logview", "IMPORT dom 1", "IMPORT shell_view 1", "('Nav:membrama_log').title", "DomNode:shell:slot:membrama_log", "('Nav:membrama_log').filled = True", LOGVIEW] {
        assert!(v.contains(want), "the viewer should contain {want:?}:\n{v}");
    }
    let g = guard_pack("membrama");
    for want in ["PACK membrama_logguard", "Vm:membrama", "!= 'up'", "('Pack:membrama_logview').text = None", "('Pack:membrama_logguard').text = None"] {
        assert!(g.contains(want), "the guard should contain {want:?}:\n{g}");
    }
    assert!(!g.contains("IMPORT dom"), "the guard runs on the seat, so it must not import the page's pack");
    // another namespace is another pack text
    assert!(viewer_pack("brain2").contains("membrane::LogView:brain2") && guard_pack("brain2").contains("Vm:brain2"));
}

#[test]
fn the_loop_subscribes_to_its_logview_and_its_doctrine() {
    let mut r = Rig::new("l2", Some("d"), cfg());
    r.drive(4);
    assert!(r.sim.subs.contains(&LOGVIEW.to_string()) && r.sim.subs.contains(&DOCTRINE_ID.to_string()), "{:?}", r.sim.subs);
}

#[test]
fn a_seat_with_no_doctrine_gets_the_first_draft_and_one_with_a_doctrine_keeps_the_operators_words() {
    let mut r = Rig::new("l3", None, cfg());
    r.drive(8);
    assert_eq!(r.sim.written(DOCTRINE_ID, "text"), vec![json!(DOCTRINE_DRAFT)]);
    let mut r = Rig::new("l4", Some("The operator's own wording."), cfg());
    r.drive(8);
    assert!(r.sim.written(DOCTRINE_ID, "text").is_empty(), "the operator's text must not be overwritten");
}

#[test]
fn the_draft_doctrine_is_the_operators_own_words() {
    assert_eq!(DOCTRINE_DRAFT.trim_end(), "Your state is stored in a dynamic programmable entity graph of a cliffhanger virtual machine. The machine is Turing-complete so the graph can sense the environment outside of itself and auto-update itself with sensed information. Use your tools to read, manipulate and program the graph state. Information about cliffhanger and VM internals can be found in \"Help\" entities. Every turn, vm sends the log of externally changed entities for you to either react yourself or use cliffhanger to program a behaviour onto the graph. Process the changes, if any, using the provided tools.");
}

#[test]
fn the_loop_asks_for_the_log_through_its_own_logask_and_moves_the_cursor_on() {
    let mut r = Rig::new("l5", Some("d"), BrainConfig { start: Start::Beginning, ..cfg() });
    r.sim.ext("Message:1", "text", "hello");
    r.sim.ext("Message:2", "text", "again");
    r.drive(10);
    let n1 = r.own("membrama::LogAsk:main", "n").unwrap().as_f64().unwrap();
    let from = r.own("membrama::LogAsk:main", "from").unwrap().as_u64().unwrap();
    assert!(from >= 2, "the cursor moves past the lines taken: {from}");
    r.drive(5);
    assert!(r.own("membrama::LogAsk:main", "n").unwrap().as_f64().unwrap() > n1, "each ask is a new n (the seat caches an answer by its arguments)");
}

#[test]
fn an_outside_change_wakes_a_step_told_doctrine_and_delta_with_pointers_not_values() {
    let mut r = Rig::new("l6", Some("Process these graph changes."), cfg());
    r.drive(6);
    assert!(r.model.seen.is_empty(), "nothing outside changed, no step");
    r.sim.ext("Message:7", "text", "SECRET-BODY");
    r.drive(8);
    assert_eq!(r.model.seen.len(), 1);
    let (messages, tools) = &r.model.seen[0];
    assert_eq!(messages[0], json!({"role": "system", "content": "Process these graph changes."}));
    let u = user(&r.model.seen[0]);
    assert!(u.contains("Message:7.text"), "{u}");
    assert!(u.contains("person:1"), "the delta says who wrote it: {u}");
    assert!(!u.contains("SECRET-BODY"), "pointers only, the model reads the value itself: {u}");
    let names: Vec<&str> = tools.iter().map(|t| t["function"]["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["write", "read", "alarm", "run_cliff"]);
}

#[test]
fn the_step_is_stateless_and_the_doctrine_is_read_from_the_seat_each_time() {
    let mut r = Rig::new("l7", Some("first wording"), cfg());
    r.drive(4);
    r.sim.ext("Message:1", "text", "a");
    r.drive(6);
    r.sim.set_node(DOCTRINE_ID, "text", json!("second wording"));
    r.sim.ext("Message:2", "text", "b");
    r.drive(6);
    assert_eq!(r.model.seen.len(), 2);
    let second = &r.model.seen[1];
    assert_eq!(second.0.len(), 2, "doctrine + delta, no history: {:?}", second.0);
    assert_eq!(second.0[0]["content"], "second wording");
    assert!(!user(second).contains("Message:1"), "the first step's delta is not repeated");
}

#[test]
fn history_before_the_loop_started_is_skipped_when_it_starts_at_the_head_and_read_when_it_starts_at_the_beginning() {
    let mut r = Rig::new("l8", Some("d"), cfg());
    r.sim.ext("Old:1", "text", "x"); r.sim.ext("Old:2", "text", "x");
    r.drive(8);
    assert!(r.model.seen.is_empty(), "history is not a change");
    r.sim.ext("New:1", "text", "x");
    r.drive(6);
    assert_eq!(r.model.seen.len(), 1);
    assert!(user(&r.model.seen[0]).contains("New:1") && !user(&r.model.seen[0]).contains("Old:"));
    let mut b = Rig::new("l9", Some("d"), BrainConfig { start: Start::Beginning, ..cfg() });
    b.sim.ext("Old:1", "text", "x");
    b.drive(8);
    assert!(user(&b.model.seen[0]).contains("Old:1"));
}

#[test]
fn the_models_own_write_does_not_wake_a_step_but_is_in_the_next_delta_marked_as_its_own() {
    let mut r = Rig::new("l10", Some("d"), cfg());
    r.drive(4);
    r.model.script.push_back(calls(vec![("write", json!({"id": "Note:1", "prop": "text", "value": "hi"}))]));
    r.sim.ext("Message:1", "text", "x");
    r.drive(10);
    assert_eq!(r.model.seen.len(), 2, "round one asked for the write, round two saw its result");
    assert_eq!(r.sim.written("Note:1", "text"), vec![json!("hi")]);
    assert!(r.model.seen[1].0.last().unwrap()["content"].as_str().unwrap().contains("ok"), "{:?}", r.model.seen[1].0.last());
    r.drive(12);
    assert_eq!(r.model.seen.len(), 2, "its own write in the log must not wake it");
    r.sim.ext("Message:2", "text", "y");
    r.drive(8);
    assert_eq!(r.model.seen.len(), 3);
    let u = user(&r.model.seen[2]);
    assert!(u.contains("Note:1.text") && u.contains("yours"), "its own write is in the next delta, marked: {u}");
    assert!(u.contains("Message:2.text"), "{u}");
}

#[test]
fn a_refused_write_is_told_to_the_model_and_shown_in_the_loops_error() {
    let mut r = Rig::new("l11", Some("d"), cfg());
    r.drive(4);
    r.sim.refuse = Some("Note is not writable by this peer".into());
    r.model.script.push_back(calls(vec![("write", json!({"id": "Note:1", "prop": "text", "value": "hi"}))]));
    r.sim.ext("Message:1", "text", "x");
    r.drive(10);
    let told = r.model.seen[1].0.last().unwrap()["content"].as_str().unwrap().to_string();
    assert!(told.contains("refused") && told.contains("Note is not writable by this peer"), "{told}");
    let e = r.error().expect("the error is visible in the graph");
    assert!(e.contains("Note is not writable by this peer"), "{e}");
}

#[test]
fn the_read_tool_gets_the_value_from_the_seat_and_the_model_continues_with_it() {
    let mut r = Rig::new("l12", Some("d"), cfg());
    r.drive(4);
    r.model.script.push_back(calls(vec![("read", json!({"id": "Message:7", "props": ["text"]}))]));
    r.sim.ext("Message:7", "text", "SECRET-BODY");
    r.drive(10);
    assert_eq!(r.model.seen.len(), 2);
    let told = r.model.seen[1].0.last().unwrap();
    assert_eq!(told["role"], "tool");
    assert!(told["content"].as_str().unwrap().contains("SECRET-BODY"), "{told}");
    // the assistant's own call is in the continued conversation, before the result
    assert!(r.model.seen[1].0.iter().any(|m| m["role"] == "assistant" && m["tool_calls"].is_array()));
}

#[test]
fn read_gets_the_props_from_the_cells_the_seat_publishes_and_a_missing_node_is_told() {
    let mut r = Rig::new("l13", Some("d"), cfg());
    r.drive(4);
    r.sim.nodes.entry("Room:a".into()).or_default().insert("t".into(), json!(1));
    r.model.script.push_back(calls(vec![("read", json!({"id": "Room:a", "props": ["t"]})), ("read", json!({"id": "Room:none", "props": ["t"]}))]));
    r.sim.ext("Message:1", "text", "x");
    r.drive(10);
    let told: Vec<String> = r.model.seen[1].0.iter().filter(|m| m["role"] == "tool").map(|m| m["content"].as_str().unwrap().to_string()).collect();
    assert!(told[0].contains("\"t\":1"), "{told:?}");
    assert!(told[1].contains("no such node"), "{told:?}");
}

#[test]
fn there_is_no_query_tool_because_the_seat_does_not_answer_it_over_the_link() {
    assert!(tool_specs().iter().all(|t| t["function"]["name"] != "query"));
}

#[test]
fn a_step_that_waits_says_what_for_on_the_loops_node_and_stops_saying_it_when_it_has_the_answer() {
    let mut r = Rig::new("l13b", Some("d"), cfg());
    r.drive(4);
    r.model.script.push_back(calls(vec![("run_cliff", json!({"code": "1"}))]));
    r.sim.ext("Message:1", "text", "x");
    r.drive(8);
    let w = r.own(LOOP_ID, "waiting").and_then(|v| v.as_str().map(String::from)).expect("a waiting step is visible");
    assert!(w.contains("run_cliff") && w.contains("answer"), "{w}");
    let node = cliff_node(&r);
    r.sim.set_node(&node, "result", json!(1));
    r.drive(10);
    assert_eq!(r.own(LOOP_ID, "waiting"), Some(J::Null));
}

#[test]
fn an_alarm_is_the_only_way_time_reaches_the_model_and_it_fires_once() {
    let mut r = Rig::new("l14", Some("d"), cfg());
    r.drive(4);
    r.model.script.push_back(calls(vec![("alarm", json!({"after_seconds": 5, "note": "check the boat"}))]));
    r.sim.ext("Message:1", "text", "x");
    r.drive(10);
    assert_eq!(r.model.seen.len(), 2, "the alarm tool answered inside the step");
    assert!(r.own("membrama::Alarm:1", "note").is_some(), "the alarm is a node the seat can read");
    r.clock += 4_000;
    r.drive(5);
    assert_eq!(r.model.seen.len(), 2, "not yet");
    r.clock += 2_000;
    r.drive(5);
    assert_eq!(r.model.seen.len(), 3, "the alarm woke a step with no log line behind it");
    assert!(user(&r.model.seen[2]).contains("check the boat"), "{}", user(&r.model.seen[2]));
    assert!(r.own("membrama::Alarm:1", "note").is_none(), "a fired alarm is retired");
    r.clock += 60_000;
    r.drive(8);
    assert_eq!(r.model.seen.len(), 3, "fired once");
}

#[test]
fn the_delta_cap_bounds_one_step_and_the_rest_is_the_next_step() {
    let mut r = Rig::new("l15", Some("d"), BrainConfig { delta_cap: 2, ..cfg() });
    r.drive(4);
    for i in 0..5 { r.sim.ext(&format!("Message:{i}"), "text", "x"); }
    r.drive(20);
    let counts: Vec<usize> = r.model.seen.iter().map(|m| user(m).matches("Message:").count()).collect();
    assert_eq!(counts, vec![2, 2, 1], "no line is lost and none is read twice");
}

#[test]
fn a_full_backlog_of_its_own_lines_does_not_stop_the_reading() {
    // seen live: the loop's own writes when it connects filled the delta cap; none of them wakes a step, so nothing drained them and the cursor never moved on
    let mut r = Rig::new("l15b", Some("d"), BrainConfig { delta_cap: 3, start: Start::Beginning, ..cfg() });
    for i in 0..6 { r.sim.log(&format!("Pack:p{i}"), "text", "link:membrama", 5); }
    r.drive(8);
    r.sim.ext("Message:late", "text", "x");
    r.drive(20);
    assert_eq!(r.model.seen.len(), 1, "the outside line woke a step although its own lines filled the cap before it");
    let u = user(&r.model.seen[0]);
    assert!(u.contains("Message:late"), "{u}");
    assert!(r.own(LOOP_ID, "cursor").unwrap().as_u64().unwrap() >= 7, "{:?}", r.own(LOOP_ID, "cursor"));
}

#[test]
fn the_rounds_of_one_step_are_bounded_and_the_error_says_so() {
    let mut r = Rig::new("l16", Some("d"), BrainConfig { max_rounds: 3, ..cfg() });
    r.drive(4);
    for _ in 0..10 { r.model.script.push_back(calls(vec![("read", json!({"id": "Message:1", "props": ["text"]}))])); }
    r.sim.ext("Message:1", "text", "x");
    r.drive(30);
    assert_eq!(r.model.seen.len(), 3);
    assert!(r.error().unwrap().contains("3 rounds"), "{:?}", r.error());
}

#[test]
fn a_failed_model_call_is_visible_and_not_retried_in_a_loop() {
    let mut r = Rig::new("l17", Some("d"), cfg());
    r.drive(4);
    r.model.script.push_back(Err("the model is not loaded".into()));
    r.sim.ext("Message:1", "text", "x");
    r.drive(20);
    assert_eq!(r.model.seen.len(), 1);
    assert!(r.error().unwrap().contains("the model is not loaded"), "{:?}", r.error());
    r.sim.ext("Message:2", "text", "y");
    r.drive(8);
    assert_eq!(r.model.seen.len(), 2, "the next outside change steps again");
}

#[test]
fn a_bad_tool_call_is_told_to_the_model_not_dropped() {
    let mut r = Rig::new("l18", Some("d"), cfg());
    r.drive(4);
    r.model.script.push_back(calls(vec![("write", json!({"id": "Note:1"})), ("no_such_tool", json!({}))]));
    r.sim.ext("Message:1", "text", "x");
    r.drive(10);
    let tail: Vec<String> = r.model.seen[1].0.iter().filter(|m| m["role"] == "tool").map(|m| m["content"].as_str().unwrap().to_string()).collect();
    assert_eq!(tail.len(), 2);
    assert!(tail[0].contains("prop") && tail[1].contains("no_such_tool"), "{tail:?}");
    assert!(r.sim.writes.iter().all(|w| w.0 != "Note:1"));
}

#[test]
fn with_no_doctrine_on_the_seat_the_step_waits_with_the_lines_kept_and_the_error_names_it() {
    let mut r = Rig::new("l19", None, cfg());
    r.sim.refuse = Some("no write rights".into());
    r.drive(6);
    r.sim.ext("Message:1", "text", "x");
    r.drive(10);
    assert!(r.model.seen.is_empty());
    assert!(r.error().unwrap().contains(DOCTRINE_ID), "{:?}", r.error());
    r.sim.refuse = None;
    r.sim.set_node(DOCTRINE_ID, "text", json!("now there is one"));
    r.drive(10);
    assert_eq!(r.model.seen.len(), 1);
    assert!(user(&r.model.seen[0]).contains("Message:1"), "the line waited for the doctrine");
    assert_eq!(r.model.seen[0].0[0]["content"], "now there is one");
}

#[test]
fn a_log_page_too_big_for_the_peer_is_an_error_in_plain_words() {
    let mut r = Rig::new("l20", Some("d"), BrainConfig { start: Start::Beginning, ..cfg() });
    r.drive(4);
    r.sim.rows_too_big = true;
    r.drive(6);
    let e = r.error().expect("visible");
    assert!(e.to_lowercase().contains("log"), "{e}");
    for word in ["slot", "ring", "cell", "revision", "table"] { assert!(!e.to_lowercase().contains(word), "mechanics must not leak ({word}): {e}"); }
}

// ---- the model behind the loop: the model's own chat template and tool-call format ----
mod template_model {
    use super::*;
    use membrama::turn::{Generator, Job, Usage};
    struct Gen { template: Option<String>, reply: String, jobs: Vec<Job> }
    impl Generator for Gen {
        fn chat_template(&self) -> Option<String> { self.template.clone() }
        fn generate(&mut self, job: &Job, on_text: &mut dyn FnMut(&str) -> bool) -> Result<Usage, String> { self.jobs.push(job.clone()); on_text(&self.reply); Ok(Usage::default()) }
    }
    fn msgs() -> Vec<J> { vec![json!({"role": "system", "content": "Process these graph changes."}), json!({"role": "user", "content": "3 Message:1.text set by person:1"})] }
    fn tools() -> Vec<J> { vec![json!({"type": "function", "function": {"name": "alarm", "description": "d", "parameters": {"type": "object"}}})] }

    #[test]
    fn the_step_is_rendered_with_the_models_template_run_with_the_owners_max_new_and_its_tool_calls_are_read() {
        let mut g = Gen { template: Some(include_str!("fixtures/qwen3.jinja").into()), reply: "<tool_call>{\"name\":\"alarm\",\"arguments\":{\"after_seconds\":3,\"note\":\"n\"}}</tool_call>".into(), jobs: vec![] };
        let reply = TemplateModel { generator: &mut g, max_new: 77 }.step(&msgs(), &tools()).unwrap();
        assert_eq!(reply.tool_calls, vec![ToolCall { name: "alarm".into(), arguments: json!({"after_seconds": 3, "note": "n"}) }]);
        let job = &g.jobs[0];
        assert_eq!(job.max_new, 77);
        let text = &job.parts[0].text;
        assert!(text.contains("Process these graph changes.") && text.contains("Message:1.text") && text.contains("\"alarm\""), "{text}");
    }

    #[test]
    fn a_model_with_no_chat_template_is_an_error_that_says_so() {
        let mut g = Gen { template: None, reply: String::new(), jobs: vec![] };
        let e = TemplateModel { generator: &mut g, max_new: 8 }.step(&msgs(), &tools()).unwrap_err();
        assert!(e.contains("template"), "{e}");
    }

    #[test]
    fn a_reply_that_is_only_words_has_no_tool_calls() {
        let mut g = Gen { template: Some(include_str!("fixtures/qwen3.jinja").into()), reply: "nothing to do".into(), jobs: vec![] };
        let reply = TemplateModel { generator: &mut g, max_new: 8 }.step(&msgs(), &tools()).unwrap();
        assert!(reply.tool_calls.is_empty());
        assert_eq!(reply.text, "nothing to do");
    }
}

// ---- run_cliff: the seat-side runner is babyseat's (a node `membrane::Cliff:<ns>:<id>`: the peer writes `code`, the runner answers `result` or `error`, and retires the node when the peer retires it)

fn cliff_node(r: &Rig) -> String { r.sim.writes.iter().find(|w| w.1 == "code").map(|w| w.0.clone()).expect("no code was written") }

#[test]
fn run_cliff_is_one_of_the_tools_and_takes_code() {
    let t = tool_specs();
    let f = t.iter().find(|x| x["function"]["name"] == "run_cliff").expect("run_cliff is a tool");
    assert!(f["function"]["parameters"]["required"].as_array().unwrap().iter().any(|v| v == "code"));
}

#[test]
fn run_cliff_writes_the_code_to_a_cliff_node_and_the_result_is_the_tool_result() {
    let mut r = Rig::new("c1", Some("d"), cfg());
    r.drive(4);
    r.model.script.push_back(calls(vec![("run_cliff", json!({"code": "1 + 1"}))]));
    r.sim.ext("Message:1", "text", "x");
    r.drive(6);
    let node = cliff_node(&r);
    assert!(node.starts_with("membrane::Cliff:membrama:"), "{node}");
    assert_eq!(r.sim.written(&node, "code"), vec![json!("1 + 1")]);
    assert_eq!(r.model.seen.len(), 1, "the step waits for the runner, across turns, with no timeout");
    r.drive(20);
    assert_eq!(r.model.seen.len(), 1);
    r.sim.set_node(&node, "result", json!(2));
    r.drive(10);
    assert_eq!(r.model.seen.len(), 2);
    let told = r.model.seen[1].0.last().unwrap();
    assert_eq!(told["role"], "tool");
    assert_eq!(told["content"], "2");
    assert!(r.sim.written(&node, "retire").contains(&json!(true)), "the peer retires the node when it has the result");
}

#[test]
fn run_cliff_tells_the_runners_error_text() {
    let mut r = Rig::new("c2", Some("d"), cfg());
    r.drive(4);
    r.model.script.push_back(calls(vec![("run_cliff", json!({"code": "boom("}))]));
    r.sim.ext("Message:1", "text", "x");
    r.drive(6);
    let node = cliff_node(&r);
    r.sim.set_node(&node, "error", json!("syntax error at 1"));
    r.drive(10);
    let told = r.model.seen[1].0.last().unwrap()["content"].as_str().unwrap().to_string();
    assert!(told.starts_with("error:") && told.contains("syntax error at 1"), "{told}");
    assert!(r.sim.written(&node, "retire").contains(&json!(true)));
}

#[test]
fn run_cliff_refused_write_and_missing_code_are_told_not_waited_for() {
    let mut r = Rig::new("c3", Some("d"), cfg());
    r.drive(4);
    r.model.script.push_back(calls(vec![("run_cliff", json!({}))]));
    r.sim.ext("Message:1", "text", "x");
    r.drive(8);
    assert!(r.model.seen[1].0.last().unwrap()["content"].as_str().unwrap().contains("code"));
    r.model.script.push_back(calls(vec![("run_cliff", json!({"code": "1"}))]));
    r.sim.refuse = Some("no run rights".into());
    r.sim.ext("Message:2", "text", "x");
    r.drive(10);
    let told = r.model.seen.last().unwrap().0.last().unwrap()["content"].as_str().unwrap().to_string();
    assert!(told.contains("refused") && told.contains("no run rights"), "{told}");
}

#[test]
fn two_cliff_calls_in_one_round_and_in_two_steps_use_different_nodes() {
    let mut r = Rig::new("c4", Some("d"), cfg());
    r.drive(4);
    r.model.script.push_back(calls(vec![("run_cliff", json!({"code": "1"})), ("run_cliff", json!({"code": "2"}))]));
    r.sim.ext("Message:1", "text", "x");
    r.drive(6);
    let nodes: Vec<String> = r.sim.writes.iter().filter(|w| w.1 == "code").map(|w| w.0.clone()).collect();
    assert_eq!(nodes.len(), 2);
    assert_ne!(nodes[0], nodes[1]);
    for (n, v) in nodes.iter().zip([1, 2]) { r.sim.set_node(n, "result", json!(v)); }
    r.drive(10);
    r.model.script.push_back(calls(vec![("run_cliff", json!({"code": "3"}))]));
    r.sim.ext("Message:2", "text", "x");
    r.drive(8);
    let all: Vec<String> = r.sim.writes.iter().filter(|w| w.1 == "code").map(|w| w.0.clone()).collect();
    assert_eq!(all.len(), 3);
    assert!(!nodes.contains(&all[2]), "a later step's call must not reuse a node whose result is already there");
}

#[test]
fn every_call_id_is_letters_and_digits_only_and_unique_across_steps_rounds_calls_and_a_restart() {
    // the seat's runner refuses an id with `:` `-` `_` or `.` (it becomes part of pack names), and never answers it
    let ok = |id: &str| !id.is_empty() && id.replace([':', '-', '_', '.'], "") == id && id.chars().all(|c| c.is_ascii_alphanumeric());
    let mut ids: Vec<String> = vec![];
    for (tag, run) in [("c5", 0), ("c6", 1)] {
        let mut r = Rig::new(tag, Some("d"), cfg());
        if run == 1 { std::thread::sleep(std::time::Duration::from_millis(5)); r.brain = Brain::new(cfg(), "membrama"); }
        r.drive(4);
        for k in 0..2 {
            r.model.script.push_back(calls(vec![("run_cliff", json!({"code": "1"})), ("run_cliff", json!({"code": "2"}))]));
            r.sim.ext(&format!("Message:{k}"), "text", "x");
            r.drive(6);
            let nodes: Vec<String> = r.sim.writes.iter().filter(|w| w.1 == "code").map(|w| w.0.clone()).collect();
            for n in &nodes { r.sim.set_node(n, "result", json!(1)); }
            r.drive(10);
        }
        for (_, m) in r.model.seen.iter().enumerate() { for msg in &m.0 { for c in msg["tool_calls"].as_array().into_iter().flatten() { ids.push(c["id"].as_str().unwrap().to_string()); } } }
        for w in r.sim.writes.iter().filter(|w| w.1 == "code") { ids.push(w.0.rsplit(':').next().unwrap().to_string()); }
    }
    assert!(!ids.is_empty());
    for id in &ids { assert!(ok(id), "{id}"); }
    let mut nodes: Vec<&String> = ids.iter().collect();
    nodes.sort(); nodes.dedup();
    // the two runs each made 4 distinct calls: the same step/round/call positions in a second run must not repeat an id
    assert!(nodes.len() >= 8, "{nodes:?}");
}

// ---- the trail of the last step: `Loop:main.trail`

fn trail(r: &Rig) -> Vec<J> { r.own(LOOP_ID, "trail").and_then(|v| v.as_array().cloned()).unwrap_or_default() }

#[test]
fn the_trail_has_each_rounds_visible_text_and_each_tool_call_with_whole_args_and_result() {
    let mut r = Rig::new("t1", Some("d"), cfg());
    r.drive(4);
    r.model.script.push_back(Ok(Reply { text: "<think>hidden</think>looking at it".into(), tool_calls: vec![ToolCall { name: "read".into(), arguments: json!({"id": "Message:7", "props": ["text"]}) }] }));
    r.model.script.push_back(Ok(Reply { text: "all done".into(), tool_calls: vec![] }));
    r.sim.ext("Message:7", "text", "THE-WHOLE-BODY");
    r.drive(10);
    let t = trail(&r);
    assert_eq!(t.len(), 3, "{t:?}");
    assert_eq!(t[0], json!({"step": 1, "round": 1, "text": "looking at it"}));
    assert_eq!(t[1]["tool"], "read");
    assert_eq!(t[1]["args"], json!({"id": "Message:7", "props": ["text"]}));
    assert!(t[1]["result"].as_str().unwrap().contains("THE-WHOLE-BODY"));
    assert_eq!(t[2], json!({"step": 1, "round": 2, "text": "all done"}));
}

#[test]
fn the_trail_is_the_last_step_only_and_a_call_not_run_at_the_round_limit_is_in_it_as_not_run() {
    let mut r = Rig::new("t2", Some("d"), BrainConfig { max_rounds: 1, ..cfg() });
    r.drive(4);
    r.sim.ext("Message:1", "text", "x");
    r.drive(6);
    assert_eq!(trail(&r).len(), 1, "round 1 said nothing and has no call");
    r.model.script.push_back(calls(vec![("alarm", json!({"after_seconds": 5, "note": "n"}))]));
    r.sim.ext("Message:2", "text", "x");
    r.drive(6);
    let t = trail(&r);
    assert_eq!(t.len(), 1, "{t:?}");
    assert_eq!(t[0]["step"], 2);
    assert!(t[0]["result"].as_str().unwrap().contains("not run"), "{t:?}");
}

#[test]
fn a_trail_that_does_not_fit_the_peers_cell_drops_the_oldest_entries_and_says_how_many() {
    let mut r = Rig::new("t3", Some("d"), BrainConfig { max_rounds: 40, ..cfg() });
    r.drive(4);
    let big = "y".repeat(1000);
    for _ in 0..8 { r.model.script.push_back(calls(vec![("write", json!({"id": "Note:1", "prop": "p", "value": big}))])); }
    r.sim.ext("Message:1", "text", "x");
    r.drive(60);
    let t = trail(&r);
    assert!(t[0]["dropped"].as_u64().unwrap() > 0, "{:?}", t.first());
    assert!(serde_json::to_string(&t).unwrap().len() <= r.peer.val_cap() as usize);
    assert!(t.last().unwrap()["text"] == "ok" && t.iter().any(|e| e["tool"] == "write"), "the newest entries are kept");
}

// ---- the banner, the tool rows and the skip rule

const BANNER_MAIN: &str = "membrane::Banner:admin:main";
fn banners(r: &mut Rig, main: &str, others: &[(&str, &str)]) {
    r.sim.set_node(BANNER_MAIN, "text", json!(main));
    for (eid, t) in others { r.sim.set_node(&format!("membrane::Banner:{eid}"), "text", json!(t)); }
}

#[test]
fn the_loop_subscribes_to_the_banners_by_prefix_and_not_to_anything_the_seat_owes() {
    let mut r = Rig::new("b0", Some("d"), cfg());
    r.drive(4);
    assert!(r.sim.prefix_subs.iter().any(|s| s == "membrane::Banner:"), "{:?}", r.sim.prefix_subs);
    assert!(r.sim.subs.iter().all(|s| !s.contains("Owed")), "{:?}", r.sim.subs);
}

#[test]
fn the_step_starts_with_the_main_banner_then_the_other_banners_in_banner_tags_then_the_changes() {
    let mut r = Rig::new("b1", Some("d"), cfg());
    banners(&mut r, "It is Thursday. You are Blurp.", &[("zeta", "second"), ("alpha", "first note"), ("empty", "  ")]);
    r.drive(6);
    r.sim.ext("Message:1", "text", "x");
    r.drive(8);
    let u = user(&r.model.seen[0]);
    assert!(u.starts_with("<banner>\nIt is Thursday. You are Blurp.\nfirst note\nsecond\n</banner>\n\nGraph changes"), "{u}");
}

#[test]
fn an_edit_of_the_banner_shows_in_the_next_step_and_no_banner_is_no_block() {
    let mut r = Rig::new("b2", Some("d"), cfg());
    r.drive(4);
    r.sim.ext("Message:1", "text", "x");
    r.drive(8);
    assert!(!user(&r.model.seen[0]).contains("<banner>"));
    banners(&mut r, "v1", &[]);
    r.drive(4);
    r.sim.ext("Message:2", "text", "x");
    r.drive(8);
    assert!(user(&r.model.seen[1]).starts_with("<banner>\nv1\n</banner>"));
    banners(&mut r, "v2, the model's own words", &[]);
    r.drive(4);
    r.sim.ext("Message:3", "text", "x");
    r.drive(8);
    assert!(user(&r.model.seen[2]).starts_with("<banner>\nv2, the model's own words\n</banner>"));
}

#[test]
fn a_step_wakes_on_a_chat_message_row_and_only_on_an_outside_row_or_an_alarm() {
    let mut r = Rig::new("b3", Some("d"), cfg());
    r.drive(10);
    assert!(r.model.seen.is_empty());
    banners(&mut r, "a banner change alone is not a wake", &[]);
    r.drive(10);
    assert!(r.model.seen.is_empty());
    r.sim.ext("membrane::Chat:main:message:1-1", "text", "hello");
    r.drive(8);
    assert_eq!(r.model.seen.len(), 1);
    assert!(user(&r.model.seen[0]).contains("Chat:main:message:1-1.text"));
}

// ---- the tool rows of a step, in the person's chat, as the calls are issued

const MSG: &str = "membrane::Chat:main:message:1-1";
fn tool_nodes(r: &Rig) -> Vec<String> { let mut v: Vec<String> = r.sim.writes.iter().filter(|w| w.1 == "name" && w.0.contains(":turn:tool:")).map(|w| w.0.clone()).collect(); v.sort(); v }

#[test]
fn a_call_is_a_tool_row_under_the_turn_of_each_chat_message_the_step_was_shown_and_its_result_fills_in() {
    let mut r = Rig::new("w1", Some("d"), cfg());
    r.drive(4);
    r.model.script.push_back(calls(vec![("run_cliff", json!({"code": "1 + 1"}))]));
    r.sim.ext("membrane::Chat:main:message:1-1", "text", "one");
    r.sim.ext("membrane::Chat:main:message:1-2", "text", "two");
    r.drive(8);
    let nodes = tool_nodes(&r);
    assert_eq!(nodes.len(), 2, "{nodes:?}");
    assert!(nodes[0].starts_with("membrane::Chat:main:message:1-1:turn:tool:") && nodes[1].starts_with("membrane::Chat:main:message:1-2:turn:tool:"), "{nodes:?}");
    let id = nodes[0].rsplit(':').next().unwrap().to_string();
    assert!(id.chars().all(|c| c.is_ascii_alphanumeric()), "{id}");
    for n in &nodes {
        assert_eq!(r.sim.written(n, "name"), vec![json!("run_cliff")]);
        assert_eq!(r.sim.written(n, "input"), vec![json!({"code": "1 + 1"})]);
        assert!(r.sim.written(n, "result").is_empty(), "the result is not there before the call has run");
    }
    for m in ["1-1", "1-2"] { assert_eq!(r.sim.written(&format!("membrane::Chat:main:message:{m}"), "tool_ids"), vec![json!(id.clone())], "the message names its tools in the order called"); }
    let cliff = cliff_node(&r);
    r.sim.set_node(&cliff, "result", json!(2));
    r.drive(10);
    for n in &nodes { assert_eq!(r.sim.written(n, "result"), vec![json!("2")]); }
}

#[test]
fn a_step_shown_no_chat_message_and_writing_to_no_response_writes_no_tool_rows() {
    let mut r = Rig::new("w2", Some("d"), cfg());
    r.drive(4);
    r.model.script.push_back(calls(vec![("alarm", json!({"after_seconds": 5, "note": "n"}))]));
    r.sim.ext("Message:1", "text", "x");
    r.drive(8);
    assert!(tool_nodes(&r).is_empty());
    assert!(r.sim.writes.iter().all(|w| w.1 != "tool_ids"));
}

#[test]
fn a_write_to_a_turns_response_puts_that_call_on_that_turn_and_the_later_calls_too() {
    let mut r = Rig::new("w3", Some("d"), cfg());
    r.drive(4);
    let resp = format!("{MSG}:turn:response");
    r.model.script.push_back(calls(vec![("write", json!({"id": resp, "prop": "text", "value": "hi"}))]));
    r.model.script.push_back(calls(vec![("alarm", json!({"after_seconds": 6, "note": "b"}))]));
    r.sim.ext("Message:1", "text", "x");
    r.drive(14);
    let ids = r.sim.written(MSG, "tool_ids");
    assert_eq!(ids.len(), 2, "{ids:?}");
    let (one, two) = (ids[0].as_str().unwrap(), ids[1].as_str().unwrap());
    assert!(two.starts_with(one) && two.split(' ').count() == 2, "{ids:?}");
    assert_eq!(tool_nodes(&r).len(), 2);
}

#[test]
fn a_tool_row_the_seat_refuses_is_an_error_in_plain_words() {
    let mut r = Rig::new("w4", Some("d"), cfg());
    r.drive(4);
    r.sim.refuse_for = Some(":turn:tool:".into());
    r.model.script.push_back(calls(vec![("alarm", json!({"after_seconds": 5, "note": "a"}))]));
    r.sim.ext("membrane::Chat:main:message:1-1", "text", "x");
    r.drive(10);
    let e = r.error().expect("shown");
    assert!(e.contains("tool row") && e.contains("no tool rows for you"), "{e}");
}

#[test]
fn a_second_write_to_the_same_node_and_property_in_one_step_is_skipped_with_no_write_and_no_row() {
    let mut r = Rig::new("k1", Some("d"), cfg());
    r.drive(4);
    let resp = format!("{MSG}:turn:response");
    r.model.script.push_back(calls(vec![("write", json!({"id": resp, "prop": "text", "value": "hi"}))]));
    r.model.script.push_back(calls(vec![("write", json!({"id": resp, "prop": "text", "value": "hi again, differently"})), ("write", json!({"id": "Note:9", "prop": "text", "value": "other"})), ("write", json!({"id": resp, "prop": "other", "value": "p"}))]));
    r.sim.ext("Message:1", "text", "x");
    r.drive(16);
    assert_eq!(r.sim.written(&resp, "text"), vec![json!("hi")], "the second write never reached the seat");
    assert_eq!(r.sim.written("Note:9", "text"), vec![json!("other")], "a different node still goes through");
    assert_eq!(r.sim.written(&resp, "other"), vec![json!("p")], "a different property too");
    let told: Vec<String> = r.model.seen[2].0.iter().filter(|m| m["role"] == "tool").map(|m| m["content"].as_str().unwrap().to_string()).collect();
    assert!(told.iter().any(|t| t == "skipped: this node and property were already written in this step"), "{told:?}");
    assert_eq!(tool_nodes(&r).len(), 3, "rows for the three writes that went out, none for the skipped one: {:?}", tool_nodes(&r));
}


// ---- the live view of the round being generated: `Loop:main.live`

/// A model that writes its round in pieces and looks at the loop's `live` cell after each one, as the seat would between two of its turns.
struct Chunked { pieces: Vec<String>, reader: Option<CellsReader>, name: String, partials: Vec<String> }
impl Model for Chunked {
    fn step(&mut self, _m: &[J], _t: &[J]) -> Result<Reply, String> { self.step_live(_m, _t, &mut |_| {}) }
    fn step_live(&mut self, _m: &[J], _t: &[J], on_text: &mut dyn FnMut(&str)) -> Result<Reply, String> {
        if self.reader.is_none() { self.reader = CellsReader::attach(format!("/gl-{}-pc", self.name)).ok(); }
        for p in self.pieces.clone() {
            on_text(&p);
            self.partials.push(cell(self.reader.as_ref().unwrap(), LOOP_ID, "live").and_then(|v| v.as_str().map(String::from)).unwrap_or_else(|| "<none>".into()));
        }
        Ok(Reply { text: self.pieces.concat(), tool_calls: vec![] })
    }
}

fn live_rig(tag: &str, pieces: Vec<String>) -> (Sim, Peer, Brain, Chunked) {
    let (mut sim, mut peer) = Sim::new(tag, Some("d"));
    let mut brain = Brain::new(cfg(), "membrama");
    let mut model = Chunked { pieces, reader: None, name: sim.seat.name.clone(), partials: vec![] };
    for i in 0..4 { let ev = sim.pump(&mut peer); brain.tick(&mut peer, &mut model, &ev, 1_000 + i * 10).unwrap(); }
    (sim, peer, brain, model)
}

#[test]
fn the_text_of_the_round_so_far_is_published_as_it_grows_and_cleared_when_the_round_ends() {
    let (mut sim, mut peer, mut brain, mut model) = live_rig("v1", vec!["<think>hm".into(), "m</think>Hel".into(), "lo".into()]);
    sim.ext("Message:1", "text", "x");
    for i in 0..6 { let ev = sim.pump(&mut peer); brain.tick(&mut peer, &mut model, &ev, 2_000 + i * 10).unwrap(); }
    assert_eq!(model.partials, vec!["<think>hm", "<think>hmm</think>Hel", "<think>hmm</think>Hello"], "thinking included, not stripped; it grows");
    assert_eq!(cell(&sim.seat.reader(), LOOP_ID, "live"), Some(json!("")), "cleared at the end of the round");
}

#[test]
fn the_live_text_never_exceeds_the_peers_cell_and_keeps_the_tail_after_a_marker() {
    let cap = membrama::peer::Numbers { slots: 0, key_cap: 0, val_cap: 4096, epoch: 0, spins: 0, depth: 0 }.val_cap as usize;
    let piece = "abcdefghij".repeat(150);
    let (mut sim, mut peer, mut brain, mut model) = live_rig("v2", vec![piece.clone(), piece.clone(), piece.clone(), piece.clone(), "THE-END".into()]);
    sim.ext("Message:1", "text", "x");
    for i in 0..6 { let ev = sim.pump(&mut peer); brain.tick(&mut peer, &mut model, &ev, 2_000 + i * 10).unwrap(); }
    assert!(model.partials.iter().all(|p| serde_json::to_string(p).unwrap().len() <= cap), "never above val_cap");
    let last = model.partials.last().unwrap();
    assert!(last.starts_with("[\u{2026}earlier text dropped]\n") && last.ends_with("THE-END"), "{}", &last[..40]);
}

