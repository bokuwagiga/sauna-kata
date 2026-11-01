//! Loads fixtures, seeds the API, calls the agent with a transcript, runs the
//! handlers and checks the world afterwards.

mod assert;
mod fixture;
mod judge;
mod run;

pub use assert::*;
pub use fixture::*;
pub use judge::*;
pub use run::*;
