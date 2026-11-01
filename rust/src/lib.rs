//! Pirtis on Wheels: the phone receptionist for a mobile sauna rental.
//!
//! - [`transcript`]: what the harness feeds the agent and what the agent returns.
//! - [`api`]: the rental system, with an in-memory database. Ours, do not change.
//! - [`llm`]: a chat client and a small tool loop. Ours, do not change.
//! - [`harness`]: the test runner. Ours, do not change.
//! - [`agent`]: the receptionist. Yours.

pub mod agent;
pub mod api;
pub mod harness;
pub mod llm;
pub mod transcript;
