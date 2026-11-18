//! The turn handler over a real model: the seat's request is answered by the model. Needs MEMBRAMA_TEST_MODEL (a Qwen3 chat gguf); without it the test says so and passes.
mod common;
use common::{cell, FakeSeat};
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::LlamaModel;
use membrama::engine::{EngineGenerator, Runner};
use membrama::turn::Handler;
use serde_json::json;

#[test]
fn a_request_over_a_real_model_is_answered() {
    let Ok(path) = std::env::var("MEMBRAMA_TEST_MODEL") else { eprintln!("SKIPPED: MEMBRAMA_TEST_MODEL is not set, so the turn was NOT tested against a model"); return; };
    let backend = LlamaBackend::init().expect("backend");
    let model = LlamaModel::load_from_file(&backend, &path, &LlamaModelParams::default().with_n_gpu_layers(99)).expect("model");
    let mut gen = EngineGenerator::new(Runner::new(&model, &backend, 2048).expect("runner"));

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
