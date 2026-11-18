//! The turn handler over a real model: the seat's request is answered by the model, entity parts are banked or read as text. Needs MEMBRAMA_TEST_MODEL (a Qwen3 chat gguf);
//! without it the test says so and passes. One test owns the model and runs the scenarios in turn (see tests/common).
mod common;
use common::{cell, model_path, FakeSeat};
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::LlamaModel;
use membrama::engine::{EngineGenerator, Runner};
use membrama::turn::Handler;
use serde_json::json;

fn a_request_over_a_real_model_is_answered(backend: &LlamaBackend, model: &LlamaModel) {
    let mut gen = EngineGenerator::new(Runner::new(model, backend, 2048).expect("runner"), "test-model", 100_000_000, 1);

    let mut seat = FakeSeat::start("m1");
    let mut peer = seat.peer();
    seat.run(&mut peer);
    let texts = ["<|im_start|>user\nContext about the entity:", " Zorvex lives in Lima. Zorvex keeps a pet hamster.", "\nAnswer in a few words. Where does it live?<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n"];
    for (i, t) in texts.iter().enumerate() {
        let eid = format!("membrane::LlmPart:1:{i}");
        seat.put(&eid, "kind", json!("text")); seat.put(&eid, "text", json!(t));
    }
    seat.put("membrane::LlmRequest:1", "server", json!("m"));
    seat.put("membrane::LlmRequest:1", "parts", json!((0..3).map(|i| format!("membrane::LlmPart:1:{i}")).collect::<Vec<_>>()));
    seat.put("membrane::LlmRequest:1", "settings", json!({"max_new": 12}));
    let mut h = Handler::new();
    for _ in 0..5 { h.tick(&mut peer, &mut gen).unwrap(); }
    let r = seat.reader();
    assert_eq!(cell(&r, "membrama::Reply:1", "state"), Some(json!("done")));
    let answer = cell(&r, "membrama::Chunk:1:0", "text").expect("a chunk").as_str().unwrap().to_lowercase();
    assert!(answer.contains("lima"), "the model should answer from the parts: {answer:?}");
}

const PRE: &str = "<|im_start|>user\nContext about the entity:";
fn post(question: &str) -> String { format!("\nAnswer in a few words. {question}<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n") }

/// The seat writes a request of `[text PRE, entity parts..., text POST]`; the entity parts are `(eid, version, text)`.
fn ask(seat: &mut FakeSeat, id: &str, entities: &[(&str, &str, &str)], question: &str) {
    let mut ids = Vec::new();
    let mut put_part = |seat: &mut FakeSeat, n: usize, kind: &str, text: &str, ent: Option<(&str, &str)>| {
        let eid = format!("membrane::LlmPart:{id}:{n}");
        seat.put(&eid, "kind", json!(kind)); seat.put(&eid, "text", json!(text));
        if let Some((e, v)) = ent { seat.put(&eid, "entity", json!(e)); seat.put(&eid, "version", json!(v)); }
        ids.push(eid);
    };
    put_part(seat, 0, "text", PRE, None);
    for (i, (e, v, t)) in entities.iter().enumerate() { put_part(seat, i + 1, "entity", t, Some((e, v))); }
    put_part(seat, entities.len() + 1, "text", &post(question), None);
    let req = format!("membrane::LlmRequest:{id}");
    seat.put(&req, "server", json!("m")); seat.put(&req, "parts", json!(ids)); seat.put(&req, "settings", json!({"max_new": 12}));
}

fn answer(r: &graphlink_shm::cells::CellsReader, id: &str) -> String { cell(r, &format!("membrama::Chunk:{id}:0"), "text").map(|v| v.as_str().unwrap().to_lowercase()).unwrap_or_default() }
fn usage(r: &graphlink_shm::cells::CellsReader, id: &str, what: &str) -> u64 { cell(r, &format!("membrama::Reply:{id}"), "usage").unwrap()[what].as_u64().unwrap() }

fn an_entity_part_is_banked_once_and_served_from_the_bank_after(backend: &LlamaBackend, model: &LlamaModel) {
    let mut gen = EngineGenerator::new(Runner::new(model, backend, 2048).expect("runner"), "test-model", 100_000_000, 1);
    let mut seat = FakeSeat::start("m2");
    let mut peer = seat.peer();
    seat.run(&mut peer);
    let zorvex = ("Zorvex", "v1", " Zorvex lives in Lima. Zorvex keeps a pet hamster.");
    let mut h = Handler::new();
    ask(&mut seat, "1", &[zorvex], "Where does it live?");
    for _ in 0..5 { h.tick(&mut peer, &mut gen).unwrap(); }
    ask(&mut seat, "2", &[zorvex], "Where does it live?");
    for _ in 0..5 { h.tick(&mut peer, &mut gen).unwrap(); }
    let r = seat.reader();
    assert!(answer(&r, "1").contains("lima") && answer(&r, "2").contains("lima"), "{:?} {:?}", answer(&r, "1"), answer(&r, "2"));
    assert!(gen.has_bank("Zorvex", "v1", zorvex.2), "the entity is banked");
    assert!(usage(&r, "1", "loaded") > 0 && usage(&r, "2", "loaded") > 0, "both requests load the bank");
    assert!(usage(&r, "2", "read") < usage(&r, "1", "read"), "the second request does not read the entity's text again");
}

