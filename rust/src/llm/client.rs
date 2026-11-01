use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Message is one chat message. The system prompt is a Message with role
/// "system", by convention the first one.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Message {
    /// system | user | assistant | tool
    pub role: String,
    /// text
    #[serde(skip_serializing_if = "String::is_empty")]
    pub content: String,
    /// assistant only
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// tool only
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tool_call_id: String,
}

impl Message {
    pub fn new(role: &str, content: impl Into<String>) -> Message {
        Message { role: role.to_string(), content: content.into(), ..Message::default() }
    }
}

/// ToolCall is the model asking for a tool to run.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// JSON text
    pub arguments: String,
}

/// ToolSchema tells the model what a tool is and what it takes.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    /// JSON schema object
    pub parameters: Value,
}

/// ResponseFormat asks for structured output: the reply is JSON matching
/// `schema`. `schema` is a JSON Schema object. With `strict` the model is held
/// to it exactly, which requires additionalProperties: false and every
/// property listed as required.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ResponseFormat {
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub schema: Value,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub strict: bool,
}

/// Request is one chat completion request.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Request {
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolSchema>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<ResponseFormat>,
}

/// Usage is what the call cost in tokens.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Usage {
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
}

/// Response is what came back: text, tool calls, or both.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Response {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub content: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    pub usage: Usage,
}

impl Response {
    /// A text-only response, handy for scripting a [`super::Fake`].
    pub fn text(content: impl Into<String>) -> Response {
        Response { content: content.into(), ..Response::default() }
    }
}

/// Error is what a chat call or the tool loop can fail with.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A 429, a 5xx, or a failed connection. Retried by [`OpenAI::chat`].
    #[error("{msg}")]
    Retryable { msg: String, after: Option<Duration> },
    /// A non-retryable answer from the endpoint: a 4xx, an unreadable body,
    /// or a body with no choices.
    #[error("{0}")]
    Status(String),
    /// The model was still asking for tools after `max_calls` rounds. `text`
    /// holds whatever it said along the way.
    #[error("llm: tool loop hit the call limit")]
    MaxCalls { text: String },
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Client is anything that can answer a chat request.
pub trait Client {
    fn chat(&self, req: &Request) -> Result<Response, Error>;
}

/// The one model the kata runs on. Everyone is graded against the same model,
/// so it is not configurable.
pub const MODEL: &str = "gpt-4o-mini";

/// OpenAI talks to an OpenAI-compatible chat completions endpoint.
pub struct OpenAI {
    pub base_url: String,
    pub api_key: String,
    pub http: reqwest::blocking::Client,
}

impl OpenAI {
    /// Reads LLM_API_KEY (required) and LLM_BASE_URL (optional).
    pub fn from_env() -> anyhow::Result<OpenAI> {
        let key = std::env::var("LLM_API_KEY").unwrap_or_default();
        if key.is_empty() {
            anyhow::bail!("LLM_API_KEY is not set");
        }
        let mut base = std::env::var("LLM_BASE_URL").unwrap_or_default();
        if base.is_empty() {
            base = "https://api.openai.com/v1".to_string();
        }
        Ok(OpenAI::new(base, key))
    }

