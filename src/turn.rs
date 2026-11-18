//! The turn (docs/design.md, "The turn, over GraphLink"): a request the seat wrote, run through a generator, answered with reply and chunk nodes.
//! The handler is sans-I/O like the peer: `tick` is called by whoever owns the thread, and runs a complete request to its end before it returns.
use crate::peer::Peer;
use graphlink_core::frame::Target;
use graphlink_core::link::Event;
use serde_json::{json, Value as J};
use std::collections::{BTreeMap, HashSet};

const REQUEST: &str = "membrane::LlmRequest:";
const PART: &str = "membrane::LlmPart:";
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
pub struct Job { pub id: String, pub server: String, pub parts: Vec<Part>, pub max_new: usize }

/// Which parts are served from banks: the last `max_banks` entity parts, in order (the seat puts the most relevant entity last, and recency was
/// the strongest effect when banks were glued together); text parts never, and any entity part beyond that is read as text.
pub fn bank_plan(parts: &[Part], max_banks: usize) -> Vec<usize> {
    let entities: Vec<usize> = parts.iter().enumerate().filter(|(_, p)| matches!(p.kind, PartKind::Entity { .. })).map(|(i, _)| i).collect();
    entities[entities.len().saturating_sub(max_banks)..].to_vec()
}

/// Whatever turns a job into text. `on_text` receives the text as it is produced and answers `false` to stop (an abort).
pub trait Generator {
    fn generate(&mut self, job: &Job, on_text: &mut dyn FnMut(&str) -> bool) -> Result<Usage, String>;
}

#[derive(Default)]
pub struct Handler {
    subscribed: bool,
    /// requests already answered or running: each is run once
    started: HashSet<String>,
}

/// What reading a request found.
enum Read { Wait, Aborted, Failed(String), Ready(Job) }

fn truthy(v: Option<&J>) -> bool { matches!(v, Some(J::Bool(true))) }

fn read_request(peer: &Peer, eid: &str, id: &str) -> Result<Read, String> {
    if truthy(peer.read(eid, "abort")?.as_ref()) { return Ok(Read::Aborted); }
    let (Some(server), Some(parts), Some(settings)) = (peer.read(eid, "server")?, peer.read(eid, "parts")?, peer.read(eid, "settings")?) else { return Ok(Read::Wait) };
    let Some(max_new) = settings.get("max_new").and_then(J::as_u64) else { return Ok(Read::Failed("settings.max_new is required: the module has no default for it".into())) };
    let mut out = Vec::new();
    for pid in parts.as_array().into_iter().flatten().filter_map(J::as_str) {
        let (Some(kind), Some(text)) = (peer.read(pid, "kind")?, peer.read(pid, "text")?) else { return Ok(Read::Wait) };
        let Some(text) = text.as_str().map(str::to_string) else { return Ok(Read::Failed(format!("{pid}: text is not a string"))) };
        let kind = match kind.as_str() {
            Some("text") => PartKind::Text,
            Some("entity") => {
                let (Some(e), Some(v)) = (peer.read(pid, "eid")?, peer.read(pid, "version")?) else { return Ok(Read::Wait) };
                PartKind::Entity { eid: e.as_str().unwrap_or_default().to_string(), version: v.as_str().unwrap_or_default().to_string() }
            }
            other => return Ok(Read::Failed(format!("{pid}: unknown kind {other:?}"))),
        };
        out.push(Part { kind, text });
    }
    Ok(Read::Ready(Job { id: id.to_string(), server: server.as_str().unwrap_or_default().to_string(), parts: out, max_new: max_new as usize }))
}

impl Handler {
    pub fn new() -> Handler { Handler::default() }

    /// Takes what the seat wrote and runs every request that is complete and not yet run; returns the peer's events.
    pub fn tick(&mut self, peer: &mut Peer, generator: &mut dyn Generator) -> Result<Vec<Event>, String> {
        let events = peer.poll()?;
        if !self.subscribed && peer.is_open() {
            peer.subscribe(Target::Prefix(REQUEST.into()), None)?;
            peer.subscribe(Target::Prefix(PART.into()), None)?;
            self.subscribed = true;
        }
        let ids: BTreeMap<String, String> = peer.scan(REQUEST)?.into_iter().map(|(eid, _, _)| { let id = eid[REQUEST.len()..].to_string(); (eid, id) }).collect();
        for (eid, id) in ids {
            if self.started.contains(&eid) { continue; }
            match read_request(peer, &eid, &id)? {
                Read::Wait => {}
                Read::Aborted => { self.started.insert(eid); finish(peer, &id, "aborted", 0, None, None)?; }
                Read::Failed(why) => { self.started.insert(eid); finish(peer, &id, "error", 0, None, Some(&why))?; }
                Read::Ready(job) => { self.started.insert(eid.clone()); run(peer, generator, &eid, &job)?; }
            }
        }
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
    let result = generator.generate(job, &mut |piece: &str| {
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
        (None, Ok(usage)) => finish(peer, &job.id, if aborted { "aborted" } else { "done" }, chunks, Some(&usage), None),
    }
}
