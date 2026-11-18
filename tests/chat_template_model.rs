//! The renderer against an independent one: llama.cpp's own template function on the model file. Needs MEMBRAMA_TEST_MODEL; without it the test says so and passes.
mod common;
use common::model_path;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{LlamaChatMessage, LlamaModel};
use membrama::template::{render, Options};
use serde_json::json;

#[test]
fn the_models_template_renders_a_plain_chat_like_llama_cpp_does() {
    let Some(path) = model_path() else { return };
    let backend = LlamaBackend::init().expect("backend");
    let model = LlamaModel::load_from_file(&backend, &path, &LlamaModelParams::default().with_n_gpu_layers(0)).expect("model");
    let tmpl = model.chat_template(None).expect("the model has a chat template");
    let chat = [("system", "Be brief."), ("user", "hi"), ("assistant", "Hello."), ("user", "What is 2 plus 2?")];
    let theirs = model.apply_chat_template(&tmpl, &chat.iter().map(|(r, c)| LlamaChatMessage::new(r.to_string(), c.to_string()).unwrap()).collect::<Vec<_>>(), true).expect("llama.cpp renders");
    let ours = render(tmpl.to_str().unwrap(), &chat.iter().map(|(r, c)| json!({"role": r, "content": c})).collect::<Vec<_>>(), &[], &Options { add_generation_prompt: true, enable_thinking: None }).expect("we render");
    assert_eq!(ours, theirs);
}
