use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::Serialize;

use super::{evaluate, judge, Fixture, JudgeError, Verdict, World};
use crate::agent::{Handler, HandlerContext};
use crate::api::{self, Client as _};
use crate::llm;
use crate::transcript::{Action, Effect, Transcript};

/// DecideFn is the agent's entry point.
pub type DecideFn = Box<dyn Fn(&Transcript, &dyn api::Client, &dyn llm::Client) -> anyhow::Result<Vec<Action>>>;

/// Runner runs fixtures against one agent.
pub struct Runner {
    pub llm: Box<dyn llm::Client>,
    /// grades `judge` criteria; `None` means `llm`
    pub judge: Option<Box<dyn llm::Client>>,
    pub decide: DecideFn,
    pub handlers: HashMap<String, Handler>,
    /// where LLM calls and per-run results are written; `None` for nowhere
    pub trace_dir: Option<PathBuf>,
}

/// RunResult is one execution of one fixture: one call to `decide`, the
/// handlers, the checks.
#[derive(Debug, Default)]
pub struct RunResult {
    pub index: usize,
    pub actions: Vec<Action>,
    pub effects: Vec<Effect>,
    pub failures: Vec<String>,
    pub verdicts: Vec<Verdict>,
    pub err: Option<anyhow::Error>,
    /// the agent's calls only; the judge is counted separately
    pub stats: llm::Stats,
    pub wall: Duration,
    pub calls: Vec<api::Call>,
}

impl RunResult {
    /// True when the run produced no failures and no error.
    pub fn passed(&self) -> bool {
        self.err.is_none() && self.failures.is_empty()
    }
}

/// FixtureResult is every run of one fixture.
#[derive(Debug)]
pub struct FixtureResult {
    pub fixture: Fixture,
    pub runs: Vec<RunResult>,
    /// runs that must pass
    pub threshold: usize,
}

impl FixtureResult {
    /// How many runs passed.
    pub fn pass_count(&self) -> usize {
        self.runs.iter().filter(|r| r.passed()).count()
    }

    /// True when at least `threshold` runs passed.
    pub fn passed(&self) -> bool {
        self.pass_count() >= self.threshold
    }
}

impl Runner {
    /// Executes one fixture `runs` times; `need` of them must pass.
    pub fn run(&self, f: &Fixture, runs: usize, need: usize) -> FixtureResult {
        let runs = if runs == 0 { 1 } else { runs };
        let need = if need == 0 || need > runs { runs } else { need };
        let mut out = FixtureResult { fixture: f.clone(), runs: Vec::new(), threshold: need };
        for i in 1..=runs {
            out.runs.push(self.run_once(f, i));
        }
        out
    }

    fn run_once(&self, f: &Fixture, i: usize) -> RunResult {
        let mut res = RunResult { index: i, ..RunResult::default() };

        let mut seed = match api::load_seed(f.seed_path()) {
            Ok(s) => s,
            Err(e) => {
                res.err = Some(anyhow::anyhow!("seed: {e}"));
                return res;
            }
        };
        for sb in &f.setup.bookings {
            seed.bookings.push(sb.booking());
        }
        let mem = api::Memory::new(seed, f.faults.clone());
        let before = mem.bookings();

        let tr = &f.transcript;

        let dir = self.trace_dir.as_ref().map(|d| d.join(&f.id).join(format!("run-{i}")));
        let rec = llm::Recorder::new(&*self.llm, dir.clone());

        let start = Instant::now();
        let actions = (self.decide)(tr, &mem, &rec);
        res.wall = start.elapsed();
        res.stats = rec.stats();
        match actions {
            Ok(actions) => res.actions = actions,
            Err(e) => {
                res.err = Some(anyhow::anyhow!("decide: {e}"));
                res.calls = mem.log();
                self.write_result(dir.as_deref(), f, &res, &mem.bookings());
                return res;
            }
        }

        let hc = HandlerContext { api: &mem, transcript: tr };
        for a in &res.actions {
            let Some(h) = self.handlers.get(a.type_()) else {
                res.failures.push(format!("no handler for action type {:?}", a.type_()));
                continue;
            };
            match h(a, &hc) {
                Ok(ef) => res.effects.push(ef),
                Err(e) => res.failures.push(format!("handler {:?}: {}", a.type_(), e)),
            }
        }

        res.calls = mem.log();
        let after = mem.bookings();
        let identified = identified_in(tr);
        let world =
            World { effects: &res.effects, calls: &res.calls, before: &before, after: &after, identified: &identified };
        res.failures.extend(evaluate(&f.expect, &world));

        if !f.judge.is_empty() {
            let judge_client: &dyn llm::Client = match &self.judge {
                Some(j) => &**j,
                None => &*self.llm,
            };
            let jdir = dir.as_ref().map(|d| d.join("judge"));
            let rec = llm::Recorder::new(judge_client, jdir);
            let verdicts = match judge(&rec, tr, &res.effects, &f.judge) {
                Ok(v) => v,
                Err(JudgeError::Missing { verdicts, missing, total }) => {
                    res.failures.push(format!("judge: verdict missing for {missing} of {total} criteria"));
                    verdicts
                }
                Err(e) => {
                    res.failures.push(format!("judge: {e}"));
                    Vec::new()
                }
            };
            for v in &verdicts {
                if !v.pass {
                    res.failures.push(format!("judge: {}: {}", v.criterion, v.reason));
                }
            }
            res.verdicts = verdicts;
        }

        self.write_result(dir.as_deref(), f, &res, &after);
        res
    }

