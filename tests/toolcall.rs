//! Reading tool calls out of a reply (docs/design.md). Hermes style: `<tool_call>` JSON `</tool_call>`.
use membrama::toolcall::{detect_format, parse, Format, ToolCall};
use serde_json::json;

const QWEN3: &str = include_str!("fixtures/qwen3.jinja");
fn call(name: &str, args: serde_json::Value) -> ToolCall { ToolCall { name: name.into(), arguments: args } }

#[test]
fn a_reply_with_no_tool_call_is_all_content() {
    let p = parse("It is 18C in Lima.", Format::Hermes).unwrap();
    assert_eq!((p.content.as_str(), p.tool_calls.len()), ("It is 18C in Lima.", 0));
}

#[test]
fn one_tool_call_is_read_and_removed_from_the_content() {
    let p = parse("<tool_call>\n{\"name\": \"get_weather\", \"arguments\": {\"city\": \"Lima\"}}\n</tool_call>", Format::Hermes).unwrap();
    assert_eq!(p.content, "");
    assert_eq!(p.tool_calls, vec![call("get_weather", json!({"city": "Lima"}))]);
}

#[test]
fn text_before_and_several_calls_keep_their_order() {
    let r = "Let me check.\n<tool_call>\n{\"name\": \"a\", \"arguments\": {\"x\": 1}}\n</tool_call>\n<tool_call>\n{\"name\": \"b\", \"arguments\": {}}\n</tool_call>";
    let p = parse(r, Format::Hermes).unwrap();
    assert_eq!(p.content, "Let me check.");
    assert_eq!(p.tool_calls, vec![call("a", json!({"x": 1})), call("b", json!({}))]);
}

#[test]
fn arguments_left_out_are_an_empty_object_and_arguments_given_as_a_json_string_are_read() {
    assert_eq!(parse("<tool_call>{\"name\": \"a\"}</tool_call>", Format::Hermes).unwrap().tool_calls, vec![call("a", json!({}))]);
    assert_eq!(parse("<tool_call>{\"name\": \"a\", \"arguments\": \"{\\\"x\\\": 2}\"}</tool_call>", Format::Hermes).unwrap().tool_calls, vec![call("a", json!({"x": 2}))]);
}

#[test]
fn a_call_that_is_not_json_or_has_no_name_is_an_error_that_says_so() {
    assert!(parse("<tool_call>not json</tool_call>", Format::Hermes).unwrap_err().contains("tool_call"));
    assert!(parse("<tool_call>{\"arguments\": {}}</tool_call>", Format::Hermes).unwrap_err().contains("name"));
}

#[test]
fn a_reply_cut_off_inside_a_call_is_an_error_not_half_a_call() {
    assert!(parse("<tool_call>\n{\"name\": \"a\", \"argum", Format::Hermes).unwrap_err().contains("unterminated"));
}

#[test]
fn the_format_is_known_from_the_template_and_an_unknown_one_is_none() {
    assert_eq!(detect_format(QWEN3), Some(Format::Hermes));
    assert_eq!(detect_format("{% for m in messages %}{{ m.content }}{% endfor %}"), None);
}
