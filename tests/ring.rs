//! Reusing a request's node (docs/design.md): a shared-memory table never reuses a slot, so the seat reuses a small ring of request ids. A request whose `n` changes is a
//! new request on the same node (the seat writes `n` last, as the signal), and a reply is reset at the start of each run and says which `n` it answers.
mod common;
use common::{cell, FakeSeat};
use membrama::peer::Peer;
use membrama::turn::{Generator, Handler, Job, Usage};
use serde_json::{json, Value};

const TMPL: &str = "{%- for m in messages %}<|{{ m.role }}|>{{ m.content }}\n{% endfor %}{% if add_generation_prompt %}<|assistant|>{% endif %}<tool_call></tool_call>";

#[derive(Default)]
struct Gen { replies: Vec<Vec<String>>, runs: usize }
impl Generator for Gen {
    fn chat_template(&self) -> Option<String> { Some(TMPL.into()) }
    fn generate(&mut self, _job: &Job, on_text: &mut dyn FnMut(&str) -> bool) -> Result<Usage, String> {
        let r = self.replies.get(self.runs).cloned().unwrap_or_default();
        self.runs += 1;
        for p in &r { if !on_text(p) { break; } }
        Ok(Usage::default())
    }
}
fn started(tag: &str) -> (FakeSeat, Peer, Handler) { let mut s = FakeSeat::start(tag); let mut p = s.peer(); s.run(&mut p); (s, p, Handler::new()) }
fn ticks(h: &mut Handler, p: &mut Peer, g: &mut Gen) { for _ in 0..5 { h.tick(p, g).unwrap(); } }

/// the seat writes one chat request on the ring node `r0`: its message, its settings, and `n` last
fn ask(seat: &mut FakeSeat, n: u64, text: &str, settings: Value) {
    seat.put("membrane::LlmMsg:r0:0", "role", json!("user")); seat.put("membrane::LlmMsg:r0:0", "content", json!(text));
    seat.put("membrane::LlmRequest:r0", "server", json!("m")); seat.put("membrane::LlmRequest:r0", "messages", json!(["membrane::LlmMsg:r0:0"]));
    seat.put("membrane::LlmRequest:r0", "settings", settings);
    seat.put("membrane::LlmRequest:r0", "n", json!(n));
}

#[test]
fn a_new_n_on_the_same_node_is_a_new_request_and_the_same_n_is_not_run_again() {
    let (mut seat, mut peer, mut h) = started("r1");
    let mut g = Gen { replies: vec![vec!["first".into()], vec!["second".into()]], ..Default::default() };
    ask(&mut seat, 1, "a", json!({"max_new": 8}));
    ticks(&mut h, &mut peer, &mut g); ticks(&mut h, &mut peer, &mut g);
    assert_eq!(g.runs, 1, "the same n runs once");
    assert_eq!(cell(&seat.reader(), "membrama::Chunk:r0:0", "text"), Some(json!("first")));
    ask(&mut seat, 2, "b", json!({"max_new": 8}));
    ticks(&mut h, &mut peer, &mut g);
    assert_eq!(g.runs, 2);
    assert_eq!(cell(&seat.reader(), "membrama::Chunk:r0:0", "text"), Some(json!("second")), "the chunk of the first use is overwritten");
}

#[test]
fn the_reply_says_which_n_it_answers() {
    let (mut seat, mut peer, mut h) = started("r2");
    let mut g = Gen { replies: vec![vec!["x".into()], vec!["y".into()]], ..Default::default() };
    ask(&mut seat, 7, "a", json!({"max_new": 8})); ticks(&mut h, &mut peer, &mut g);
    assert_eq!(cell(&seat.reader(), "membrama::Reply:r0", "n"), Some(json!(7)));
    ask(&mut seat, 8, "b", json!({"max_new": 8})); ticks(&mut h, &mut peer, &mut g);
    assert_eq!(cell(&seat.reader(), "membrama::Reply:r0", "n"), Some(json!(8)));
}

#[test]
fn what_the_last_use_left_behind_is_cleared_when_the_next_one_runs() {
    let (mut seat, mut peer, mut h) = started("r3");
    let call = "<tool_call>\n{\"name\": \"a\", \"arguments\": {}}\n</tool_call>";
    let mut g = Gen { replies: vec![vec![call.into()], vec!["plain answer".into()]], ..Default::default() };
    ask(&mut seat, 1, "a", json!({"max_new": 40})); ticks(&mut h, &mut peer, &mut g);
    let r = seat.reader();
    assert_eq!(cell(&r, "membrama::Reply:r0", "finish"), Some(json!("tool_calls")));
    assert!(cell(&r, "membrama::Reply:r0", "tool_calls").is_some());
    ask(&mut seat, 2, "b", json!({"max_new": 40})); ticks(&mut h, &mut peer, &mut g);
    let r = seat.reader();
    assert_eq!(cell(&r, "membrama::Reply:r0", "finish"), Some(json!("stop")));
    assert_eq!(cell(&r, "membrama::Reply:r0", "tool_calls"), Some(Value::Null), "the earlier tool calls are not the new answer's");
}

#[test]
fn a_failed_second_use_clears_the_first_ones_result_and_says_which_n_failed() {
    let (mut seat, mut peer, mut h) = started("r4");
    let mut g = Gen { replies: vec![vec!["fine".into()]], ..Default::default() };
    ask(&mut seat, 1, "a", json!({"max_new": 8})); ticks(&mut h, &mut peer, &mut g);
    ask(&mut seat, 2, "b", json!({}));   // no max_new
    ticks(&mut h, &mut peer, &mut g);
    let r = seat.reader();
    assert_eq!(cell(&r, "membrama::Reply:r0", "state"), Some(json!("error")));
    assert_eq!(cell(&r, "membrama::Reply:r0", "n"), Some(json!(2)));
    assert_eq!(cell(&r, "membrama::Reply:r0", "finish"), Some(Value::Null));
    assert_eq!(cell(&r, "membrama::Reply:r0", "usage"), Some(Value::Null));
}

#[test]
fn a_shorter_second_reply_resets_the_chunk_count() {
    let (mut seat, mut peer, mut h) = started("r5");
    let long = "é".repeat(2500);   // two chunks
    let mut g = Gen { replies: vec![vec![long], vec!["short".into()]], ..Default::default() };
    ask(&mut seat, 1, "a", json!({"max_new": 4000})); ticks(&mut h, &mut peer, &mut g);
    assert_eq!(cell(&seat.reader(), "membrama::Reply:r0", "chunks"), Some(json!(2)));
    ask(&mut seat, 2, "b", json!({"max_new": 8})); ticks(&mut h, &mut peer, &mut g);
    assert_eq!(cell(&seat.reader(), "membrama::Reply:r0", "chunks"), Some(json!(1)));
}
