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

/// A second request that starts with the same parts as the first reads only what differs, and answers exactly like a request on a fresh generator.
fn a_request_with_the_same_start_reads_only_what_differs(backend: &LlamaBackend, model: &LlamaModel) {
    let long = format!("<|im_start|>system\n{}<|im_end|>\n<|im_start|>user\n", "You answer in a few words. ".repeat(40));
    let run = |gen: &mut EngineGenerator, seat: &mut FakeSeat, peer: &mut membrama::peer::Peer, h: &mut Handler, id: &str, q: &str| {
        let eid = format!("membrane::LlmPart:{id}:0"); seat.put(&eid, "kind", json!("text")); seat.put(&eid, "text", json!(long));
        let eid2 = format!("membrane::LlmPart:{id}:1"); seat.put(&eid2, "kind", json!("text")); seat.put(&eid2, "text", json!(post(q)));
        let req = format!("membrane::LlmRequest:{id}");
        seat.put(&req, "server", json!("m")); seat.put(&req, "parts", json!([eid, eid2])); seat.put(&req, "settings", json!({"max_new": 8}));
        for _ in 0..5 { h.tick(peer, gen).unwrap(); }
    };
    let mut gen = EngineGenerator::new(Runner::new(model, backend, 2048).expect("runner"), "test-model", 100_000_000, 1);
    let mut seat = FakeSeat::start("m5"); let mut peer = seat.peer(); seat.run(&mut peer); let mut h = Handler::new();
    run(&mut gen, &mut seat, &mut peer, &mut h, "1", "What is 2 plus 2?");
    run(&mut gen, &mut seat, &mut peer, &mut h, "2", "What is the capital of France?");
    let r = seat.reader();
    assert!(usage(&r, "2", "read") * 3 < usage(&r, "1", "read"), "the second request reads only the question ({} against {})", usage(&r, "2", "read"), usage(&r, "1", "read"));
    // the same second request on a generator that has never seen the first
    let mut fresh = EngineGenerator::new(Runner::new(model, backend, 2048).expect("runner"), "test-model", 100_000_000, 1);
    let mut seat2 = FakeSeat::start("m6"); let mut peer2 = seat2.peer(); seat2.run(&mut peer2); let mut h2 = Handler::new();
    run(&mut fresh, &mut seat2, &mut peer2, &mut h2, "2", "What is the capital of France?");
    assert_eq!(answer(&r, "2"), answer(&seat2.reader(), "2"), "reusing the start must not change the answer");
    assert!(answer(&r, "2").contains("paris"), "{:?}", answer(&r, "2"));
}

/// The whole tool loop over the real model: a chat request with a tool, the model asks for it, the result goes back, the model answers from it.
fn the_model_asks_for_a_tool_and_answers_from_its_result(backend: &LlamaBackend, model: &LlamaModel) {
    let mut gen = EngineGenerator::new(Runner::new(model, backend, 4096).expect("runner"), "test-model", 100_000_000, 1);
    let mut seat = FakeSeat::start("m7"); let mut peer = seat.peer(); seat.run(&mut peer); let mut h = Handler::new();
    let msg = |seat: &mut FakeSeat, id: &str, n: usize, v: serde_json::Value| { let e = format!("membrane::LlmMsg:{id}:{n}"); for (k, x) in v.as_object().unwrap() { seat.put(&e, k, x.clone()); } };
    seat.put("membrane::LlmTool:1:0", "spec", json!({"type": "function", "function": {"name": "get_weather", "description": "Current weather of a city", "parameters": {"type": "object", "properties": {"city": {"type": "string"}}, "required": ["city"]}}}));
    msg(&mut seat, "1", 0, json!({"role": "user", "content": "What is the weather in Lima right now? Use the tool."}));
    let ask = |seat: &mut FakeSeat, id: &str, n: usize| {
        let req = format!("membrane::LlmRequest:{id}");
        seat.put(&req, "server", json!("m")); seat.put(&req, "messages", json!((0..n).map(|i| format!("membrane::LlmMsg:{id}:{i}")).collect::<Vec<_>>()));
        seat.put(&req, "tools", json!(["membrane::LlmTool:1:0"])); seat.put(&req, "settings", json!({"max_new": 80, "enable_thinking": false}));
    };
    ask(&mut seat, "1", 1);
    for _ in 0..5 { h.tick(&mut peer, &mut gen).unwrap(); }
    let r = seat.reader();
    assert_eq!(cell(&r, "membrama::Reply:1", "state"), Some(json!("done")), "{:?}", cell(&r, "membrama::Reply:1", "error"));
    assert_eq!(cell(&r, "membrama::Reply:1", "finish"), Some(json!("tool_calls")), "{:?}", cell(&r, "membrama::Chunk:1:0", "text"));
    let calls = cell(&r, "membrama::Reply:1", "tool_calls").unwrap();
    assert_eq!(calls[0]["name"], json!("get_weather"));
    assert!(calls[0]["arguments"]["city"].as_str().unwrap().to_lowercase().contains("lima"), "{calls}");
    // the second request: the same chat, the assistant's call, and the tool's result
    msg(&mut seat, "2", 0, json!({"role": "user", "content": "What is the weather in Lima right now? Use the tool."}));
    msg(&mut seat, "2", 1, json!({"role": "assistant", "content": "", "tool_calls": [{"type": "function", "function": {"name": "get_weather", "arguments": calls[0]["arguments"]}}]}));
    msg(&mut seat, "2", 2, json!({"role": "tool", "content": "{\"temperature_c\": 19, \"sky\": \"fog\"}"}));
    seat.put("membrane::LlmRequest:2", "server", json!("m")); seat.put("membrane::LlmRequest:2", "messages", json!((0..3).map(|i| format!("membrane::LlmMsg:2:{i}")).collect::<Vec<_>>()));
    seat.put("membrane::LlmRequest:2", "tools", json!(["membrane::LlmTool:1:0"])); seat.put("membrane::LlmRequest:2", "settings", json!({"max_new": 60, "enable_thinking": false}));
    for _ in 0..5 { h.tick(&mut peer, &mut gen).unwrap(); }
    let r = seat.reader();
    assert_eq!(cell(&r, "membrama::Reply:2", "finish"), Some(json!("stop")), "{:?}", cell(&r, "membrama::Chunk:2:0", "text"));
    let a = answer(&r, "2");
    assert!(a.contains("19") || a.contains("fog"), "the answer should use the tool's result: {a:?}");
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
        ("the_model_asks_for_a_tool_and_answers_from_its_result", the_model_asks_for_a_tool_and_answers_from_its_result as fn(&LlamaBackend, &LlamaModel)),
        ("a_request_with_the_same_start_reads_only_what_differs", a_request_with_the_same_start_reads_only_what_differs as fn(&LlamaBackend, &LlamaModel)),
        ("a_request_with_more_entity_parts_than_the_limit_is_read_as_text_and_answers_right", a_request_with_more_entity_parts_than_the_limit_is_read_as_text_and_answers_right as fn(&LlamaBackend, &LlamaModel)),
    ] {
        eprintln!("scenario: {name}");
        scenario(&backend, &model);
    }
}
