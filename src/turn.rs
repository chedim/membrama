//! The turn (docs/design.md, "The turn, over GraphLink"): a request the seat wrote, run through a generator, answered with reply and chunk nodes.
//! RED STUB: the types are real, `tick` only polls the peer.
use crate::peer::Peer;
use graphlink_core::link::Event;

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

/// Whatever turns a job into text. `on_text` receives the text as it is produced and answers `false` to stop (an abort).
pub trait Generator {
    fn generate(&mut self, job: &Job, on_text: &mut dyn FnMut(&str) -> bool) -> Result<Usage, String>;
}

#[derive(Default)]
pub struct Handler {}

impl Handler {
    pub fn new() -> Handler { Handler::default() }
    pub fn tick(&mut self, peer: &mut Peer, _generator: &mut dyn Generator) -> Result<Vec<Event>, String> { peer.poll() }
}
