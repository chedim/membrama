//! The model's own chat template, rendered by minijinja (docs/design.md). The fixture is the template stored in the Qwen3 model file (qwen3:8b), unchanged.
use membrama::template::{render, Options};
use serde_json::{json, Value};

const QWEN3: &str = include_str!("fixtures/qwen3.jinja");
fn gen() -> Options { Options { add_generation_prompt: true, enable_thinking: None } }
fn weather() -> Value { json!({"type": "function", "function": {"name": "get_weather", "description": "Weather of a city", "parameters": {"type": "object", "properties": {"city": {"type": "string"}}, "required": ["city"]}}}) }

#[test]
fn a_plain_chat_is_the_models_format() {
    let out = render(QWEN3, &[json!({"role": "system", "content": "Be brief."}), json!({"role": "user", "content": "hi"})], &[], &gen()).unwrap();
    assert_eq!(out, "<|im_start|>system\nBe brief.<|im_end|>\n<|im_start|>user\nhi<|im_end|>\n<|im_start|>assistant\n");
}

#[test]
fn thinking_switched_off_closes_the_think_block() {
    let out = render(QWEN3, &[json!({"role": "user", "content": "hi"})], &[], &Options { add_generation_prompt: true, enable_thinking: Some(false) }).unwrap();
    assert!(out.ends_with("<|im_start|>assistant\n<think>\n\n</think>\n\n"), "{out:?}");
}

#[test]
fn without_a_generation_prompt_the_assistant_turn_is_not_opened() {
    let out = render(QWEN3, &[json!({"role": "user", "content": "hi"})], &[], &Options { add_generation_prompt: false, enable_thinking: None }).unwrap();
    assert_eq!(out, "<|im_start|>user\nhi<|im_end|>\n");
}

#[test]
fn tools_are_listed_in_the_system_turn_as_json() {
    let out = render(QWEN3, &[json!({"role": "user", "content": "weather in Lima?"})], &[weather()], &gen()).unwrap();
    assert!(out.starts_with("<|im_start|>system\n# Tools"), "{out:?}");
    assert!(out.contains("<tools>\n{\"type\": \"function\", \"function\": {\"name\": \"get_weather\""), "{out:?}");
    assert!(out.contains("</tools>"));
    assert!(out.ends_with("<|im_start|>user\nweather in Lima?<|im_end|>\n<|im_start|>assistant\n"));
}

#[test]
fn a_tool_call_and_its_result_are_in_the_models_format() {
    let msgs = [
        json!({"role": "user", "content": "weather in Lima?"}),
        json!({"role": "assistant", "content": "", "tool_calls": [{"type": "function", "function": {"name": "get_weather", "arguments": {"city": "Lima"}}}]}),
        json!({"role": "tool", "content": "18C, fog"}),
    ];
    let out = render(QWEN3, &msgs, &[weather()], &gen()).unwrap();
    assert!(out.contains("<tool_call>\n{\"name\": \"get_weather\", \"arguments\": {\"city\": \"Lima\"}}\n</tool_call>"), "{out:?}");
    assert!(out.contains("<|im_start|>user\n<tool_response>\n18C, fog\n</tool_response><|im_end|>\n"), "{out:?}");
}

#[test]
fn a_template_that_does_not_compile_is_an_error_not_a_guess() {
    assert!(render("{% if %}", &[], &[], &gen()).is_err());
}
