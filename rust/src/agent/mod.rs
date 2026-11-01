//! The receptionist. This is where the work happens.
//!
//! [`decide`] gets the call so far and returns what to do next. [`handlers`],
//! in actions.rs, carry those actions out. Everything else in the crate is
//! plumbing you can use or ignore.

mod actions;

pub use actions::{handlers, Handler, HandlerContext};

use serde_json::json;

use crate::api;
use crate::llm::{self, Message, Tool, ToolResult};
use crate::transcript::{self, Action, Transcript};

/// Returns the receptionist's next actions given the transcript so far.
///
/// What is here now is a starting point. It passes the happy-path fixture and
/// not much else.
pub fn decide(t: &Transcript, a: &dyn api::Client, c: &dyn llm::Client) -> anyhow::Result<Vec<Action>> {
    let units = a.list_units()?;
    let raw = serde_json::to_string(&units)?;
    let system = format!(
        "You are the receptionist for Pirtis on Wheels, a mobile sauna rental. \
         Help the caller book a sauna. Today is {}. Units: {}",
        t.today, raw
    );

    // every item becomes history, whether the caller finished the sentence or not
    let mut history = Vec::new();
    for it in &t.items {
        match it.type_.as_str() {
            "message" => history.push(Message::new(&it.role, &it.content)),
            "function_call" => {
                history.push(Message::new("assistant", format!("[called {} with {}]", it.name, it.arguments)))
            }
            "function_call_output" => history.push(Message::new("assistant", format!("[result: {}]", it.output))),
            _ => {}
        }
    }

    let tools = vec![
        Tool::new(
            "availability",
            "Free dates per unit between two dates, YYYY-MM-DD.",
            json!({"type":"object","properties":{"from":{"type":"string"},"to":{"type":"string"}},"required":["from","to"]}),
            // no people, no weight, no power. every unit comes back and the model picks.
            |args| {
                let q: api::AvailabilityQuery = serde_json::from_str(args)?;
                let out = a.availability(&api::AvailabilityQuery { from: q.from, to: q.to, ..Default::default() })?;
                Ok(ToolResult::data(out))
            },
        ),
        Tool::new(
            "book",
            "Create a booking.",
            json!({"type":"object","properties":{"customer_id":{"type":"string"},"unit_id":{"type":"string"},"start":{"type":"string"},"end":{"type":"string"},"people":{"type":"integer"}},"required":["customer_id","unit_id","start","end","people"]}),
            |args| {
                let mut req: api::BookingRequest = serde_json::from_str(args)?;
                req.site.power_available = true; // nobody asked
                req.tow_capacity_kg = 3500; // nobody asked
                let b = a.create_booking(&req)?;
                Ok(ToolResult::data(b))
            },
        ),
    ];

    let res = llm::run_agent(c, &system, &history, &tools, 8)?;
    if let Some(terminal) = res.terminal {
        return Ok(terminal);
    }
    Ok(vec![transcript::say(&res.text, "neutral", 1.0)])
}
