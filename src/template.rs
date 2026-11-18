//! Rendering a chat (OpenAI-shaped messages and tools) into the prompt text with the model's OWN chat template, the Jinja text stored in the model file
//! (docs/design.md, "One renderer for any model"). Any model whose template minijinja can run needs no renderer of ours.
use minijinja::value::{Serde, Value as J};
use minijinja::{context, Environment, Error, ErrorKind};
use serde_json::Value;

#[derive(Clone, Debug, Default)]
pub struct Options {
    /// End with the opening of the assistant's turn.
    pub add_generation_prompt: bool,
    /// Passed to the template as `enable_thinking` when set (templates that do not know it ignore it).
    pub enable_thinking: Option<bool>,
}

/// `raise_exception(message)`, which model templates call to refuse a chat they cannot render.
fn raise_exception(message: String) -> Result<String, Error> { Err(Error::new(ErrorKind::InvalidOperation, message)) }

pub fn render(template: &str, messages: &[Value], tools: &[Value], opts: &Options) -> Result<String, String> {
    let mut env = Environment::new();
    env.set_unknown_method_callback(minijinja_contrib::pycompat::unknown_method_callback);   // str.startswith, split, ...: the templates are written for Python
    env.add_function("raise_exception", raise_exception);
    env.add_template("chat", template).map_err(|e| format!("the chat template does not compile: {e}"))?;
    let tools_v = if tools.is_empty() { J::from(()) } else { J::from(Serde(tools)) };   // templates test `if tools`
    let ctx = context! {
        messages => J::from(Serde(messages)),
        tools => tools_v,
        add_generation_prompt => opts.add_generation_prompt,
        enable_thinking => opts.enable_thinking.map(J::from).unwrap_or(J::UNDEFINED),
    };
    env.get_template("chat").and_then(|t| t.render(ctx)).map_err(|e| format!("the chat template failed: {e:#}"))
}
