//! A value the seat's table could not store is a `Refused` cell, and membrama must say so in its reply (an error that names the cell and the reason) instead of waiting for a value that will never come.
mod common;
use common::{cell, FakeSeat};
use membrama::peer::Peer;
use membrama::turn::{Generator, Handler, Job, Usage};
use serde_json::json;

const TMPL: &str = "{%- for m in messages %}<|{{ m.role }}|>{{ m.content }}\n{% endfor %}{% if add_generation_prompt %}<|assistant|>{% endif %}<tool_call></tool_call>";
const WHY: &str = "the value is 9000 bytes encoded, the capacity of a value is 4096";

#[derive(Default)]
struct Gen { runs: usize }
impl Generator for Gen {
    fn chat_template(&self) -> Option<String> { Some(TMPL.into()) }
    fn generate(&mut self, _job: &Job, on_text: &mut dyn FnMut(&str) -> bool) -> Result<Usage, String> { self.runs += 1; on_text("x"); Ok(Usage::default()) }
}
fn started(tag: &str) -> (FakeSeat, Peer, Handler) { let mut s = FakeSeat::start(tag); let mut p = s.peer(); s.run(&mut p); (s, p, Handler::new()) }
fn ticks(h: &mut Handler, p: &mut Peer, g: &mut Gen) { for _ in 0..5 { h.tick(p, g).unwrap(); } }
fn msg(seat: &mut FakeSeat, n: usize, role: &str, content: &str) { let e = format!("membrane::LlmMsg:1:{n}"); seat.put(&e, "role", json!(role)); seat.put(&e, "content", json!(content)); }
fn chat(seat: &mut FakeSeat, msgs: usize, tools: usize) {
    let r = "membrane::LlmRequest:1";
    seat.put(r, "server", json!("m")); seat.put(r, "messages", json!((0..msgs).map(|i| format!("membrane::LlmMsg:1:{i}")).collect::<Vec<_>>()));
    if tools > 0 { seat.put(r, "tools", json!((0..tools).map(|i| format!("membrane::LlmTool:1:{i}")).collect::<Vec<_>>())); }
    seat.put(r, "settings", json!({"max_new": 4}));
}
fn errored(seat: &FakeSeat, g: &Gen, names: &[&str]) {
    assert_eq!(g.runs, 0, "nothing is run");
    let r = seat.reader();
    assert_eq!(cell(&r, "membrama::Reply:1", "state"), Some(json!("error")));
    let e = cell(&r, "membrama::Reply:1", "error").unwrap().as_str().unwrap().to_string();
    for n in names { assert!(e.contains(n), "the error should name {n}: {e}"); }
}

#[test]
fn a_part_whose_text_the_seat_could_not_store_is_an_error_reply() {
    let (mut seat, mut peer, mut h) = started("x1");
    seat.put("membrane::LlmPart:1:0", "kind", json!("text")); seat.refuse("membrane::LlmPart:1:0", "text", WHY);
    let r = "membrane::LlmRequest:1";
    seat.put(r, "server", json!("m")); seat.put(r, "parts", json!(["membrane::LlmPart:1:0"])); seat.put(r, "settings", json!({"max_new": 4}));
    let mut g = Gen::default(); ticks(&mut h, &mut peer, &mut g);
    errored(&seat, &g, &["membrane::LlmPart:1:0", "text", "capacity of a value"]);
}

#[test]
fn a_message_whose_content_the_seat_could_not_store_is_an_error_reply() {
    let (mut seat, mut peer, mut h) = started("x2");
    msg(&mut seat, 0, "user", "hi"); seat.put("membrane::LlmMsg:1:1", "role", json!("user")); seat.refuse("membrane::LlmMsg:1:1", "content", WHY);
    chat(&mut seat, 2, 0);
    let mut g = Gen::default(); ticks(&mut h, &mut peer, &mut g);
    errored(&seat, &g, &["membrane::LlmMsg:1:1", "content", "capacity of a value"]);
}

#[test]
fn a_tool_spec_the_seat_could_not_store_is_an_error_reply() {
    let (mut seat, mut peer, mut h) = started("x3");
    msg(&mut seat, 0, "user", "hi"); seat.refuse("membrane::LlmTool:1:0", "spec", WHY);
    chat(&mut seat, 1, 1);
    let mut g = Gen::default(); ticks(&mut h, &mut peer, &mut g);
    errored(&seat, &g, &["membrane::LlmTool:1:0", "spec"]);
}

#[test]
fn tool_calls_the_seat_could_not_store_are_an_error_and_not_dropped_from_the_history() {
    let (mut seat, mut peer, mut h) = started("x4");
    msg(&mut seat, 0, "assistant", ""); seat.refuse("membrane::LlmMsg:1:0", "tool_calls", WHY);
    chat(&mut seat, 1, 0);
    let mut g = Gen::default(); ticks(&mut h, &mut peer, &mut g);
    errored(&seat, &g, &["membrane::LlmMsg:1:0", "tool_calls"]);
}

#[test]
fn a_list_of_messages_the_seat_could_not_store_is_an_error_reply() {
    let (mut seat, mut peer, mut h) = started("x5");
    let r = "membrane::LlmRequest:1";
    seat.put(r, "server", json!("m")); seat.refuse(r, "messages", WHY); seat.put(r, "settings", json!({"max_new": 4}));
    let mut g = Gen::default(); ticks(&mut h, &mut peer, &mut g);
    errored(&seat, &g, &["membrane::LlmRequest:1", "messages"]);
}

#[test]
fn the_content_of_a_message_is_a_string_and_an_array_is_an_error() {
    // an array of pieces is still one cell value, with the same cap: it never helped, so it is not accepted
    let (mut seat, mut peer, mut h) = started("x6");
    let e = "membrane::LlmMsg:1:0"; seat.put(e, "role", json!("user")); seat.put(e, "content", json!(["first half, ", "second half"]));
    chat(&mut seat, 1, 0);
    let mut g = Gen::default(); ticks(&mut h, &mut peer, &mut g);
    errored(&seat, &g, &["membrane::LlmMsg:1:0", "content", "string"]);
}