fn a_new_version_of_the_entity_is_banked_again_and_the_old_bank_is_dropped(backend: &LlamaBackend, model: &LlamaModel) {
    let mut gen = EngineGenerator::new(Runner::new(model, backend, 2048).expect("runner"), "test-model", 100_000_000, 1);
    let mut seat = FakeSeat::start("m3");
    let mut peer = seat.peer();
    seat.run(&mut peer);
    let mut h = Handler::new();
    ask(&mut seat, "1", &[("Zorvex", "v1", " Zorvex lives in Lima.")], "Where does it live?");
    for _ in 0..5 { h.tick(&mut peer, &mut gen).unwrap(); }
    ask(&mut seat, "2", &[("Zorvex", "v2", " Zorvex lives in Cusco.")], "Where does it live?");
    for _ in 0..5 { h.tick(&mut peer, &mut gen).unwrap(); }
    let r = seat.reader();
    assert!(answer(&r, "1").contains("lima"), "{:?}", answer(&r, "1"));
    assert!(answer(&r, "2").contains("cusco") && !answer(&r, "2").contains("lima"), "{:?}", answer(&r, "2"));
    assert!(!gen.has_bank("Zorvex", "v1", " Zorvex lives in Lima."), "the old version's bank is gone");
    assert_eq!(gen.bank_count(), 1);
}

fn a_request_with_more_entity_parts_than_the_limit_is_read_as_text_and_answers_right(backend: &LlamaBackend, model: &LlamaModel) {
    // three similar entities and a limit of one: a bank among the other entities' text answered about the wrong entity, so none is banked
    let mut gen = EngineGenerator::new(Runner::new(model, backend, 2048).expect("runner"), "test-model", 100_000_000, 1);
    let mut seat = FakeSeat::start("m4");
    let mut peer = seat.peer();
    seat.run(&mut peer);
    let (a, b, c) = ("Kelmar", "Tuvash", "Zorvex");
    let ta = " Kelmar lives in Tromso."; let tb = " Tuvash lives in Kyoto."; let tc = " Zorvex lives in Lima.";
    let mut h = Handler::new();
    for (id, q) in [("1", "Where does Zorvex live?"), ("2", "Where does Tuvash live?"), ("3", "Where does Kelmar live?")] {
        ask(&mut seat, id, &[(a, "v1", ta), (b, "v1", tb), (c, "v1", tc)], q);
        for _ in 0..5 { h.tick(&mut peer, &mut gen).unwrap(); }
    }
    assert_eq!(gen.bank_count(), 0, "over the limit nothing is banked");
    let r = seat.reader();
    assert!(answer(&r, "1").contains("lima"), "{:?}", answer(&r, "1"));
    assert!(answer(&r, "2").contains("kyoto"), "{:?}", answer(&r, "2"));
    assert!(answer(&r, "3").contains("tromso"), "{:?}", answer(&r, "3"));
}

#[test]
fn the_turn_over_a_real_model() {
    let Some(path) = model_path() else { return };
    let backend = LlamaBackend::init().expect("backend");
    let model = LlamaModel::load_from_file(&backend, &path, &LlamaModelParams::default().with_n_gpu_layers(99)).expect("model");
    for (name, scenario) in [
        ("a_request_over_a_real_model_is_answered", a_request_over_a_real_model_is_answered as fn(&LlamaBackend, &LlamaModel)),
        ("an_entity_part_is_banked_once_and_served_from_the_bank_after", an_entity_part_is_banked_once_and_served_from_the_bank_after as fn(&LlamaBackend, &LlamaModel)),
        ("a_new_version_of_the_entity_is_banked_again_and_the_old_bank_is_dropped", a_new_version_of_the_entity_is_banked_again_and_the_old_bank_is_dropped as fn(&LlamaBackend, &LlamaModel)),
        ("a_request_with_more_entity_parts_than_the_limit_is_read_as_text_and_answers_right", a_request_with_more_entity_parts_than_the_limit_is_read_as_text_and_answers_right as fn(&LlamaBackend, &LlamaModel)),
    ] {
        eprintln!("scenario: {name}");
        scenario(&backend, &model);
    }
}
