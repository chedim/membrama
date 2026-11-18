//! Rendering a chat (OpenAI-shaped messages and tools) into the prompt text with the model's OWN chat template, the Jinja text stored in the model file
//! (docs/design.md, "One renderer for any model"). RED STUB.
use serde_json::Value;

#[derive(Clone, Debug, Default)]
pub struct Options {
    /// End with the opening of the assistant's turn.
    pub add_generation_prompt: bool,
    /// Passed to the template as `enable_thinking` when set (templates that do not know it ignore it).
    pub enable_thinking: Option<bool>,
}

pub fn render(_template: &str, _messages: &[Value], _tools: &[Value], _opts: &Options) -> Result<String, String> { Err("not built".into()) }
