//! The turn (docs/design.md, "The turn, over GraphLink"): a request the seat wrote, run through a generator, answered with reply and chunk nodes.
//! The handler is sans-I/O like the peer: `tick` is called by whoever owns the thread, and runs a complete request to its end before it returns.
use crate::peer::{Peer, Seen};
use crate::template::{render, Options};
use crate::toolcall::{detect_format, parse, Format};
use graphlink_core::frame::Target;
use graphlink_core::link::Event;
use serde_json::{json, Value as J};
use std::collections::{BTreeMap, HashMap, HashSet};

const REQUEST: &str = "membrane::LlmRequest:";
const PART: &str = "membrane::LlmPart:";
const MSG: &str = "membrane::LlmMsg:";
const TOOL: &str = "membrane::LlmTool:";
/// The header of a CBOR text string of 256 to 65535 bytes: a cell's value cap holds the whole item, so a chunk is `val_cap` minus this.
const CBOR_TEXT_HEADER: usize = 3;

#[derive(Clone, Debug, PartialEq)]
pub enum PartKind { Text, Entity { eid: String, version: String } }

/// One piece of the prompt, exactly as the seat rendered it.
#[derive(Clone, Debug, PartialEq)]
pub struct Part { pub kind: PartKind, pub text: String }

/// What a run cost: tokens read, tokens loaded from banks, tokens generated.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Usage { pub read: usize, pub loaded: usize, pub generated: usize }

/// A request that is complete: its parts have all arrived.
#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub id: String,
    pub server: String,
    pub parts: Vec<Part>,
    pub max_new: usize,
    /// Set for a chat request: the format the model writes its tool calls in, when it is known. The reply then says why it stopped and carries the calls.
    pub chat: Option<Option<Format>>,
}

/// Which parts are served from banks: every entity part when there are at most `max_banks` of them, else none. All or nothing, because a bank
/// among other entities' text, or beside another bank, answered about the wrong entity (docs/design.md); a request over the limit is read as text.
pub fn bank_plan(parts: &[Part], max_banks: usize) -> Vec<usize> {
    let entities: Vec<usize> = parts.iter().enumerate().filter(|(_, p)| matches!(p.kind, PartKind::Entity { .. })).map(|(i, _)| i).collect();
    if entities.len() > max_banks { Vec::new() } else { entities }
}

/// Whatever turns a job into text. `on_text` receives the text as it is produced and answers `false` to stop (an abort).
pub trait Generator {
    /// The model's own chat template (Jinja), when it has one: a chat request is rendered with it.
    fn chat_template(&self) -> Option<String> { None }
    fn generate(&mut self, job: &Job, on_text: &mut dyn FnMut(&str) -> bool) -> Result<Usage, String>;
}

#[derive(Default)]
pub struct Handler {
    subscribed: bool,
    /// requests already answered or running: each is run once
    started: HashSet<String>,
    /// requests that ran: request node -> its id; when the seat retires the request, the reply and its chunks are retired
    finished: HashMap<String, String>,
    reclaim_warned: bool,
}

/// What reading a request found.
enum Read { Wait, Aborted, Failed(String), Ready(Job) }

fn truthy(v: Option<&J>) -> bool { matches!(v, Some(J::Bool(true))) }

/// Why a request cannot be read: a value the seat's table could not store (a `Refused` cell) never comes, so it is an error that names the cell and the reason.
fn refusal(eid: &str, prop: &str, why: &str) -> String {
    format!("{eid} {prop}: the seat's table could not store it ({why}); the value size of the peer's tables must be above the largest message, tool or part")
}

/// A cell the request cannot do without: its value, `Wait` when it has not arrived, `Failed` when the seat could not store it.
macro_rules! need {
    ($peer:expr, $eid:expr, $prop:expr) => {
        match $peer.seen($eid, $prop)? { Seen::Live(v) => v, Seen::Absent => return Ok(Read::Wait), Seen::Refused(why) => return Ok(Read::Failed(refusal($eid, $prop, &why))) }
    };
}

/// A cell that may be left out: its value, or none; but one the seat could not store is an error (it must not be silently dropped from what the model is told).
macro_rules! maybe {
    ($peer:expr, $eid:expr, $prop:expr) => {
        match $peer.seen($eid, $prop)? { Seen::Live(v) => Some(v), Seen::Absent => None, Seen::Refused(why) => return Ok(Read::Failed(refusal($eid, $prop, &why))) }
    };
}

