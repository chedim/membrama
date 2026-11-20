//! Reading the tool calls out of a model's reply (docs/design.md, "One renderer for any model"). The reply format differs by model family; the one known
//! here is the Hermes style (`<tool_call>{json}</tool_call>`, used by Qwen3 and others). A model whose format is not known gets no tools: an error, never a guess.
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format { Hermes }

#[derive(Clone, Debug, PartialEq)]
pub struct ToolCall { pub name: String, pub arguments: Value }

#[derive(Clone, Debug, PartialEq)]
pub struct Parsed { pub content: String, pub tool_calls: Vec<ToolCall> }

/// The reply format a model's chat template shows it expects, or `None` when it is not one we know. Hermes style: the template itself writes
/// `<tool_call>` ... `</tool_call>` around the calls it puts in the prompt.
pub fn detect_format(template: &str) -> Option<Format> {
    if template.contains("<tool_call>") && template.contains("</tool_call>") { Some(Format::Hermes) } else { None }
}

const OPEN: &str = "<tool_call>";
const CLOSE: &str = "</tool_call>";

pub fn parse(reply: &str, format: Format) -> Result<Parsed, String> {
    match format { Format::Hermes => hermes(reply) }
}

fn hermes(reply: &str) -> Result<Parsed, String> {
    let mut content = String::new();
    let mut calls = Vec::new();
    let mut rest = reply;
    while let Some(start) = rest.find(OPEN) {
        content.push_str(&rest[..start]);
        let after = &rest[start + OPEN.len()..];
        let Some(end) = after.find(CLOSE) else { return Err("unterminated tool_call: the reply ends inside a call".into()) };
        calls.push(one(after[..end].trim())?);
        rest = &after[end + CLOSE.len()..];
    }
    content.push_str(rest);
    Ok(Parsed { content: content.trim().to_string(), tool_calls: calls })
}

fn one(body: &str) -> Result<ToolCall, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| format!("tool_call is not JSON ({e}): {}", body.chars().take(80).collect::<String>()))?;
    let name = v.get("name").and_then(Value::as_str).filter(|n| !n.is_empty()).ok_or_else(|| "tool_call has no name".to_string())?.to_string();
    let arguments = match v.get("arguments") {
        None | Some(Value::Null) => Value::Object(Default::default()),
        // some models write the arguments as a JSON text
        Some(Value::String(t)) => serde_json::from_str(t).map_err(|e| format!("tool_call arguments are not JSON ({e}): {}", t.chars().take(80).collect::<String>()))?,
        Some(other) => other.clone(),
    };
    Ok(ToolCall { name, arguments })
}

/// What a reply says to a reader: the model's thinking removed (a `<think>` block, or the text before a lone `</think>` when the template opened the block in the prompt), the rest trimmed.
pub fn visible(text: &str) -> String {
    let mut t = text;
    if let Some(i) = t.rfind("</think>") { t = &t[i + "</think>".len()..]; }
    let mut out = String::new();
    let mut rest = t;
    while let Some(i) = rest.find("<think>") {
        out.push_str(&rest[..i]);
        match rest[i..].find("</think>") { Some(e) => rest = &rest[i + e + "</think>".len()..], None => { rest = ""; break; } }
    }
    out.push_str(rest);
    out.trim().to_string()
}