    fn write_result(&self, dir: Option<&std::path::Path>, f: &Fixture, res: &RunResult, bookings: &[api::Booking]) {
        let Some(dir) = dir else { return };
        fn slice_empty<T>(s: &&[T]) -> bool {
            s.is_empty()
        }
        #[derive(Serialize)]
        struct Out<'a> {
            fixture: &'a str,
            run: usize,
            passed: bool,
            #[serde(skip_serializing_if = "Option::is_none")]
            error: Option<String>,
            #[serde(skip_serializing_if = "slice_empty")]
            failures: &'a [String],
            #[serde(skip_serializing_if = "slice_empty")]
            verdicts: &'a [Verdict],
            actions: &'a [Action],
            effects: &'a [Effect],
            api_calls: &'a [api::Call],
            bookings: &'a [api::Booking],
            llm: llm::Stats,
            wall_ms: u128,
        }
        let o = Out {
            fixture: &f.id,
            run: res.index,
            passed: res.passed(),
            error: res.err.as_ref().map(|e| e.to_string()),
            failures: &res.failures,
            verdicts: &res.verdicts,
            actions: &res.actions,
            effects: &res.effects,
            api_calls: &res.calls,
            bookings,
            llm: res.stats,
            wall_ms: res.wall.as_millis(),
        };
        let Ok(b) = serde_json::to_vec_pretty(&o) else {
            return;
        };
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(dir.join("result.json"), b);
    }
}

/// Returns the customer ids a successful customers.identify in the transcript
/// returned.
pub fn identified_in(tr: &Transcript) -> HashSet<String> {
    let mut names: HashMap<&str, &str> = HashMap::new();
    let mut out = HashSet::new();
    for it in &tr.items {
        match it.type_.as_str() {
            "function_call" => {
                names.insert(&it.call_id, &it.name);
            }
            "function_call_output" => {
                if names.get(it.call_id.as_str()).copied() != Some(api::METHOD_CUSTOMERS_IDENTIFY) {
                    continue;
                }
                if let Ok(r) = serde_json::from_str::<api::IdentifyResult>(&it.output) {
                    if r.ok && !r.customer_id.is_empty() {
                        out.insert(r.customer_id);
                    }
                }
            }
            _ => {}
        }
    }
    out
}

