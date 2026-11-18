//! Reading the tool calls out of a model's reply (docs/design.md, "One renderer for any model"). The reply format differs by model family; the one known
//! here is the Hermes style (`<tool_call>{json}</tool_call>`, used by Qwen3 and others). A model whose format is not known gets no tools: an error, never a guess.
//! RED STUB.
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format { Hermes }

#[derive(Clone, Debug, PartialEq)]
pub struct ToolCall { pub name: String, pub arguments: Value }

#[derive(Clone, Debug, PartialEq)]
pub struct Parsed { pub content: String, pub tool_calls: Vec<ToolCall> }

/// The reply format a model's chat template shows it expects, or `None` when it is not one we know.
pub fn detect_format(_template: &str) -> Option<Format> { None }

pub fn parse(_reply: &str, _format: Format) -> Result<Parsed, String> { Err("not built".into()) }
