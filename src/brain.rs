//! The loop (docs/design.md, "The loop"): membrama as an always-on co-processor on its own clock. It reads the seat's boundary log through the seat's LogView
//! (it asks by writing its own `membrama::LogAsk:main`, and reads `rows`: pointers, never values), builds its own step prompt (the doctrine, a node of the seat's
//! that the operator can edit, and the delta rows), calls its model, and acts with four tools over the link: write (`wr`), read (from the cells the seat publishes), alarm and run_cliff.
//! Sans-I/O like the rest of the peer: `tick` is called by whoever owns the thread. A step is a few model calls (a round each, bounded by the owner) and
//! nothing is kept from one step to the next: doctrine + delta is the whole contract.
use crate::peer::{Peer, Seen};
use crate::template::{render, Options};
use crate::toolcall::{detect_format, parse, visible, ToolCall};
use crate::turn::{Generator, Job, Part, PartKind};
use graphlink_core::frame::{Op, Target};
use graphlink_core::link::Event;
use serde_json::{json, Value as J};
use std::collections::HashMap;

/// Where the loop starts reading the log when it connects. The owner chooses (no default): at the head, what happened while membrama was away is not read;
/// at the beginning, the whole history is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Start { Head, Beginning }

/// The owner's numbers. None has a default (docs/design.md, "The loop").
#[derive(Clone, Debug, PartialEq)]
pub struct BrainConfig {
    /// The most log lines in one step's delta (and the most held waiting for a step).
    pub delta_cap: usize,
    /// The most tokens the model writes in one call.
    pub max_new: usize,
    /// The most model calls in one step (a call that asks for tools is followed by one that has their results).
    pub max_rounds: usize,
    /// How often the loop asks the seat for the log, in milliseconds (the seat caches an answer by its arguments, so a new ask is how a new line shows).
    pub ask_every_ms: u64,
    pub start: Start,
}

/// What the model said: its words and the tools it asked for.
#[derive(Clone, Debug, PartialEq)]
pub struct Reply { pub text: String, pub tool_calls: Vec<ToolCall> }

/// Whatever turns a step's messages (OpenAI shape) and tools into a reply: the real one is `TemplateModel`; a test uses a stub.
pub trait Model {
    fn step(&mut self, messages: &[J], tools: &[J]) -> Result<Reply, String>;
    /// The same call, handing each piece of text to `on_text` as the model writes it (the loop shows the round's text so far); a model that cannot stream hands nothing.
    fn step_live(&mut self, messages: &[J], tools: &[J], _on_text: &mut dyn FnMut(&str)) -> Result<Reply, String> { self.step(messages, tools) }
}

/// The seat's node holding the doctrine (its `text` is the words a step starts with). The operator edits it there; membrama writes the first draft only when the node does not exist.
pub const DOCTRINE_ID: &str = "membrane::Doctrine:membrama";
/// The first draft of the doctrine: domain-neutral, for the operator to reword.
pub const DOCTRINE_DRAFT: &str = include_str!("brain/doctrine.md");
/// The loop's own node in membrama's namespace: `cursor`, `steps`, `said` (the last words of a step) and `error` (what last went wrong, in plain words; null when the last step was clean).
pub const LOOP_ID: &str = "membrama::Loop:main";
const ASK_ID: &str = "membrama::LogAsk:main";
/// The seat's node of what it owes this peer: `items`, oldest first, `{message, response}` (the answer goes to the response node as a write of `text`, or `declined` with a reason).
fn owed_id(ns: &str) -> String { format!("membrane::Owed:{ns}") }
const ALARM: &str = "membrama::Alarm:";
/// The seat's node a clifflet is run on: `membrane::Cliff:<ns>:<call id>` (the peer writes `code`; the seat's runner writes `result` or `error`).
const CLIFF: &str = "membrane::Cliff:";
/// The names of the two packs written to the seat on connect.
pub const VIEWER_PACK: &str = "membrama_logview";
pub const GUARD_PACK: &str = "membrama_logguard";

fn fill(text: &str, ns: &str) -> String { text.replace("@NS@", ns).replace("@VIEWER@", VIEWER_PACK).replace("@GUARD@", GUARD_PACK) }
/// The log tab's pack for the namespace of this peer.
pub fn viewer_pack(ns: &str) -> String { fill(include_str!("brain/logview.c1f"), ns) }
/// The seat-side guard: removes both packs when `Vm:<ns>.link` is not up.
pub fn guard_pack(ns: &str) -> String { fill(include_str!("brain/logguard.c1f"), ns) }

