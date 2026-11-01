use std::sync::Mutex;

use super::{Client, Error, Request, Response};

/// Fake answers from a script, then from `default`. It is for tests: the
/// harness's own, and yours, for driving your loop through a scripted model
/// without spending tokens. It does not understand anything.
pub struct Fake {
    pub script: Vec<Response>,
    pub default: Response,
    i: Mutex<usize>,
}

impl Fake {
    /// Returns a Fake that replays `script` in order, then keeps returning a
    /// fixed line of text with no tool calls.
    pub fn new(script: Vec<Response>) -> Fake {
        Fake::with_default(script, Response::text("This is the fake model."))
    }

    /// Returns a Fake that replays `script` in order, then `default` forever.
    pub fn with_default(script: Vec<Response>, default: Response) -> Fake {
        Fake { script, default, i: Mutex::new(0) }
    }

    /// How many times the model has been called.
    pub fn calls(&self) -> usize {
        *self.i.lock().unwrap()
    }
}

impl Client for Fake {
    fn chat(&self, _req: &Request) -> Result<Response, Error> {
        let mut i = self.i.lock().unwrap();
        let r = self.script.get(*i).cloned().unwrap_or_else(|| self.default.clone());
        *i += 1;
        Ok(r)
    }
}
