use std::collections::HashMap;

use crate::api;
use crate::transcript::{Action, Effect, Transcript};

/// HandlerContext is what a handler can reach while carrying out an action.
pub struct HandlerContext<'a> {
    pub api: &'a dyn api::Client,
    pub transcript: &'a Transcript,
}

/// Handler carries out one action and reports what happened as an [`Effect`].
pub type Handler = Box<dyn Fn(&Action, &HandlerContext<'_>) -> anyhow::Result<Effect>>;

/// Maps an action type to the code that carries it out. The harness runs
/// every action [`super::decide`] returns through the handler for its type
/// and asserts on the effects. An action with no handler fails the fixture.
///
/// These four are the starting set. Add your own, change these, or wrap them.
pub fn handlers() -> HashMap<String, Handler> {
    let mut h: HashMap<String, Handler> = HashMap::new();
    h.insert(
        "say".into(),
        Box::new(|a, _| {
            Ok(Effect::new("speech")
                .with("text", a.field("text"))
                .with("generation_config", a.field("generation_config")))
        }),
    );
    h.insert("wait".into(), Box::new(|a, _| Ok(Effect::new("silence").with("seconds", a.field("seconds")))));
    h.insert(
        "transfer".into(),
        Box::new(|a, _| Ok(Effect::new("transfer").with("to", a.field("to")).with("reason", a.field("reason")))),
    );
    h.insert("end_call".into(), Box::new(|a, _| Ok(Effect::new("hangup").with("reason", a.field("reason")))));
    h
}
