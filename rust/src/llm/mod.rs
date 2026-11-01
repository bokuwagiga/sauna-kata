//! A non-streaming chat client in the OpenAI message format, plus a small
//! tool loop. A [`Recorder`] counts calls and tokens, and writes every request
//! and response to disk when the harness is run with `--trace`.

mod client;
mod fake;
mod tool_loop;

pub use client::*;
pub use fake::Fake;
pub use tool_loop::*;
