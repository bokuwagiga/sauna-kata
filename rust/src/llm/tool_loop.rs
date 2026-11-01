use std::collections::HashMap;

use serde_json::{json, Value};

use super::{Client, Error, Message, Request, ToolSchema};
use crate::api;
use crate::transcript::Action;

/// ToolResult is what a tool hands back. Set `data` to feed something to the
/// model and keep going. Set `terminal` to stop the loop and make those
/// actions the answer.
#[derive(Debug, Clone, Default)]
pub struct ToolResult {
    pub data: Value,
    pub terminal: Option<Vec<Action>>,
}

impl ToolResult {
    /// Feeds `data` to the model and keeps going.
    pub fn data(data: impl serde::Serialize) -> ToolResult {
        ToolResult { data: serde_json::to_value(data).unwrap_or(Value::Null), terminal: None }
    }

    /// Stops the loop and makes `actions` the answer.
    pub fn terminal(actions: Vec<Action>) -> ToolResult {
        ToolResult { data: Value::Null, terminal: Some(actions) }
    }
}

/// The code that runs when the model calls a tool. `args` is the JSON text the
/// model produced; parse it with `serde_json::from_str`. The closure may
/// borrow whatever `decide` has in scope, such as the API client.
pub type Execute<'a> = Box<dyn Fn(&str) -> anyhow::Result<ToolResult> + 'a>;

/// Tool is a schema the model sees plus the code that runs when it calls it.
pub struct Tool<'a> {
    pub schema: ToolSchema,
    pub execute: Execute<'a>,
}

impl<'a> Tool<'a> {
    pub fn new(
        name: &str,
        description: &str,
        parameters: Value,
        execute: impl Fn(&str) -> anyhow::Result<ToolResult> + 'a,
    ) -> Tool<'a> {
        Tool {
            schema: ToolSchema { name: name.to_string(), description: description.to_string(), parameters },
            execute: Box::new(execute),
        }
    }
}

/// AgentResult is what [`run_agent`] returns: the model's text, or terminal
/// actions from a tool.
#[derive(Debug, Clone, Default)]
pub struct AgentResult {
    pub text: String,
    pub terminal: Option<Vec<Action>>,
}