fn read_request(peer: &Peer, eid: &str, id: &str, generator: &dyn Generator) -> Result<Read, String> {
    if truthy(peer.read(eid, "abort")?.as_ref()) { return Ok(Read::Aborted); }
    if maybe!(peer, eid, "messages").is_some() { return read_chat(peer, eid, id, generator); }
    let server = need!(peer, eid, "server"); let parts = need!(peer, eid, "parts"); let settings = need!(peer, eid, "settings");
    let Some(max_new) = settings.get("max_new").and_then(J::as_u64) else { return Ok(Read::Failed("settings.max_new is required: the module has no default for it".into())) };
    let mut out = Vec::new();
    for pid in parts.as_array().into_iter().flatten().filter_map(J::as_str) {
        let kind = need!(peer, pid, "kind"); let text = need!(peer, pid, "text");
        let Some(text) = text.as_str().map(str::to_string) else { return Ok(Read::Failed(format!("{pid}: text is not a string"))) };
        let kind = match kind.as_str() {
            Some("text") => PartKind::Text,
            Some("entity") => {
                let e = need!(peer, pid, "entity"); let v = need!(peer, pid, "version");
                PartKind::Entity { eid: e.as_str().unwrap_or_default().to_string(), version: v.as_str().unwrap_or_default().to_string() }
            }
            other => return Ok(Read::Failed(format!("{pid}: unknown kind {other:?}"))),
        };
        out.push(Part { kind, text });
    }
    Ok(Read::Ready(Job { id: id.to_string(), server: server.as_str().unwrap_or_default().to_string(), parts: out, max_new: max_new as usize, chat: None }))
}

/// A chat request: `messages` (the ids of `membrane::LlmMsg:<id>:<n>` nodes: `role`, `content` (a string), and `tool_calls` on an assistant message that made calls) and
/// optionally `tools` (the ids of `membrane::LlmTool:<id>:<n>` nodes: `spec`). It is rendered with the model's own template into one text part.
fn read_chat(peer: &Peer, eid: &str, id: &str, generator: &dyn Generator) -> Result<Read, String> {
    let server = need!(peer, eid, "server"); let settings = need!(peer, eid, "settings"); let message_ids = need!(peer, eid, "messages");
    let Some(max_new) = settings.get("max_new").and_then(J::as_u64) else { return Ok(Read::Failed("settings.max_new is required: the module has no default for it".into())) };
    let mut messages = Vec::new();
    for mid in message_ids.as_array().into_iter().flatten().filter_map(J::as_str) {
        let role = need!(peer, mid, "role"); let content = need!(peer, mid, "content");
        let Some(text) = content.as_str() else { return Ok(Read::Failed(format!("{mid} content: it must be a string (a value is one cell, whatever it holds)"))) };
        let mut m = json!({"role": role, "content": text});
        if let Some(calls) = maybe!(peer, mid, "tool_calls") { m["tool_calls"] = calls; }
        messages.push(m);
    }
    let mut tools = Vec::new();
    if let Some(ids) = maybe!(peer, eid, "tools") {
        for tid in ids.as_array().into_iter().flatten().filter_map(J::as_str) { tools.push(need!(peer, tid, "spec")); }
    }
    let Some(template) = generator.chat_template() else { return Ok(Read::Failed("the model has no chat template, so a chat request cannot be rendered".into())) };
    let format = detect_format(&template);
    if !tools.is_empty() && format.is_none() { return Ok(Read::Failed("tools are not supported for this model: the format its replies use for tool calls is not known".into())) }
    let opts = Options { add_generation_prompt: true, enable_thinking: settings.get("enable_thinking").and_then(J::as_bool) };
    let text = match render(&template, &messages, &tools, &opts) { Ok(t) => t, Err(e) => return Ok(Read::Failed(e)) };
    Ok(Read::Ready(Job { id: id.to_string(), server: server.as_str().unwrap_or_default().to_string(), parts: vec![Part { kind: PartKind::Text, text }], max_new: max_new as usize, chat: Some(format) }))
}

impl Handler {
    pub fn new() -> Handler { Handler::default() }

    /// Takes what the seat wrote and runs every request that is complete and not yet run; returns the peer's events.
    pub fn tick(&mut self, peer: &mut Peer, generator: &mut dyn Generator) -> Result<Vec<Event>, String> {
        let events = peer.poll()?;
        let seen_removals = peer.before();   // the seat's removals counted before this turn's reading, acknowledged after it
        if !self.subscribed && peer.is_open() {
            peer.subscribe(Target::Prefix(REQUEST.into()), None)?;
            peer.subscribe(Target::Prefix(PART.into()), None)?;
            peer.subscribe(Target::Prefix(MSG.into()), None)?;
            peer.subscribe(Target::Prefix(TOOL.into()), None)?;
            self.subscribed = true;
        }
        let ids: BTreeMap<String, String> = peer.scan(REQUEST)?.into_iter().map(|(eid, _, _)| { let id = eid[REQUEST.len()..].to_string(); (eid, id) }).collect();
        // a request the seat has retired: its reply and chunks are retired too (their cells become Gone, and their slots can be reused once the seat has acknowledged)
        let retired: Vec<String> = self.finished.keys().filter(|e| !ids.contains_key(*e)).cloned().collect();
        for e in retired {
            if let Some(id) = self.finished.remove(&e) { peer.retire(&reply(&id))?; peer.retire_prefix(&format!("membrama::Chunk:{id}:"))?; }
            self.started.remove(&e);
        }
        for (eid, id) in ids {
            if self.started.contains(&eid) { continue; }
            match read_request(peer, &eid, &id, &*generator)? {
                Read::Wait => {}
                Read::Aborted => { self.started.insert(eid.clone()); self.finished.insert(eid, id.clone()); finish(peer, &id, "aborted", 0, None, None)?; }
                Read::Failed(why) => { self.started.insert(eid.clone()); self.finished.insert(eid, id.clone()); finish(peer, &id, "error", 0, None, Some(&why))?; }
                Read::Ready(job) => { self.started.insert(eid.clone()); self.finished.insert(eid.clone(), id.clone()); run(peer, generator, &eid, &job)?; }
            }
        }
        // say the seat's removals have been seen and take the slots the seat has acknowledged; a table with no slot left for the acknowledgement is said once and tried again next turn
        if let Err(e) = peer.after(seen_removals) { if !self.reclaim_warned { self.reclaim_warned = true; eprintln!("membrama: reclaiming slots: {e}"); } }
        Ok(events)
    }
}