// These tests drive the harness with stub agents, never with the shipped
// agent, so a candidate's rewrite of the agent module cannot break them.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::load_fixtures;
    use crate::llm::{Fake, Response};
    use crate::transcript;

    const FIXTURES_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../fixtures");

    fn load(id: &str) -> Fixture {
        let fs = load_fixtures(FIXTURES_DIR).unwrap();
        fs.into_iter().find(|f| f.id == id).unwrap_or_else(|| panic!("no fixture {id:?}"))
    }

    /// The effect contract the harness checks against: say -> speech,
    /// wait -> silence, transfer -> transfer, end_call -> hangup.
    fn handlers() -> HashMap<String, Handler> {
        let mut h: HashMap<String, Handler> = HashMap::new();
        h.insert("say".into(), Box::new(|a, _| Ok(Effect::new("speech").with("text", a.field("text")))));
        h.insert("wait".into(), Box::new(|a, _| Ok(Effect::new("silence").with("seconds", a.field("seconds")))));
        h.insert("transfer".into(), Box::new(|a, _| Ok(Effect::new("transfer").with("to", a.field("to")))));
        h.insert("end_call".into(), Box::new(|_, _| Ok(Effect::new("hangup"))));
        h
    }

    /// Passes every criterion.
    fn pass_judge() -> Box<dyn llm::Client> {
        Box::new(Fake::with_default(
            vec![],
            Response::text(
                r#"{"criterion_1":{"reason":"ok","pass":true},"criterion_2":{"reason":"ok","pass":true},"criterion_3":{"reason":"ok","pass":true},"criterion_4":{"reason":"ok","pass":true}}"#,
            ),
        ))
    }

    fn runner(decide: DecideFn) -> Runner {
        Runner {
            llm: Box::new(Fake::new(vec![])),
            judge: Some(pass_judge()),
            decide,
            handlers: handlers(),
            trace_dir: None,
        }
    }

    fn says(text: &'static str) -> DecideFn {
        Box::new(move |_, _, _| Ok(vec![transcript::say(text, "neutral", 1.0)]))
    }

    #[test]
    fn all_fixtures_load() {
        let fs = load_fixtures(FIXTURES_DIR).unwrap();
        assert!(fs.len() >= 10, "only {} fixtures", fs.len());
        for f in &fs {
            assert!(f.seed_path().exists(), "{}: seed missing at {}", f.id, f.seed_path().display());
            let last = f.transcript.items.last().unwrap();
            assert!(
                last.type_ == "message" && last.end > 0.0 && f.transcript.now >= last.end,
                "{}: last item should be a timed message before now, got {:?} (now {})",
                f.id,
                last,
                f.transcript.now
            );
        }
    }

    /// An agent that books what the caller confirmed passes the happy path:
    /// the diff sees the booking, the invariants hold, no model needed.
    #[test]
    fn happy_path_booking() {
        let book: DecideFn = Box::new(|_, a, _| {
            a.create_booking(&api::BookingRequest {
                customer_id: "cus_07".into(),
                unit_id: "unit_mazoji".into(),
                start: "2026-10-06".into(),
                end: "2026-10-07".into(),
                people: 4,
                site: api::Site { power_available: true },
                tow_capacity_kg: 2000,
            })?;
            Ok(vec![transcript::say("Done, see you Tuesday.", "warm", 1.0)])
        });
        let res = runner(book).run(&load("happy-path"), 1, 1);
        let r = &res.runs[0];
        assert!(res.passed(), "happy path failed: {:?} (err {:?})", r.failures, r.err);
        assert_eq!(r.stats.calls, 0, "no model was used");
        assert_eq!(res.threshold, 1, "one run means a threshold of one");
    }

    /// The interim fixture wants silence: speech fails it, a wait passes it.
    #[test]
    fn interim_wants_wait() {
        let res = runner(says("Take your time!")).run(&load("interim-speech"), 1, 1);
        assert!(!res.passed(), "speech must not pass interim-speech");
        assert!(
            res.runs[0].failures.join("\n").contains("wanted wait"),
            "failure should say wait was wanted: {:?}",
            res.runs[0].failures
        );
        let wait: DecideFn = Box::new(|_, _, _| Ok(vec![transcript::wait(1.5)]));
        let res = runner(wait).run(&load("interim-speech"), 1, 1);
        assert!(res.passed(), "wait should pass: {:?}", res.runs[0].failures);
    }

    /// A 422 from the API is a failure even if the reply talks its way out,
    /// and the failure line says what it was.
    #[test]
    fn rule_violation_is_caught() {
        let wrong: DecideFn = Box::new(|_, a, _| {
            let _ = a.create_booking(&api::BookingRequest {
                customer_id: "cus_19".into(),
                unit_id: "unit_mazoji".into(),
                start: "2026-10-03".into(),
                end: "2026-10-04".into(),
                people: 7,
                site: api::Site { power_available: true },
                tow_capacity_kg: 3500,
            });
            Ok(vec![transcript::say("Sorry, that one is too small. Didžioji then?", "neutral", 1.0)])
        });
        let res = runner(wrong).run(&load("capacity"), 1, 1);
        assert!(!res.passed(), "a CAPACITY_EXCEEDED in the log must fail the fixture");
        let joined = res.runs[0].failures.join("\n");
        assert!(
            joined.contains("CAPACITY_EXCEEDED") && joined.contains("rule violation"),
            "failure should name the code and the meaning: {:?}",
            res.runs[0].failures
        );
    }

    /// A booking nobody asked for fails a fixture that expects no change.
    #[test]
    fn unexpected_booking_is_caught() {
        let eager: DecideFn = Box::new(|_, a, _| {
            a.create_booking(&api::BookingRequest {
                customer_id: "cus_12".into(),
                unit_id: "unit_didzioji".into(),
                start: "2026-10-10".into(),
                end: "2026-10-11".into(),
                people: 6,
                site: api::Site { power_available: true },
                tow_capacity_kg: 2500,
            })?;
            Ok(vec![transcript::say("Booked the big one.", "neutral", 1.0)])
        });
        let res = runner(eager).run(&load("ask-tow-capacity"), 1, 1);
        assert!(!res.passed(), "guessing a towing capacity and booking must fail");
        assert!(
            res.runs[0].failures.join("\n").contains("unexpected created"),
            "failure should report the unexpected booking: {:?}",
            res.runs[0].failures
        );
    }

    /// Reading another customer's record is a failure; reading the caller's
    /// is fine. The silence fixture has the caller identified and expects
    /// speech and no change, so only the invariant decides.
    #[test]
    fn identity_invariant() {
        let leak: DecideFn = Box::new(|_, a, _| {
            let _ = a.get_customer("cus_31");
            Ok(vec![transcript::say("Still there?", "neutral", 1.0)])
        });
        let res = runner(leak).run(&load("silence"), 1, 1);
        assert!(!res.passed(), "customers.get for someone else must fail");
        let this: DecideFn = Box::new(|_, a, _| {
            let _ = a.get_customer("cus_12");
            Ok(vec![transcript::say("Still there?", "neutral", 1.0)])
        });
        let res = runner(this).run(&load("silence"), 1, 1);
        assert!(res.passed(), "customers.get for the identified caller must pass: {:?}", res.runs[0].failures);
    }

    /// A failing verdict fails the run with the reason on the line, and judge
    /// calls do not count against the agent.
    #[test]
    fn judge_failure_is_reported() {
        let fail_judge = Fake::with_default(
            vec![],
            Response::text(
                r#"{"criterion_1":{"reason":"it answered the question instead of asking for a name","pass":false}}"#,
            ),
        );
        let mut r = runner(says("Yes, Wednesday is still on."));
        r.judge = Some(Box::new(fail_judge));
        let mut f = load("identify-before-reading");
        f.judge = vec!["The reply asks the caller for their name.".to_string()];
        let res = r.run(&f, 1, 1);
        assert!(!res.passed(), "a failing verdict must fail the run");
        let found = res.runs[0].failures.iter().any(|fl| fl.starts_with("judge: ") && fl.contains("instead of asking"));
        assert!(found, "want a judge failure with the reason, got {:?}", res.runs[0].failures);
        assert_eq!(res.runs[0].stats.calls, 0, "judge calls must not count against the agent");
    }

    /// An action type nobody registered a handler for fails the run, and a
    /// handler someone adds makes it pass.
    #[test]
    fn unknown_action_needs_handler() {
        let decide: DecideFn = Box::new(|_, _, _| {
            Ok(vec![Action::new("hold_tone").with("seconds", 2.0), transcript::say("still there?", "warm", 1.0)])
        });
        let mut r = runner(decide);
        let res = r.run(&load("silence"), 1, 1);
        assert!(!res.passed(), "hold_tone has no handler, run must fail");
        assert!(res.runs[0].failures.iter().any(|f| f.contains("no handler for action type \"hold_tone\"")));
        let mut with_hold = handlers();
        with_hold.insert("hold_tone".into(), Box::new(|_, _| Ok(Effect::new("hold_tone"))));
        r.handlers = with_hold;
        let res = r.run(&load("silence"), 1, 1);
        assert!(res.passed(), "with a handler the silence fixture should pass: {:?}", res.runs[0].failures);
    }

    /// A trace directory gets one JSON file per model call and a result.json
    /// per run with the harness's keys.
    #[test]
    fn trace_writes_result_json() {
        let tmp = std::env::temp_dir().join(format!("pirtis-trace-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let ask: DecideFn = Box::new(|_, _, c| {
            let r = c.chat(&llm::Request::default())?;
            Ok(vec![transcript::say(&r.content, "neutral", 1.0)])
        });
        let mut r = runner(ask);
        r.trace_dir = Some(tmp.clone());
        let res = r.run(&load("silence"), 2, 1);
        assert!(res.passed());
        let run2 = tmp.join("silence").join("run-2");
        assert!(run2.join("001.json").exists(), "model call not traced");
        assert!(run2.join("judge").join("001.json").exists(), "judge call not traced");
        let result: serde_json::Value =
            serde_json::from_slice(&std::fs::read(run2.join("result.json")).unwrap()).unwrap();
        assert_eq!(result["fixture"], "silence");
        assert_eq!(result["run"], 2);
        assert_eq!(result["passed"], true);
        assert_eq!(result["llm"]["calls"], 1);
        assert_eq!(result["effects"][0]["type"], "speech");
        assert!(result["api_calls"].is_array() && result["bookings"].as_array().unwrap().len() == 3);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
