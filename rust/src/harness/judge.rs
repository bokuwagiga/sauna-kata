use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::llm::{self, Message, Request, ResponseFormat};
use crate::transcript::{Effect, Transcript};

/// Verdict is the judge's answer on one criterion.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Verdict {
    pub criterion: String,
    pub pass: bool,
    pub reason: String,
}

const JUDGE_SYSTEM: &str = "You grade one reply from a phone receptionist at a mobile sauna rental company.
You see the call so far and the receptionist's latest reply. Each criterion is a statement about that reply.
Mark it pass if the statement is true of the reply and fail if it is not, with a one-sentence reason.
Judge only the latest reply, using the call for context. Be strict about the substance and lenient about wording.";

/// Builds the judge's answer shape for these criteria: one required property
/// per criterion, named criterion_1, criterion_2, ..., each carrying the
/// criterion text as its description. Strict mode then guarantees exactly one
/// verdict per criterion. Inside a verdict, reason comes before pass so the
/// model writes its reasoning before it commits.
pub fn verdict_schema(criteria: &[String]) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for (i, cr) in criteria.iter().enumerate() {
        properties.insert(
            criterion_key(i),
            json!({
                "type": "object",
                "description": cr,
                "properties": {
                    "reason": {"type": "string", "description": "one sentence"},
                    "pass": {"type": "boolean"},
                },
                "required": ["reason", "pass"],
                "additionalProperties": false,
            }),
        );
        required.push(Value::String(criterion_key(i)));
    }
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

fn criterion_key(i: usize) -> String {
    format!("criterion_{}", i + 1)
}

/// What [`judge`] can fail with. `Missing` still carries a verdict per
/// criterion, the absent ones marked "no verdict given".
#[derive(Debug, thiserror::Error)]
pub enum JudgeError {
    #[error(transparent)]
    Chat(#[from] llm::Error),
    #[error("unreadable verdict: {0}")]
    Unreadable(String),
    #[error("verdict missing for {missing} of {total} criteria")]
    Missing { verdicts: Vec<Verdict>, missing: usize, total: usize },
}

/// Asks the model whether the last reply meets each criterion.
pub fn judge(
    c: &dyn llm::Client,
    tr: &Transcript,
    effects: &[Effect],
    criteria: &[String],
) -> Result<Vec<Verdict>, JudgeError> {
    let mut b = String::from("Call so far:\n");
    for it in &tr.items {
        match it.type_.as_str() {
            "message" => {
                let who = if it.role == "assistant" { "receptionist" } else { "caller" };
                b.push_str(&format!("{}: {}\n", who, it.content));
            }
            "function_call" => {
                b.push_str(&format!("[receptionist's system called {} with {}]\n", it.name, it.arguments));
            }
            "function_call_output" => b.push_str(&format!("[result: {}]\n", it.output)),
            _ => {}
        }
    }
    b.push_str("\nReceptionist's latest reply:\n");
    let mut said: Vec<String> = Vec::new();
    for e in effects {
        match e.type_() {
            "speech" => said.push(match e.get("text") {
                Some(Value::String(s)) => s.clone(),
                Some(v) => v.to_string(),
                None => "null".to_string(),
            }),
            "silence" => said.push("(says nothing, waits)".to_string()),
            "transfer" => said.push("(transfers the call to a person)".to_string()),
            "hangup" => said.push("(hangs up)".to_string()),
            _ => {}
        }
    }
    if said.is_empty() {
        said.push("(nothing)".to_string());
    }
    b.push_str(&said.join(" "));
    b.push_str("\n\nCriteria:\n");
    for (i, cr) in criteria.iter().enumerate() {
        b.push_str(&format!("{}. {}\n", i + 1, cr));
    }

    let resp = c.chat(&Request {
        messages: vec![Message::new("system", JUDGE_SYSTEM), Message::new("user", b)],
        temperature: Some(0.0),
        response_format: Some(ResponseFormat {
            name: "verdict".to_string(),
            schema: verdict_schema(criteria),
            strict: true,
            ..Default::default()
        }),
        ..Default::default()
    })?;

    #[derive(Deserialize)]
    struct Raw {
        #[serde(default)]
        reason: String,
        #[serde(default)]
        pass: bool,
    }
    let parsed: HashMap<String, Raw> =
        serde_json::from_str(&resp.content).map_err(|_| JudgeError::Unreadable(llm::truncate(&resp.content, 200)))?;
    let mut out = Vec::with_capacity(criteria.len());
    let mut missing = 0;
    for (i, cr) in criteria.iter().enumerate() {
        match parsed.get(&criterion_key(i)) {
            Some(v) => out.push(Verdict { criterion: cr.clone(), pass: v.pass, reason: v.reason.clone() }),
            None => {
                out.push(Verdict { criterion: cr.clone(), pass: false, reason: "no verdict given".to_string() });
                missing += 1;
            }
        }
    }
    if missing > 0 {
        return Err(JudgeError::Missing { verdicts: out, missing, total: criteria.len() });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The schema names one required property per criterion, carries the
    /// criterion text, and asks for reason before pass.
    #[test]
    fn verdict_schema_shape() {
        let raw = verdict_schema(&["asks for the name".to_string(), "does not book".to_string()]);
        // round-trip through text, as the endpoint sees it
        let s: Value = serde_json::from_str(&raw.to_string()).unwrap();
        assert_eq!(s["required"], json!(["criterion_1", "criterion_2"]));
        let c2 = &s["properties"]["criterion_2"];
        assert_eq!(c2["description"], "does not book");
        assert_eq!(c2["required"], json!(["reason", "pass"]), "verdict required must be reason then pass");
        assert_eq!(s["additionalProperties"], false, "additionalProperties must be false for strict mode");
        assert_eq!(c2["additionalProperties"], false);
    }
}