    /// Builds a client with a 60 second timeout per request.
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> OpenAI {
        let base_url: String = base_url.into();
        OpenAI {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.into(),
            http: reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(60))
                .build()
                .expect("reqwest client"),
        }
    }

    fn chat_once(&self, req: &Request) -> Result<Response, Error> {
        let wr = WireRequest {
            model: MODEL,
            messages: req
                .messages
                .iter()
                .map(|m| WireMessage {
                    role: m.role.clone(),
                    // an assistant message that only carries tool calls has null content
                    content: if !m.content.is_empty() || m.tool_calls.is_empty() {
                        Some(m.content.clone())
                    } else {
                        None
                    },
                    tool_calls: m
                        .tool_calls
                        .iter()
                        .map(|tc| WireToolCall {
                            id: tc.id.clone(),
                            type_: "function".to_string(),
                            function: WireFunction { name: tc.name.clone(), arguments: tc.arguments.clone() },
                        })
                        .collect(),
                    tool_call_id: m.tool_call_id.clone(),
                })
                .collect(),
            tools: req
                .tools
                .iter()
                .map(|t| WireTool {
                    type_: "function",
                    function: WireToolDef {
                        name: t.name.clone(),
                        description: t.description.clone(),
                        parameters: if t.parameters.is_null() {
                            json!({"type": "object", "properties": {}})
                        } else {
                            t.parameters.clone()
                        },
                    },
                })
                .collect(),
            temperature: req.temperature,
            response_format: req
                .response_format
                .as_ref()
                .map(|rf| WireResponseFormat { type_: "json_schema", json_schema: rf.clone() }),
        };
        let body = serde_json::to_vec(&wr).map_err(|e| Error::Other(e.into()))?;
        let resp = self
            .http
            .post(format!("{}/chat/completions", self.base_url))
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("api-key", &self.api_key) // Azure OpenAI reads this one
            .body(body)
            .send()
            .map_err(|e| Error::Retryable { msg: e.to_string(), after: None })?;
        let status = resp.status().as_u16();
        let retry_after = resp
            .headers()
            .get("Retry-After")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|s| *s > 0)
            .map(Duration::from_secs);
        let raw = resp.text().map_err(|e| Error::Retryable { msg: e.to_string(), after: None })?;
        if status == 429 || status >= 500 {
            return Err(Error::Retryable {
                msg: format!("llm: status {}: {}", status, truncate(&raw, 300)),
                after: retry_after,
            });
        }
        let w: WireResponse = serde_json::from_str(&raw)
            .map_err(|_| Error::Status(format!("llm: status {}, unreadable body: {}", status, truncate(&raw, 300))))?;
        if status != 200 || w.error.is_some() {
            let msg = match &w.error {
                Some(e) => e.message.clone(),
                None => truncate(&raw, 300),
            };
            return Err(Error::Status(format!("llm: status {status}: {msg}")));
        }
        let Some(choice) = w.choices.into_iter().next() else {
            return Err(Error::Status("llm: no choices in response".to_string()));
        };
        Ok(Response {
            content: choice.message.content.unwrap_or_default(),
            tool_calls: choice
                .message
                .tool_calls
                .into_iter()
                .map(|tc| ToolCall { id: tc.id, name: tc.function.name, arguments: tc.function.arguments })
                .collect(),
            usage: w.usage,
        })
    }
}

impl Client for OpenAI {
    /// Rate limits, server errors and network errors are retried three times
    /// with a short backoff, honouring Retry-After.
    fn chat(&self, req: &Request) -> Result<Response, Error> {
        let mut last: Option<Error> = None;
        for attempt in 0..3u32 {
            if attempt > 0 {
                let mut wait = Duration::from_secs(1 << attempt);
                if let Some(Error::Retryable { after: Some(after), .. }) = &last {
                    wait = *after;
                }
                std::thread::sleep(wait);
            }
            match self.chat_once(req) {
                Ok(resp) => return Ok(resp),
                Err(e @ Error::Retryable { .. }) => last = Some(e),
                Err(e) => return Err(e),
            }
        }
        Err(last.expect("three attempts"))
    }
}

#[derive(Serialize)]
struct WireFunction {
    name: String,
    arguments: String,
}

#[derive(Serialize)]
struct WireToolCall {
    id: String,
    #[serde(rename = "type")]
    type_: String,
    function: WireFunction,
}

#[derive(Serialize)]
struct WireMessage {
    role: String,
    content: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tool_calls: Vec<WireToolCall>,
    #[serde(skip_serializing_if = "String::is_empty")]
    tool_call_id: String,
}

#[derive(Serialize)]
struct WireToolDef {
    name: String,
    description: String,
    parameters: Value,
}

#[derive(Serialize)]
struct WireTool {
    #[serde(rename = "type")]
    type_: &'static str,
    function: WireToolDef,
}

#[derive(Serialize)]
struct WireResponseFormat {
    /// "json_schema"
    #[serde(rename = "type")]
    type_: &'static str,
    json_schema: ResponseFormat,
}

#[derive(Serialize)]
struct WireRequest {
    model: &'static str,
    messages: Vec<WireMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<WireTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<WireResponseFormat>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct WireFunctionIn {
    name: String,
    arguments: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct WireToolCallIn {
    id: String,
    function: WireFunctionIn,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct WireChoiceMessage {
    content: Option<String>,
    tool_calls: Vec<WireToolCallIn>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct WireChoice {
    message: WireChoiceMessage,
    #[allow(dead_code)]
    finish_reason: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct WireError {
    message: String,
    #[allow(dead_code)]
    #[serde(rename = "type")]
    type_: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct WireResponse {
    choices: Vec<WireChoice>,
    usage: Usage,
    error: Option<WireError>,
}

pub(crate) fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_string();
    }
    let mut end = n;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &s[..end])
}

/// Stats is what a [`Recorder`] has seen so far.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Stats {
    pub calls: i64,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    #[serde(rename = "duration_ns", with = "duration_ns")]
    pub duration: Duration,
    pub errors: i64,
}

mod duration_ns {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u64(d.as_nanos() as u64)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        Ok(Duration::from_nanos(u64::deserialize(d)?))
    }
}

/// Recorder wraps a [`Client`], counts calls and tokens, and writes every
/// request and response to a directory, one JSON file per call.
pub struct Recorder<'a> {
    inner: &'a dyn Client,
    dir: Option<PathBuf>,
    state: Mutex<(u64, Stats)>,
}