/// The tools of a step, as the model is shown them.
pub fn tool_specs() -> Vec<J> {
    let f = |name: &str, description: &str, parameters: J| json!({"type": "function", "function": {"name": name, "description": description, "parameters": parameters}});
    vec![
        f("write", "Set one property of a node.", json!({"type": "object", "properties": {"id": {"type": "string"}, "prop": {"type": "string"}, "value": {}}, "required": ["id", "prop", "value"]})),
        f("read", "Read some properties of a node.", json!({"type": "object", "properties": {"id": {"type": "string"}, "props": {"type": "array", "items": {"type": "string"}}}, "required": ["id", "props"]})),
        f("alarm", "Wake yourself after a number of seconds, with a note. Time reaches you only this way.", json!({"type": "object", "properties": {"after_seconds": {"type": "number"}, "note": {"type": "string"}}, "required": ["after_seconds", "note"]})),
        f("run_cliff", "Run a clifflet (cliffhanger code) on the seat and get its value back, or the error. Use it to program a behaviour onto the graph.", json!({"type": "object", "properties": {"code": {"type": "string"}}, "required": ["code"]})),
    ]
}

/// One line of the boundary log: a pointer to what changed, never the value.
#[derive(Clone, Debug)]
struct Row { n: u64, eid: String, prop: String, op: String, writer: String, size: u64, own: bool }

impl Row {
    fn line(&self) -> String {
        format!("{} {}.{} {} by {}, {} bytes{}", self.n, self.eid, self.prop, self.op, self.writer, self.size, if self.own { " (yours)" } else { "" })
    }
}

struct Alarm { id: u64, at_ms: u64, note: String }

/// What an op waiting for the seat's answer is, so the answer can be told in the right words.
enum Inflight { Write { id: String, prop: String }, System(String), Cliff, Quiet }

/// A tool call of the current round: its result, or the id of the op that will bring it.
enum Pend { Done(String), Wait { id: String, what: String }, Read { id: String, props: Vec<String> }, Cliff { node: String, wid: String } }

/// `slots`: for each pending call, the place of its entry in the trail.
struct Step { messages: Vec<J>, round: usize, pending: Vec<Pend>, slots: Vec<usize>, failed: bool, owed_shown: Vec<String>, answered: Vec<String>, turns: Vec<(String, String)>, call_ids: Vec<String>, issued: Vec<String> }


pub struct Brain {
    cfg: BrainConfig,
    ns: String,
    writer: String,
    tools: Vec<J>,
    now: u64,
    subscribed: bool,
    seeded: bool,
    cursor: Option<u64>,
    asks: u64,
    next_ask: Option<u64>,
    backlog: Vec<Row>,
    alarms: Vec<Alarm>,
    alarm_seq: u64,
    fired: Vec<(u64, String)>,
    inflight: HashMap<String, Inflight>,
    answers: HashMap<String, String>,
    step: Option<Step>,
    steps: u64,
    error: Option<String>,
    published_cursor: Option<u64>,
    /// the wall clock at construction, in milliseconds, in hex: part of every call id so a restart (the step counter starts at 0 again) cannot repeat one
    run: String,
    waiting: Option<String>,
    /// the last step's trail: for each round the model's visible text, for each tool call `{step, round, tool, args, result}`
    /// what the seat says it owes now: `(message, response)`
    owed: Vec<(String, String, String)>,
    /// the banner text the seat hands over with what it owes
    banner: String,
    /// the messages of the last set seen, to tell a new item from one already there
    owed_known: Vec<String>,
    /// a new item came and no step has started since
    owed_wake: bool,
    trail: Vec<J>,
    published_trail: Option<String>,
}

/// The writer the seat records for a write this peer makes (`link:<namespace>`): the lines in the log with it are the loop's own.
pub fn own_writer(ns: &str) -> String { format!("link:{ns}") }

