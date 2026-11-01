//! The shapes the harness feeds the agent and the shapes the agent returns.
//! Field names follow what voice stacks already use: role and item type from
//! the OpenAI Realtime API, is_final and speech_final from Deepgram,
//! interrupted from LiveKit, prosody from Hume, and generation_config on the
//! output from Cartesia.

use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

/// Transcript is the call so far, as of `now`.
///
/// Reading it:
///
/// - A user item with `is_final` false is text still changing; `is_final` true
///   and `speech_final` false is stable text from someone still talking;
///   `speech_final` true means the speaker paused.
/// - A large gap between `now` and the last item's `end`, with `speech_final`
///   true, is silence.
/// - A user item whose `start` falls inside an assistant item's range is a
///   barge-in; that assistant item carries `interrupted` true.
/// - Two user items in a row is a caller who paused and went on.
/// - `function_call` and `function_call_output` items are API calls made
///   earlier in the call, named by API method ("customers.identify",
///   "bookings.create"), with the request as `arguments` and the response,
///   or `{"error", "detail"}`, as `output`. Whatever tools you expose to the
///   model, the transcript records what reached the API.
///
/// Callers are people. They hedge, restart, correct themselves, talk to
/// someone in the room, and change the plan halfway.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Transcript {
    pub call_id: String,
    /// "2026-09-28", fixed per fixture so date words resolve.
    pub today: String,
    /// Seconds since the call started.
    pub now: f64,
    pub caller: Caller,
    pub items: Vec<Item>,
}

/// Caller is what the phone system knows before anyone speaks.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Caller {
    pub phone: String,
}

/// Item is one conversation item. `type` decides which fields are set.
///
/// ```text
/// message               role, start, end, content, and either interrupted
///                       (assistant) or is_final, speech_final, confidence,
///                       prosody (user)
/// function_call         call_id, name, arguments, start
/// function_call_output  call_id, output, end
/// ```
///
/// Function calls are API calls made earlier in the call, named by API method
/// ("customers.identify", "bookings.create"), with the request as `arguments`
/// and the response, or `{"error", "detail"}`, as `output`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Item {
    #[serde(rename = "type")]
    pub type_: String,

    /// "user" | "assistant"
    #[serde(skip_serializing_if = "String::is_empty")]
    pub role: String,
    pub start: f64,
    pub end: f64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub content: String,
    /// assistant: cut off by a barge-in
    #[serde(skip_serializing_if = "is_false")]
    pub interrupted: bool,
    /// user: transcript text is stable
    #[serde(skip_serializing_if = "is_false")]
    pub is_final: bool,
    /// user: the speaker paused
    #[serde(skip_serializing_if = "is_false")]
    pub speech_final: bool,
    #[serde(skip_serializing_if = "is_zero")]
    pub confidence: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prosody: Option<Prosody>,

    #[serde(skip_serializing_if = "String::is_empty")]
    pub call_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// JSON text
    #[serde(skip_serializing_if = "String::is_empty")]
    pub arguments: String,
    /// JSON text
    #[serde(skip_serializing_if = "String::is_empty")]
    pub output: String,
}

/// Prosody is a rough read of how the line was said, not what was said.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prosody {
    pub emotions: Vec<Emotion>,
}

/// Emotion is one named dimension and its score in [0, 1].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Emotion {
    pub name: String,
    pub score: f64,
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn is_zero(f: &f64) -> bool {
    *f == 0.0
}

/// Action is what the receptionist does next. It is a map with a "type" key so
/// new action types and new fields need no schema change. The harness runs each
/// action through the handler registered for its type; an action with no
/// handler fails the fixture. [`say`], [`wait`], [`transfer`] and [`end_call`]
/// build the four the shipped handlers know; build others by hand.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Action(pub Map<String, Value>);

/// Effect is what a handler produced when it carried out an action. The
/// fixtures assert on effects.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Effect(pub Map<String, Value>);

fn type_of(m: &Map<String, Value>) -> &str {
    m.get("type").and_then(Value::as_str).unwrap_or("")
}

/// The JSON value handed to [`Action::try_from`] or [`Effect::try_from`] was
/// not an object.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("expected a JSON object, got {0}")]
pub struct NotAnObject(pub Value);

