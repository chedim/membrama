//! The chat request (docs/design.md): the seat writes messages and tools as nodes; membrama renders them with the model's own template, runs the text,
//! and answers with the reply, the tool calls read out of it, and why it stopped. A fake generator (with a small template) stands in for the model.
mod common;
use common::{cell, FakeSeat};
use membrama::peer::Peer;
use membrama::turn::{Generator, Handler, Job, Usage};
use serde_json::{json, Value};

/// a tiny template in the Hermes style (it names `<tool_call>`, so the reply format is known)
const TMPL: &str = "{%- for m in messages %}<|{{ m.role }}|>{{ m.content }}{%- if m.tool_calls %}{% for c in m.tool_calls %}<tool_call>{{ c.function.name }}</tool_call>{% endfor %}{% endif %}\n{% endfor %}{%- if tools %}[tools:{% for t in tools %}{{ t.function.name }};{% endfor %}]{% endif %}{% if add_generation_prompt %}<|assistant|>{% endif %}";
/// a template that never mentions a tool call: its reply format is not one we know
const PLAIN: &str = "{%- for m in messages %}<|{{ m.role }}|>{{ m.content }}\n{% endfor %}{% if add_generation_prompt %}<|assistant|>{% endif %}";

#[derive(Default)]
struct Gen { template: Option<String>, reply: Vec<String>, seen: Vec<String> }
impl Generator for Gen {
    fn chat_template(&self) -> Option<String> { self.template.clone() }
    fn generate(&mut self, job: &Job, on_text: &mut dyn FnMut(&str) -> bool) -> Result<Usage, String> {
        self.seen.push(job.parts.iter().map(|p| p.text.as_str()).collect());
        for p in &self.reply { if !on_text(p) { break; } }
        Ok(Usage::default())
    }
}

fn started(tag: &str) -> (FakeSeat, Peer, Handler) {
    let mut seat = FakeSeat::start(tag); let mut peer = seat.peer(); seat.run(&mut peer);
    (seat, peer, Handler::new())
}
fn ticks(h: &mut Handler, peer: &mut Peer, g: &mut Gen) { for _ in 0..5 { h.tick(peer, g).unwrap(); } }

fn msg(seat: &mut FakeSeat, id: &str, n: usize, role: &str, content: Value) {
    let eid = format!("membrane::LlmMsg:{id}:{n}");
    seat.put(&eid, "role", json!(role)); seat.put(&eid, "content", content);
}
fn tool(seat: &mut FakeSeat, id: &str, n: usize, name: &str) {
    seat.put(&format!("membrane::LlmTool:{id}:{n}"), "spec", json!({"type": "function", "function": {"name": name, "description": "d", "parameters": {"type": "object", "properties": {}}}}));
}
fn chat(seat: &mut FakeSeat, id: &str, msgs: usize, tools: usize, settings: Value) {
    let req = format!("membrane::LlmRequest:{id}");
    seat.put(&req, "server", json!("m"));
    seat.put(&req, "messages", json!((0..msgs).map(|i| format!("membrane::LlmMsg:{id}:{i}")).collect::<Vec<_>>()));
    if tools > 0 { seat.put(&req, "tools", json!((0..tools).map(|i| format!("membrane::LlmTool:{id}:{i}")).collect::<Vec<_>>())); }
    seat.put(&req, "settings", settings);
}

#[test]
fn messages_are_rendered_with_the_models_template_and_run_as_one_text() {
    let (mut seat, mut peer, mut h) = started("c1");
    msg(&mut seat, "1", 0, "system", json!("S")); msg(&mut seat, "1", 1, "user", json!("hi"));
    chat(&mut seat, "1", 2, 0, json!({"max_new": 8}));
    let mut g = Gen { template: Some(TMPL.into()), reply: vec!["Hello".into()], ..Default::default() };
    ticks(&mut h, &mut peer, &mut g);
    assert_eq!(g.seen, vec!["<|system|>S\n<|user|>hi\n<|assistant|>".to_string()]);
    let r = seat.reader();
    assert_eq!(cell(&r, "membrama::Reply:1", "state"), Some(json!("done")));
    assert_eq!(cell(&r, "membrama::Chunk:1:0", "text"), Some(json!("Hello")));
    assert_eq!(cell(&r, "membrama::Reply:1", "finish"), Some(json!("stop")));
}