impl Brain {
    pub fn new(cfg: BrainConfig, ns: &str) -> Brain {
        Brain {
            cfg, ns: ns.into(), writer: own_writer(ns), tools: tool_specs(), now: 0, subscribed: false, seeded: false, cursor: None, asks: 0, next_ask: None,
            backlog: vec![], alarms: vec![], alarm_seq: 0, fired: vec![], inflight: HashMap::new(), answers: HashMap::new(), step: None, steps: 0, error: None, published_cursor: None, run: format!("{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)), waiting: None, owed: vec![], banner: String::new(), owed_known: vec![], owed_wake: false, trail: vec![], published_trail: None,
        }
    }

    /// One turn: take the answers that came, connect (once), read the log, fire alarms, and run or continue a step. `events` are what the peer's `poll` returned.
    pub fn tick(&mut self, peer: &mut Peer, model: &mut dyn Model, events: &[Event], now_ms: u64) -> Result<(), String> {
        self.now = now_ms;
        self.absorb(peer, events);
        if !peer.is_open() { return Ok(()); }
        self.connect(peer)?;
        self.read_owed(peer)?;
        self.read_log(peer)?;
        self.fire_alarms(peer)?;
        self.run(peer, model)
    }

    /// Records what the seat answered: the result of a write, in words for the model.
    fn absorb(&mut self, peer: &mut Peer, events: &[Event]) {
        for e in events {
            let Event::Op(op) = e else { continue };
            let (id, text) = match op {
                Op::Res { wid, refused } => (wid.clone(), match refused { None => "ok".to_string(), Some(why) => format!("refused: {why}") }),
                _ => continue,
            };
            match self.inflight.remove(&id) {
                Some(Inflight::System(what)) => { if let Some(why) = text.strip_prefix("refused: ") { self.fail(peer, &format!("the seat refused {what}: {why}")); } }
                Some(Inflight::Write { id: eid, prop }) => {
                    if let Some(why) = text.strip_prefix("refused: ") { self.fail(peer, &format!("write to {eid}.{prop} refused: {why}")); }
                    let mut text = text;
                    if !text.starts_with("refused: ") && (prop == "text" || prop == "declined") && self.owed.iter().any(|(_, r, _)| *r == eid) {
                        if let Some(s) = &mut self.step { s.answered.push(eid.clone()); }
                        text = "ok: written; this item is answered, do not write to it again".to_string();
                    }
                    self.answers.insert(id, text);
                }
                // the write of a cliff's code: an ok says nothing (the result comes on the node), a refusal ends the call
                Some(Inflight::Cliff) => { if text.starts_with("refused: ") { self.answers.insert(id, text); } }
                Some(Inflight::Quiet) => {}
                None => {}
            }
        }
    }

    /// Shows what went wrong, in plain words, on the loop's own node (and on stderr); the step in hand is marked as not clean.
    fn fail(&mut self, peer: &mut Peer, text: &str) {
        if let Some(s) = &mut self.step { s.failed = true; }
        if self.error.as_deref() == Some(text) { return; }
        eprintln!("membrama: {text}");
        self.error = Some(text.to_string());
        let _ = peer.publish(LOOP_ID, "error", &json!(text));
    }

    /// Once: subscribe to what the loop reads, write the two packs and, when the seat has no doctrine, the first draft.
    fn connect(&mut self, peer: &mut Peer) -> Result<(), String> {
        if !self.subscribed {
            peer.subscribe(Target::Id(format!("membrane::LogView:{}", self.ns)), None)?;
            peer.subscribe(Target::Id(DOCTRINE_ID.into()), None)?;
            peer.subscribe(Target::Id(owed_id(&self.ns)), None)?;
            for (pack, text, what) in [(GUARD_PACK, guard_pack(&self.ns), "the guard pack"), (VIEWER_PACK, viewer_pack(&self.ns), "the log tab pack")] {
                let wid = peer.write(&format!("Pack:{pack}"), "text", &json!(text))?;
                self.inflight.insert(wid, Inflight::System(what.into()));
            }
            peer.publish(LOOP_ID, "steps", &json!(0))?;
            self.subscribed = true;
        }
        if !self.seeded && peer.exists(DOCTRINE_ID)? == Some(false) {
            let wid = peer.write(DOCTRINE_ID, "text", &json!(DOCTRINE_DRAFT))?;
            self.inflight.insert(wid, Inflight::System("the first draft of the doctrine".into()));
            self.seeded = true;
        }
        Ok(())
    }

    /// Takes the lines of the log from the cursor on that the last answer holds, then asks again when it is time. A line is taken once: the cursor moves past it.
    fn read_log(&mut self, peer: &mut Peer) -> Result<(), String> {
        let view = format!("membrane::LogView:{}", self.ns);
        if self.cursor.is_none() && self.cfg.start == Start::Beginning { self.cursor = Some(0); }
        if self.cursor.is_none() {
            // at the head: the first answer tells where the log ends, and nothing before that is a change
            if let Seen::Live(head) = peer.seen(&view, "head")? { self.cursor = Some(head.as_u64().map(|h| h + 1).unwrap_or(0)); }
        } else {
            match peer.seen(&view, "rows")? {
                Seen::Live(rows) => {
                    let cursor = self.cursor.unwrap_or(0);
                    for r in rows.as_array().into_iter().flatten() {
                        if self.backlog.len() >= self.cfg.delta_cap {
                            // a full backlog is drained by a step, and only an outside line wakes one: held lines that are all its own are context, so the oldest makes room
                            if self.backlog.iter().any(|b| !b.own) { break; }
                            self.backlog.remove(0);
                        }
                        let Some(n) = r.get("n").and_then(J::as_u64) else { continue };
                        if n < cursor { continue; }
                        let text = |k: &str| r.get(k).and_then(J::as_str).unwrap_or("").to_string();
                        let writer = text("writer");
                        self.backlog.push(Row { n, eid: text("eid"), prop: text("prop"), op: text("op"), own: writer == self.writer, writer, size: r.get("size").and_then(J::as_u64).unwrap_or(0) });
                        self.cursor = Some(n + 1);
                    }
                }
                Seen::Refused(_) => self.fail(peer, "the log page could not be read: it is larger than this peer accepts"),
                Seen::Absent => {}
            }
        }
        if self.cursor != self.published_cursor { if let Some(c) = self.cursor { peer.publish(LOOP_ID, "cursor", &json!(c))?; self.published_cursor = Some(c); } }
        if self.next_ask.map_or(true, |t| self.now >= t) {
            self.asks += 1;
            peer.publish(ASK_ID, "from", &json!(self.cursor.unwrap_or(0)))?;
            peer.publish(ASK_ID, "n", &json!(self.asks))?;
            self.next_ask = Some(self.now + self.cfg.ask_every_ms);
        }
        Ok(())
    }

    /// What the seat owes: an item not in the last set seen is new and wakes a step; the set is kept so a step can be shown all of it.
    fn read_owed(&mut self, peer: &mut Peer) -> Result<(), String> {
        if let Seen::Live(J::String(b)) = peer.seen(&owed_id(&self.ns), "banner")? { self.banner = b; }
        let Seen::Live(items) = peer.seen(&owed_id(&self.ns), "items")? else { return Ok(()) };
        let now: Vec<(String, String, String)> = items.as_array().into_iter().flatten().filter_map(|i| Some((i.get("message")?.as_str()?.to_string(), i.get("response")?.as_str()?.to_string(), i.get("text").and_then(J::as_str).unwrap_or("").to_string()))).collect();
        if now.iter().any(|(m, _, _)| !self.owed_known.contains(m)) { self.owed_wake = true; }
        self.owed_known = now.iter().map(|(m, _, _)| m.clone()).collect();
        self.owed = now;
        Ok(())
    }

    fn fire_alarms(&mut self, peer: &mut Peer) -> Result<(), String> {
        let (due, later): (Vec<Alarm>, Vec<Alarm>) = std::mem::take(&mut self.alarms).into_iter().partition(|a| a.at_ms <= self.now);
        self.alarms = later;
        for a in due { peer.retire(&format!("{ALARM}{}", a.id))?; self.fired.push((a.id, a.note)); }
        Ok(())
    }

    /// A step starts on an outside change (a line that is not the loop's own) or an alarm, and only then.
    fn wakes(&self) -> bool { self.owed_wake || !self.fired.is_empty() || self.backlog.iter().any(|r| !r.own) }

    fn run(&mut self, peer: &mut Peer, model: &mut dyn Model) -> Result<(), String> {
        if self.step.is_none() {
            if !self.wakes() { return Ok(()); }
            let doctrine = match peer.seen(DOCTRINE_ID, "text")? {
                Seen::Live(J::String(t)) => t,
                Seen::Live(_) => { self.fail(peer, &format!("the doctrine ({DOCTRINE_ID}.text) is not text; a step waits for it")); return Ok(()); }
                Seen::Refused(_) => { self.fail(peer, &format!("the doctrine ({DOCTRINE_ID}.text) could not be read: it is larger than this peer accepts")); return Ok(()); }
                Seen::Absent => { self.fail(peer, &format!("there is no doctrine on the seat ({DOCTRINE_ID}.text); a step waits for it")); return Ok(()); }
            };
            let mut delta = String::new();
            if !self.banner.is_empty() { delta.push_str(&format!("<banner>\n{}\n</banner>\n\n", self.banner)); }
            if !self.backlog.is_empty() {
                delta.push_str("Graph changes since your last step (pointers only; read a node to see its value):\n");
                for r in self.backlog.drain(..) { delta.push_str(&r.line()); delta.push('\n'); }
            }
            let shown: Vec<String> = self.owed.iter().map(|(m, _, _)| m.clone()).collect();
            // the person's chat shows a step's calls under each turn it was shown: the turn is the response's eid without `:response`
            let turns: Vec<(String, String)> = self.owed.iter().map(|(m, r, _)| (m.clone(), r.strip_suffix(":response").unwrap_or(r).to_string())).collect();
            if !self.owed.is_empty() {
                delta.push_str("Owed (answer each with a write of text to its response node, or write declined with a reason there):\n");
                for (m, r, t) in &self.owed { delta.push_str(&format!("{m} -> {r}: {t}\n")); }
            }
            self.owed_wake = false;
            if !self.fired.is_empty() {
                delta.push_str("Alarms that went off:\n");
                for (id, note) in self.fired.drain(..) { delta.push_str(&format!("alarm {id}: {note}\n")); }
            }
            self.step = Some(Step { messages: vec![json!({"role": "system", "content": doctrine}), json!({"role": "user", "content": delta})], round: 0, pending: vec![], slots: vec![], failed: false, owed_shown: shown, answered: vec![], turns, call_ids: vec![], issued: vec![] });
            self.trail.clear();
            return self.round(peer, model);
        }
        self.settle(peer, model)
    }

    /// The current round's tool results, once every one has come: they go into the conversation and the model is called again.
    fn settle(&mut self, peer: &mut Peer, model: &mut dyn Model) -> Result<(), String> {
        let Some(step) = &mut self.step else { return Ok(()) };
        let mut retire: Vec<String> = vec![];
        let mut unsub: Vec<String> = vec![];
        for p in step.pending.iter_mut() {
            match p {
                Pend::Wait { id, .. } => { if let Some(t) = self.answers.remove(id.as_str()) { *p = Pend::Done(t); } }
                Pend::Read { id, props } => {
                    // the seat publishes what is subscribed: a node it says is gone is not there, a live one has its props in cells
                    let told = match peer.exists(id)? {
                        None => None,
                        Some(false) => Some("there is no such node".to_string()),
                        Some(true) => {
                            let mut m = serde_json::Map::new();
                            for k in props.iter() { if let Seen::Live(v) = peer.seen(id, k)? { m.insert(k.clone(), v); } }
                            Some(J::Object(m).to_string())
                        }
                    };
                    if let Some(t) = told { unsub.push(id.clone()); *p = Pend::Done(t); }
                }
                Pend::Cliff { node, wid } => {
                    let done = if let Some(t) = self.answers.remove(wid.as_str()) { Some((t, false)) } else {
                        match (peer.seen(node, "result")?, peer.seen(node, "error")?) {
                            (Seen::Live(v), _) => Some((match v { J::String(t) => t, other => other.to_string() }, true)),
                            (_, Seen::Live(J::Null)) | (_, Seen::Absent) => None,
                            (_, Seen::Live(e)) => Some((format!("error: {}", match e { J::String(t) => t, other => other.to_string() }), true)),
                            (_, Seen::Refused(why)) => Some((format!("error: the answer could not be read: {why}"), true)),
                        }
                    };
                    if let Some((t, answered)) = done { if answered { retire.push(node.clone()); } *p = Pend::Done(t); }
                }
                _ => {}
            }
        }
        for node in retire {
            // the runner retires the node when the peer retires it (a write of `retire`); this peer stops reading it
            let wid = peer.write(&node, "retire", &json!(true))?;
            self.inflight.insert(wid, Inflight::Quiet);
            peer.unsubscribe(Target::Id(node))?;
        }
        for node in unsub { peer.unsubscribe(Target::Id(node))?; }
        self.show_waiting(peer)?;
        self.record_results(peer)?;
        let Some(step) = &mut self.step else { return Ok(()) };
        if step.pending.iter().any(|p| matches!(p, Pend::Wait { .. } | Pend::Read { .. } | Pend::Cliff { .. })) { return Ok(()); }
        for p in std::mem::take(&mut step.pending) { if let Pend::Done(t) = p { step.messages.push(json!({"role": "tool", "content": t})); } }
        self.round(peer, model)
    }

    /// One call of the model; the tools it asks for are started (their results are waited for on later ticks), or the step ends.
    fn round(&mut self, peer: &mut Peer, model: &mut dyn Model) -> Result<(), String> {
        let Some(step) = &mut self.step else { return Ok(()) };
        step.round += 1;
        let round = step.round;
        let reply = match model.step(&step.messages, &self.tools) {
            Ok(r) => r,
            Err(e) => { self.fail(peer, &format!("the model call failed: {e}")); return self.finish(peer, None); }
        };
        let seen = visible(&reply.text);
        let no = self.steps + 1;
        if !seen.is_empty() { eprintln!("membrama: step {no} round {round} said {seen}"); self.trail.push(json!({"step": no, "round": round, "text": seen})); }
        if reply.tool_calls.is_empty() { self.show_trail(peer)?; return self.finish(peer, Some(reply.text)); }
        if round >= self.cfg.max_rounds {
            for c in &reply.tool_calls { self.trail_call(no, round, c, "not run: the step ended at the round limit"); }
            self.show_trail(peer)?;
            self.fail(peer, &format!("the step ended after {} rounds with the model still asking for tools; those calls were not run", self.cfg.max_rounds));
            return self.finish(peer, Some(reply.text));
        }
        let calls: Vec<J> = reply.tool_calls.iter().enumerate().map(|(i, c)| json!({"id": self.call_id(round, i), "type": "function", "function": {"name": c.name, "arguments": c.arguments}})).collect();
        if let Some(step) = &mut self.step { step.messages.push(json!({"role": "assistant", "content": reply.text, "tool_calls": calls})); }
        let mut pending = Vec::new();
        let mut slots = Vec::new();
        let mut call_ids = Vec::new();
        for (i, c) in reply.tool_calls.iter().enumerate() {
            slots.push(self.trail.len());
            call_ids.push(self.call_id(round, i));
            self.show_call(peer, &self.call_id(round, i), c)?;
            self.trail.push(json!({"step": no, "round": round, "tool": c.name, "args": c.arguments}));
            pending.push(self.call(peer, c, &self.call_id(round, i))?);
        }
        if let Some(step) = &mut self.step { step.pending = pending; step.slots = slots; step.call_ids.extend(call_ids); }
        self.record_results(peer)?;
        self.show_waiting(peer)
    }

    /// Writes one property of the tool row of call `id` under each turn the step was shown (nothing when it was shown none). The seat's answer to a refused write is in `error`.
    fn write_rows(&mut self, peer: &mut Peer, id: &str, prop: &str, value: J, what: &str) -> Result<(), String> {
        let turns: Vec<String> = self.step.as_ref().map(|s| s.turns.iter().map(|(_, t)| t.clone()).collect()).unwrap_or_default();
        for t in turns {
            let wid = peer.write(&format!("{t}:tool:{id}"), prop, &value)?;
            self.inflight.insert(wid, Inflight::System(format!("the tool row of a call under {t} ({what})")));
        }
        Ok(())
    }

    /// The call as the person's chat shows it, at the moment it is issued: a row `<turn>:tool:<call id>` with `name` and `input` under each turn the step was shown, and the messages' `tool_ids`
    /// (the ids in the order called, which is how the chat lists a message's tools, packs/chat.c1f `tool_keys`).
    fn show_call(&mut self, peer: &mut Peer, id: &str, c: &ToolCall) -> Result<(), String> {
        let Some(step) = &mut self.step else { return Ok(()) };
        if step.turns.is_empty() { return Ok(()); }
        step.issued.push(id.to_string());
        let ids = step.issued.join(" ");
        let messages: Vec<String> = step.turns.iter().map(|(m, _)| m.clone()).collect();
        self.write_rows(peer, id, "name", json!(c.name), "its name")?;
        self.write_rows(peer, id, "input", c.arguments.clone(), "its input")?;
        for m in messages {
            let wid = peer.write(&m, "tool_ids", &json!(ids))?;
            self.inflight.insert(wid, Inflight::System(format!("the list of the tools of {m}")));
        }
        Ok(())
    }

    /// A call that was not run: its entry in the trail, with the reason as its result.
    fn trail_call(&mut self, no: u64, round: usize, c: &ToolCall, result: &str) {
        eprintln!("membrama: step {no} round {round} {} {} -> {result}", c.name, c.arguments);
        self.trail.push(json!({"step": no, "round": round, "tool": c.name, "args": c.arguments, "result": result}));
    }

    /// Puts the result of each call that has one into its trail entry (and on stderr), once.
    fn record_results(&mut self, peer: &mut Peer) -> Result<(), String> {
        let Some(step) = &self.step else { return Ok(()) };
        let no = self.steps + 1;
        let mut rows: Vec<(String, String)> = vec![];
        let first = step.call_ids.len().saturating_sub(step.pending.len());
        for (k, (p, slot)) in step.pending.iter().zip(step.slots.iter()).enumerate() {
            let Pend::Done(t) = p else { continue };
            let Some(e) = self.trail.get_mut(*slot) else { continue };
            if e.get("result").is_some() { continue; }
            if let Some(id) = step.call_ids.get(first + k) { rows.push((id.clone(), t.clone())); }
            eprintln!("membrama: step {no} round {} {} {} -> {t}", e["round"], e["tool"].as_str().unwrap_or(""), e["args"]);
            e["result"] = json!(t);
        }
        for (id, t) in rows { self.write_rows(peer, &id, "result", json!(t), "its result")?; }
        self.show_trail(peer)
    }

    /// Publishes the trail when it changed. A trail the peer's cells cannot hold gives up its oldest entries, and says how many (`{dropped: n}`).
    fn show_trail(&mut self, peer: &mut Peer) -> Result<(), String> {
        let cap = peer.val_cap() as usize;
        let fits = |v: &[J]| serde_json::to_string(v).map_or(false, |t| t.len() <= cap);
        let mut shown = self.trail.clone();
        for k in 1..=self.trail.len() {
            if fits(&shown) { break; }
            shown = std::iter::once(json!({"dropped": k})).chain(self.trail[k..].iter().cloned()).collect();
        }
        let text = serde_json::to_string(&shown).unwrap_or_default();
        if self.published_trail.as_deref() != Some(text.as_str()) { peer.publish(LOOP_ID, "trail", &J::Array(shown))?; self.published_trail = Some(text); }
        Ok(())
    }

    /// A tool call's id: letters and digits only (the seat's runner makes it part of pack names and refuses anything else), and different for each run of the process, step, round and call.
    fn call_id(&self, round: usize, i: usize) -> String { format!("c{}s{}r{round}n{i}", self.run, self.steps) }

    /// Says on the loop's node what the step in hand is waiting for (null when it waits for nothing): a wait has no timeout, so it must be seen.
    fn show_waiting(&mut self, peer: &mut Peer) -> Result<(), String> {
        let now: Option<String> = self.step.as_ref().and_then(|s| {
            let w: Vec<String> = s.pending.iter().filter_map(|p| match p {
                Pend::Wait { what, .. } => Some(what.clone()),
                Pend::Read { id, .. } => Some(format!("read of {id} (the seat's cells for it)")),
                Pend::Cliff { node, .. } => Some(format!("run_cliff on {node} (`result` or `error` from the seat's runner)")),
                Pend::Done(_) => None,
            }).collect();
            if w.is_empty() { None } else { Some(format!("waiting for the answer to: {}", w.join("; "))) }
        });
        if now != self.waiting { peer.publish(LOOP_ID, "waiting", &now.as_ref().map_or(J::Null, |t| json!(t)))?; self.waiting = now; }
        Ok(())
    }

    fn finish(&mut self, peer: &mut Peer, said: Option<String>) -> Result<(), String> {
        // an item this step was shown that is still owed, and that the step did not answer or decline, is never dropped silently
        let open: Vec<String> = match &self.step {
            Some(s) => self.owed.iter().filter(|(m, r, _)| s.owed_shown.contains(m) && !s.answered.contains(r)).map(|(m, _, _)| m.clone()).collect(),
            None => vec![],
        };
        if !open.is_empty() { self.fail(peer, &format!("the step ended with {} owed items still open: {}", open.len(), open.join(", "))); }
        let failed = self.step.take().map(|s| s.failed).unwrap_or(false);
        self.show_waiting(peer)?;
        self.steps += 1;
        peer.publish(LOOP_ID, "steps", &json!(self.steps))?;
        if let Some(t) = said { peer.publish(LOOP_ID, "said", &json!(t))?; }
        if !failed && self.error.is_some() { self.error = None; peer.publish(LOOP_ID, "error", &J::Null)?; }
        Ok(())
    }

    /// Starts one tool call. A call that cannot be made says why, in words the model reads as the result.
    fn call(&mut self, peer: &mut Peer, c: &ToolCall, call_id: &str) -> Result<Pend, String> {
        let a = &c.arguments;
        let text = |k: &str| a.get(k).and_then(J::as_str).map(String::from);
        let missing = |k: &str| Ok(Pend::Done(format!("error: {} needs `{k}`", c.name)));
        match c.name.as_str() {
            "write" => {
                let Some(id) = text("id") else { return missing("id") };
                let Some(prop) = text("prop") else { return missing("prop") };
                let Some(value) = a.get("value") else { return missing("value") };
                let wid = peer.write(&id, &prop, value)?;
                let what = format!("write of {id}.{prop}");
                self.inflight.insert(wid.clone(), Inflight::Write { id, prop });
                Ok(Pend::Wait { id: wid, what })
            }
            "read" => {
                let Some(id) = text("id") else { return missing("id") };
                let Some(props) = a.get("props").and_then(J::as_array) else { return missing("props") };
                let props: Vec<String> = props.iter().filter_map(J::as_str).map(String::from).collect();
                peer.subscribe(Target::Id(id.clone()), Some(props.clone()))?;
                Ok(Pend::Read { id, props })
            }
            "alarm" => {
                let Some(secs) = a.get("after_seconds").and_then(J::as_f64).filter(|s| *s >= 0.0) else { return missing("after_seconds") };
                let Some(note) = text("note") else { return missing("note") };
                self.alarm_seq += 1;
                let id = self.alarm_seq;
                let at = self.now + (secs * 1000.0) as u64;
                let eid = format!("{ALARM}{id}");
                peer.publish(&eid, "at", &json!(at))?;
                peer.publish(&eid, "note", &json!(note))?;
                self.alarms.push(Alarm { id, at_ms: at, note });
                Ok(Pend::Done(format!("alarm {id} set for {secs} seconds from now")))
            }
            "run_cliff" => {
                let Some(code) = text("code") else { return missing("code") };
                // the seat's runner (babyseat) answers on this node: `result`, or `error` in words; the call's own id makes the node
                let node = format!("{CLIFF}{}:{call_id}", self.ns);
                peer.subscribe(Target::Id(node.clone()), None)?;
                let wid = peer.write(&node, "code", &json!(code))?;
                self.inflight.insert(wid.clone(), Inflight::Cliff);
                Ok(Pend::Cliff { node, wid })
            }
            other => Ok(Pend::Done(format!("error: there is no tool named `{other}`; the tools are write, read, alarm and run_cliff"))),
        }
    }
}

/// The real model of the loop: a `Generator` with the model's own chat template. The step is rendered with it, run with the owner's `max_new`, and the tool calls are read out of the reply.
pub struct TemplateModel<'a> { pub generator: &'a mut dyn Generator, pub max_new: usize }

impl Model for TemplateModel<'_> {
    fn step(&mut self, messages: &[J], tools: &[J]) -> Result<Reply, String> {
        let template = self.generator.chat_template().ok_or("the model has no chat template, so a step cannot be rendered")?;
        let format = detect_format(&template);
        if !tools.is_empty() && format.is_none() { return Err("tools are not supported for this model: the format its replies use for tool calls is not known".into()); }
        let text = render(&template, messages, tools, &Options { add_generation_prompt: true, enable_thinking: None })?;
        let job = Job { id: "loop".into(), server: String::new(), parts: vec![Part { kind: PartKind::Text, text }], max_new: self.max_new, chat: Some(format) };
        let mut said = String::new();
        self.generator.generate(&job, &mut |piece: &str| { said.push_str(piece); true })?;
        match format {
            Some(f) => { let p = parse(&said, f)?; Ok(Reply { text: p.content, tool_calls: p.tool_calls }) }
            None => Ok(Reply { text: said.trim().to_string(), tool_calls: vec![] }),
        }
    }
}