#[derive(Serialize)]
struct Record<'a> {
    at: String,
    duration_ms: u128,
    request: &'a Request,
    #[serde(skip_serializing_if = "Option::is_none")]
    response: Option<&'a Response>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl<'a> Recorder<'a> {
    /// Wraps `inner`. If `dir` is `None` nothing is written, only counted.
    pub fn new(inner: &'a dyn Client, dir: Option<PathBuf>) -> Recorder<'a> {
        Recorder { inner, dir, state: Mutex::new((0, Stats::default())) }
    }

    /// Returns the totals so far.
    pub fn stats(&self) -> Stats {
        self.state.lock().unwrap().1
    }
}

impl Client for Recorder<'_> {
    fn chat(&self, req: &Request) -> Result<Response, Error> {
        let at = chrono::Utc::now();
        let start = Instant::now();
        let res = self.inner.chat(req);
        let d = start.elapsed();

        let n = {
            let mut st = self.state.lock().unwrap();
            st.0 += 1;
            st.1.calls += 1;
            st.1.duration += d;
            match &res {
                Ok(resp) => {
                    st.1.prompt_tokens += resp.usage.prompt_tokens;
                    st.1.completion_tokens += resp.usage.completion_tokens;
                }
                Err(_) => st.1.errors += 1,
            }
            st.0
        };

        if let Some(dir) = &self.dir {
            let rec = Record {
                at: at.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true),
                duration_ms: d.as_millis(),
                request: req,
                response: res.as_ref().ok(),
                error: res.as_ref().err().map(|e| e.to_string()),
            };
            if let Ok(b) = serde_json::to_vec_pretty(&rec) {
                let _ = std::fs::create_dir_all(dir);
                let _ = std::fs::write(dir.join(format!("{n:03}.json")), b);
            }
        }
        res
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    /// The request on the wire has the chat completions shape: tools wrapped
    /// as functions, assistant tool calls with null content, tool results by
    /// id, and structured output as response_format.json_schema.
    #[test]
    fn openai_wire_shape() {
        let got: Arc<Mutex<Option<Value>>> = Arc::new(Mutex::new(None));
        let seen = got.clone();
        let mut server = mockito::Server::new();
        let mock = server
            .mock("POST", "/chat/completions")
            .match_header("authorization", "Bearer k")
            .match_request(move |r| {
                *seen.lock().unwrap() = serde_json::from_slice(r.body().unwrap()).ok();
                true
            })
            .with_header("Content-Type", "application/json")
            .with_body(r#"{"choices":[{"message":{"content":"{\"ok\":true}","tool_calls":[{"id":"c1","type":"function","function":{"name":"book","arguments":"{}"}}]}}],"usage":{"prompt_tokens":12,"completion_tokens":3}}"#)
            .create();

        let c = OpenAI::new(server.url(), "k");
        let resp = c
            .chat(&Request {
                messages: vec![
                    Message::new("system", "sys"),
                    Message {
                        role: "assistant".into(),
                        tool_calls: vec![ToolCall {
                            id: "t1".into(),
                            name: "book".into(),
                            arguments: r#"{"a":1}"#.into(),
                        }],
                        ..Default::default()
                    },
                    Message {
                        role: "tool".into(),
                        tool_call_id: "t1".into(),
                        content: r#"{"ok":true}"#.into(),
                        ..Default::default()
                    },
                ],
                tools: vec![ToolSchema {
                    name: "book".into(),
                    description: "d".into(),
                    parameters: json!({"type": "object"}),
                }],
                temperature: None,
                response_format: Some(ResponseFormat {
                    name: "verdict".into(),
                    schema: json!({"type": "object"}),
                    strict: true,
                    ..Default::default()
                }),
            })
            .unwrap();
        mock.assert();

        let got = got.lock().unwrap().clone().expect("request body captured");
        assert_eq!(got["model"], MODEL);
        let msgs = got["messages"].as_array().unwrap();
        assert!(
            msgs[1]["content"].is_null() && msgs[1]["tool_calls"][0]["type"] == "function",
            "assistant tool call message wrong: {}",
            msgs[1]
        );
        assert!(
            msgs[2]["tool_call_id"] == "t1" && msgs[2]["content"] == r#"{"ok":true}"#,
            "tool message wrong: {}",
            msgs[2]
        );
        let tool = &got["tools"][0];
        assert!(tool["type"] == "function" && tool["function"]["name"] == "book", "tool wrong: {tool}");
        assert_eq!(got["response_format"]["type"], "json_schema");
        let js = &got["response_format"]["json_schema"];
        assert!(js["name"] == "verdict" && js["strict"] == true, "json_schema wrong: {js}");

        assert!(
            resp.content == r#"{"ok":true}"# && resp.tool_calls.len() == 1 && resp.tool_calls[0].name == "book",
            "response parsed wrong: {resp:?}"
        );
        assert!(resp.usage.prompt_tokens == 12 && resp.usage.completion_tokens == 3, "usage wrong: {:?}", resp.usage);
    }

    /// A 4xx with an error body is reported with the endpoint's message and
    /// not retried.
    #[test]
    fn openai_error_body() {
        let mut server = mockito::Server::new();
        let mock = server
            .mock("POST", "/chat/completions")
            .with_status(400)
            .with_body(r#"{"error":{"message":"bad request","type":"invalid_request_error"}}"#)
            .expect(1)
            .create();
        let c = OpenAI::new(server.url(), "k");
        let err = c.chat(&Request::default()).unwrap_err();
        mock.assert();
        assert_eq!(err.to_string(), "llm: status 400: bad request");
    }

    /// Rate limits are retried, honouring Retry-After, and the retry is not
    /// visible to the caller.
    #[test]
    fn openai_retries_rate_limits() {
        let mut server = mockito::Server::new();
        let limited = server
            .mock("POST", "/chat/completions")
            .with_status(429)
            .with_header("Retry-After", "1")
            .with_body("slow down")
            .expect(1)
            .create();
        let ok = server
            .mock("POST", "/chat/completions")
            .with_body(
                r#"{"choices":[{"message":{"content":"fine"}}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#,
            )
            .expect(1)
            .create();
        let c = OpenAI::new(server.url(), "k");
        let started = Instant::now();
        let resp = c.chat(&Request::default()).unwrap();
        limited.assert();
        ok.assert();
        assert_eq!(resp.content, "fine");
        assert!(started.elapsed() >= Duration::from_secs(1), "Retry-After was not honoured");
    }

    /// A server that keeps failing is given up on after three attempts, and
    /// the last error is what the caller sees.
    #[test]
    fn openai_gives_up_after_three_attempts() {
        let mut server = mockito::Server::new();
        // Retry-After keeps the two waits at a second each instead of 2s + 4s
        let down = server
            .mock("POST", "/chat/completions")
            .with_status(503)
            .with_header("Retry-After", "1")
            .with_body("nope")
            .expect(3)
            .create();
        let c = OpenAI::new(server.url(), "k");
        let err = c.chat(&Request::default()).unwrap_err();
        down.assert();
        assert!(matches!(err, Error::Retryable { .. }), "{err}");
        assert_eq!(err.to_string(), "llm: status 503: nope");
    }

    /// A recorder counts every call, the tokens of the ones that succeeded and
    /// the errors, and writes one numbered file per call when given a directory.
    #[test]
    fn recorder_counts_and_traces() {
        struct Flaky(Mutex<u32>);
        impl Client for Flaky {
            fn chat(&self, _: &Request) -> Result<Response, Error> {
                let mut n = self.0.lock().unwrap();
                *n += 1;
                if *n == 1 {
                    return Err(Error::Status("llm: status 400: nope".into()));
                }
                Ok(Response { usage: Usage { prompt_tokens: 5, completion_tokens: 2 }, ..Response::text("hi") })
            }
        }
        let dir = std::env::temp_dir().join(format!("pirtis-recorder-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let flaky = Flaky(Mutex::new(0));
        let rec = Recorder::new(&flaky, Some(dir.clone()));
        assert_eq!(rec.chat(&Request::default()).unwrap_err().to_string(), "llm: status 400: nope");
        assert_eq!(rec.chat(&Request::default()).unwrap().content, "hi");
        let s = rec.stats();
        assert_eq!((s.calls, s.errors, s.prompt_tokens, s.completion_tokens), (2, 1, 5, 2));

        let first: Value = serde_json::from_slice(&std::fs::read(dir.join("001.json")).unwrap()).unwrap();
        assert_eq!(first["error"], "llm: status 400: nope");
        assert!(first.get("response").is_none());
        let second: Value = serde_json::from_slice(&std::fs::read(dir.join("002.json")).unwrap()).unwrap();
        assert_eq!(second["response"]["content"], "hi");
        assert!(second["at"].is_string() && second["duration_ms"].is_number() && second["request"].is_object());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn stats_serialize_duration_as_ns() {
        let s = Stats { duration: Duration::from_millis(3), ..Default::default() };
        let v = serde_json::to_value(s).unwrap();
        assert_eq!(v["duration_ns"], 3_000_000);
        assert!(v.get("duration").is_none());
    }
}