#[test]
fn tools_are_listed_for_the_template_and_a_tool_call_in_the_reply_is_read_out() {
    let (mut seat, mut peer, mut h) = started("c3");
    msg(&mut seat, "1", 0, "user", json!("weather in Lima?")); tool(&mut seat, "1", 0, "get_weather");
    chat(&mut seat, "1", 1, 1, json!({"max_new": 40}));
    let mut g = Gen { template: Some(TMPL.into()), reply: vec!["Checking.\n<tool_call>\n{\"name\": \"get_weather\", ".into(), "\"arguments\": {\"city\": \"Lima\"}}\n</tool_call>".into()], ..Default::default() };
    ticks(&mut h, &mut peer, &mut g);
    assert!(g.seen[0].contains("[tools:get_weather;]"), "{:?}", g.seen);
    let r = seat.reader();
    assert_eq!(cell(&r, "membrama::Reply:1", "state"), Some(json!("done")));
    assert_eq!(cell(&r, "membrama::Reply:1", "finish"), Some(json!("tool_calls")));
    assert_eq!(cell(&r, "membrama::Reply:1", "tool_calls"), Some(json!([{"name": "get_weather", "arguments": {"city": "Lima"}}])));
    assert!(cell(&r, "membrama::Chunk:1:0", "text").unwrap().as_str().unwrap().starts_with("Checking."), "the chunks hold the model's text as it wrote it");
}

#[test]
fn tools_for_a_model_whose_reply_format_is_not_known_are_refused() {
    let (mut seat, mut peer, mut h) = started("c4");
    msg(&mut seat, "1", 0, "user", json!("hi")); tool(&mut seat, "1", 0, "get_weather");
    chat(&mut seat, "1", 1, 1, json!({"max_new": 8}));
    let mut g = Gen { template: Some(PLAIN.into()), reply: vec!["x".into()], ..Default::default() };
    ticks(&mut h, &mut peer, &mut g);
    assert!(g.seen.is_empty(), "nothing is run");
    let r = seat.reader();
    assert_eq!(cell(&r, "membrama::Reply:1", "state"), Some(json!("error")));
    assert!(cell(&r, "membrama::Reply:1", "error").unwrap().as_str().unwrap().contains("tool"));
}

#[test]
fn a_chat_request_for_a_model_with_no_template_is_an_error() {
    let (mut seat, mut peer, mut h) = started("c5");
    msg(&mut seat, "1", 0, "user", json!("hi"));
    chat(&mut seat, "1", 1, 0, json!({"max_new": 8}));
    let mut g = Gen { template: None, reply: vec!["x".into()], ..Default::default() };
    ticks(&mut h, &mut peer, &mut g);
    assert!(g.seen.is_empty());
    let r = seat.reader();
    assert_eq!(cell(&r, "membrama::Reply:1", "state"), Some(json!("error")));
    assert!(cell(&r, "membrama::Reply:1", "error").unwrap().as_str().unwrap().contains("template"));
}

#[test]
fn a_chat_request_waits_for_a_missing_message_and_needs_max_new() {
    let (mut seat, mut peer, mut h) = started("c6");
    msg(&mut seat, "1", 0, "user", json!("hi"));
    chat(&mut seat, "1", 2, 0, json!({"max_new": 8}));   // two listed, one written
    let mut g = Gen { template: Some(TMPL.into()), reply: vec!["x".into()], ..Default::default() };
    ticks(&mut h, &mut peer, &mut g);
    assert!(g.seen.is_empty(), "waits for the second message");
    msg(&mut seat, "1", 1, "assistant", json!("yo"));
    ticks(&mut h, &mut peer, &mut g);
    assert_eq!(g.seen.len(), 1);
    // no max_new
    msg(&mut seat, "2", 0, "user", json!("hi")); chat(&mut seat, "2", 1, 0, json!({}));
    ticks(&mut h, &mut peer, &mut g);
    assert_eq!(cell(&seat.reader(), "membrama::Reply:2", "state"), Some(json!("error")));
}