macro_rules! map_newtype {
    ($name:ident) => {
        impl $name {
            /// One of this type, with no other fields yet. Add them with [`Self::with`].
            pub fn new(type_: &str) -> Self {
                let mut m = Map::new();
                m.insert("type".to_string(), Value::String(type_.to_string()));
                $name(m)
            }

            /// Adds one field and returns the result, for chaining.
            pub fn with(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
                self.0.insert(key.into(), value.into());
                self
            }

            /// Returns the "type" key, or "" if missing.
            pub fn type_(&self) -> &str {
                type_of(&self.0)
            }

            /// One field, if set.
            pub fn get(&self, key: &str) -> Option<&Value> {
                self.0.get(key)
            }

            /// One field, or JSON null when unset. Handy for passing fields on.
            pub fn field(&self, key: &str) -> Value {
                self.0.get(key).cloned().unwrap_or(Value::Null)
            }

            /// Sets one field, returning the value it replaced.
            pub fn insert(&mut self, key: impl Into<String>, value: impl Into<Value>) -> Option<Value> {
                self.0.insert(key.into(), value.into())
            }

            /// The fields as a JSON object.
            pub fn as_map(&self) -> &Map<String, Value> {
                &self.0
            }
        }

        impl From<Map<String, Value>> for $name {
            fn from(m: Map<String, Value>) -> Self {
                $name(m)
            }
        }

        impl TryFrom<Value> for $name {
            type Error = NotAnObject;

            /// Accepts a JSON object; anything else is an error.
            fn try_from(v: Value) -> Result<Self, NotAnObject> {
                match v {
                    Value::Object(m) => Ok($name(m)),
                    other => Err(NotAnObject(other)),
                }
            }
        }

        impl fmt::Display for $name {
            /// JSON text, for logs and the harness's verbose output.
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&Value::Object(self.0.clone()).to_string())
            }
        }
    };
}

map_newtype!(Action);
map_newtype!(Effect);

/// Speaks text. Emotion and speed are hints to the TTS; speed is 0.6 to 1.5.
pub fn say(text: &str, emotion: &str, speed: f64) -> Action {
    Action::new("say").with("text", text).with("generation_config", json!({"emotion": emotion, "speed": speed}))
}

/// Says nothing for a while and lets the caller finish or think.
pub fn wait(seconds: f64) -> Action {
    Action::new("wait").with("seconds", seconds)
}

/// Hands the call to a human queue. The queue name is yours to choose.
pub fn transfer(to: &str, reason: &str) -> Action {
    Action::new("transfer").with("to", to).with("reason", reason)
}

/// Hangs up.
pub fn end_call(reason: &str) -> Action {
    Action::new("end_call").with("reason", reason)
}

/// Reads a transcript from a JSON file, for your own tests. The fixtures carry
/// their transcripts inline in YAML with the same keys.
pub fn load_json(path: impl AsRef<Path>) -> anyhow::Result<Transcript> {
    let path = path.as_ref();
    let b = std::fs::read(path)?;
    let t = serde_json::from_slice(&b).map_err(|e| anyhow::anyhow!("{}: {}", path.display(), e))?;
    Ok(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructors_carry_the_handler_fields() {
        let a = say("Hello.", "warm", 1.1);
        assert_eq!(a.type_(), "say");
        assert_eq!(a.get("text"), Some(&json!("Hello.")));
        assert_eq!(a.get("generation_config"), Some(&json!({"emotion": "warm", "speed": 1.1})));
        assert_eq!(wait(1.5).get("seconds"), Some(&json!(1.5)));
        let t = transfer("front_desk", "unknown caller");
        assert_eq!(
            (t.type_(), t.get("to"), t.get("reason")),
            ("transfer", Some(&json!("front_desk")), Some(&json!("unknown caller")))
        );
        assert_eq!(end_call("done").type_(), "end_call");
    }

    #[test]
    fn only_objects_become_actions() {
        assert_eq!(Action::try_from(json!("say")), Err(NotAnObject(json!("say"))));
        let mut a = Action::try_from(json!({"type": "hold_tone"})).unwrap();
        assert_eq!(a.type_(), "hold_tone");
        assert_eq!(a.insert("seconds", 2.0), None);
        assert_eq!(a.to_string(), r#"{"type":"hold_tone","seconds":2.0}"#);
        assert_eq!(Action::new("hold_tone").with("seconds", 2.0), a);
        assert_eq!((a.field("seconds"), a.field("missing")), (json!(2.0), Value::Null));
        assert_eq!(Action::default().type_(), "");
        assert_eq!(Effect::try_from(json!([1])).unwrap_err().to_string(), "expected a JSON object, got [1]");
    }

    #[test]
    fn load_json_reads_a_transcript_and_skips_unset_fields() {
        let path = std::env::temp_dir().join(format!("pirtis-transcript-{}.json", std::process::id()));
        std::fs::write(
            &path,
            r#"{"call_id":"c","today":"2026-09-28","now":12.5,"caller":{"phone":"+370"},
               "items":[{"type":"message","role":"user","start":1,"end":2.5,"content":"hi","is_final":true,
                         "prosody":{"emotions":[{"name":"Calmness","score":0.5}]}},
                        {"type":"function_call","call_id":"f1","name":"customers.identify","arguments":"{}","start":3}]}"#,
        )
        .unwrap();
        let t = load_json(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!((t.now, t.caller.phone.as_str(), t.items.len()), (12.5, "+370", 2));
        assert_eq!(t.items[0].prosody.as_ref().unwrap().emotions[0].name, "Calmness");
        let v = serde_json::to_value(&t.items[0]).unwrap();
        assert_eq!(v["is_final"], true);
        assert!(v.get("speech_final").is_none() && v.get("call_id").is_none(), "unset fields are omitted: {v}");
        assert_eq!(v["start"], 1.0, "times are always written");
        assert!(load_json("/nonexistent/t.json").is_err());
    }
}