fn reply(id: &str) -> String { format!("membrama::Reply:{id}") }
fn chunk(id: &str, n: usize) -> String { format!("membrama::Chunk:{id}:{n}") }

/// The last words of a request's reply: the state, how many chunks exist, what it cost, why it failed.
fn finish(peer: &mut Peer, id: &str, state: &str, chunks: usize, usage: Option<&Usage>, error: Option<&str>) -> Result<(), String> {
    let r = reply(id);
    peer.publish(&r, "chunks", &json!(chunks))?;
    if let Some(u) = usage { peer.publish(&r, "usage", &json!({"read": u.read, "loaded": u.loaded, "generated": u.generated}))?; }
    if let Some(e) = error { peer.publish(&r, "error", &json!(e))?; }
    peer.publish(&r, "state", &json!(state))
}

/// Runs one job: the reply says `running`, the text goes into chunk nodes as it is produced (the current chunk is rewritten as it grows
/// and a new one starts when it is full), and the reply ends `done`, `aborted` or `error`.
fn run(peer: &mut Peer, generator: &mut dyn Generator, request: &str, job: &Job) -> Result<(), String> {
    let r = reply(&job.id);
    peer.publish(&r, "chunks", &json!(0))?;
    peer.publish(&r, "state", &json!("running"))?;
    let cap = (peer.val_cap() as usize).saturating_sub(CBOR_TEXT_HEADER);
    let mut chunks = 0usize;       // chunk nodes started
    let mut current = String::new();
    let mut failed: Option<String> = None;
    let mut aborted = false;
    let mut full = String::new();   // everything the model wrote: a chat reply is parsed for tool calls at the end
    let result = generator.generate(job, &mut |piece: &str| {
        full.push_str(piece);
        let mut rest = piece;
        while !rest.is_empty() {
            // the longest start of `rest` that fits the room left in this chunk, cut on a character
            let room = cap - current.len();
            let take = rest.char_indices().map(|(i, c)| i + c.len_utf8()).take_while(|end| *end <= room).last().unwrap_or(0);
            if take == 0 && current.is_empty() { failed = Some(format!("a chunk of {cap} bytes cannot hold one character")); return false; }
            if take == 0 { chunks += 1; current.clear(); continue; }   // the chunk is full: the next text starts the next node
            if chunks == 0 { chunks = 1; }
            current.push_str(&rest[..take]);
            rest = &rest[take..];
            if let Err(e) = peer.publish(&chunk(&job.id, chunks - 1), "text", &json!(current)).and_then(|_| peer.publish(&r, "chunks", &json!(chunks))) { failed = Some(e); return false; }
        }
        match peer.read(request, "abort") { Ok(v) if truthy(v.as_ref()) => { aborted = true; false } Ok(_) => true, Err(e) => { failed = Some(e); false } }
    });
    match (failed, result) {
        (Some(e), _) | (None, Err(e)) => finish(peer, &job.id, "error", chunks, None, Some(&e)),
        (None, Ok(usage)) => {
            if let (Some(format), false) = (job.chat.flatten(), aborted) {
                match parse(&full, format) {
                    Ok(p) => {
                        let calls: Vec<J> = p.tool_calls.iter().map(|c| json!({"name": c.name, "arguments": c.arguments})).collect();
                        if !calls.is_empty() { peer.publish(&r, "tool_calls", &J::Array(calls))?; }
                        peer.publish(&r, "finish", &json!(if p.tool_calls.is_empty() { "stop" } else { "tool_calls" }))?;
                    }
                    Err(e) => return finish(peer, &job.id, "error", chunks, Some(&usage), Some(&e)),
                }
            } else if job.chat.is_some() && !aborted { peer.publish(&r, "finish", &json!("stop"))?; }
            finish(peer, &job.id, if aborted { "aborted" } else { "done" }, chunks, Some(&usage), None)
        }
    }
}