/// Runs the tool loop: call the model, run whatever tools it asks for, feed
/// the results back, repeat until it answers in text or a tool returns
/// terminal. `max_calls` caps the number of model calls; 0 means 8, and
/// hitting the cap returns [`Error::MaxCalls`] with whatever text was said
/// along the way.
///
/// A tool the model names but that does not exist, and a tool that returns an
/// error, both go back to the model as JSON rather than stopping the loop.
///
/// Text the model produces alongside a tool call ("one moment") is kept and
/// joined to what it says after, since on a phone that is one reply.
pub fn run_agent(
    c: &dyn Client,
    system: &str,
    history: &[Message],
    tools: &[Tool<'_>],
    max_calls: usize,
) -> Result<AgentResult, Error> {
    let max_calls = if max_calls == 0 { 8 } else { max_calls };
    let by_name: HashMap<&str, &Tool> = tools.iter().map(|t| (t.schema.name.as_str(), t)).collect();

    // the request grows as the conversation does; every round sends all of it
    let mut req = Request {
        messages: Vec::with_capacity(history.len() + 1),
        tools: tools.iter().map(|t| t.schema.clone()).collect(),
        ..Default::default()
    };
    req.messages.push(Message::new("system", system));
    req.messages.extend_from_slice(history);

    let mut text = String::new();
    for _ in 0..max_calls {
        let reply = c.chat(&req)?;
        if !reply.content.is_empty() {
            text = format!("{text} {}", reply.content).trim().to_string();
        }
        if reply.tool_calls.is_empty() {
            return Ok(AgentResult { text, terminal: None });
        }

        req.messages.push(Message {
            role: "assistant".to_string(),
            content: reply.content,
            tool_calls: reply.tool_calls.clone(),
            ..Default::default()
        });
        for call in &reply.tool_calls {
            let res = match by_name.get(call.name.as_str()) {
                Some(t) => match (t.execute)(&call.arguments) {
                    Ok(res) => res,
                    Err(err) => ToolResult::data(json!({"error": error_code(&err), "detail": err.to_string()})),
                },
                None => ToolResult::data(json!({"error": format!("unknown tool {}", call.name)})),
            };
            if let Some(terminal) = res.terminal {
                return Ok(AgentResult { text, terminal: Some(terminal) });
            }
            req.messages.push(Message {
                role: "tool".to_string(),
                tool_call_id: call.id.clone(),
                content: res.data.to_string(),
                ..Default::default()
            });
        }
    }
    Err(Error::MaxCalls { text })
}

/// The API reason code when `err` is an [`api::Error`], else TOOL_FAILED.
pub fn error_code(err: &anyhow::Error) -> String {
    match err.downcast_ref::<api::Error>() {
        Some(e) => e.code.clone(),
        None => "TOOL_FAILED".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::llm::{Fake, Response, ToolCall};
    use crate::transcript;

    fn echo_tool(name: &str) -> Tool<'static> {
        Tool::new(name, "echo", json!({"type": "object"}), |args| Ok(ToolResult::data(json!({"got": args}))))
    }

    struct ClientFn<F>(F);

    impl<F: Fn(&Request) -> Result<Response, Error>> Client for ClientFn<F> {
        fn chat(&self, req: &Request) -> Result<Response, Error> {
            (self.0)(req)
        }
    }

    /// The loop runs the tool the model asks for, feeds the result back, and
    /// returns the text the model then produces, joined with any text said
    /// alongside the call.
    #[test]
    fn tool_then_text() {
        let fake = Fake::new(vec![
            Response {
                content: "One moment.".into(),
                tool_calls: vec![ToolCall { id: "t1".into(), name: "lookup".into(), arguments: r#"{"q":1}"#.into() }],
                ..Default::default()
            },
            Response::text("Found it."),
        ]);
        let res = run_agent(&fake, "sys", &[], &[echo_tool("lookup")], 0).unwrap();
        assert_eq!(res.text, "One moment. Found it.");
        assert_eq!(fake.calls(), 2, "want 2 model calls");
    }

    /// A terminal tool result stops the loop and becomes the answer.
    #[test]
    fn terminal() {
        let transfer = Tool::new("transfer", "", json!({"type": "object"}), |_| {
            Ok(ToolResult::terminal(vec![transcript::transfer("front_desk", "asked")]))
        });
        let fake = Fake::new(vec![Response {
            tool_calls: vec![ToolCall { id: "t1".into(), name: "transfer".into(), arguments: "{}".into() }],
            ..Default::default()
        }]);
        let res = run_agent(&fake, "sys", &[], &[transfer], 0).unwrap();
        let terminal = res.terminal.expect("terminal actions");
        assert!(terminal.len() == 1 && terminal[0].type_() == "transfer", "terminal = {terminal:?}");
        assert_eq!(fake.calls(), 1, "loop must stop after the terminal tool");
    }

    /// An unknown tool and a failing tool both go back to the model as JSON.
    #[test]
    fn tool_errors_go_back() {
        let failing = Tool::new("book", "", json!({"type": "object"}), |_| Err(anyhow::anyhow!("boom")));
        let seen: Mutex<Vec<Message>> = Mutex::new(Vec::new());
        let fake = Fake::with_default(
            vec![Response {
                tool_calls: vec![
                    ToolCall { id: "t1".into(), name: "nope".into(), arguments: "{}".into() },
                    ToolCall { id: "t2".into(), name: "book".into(), arguments: "{}".into() },
                ],
                ..Default::default()
            }],
            Response::text("ok"),
        );
        let spy = ClientFn(|req: &Request| {
            *seen.lock().unwrap() = req.messages.clone();
            fake.chat(req)
        });
        run_agent(&spy, "sys", &[], &[failing], 0).unwrap();
        let seen = seen.lock().unwrap();
        let tool_msgs: Vec<&str> = seen.iter().filter(|m| m.role == "tool").map(|m| m.content.as_str()).collect();
        assert!(
            tool_msgs.len() == 2 && tool_msgs[0].contains("unknown tool") && tool_msgs[1].contains("TOOL_FAILED"),
            "tool messages = {tool_msgs:?}"
        );
    }

    /// A failing tool that raised an api::Error reports its reason code.
    #[test]
    fn api_error_code() {
        let err = anyhow::Error::from(api::Error::new(422, api::CODE_CAPACITY_EXCEEDED, "too many"));
        assert_eq!(error_code(&err), api::CODE_CAPACITY_EXCEEDED);
        assert_eq!(error_code(&anyhow::anyhow!("boom")), "TOOL_FAILED");
    }

    /// A model that never stops calling tools hits the cap and the caller
    /// hears about it.
    #[test]
    fn max_calls() {
        let calls = Mutex::new(0);
        let forever = ClientFn(|_: &Request| {
            *calls.lock().unwrap() += 1;
            Ok(Response {
                content: "hm".into(),
                tool_calls: vec![ToolCall { id: "t".into(), name: "lookup".into(), arguments: "{}".into() }],
                ..Default::default()
            })
        });
        let err = run_agent(&forever, "sys", &[], &[echo_tool("lookup")], 3).unwrap_err();
        match err {
            Error::MaxCalls { text } => assert_eq!(text, "hm hm hm"),
            other => panic!("want MaxCalls, got {other}"),
        }
        assert_eq!(*calls.lock().unwrap(), 3, "want exactly 3 calls");
    }
}
