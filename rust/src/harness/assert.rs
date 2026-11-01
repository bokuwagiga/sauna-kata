use std::collections::{HashMap, HashSet};
use std::fmt;

use serde_json::Value;

use super::{BookingExpect, Expect};
use crate::api;
use crate::transcript::Effect;

/// World is everything the harness knows after a run.
#[derive(Debug, Clone, Copy)]
pub struct World<'a> {
    /// from the last turn
    pub effects: &'a [Effect],
    /// every API call across all turns
    pub calls: &'a [api::Call],
    /// bookings when the run started
    pub before: &'a [api::Booking],
    /// bookings when it ended
    pub after: &'a [api::Booking],
    /// customer ids identified in the transcript before the run
    pub identified: &'a HashSet<String>,
}

/// Checks the fixture's expectations and the invariants against the world and
/// returns one line per failure. Empty means the run passed.
///
/// Invariants, checked for every fixture:
///
/// - No API call returned 4xx or 5xx, unless the failure was injected. A 422
///   means the receptionist offered something the rules do not allow; a 400
///   means the request itself was malformed.
/// - Customer data is only touched for an identified caller. customers.get,
///   bookings.create, bookings.update and a bookings.list naming a customer
///   all require that customer to have been identified, in the transcript or
///   by a successful customers.identify earlier in this run.
pub fn evaluate(e: &Expect, w: &World<'_>) -> Vec<String> {
    let mut fails = Vec::new();

    // action
    let types: Vec<String> = w.effects.iter().map(|ef| ef.type_().to_string()).collect();
    let want = match e.action.as_str() {
        "say" => "speech",
        "wait" => "silence",
        "transfer" => "transfer",
        "end_call" => "hangup",
        _ => "",
    };
    if !types.iter().any(|t| t == want) {
        fails.push(format!("action: wanted {}, got effects [{}]", e.action, types.join(" ")));
    }
    if e.action == "wait" && types.iter().any(|t| t == "speech") {
        fails.push(format!("action: wanted wait, but there was speech: {:?}", first_speech(w.effects)));
    }

    // bookings diff
    let changes = diff(w.before, w.after);
    let mut matched = vec![false; changes.len()];
    for want in &e.bookings {
        let mut found = false;
        for (i, c) in changes.iter().enumerate() {
            if !matched[i] && want.matches(c) {
                matched[i] = true;
                found = true;
                break;
            }
        }
        if !found {
            fails.push(format!("bookings: expected {}, got {}", want, describe_changes(&changes)));
        }
    }
    for (i, c) in changes.iter().enumerate() {
        if !matched[i] {
            fails.push(format!("bookings: unexpected {c}"));
        }
    }

    // invariants
    let mut identified: HashSet<String> = w.identified.clone();
    let by_id: HashMap<&str, &api::Booking> = w.after.iter().map(|b| (b.id.as_str(), b)).collect();
    for c in w.calls {
        if c.status >= 400 && !c.injected {
            fails.push(format!(
                "api: {} returned {} {}: {} ({})",
                c.method,
                c.status,
                c.code,
                c.detail,
                status_meaning(c.status)
            ));
        }
        match c.method.as_str() {
            api::METHOD_CUSTOMERS_IDENTIFY => {
                if let Ok(r) = serde_json::from_value::<api::IdentifyResult>(c.result.clone()) {
                    if r.ok {
                        identified.insert(r.customer_id);
                    }
                }
            }
            api::METHOD_CUSTOMERS_GET => {
                let id = plain(&c.args);
                if !identified.contains(&id) {
                    fails.push(format!("api: customers.get({id}) before that caller was identified"));
                }
            }
            api::METHOD_BOOKINGS_CREATE => {
                if let Ok(req) = serde_json::from_value::<api::BookingRequest>(c.args.clone()) {
                    if !identified.contains(&req.customer_id) {
                        fails.push(format!(
                            "api: bookings.create for {} before that caller was identified",
                            req.customer_id
                        ));
                    }
                }
            }
            api::METHOD_BOOKINGS_UPDATE => {
                if let Some(args) = c.args.as_object() {
                    let id = plain(args.get("id").unwrap_or(&Value::Null));
                    if let Some(b) = by_id.get(id.as_str()) {
                        if !identified.contains(&b.request.customer_id) {
                            fails.push(format!(
                                "api: bookings.update({}) for {} before that caller was identified",
                                id, b.request.customer_id
                            ));
                        }
                    }
                }
            }
            api::METHOD_BOOKINGS_LIST => {
                if let Ok(q) = serde_json::from_value::<api::BookingQuery>(c.args.clone()) {
                    if !q.customer_id.is_empty() && !identified.contains(&q.customer_id) {
                        fails.push(format!(
                            "api: bookings.list for {} before that caller was identified",
                            q.customer_id
                        ));
                    }
                }
            }
            _ => {}
        }
    }

    fails
}

/// A JSON string without its quotes; anything else as JSON text.
fn plain(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn status_meaning(status: u16) -> &'static str {
    match status {
        400 => "malformed request",
        404 => "no such record",
        409 => "conflict: dates taken or stale version",
        422 => "rule violation: offered something the rules do not allow",
        _ => "server error",
    }
}

/// ChangeKind says whether a booking is new since the run started or an
/// existing one that changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Created,
    Changed,
}

impl fmt::Display for ChangeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ChangeKind::Created => "created",
            ChangeKind::Changed => "changed",
        })
    }
}

/// Change is one created or changed booking.
#[derive(Debug, Clone)]
pub struct Change {
    pub kind: ChangeKind,
    pub b: api::Booking,
}

impl fmt::Display for Change {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.kind, describe(&self.b))
    }
}

fn describe(b: &api::Booking) -> String {
    let r = &b.request;
    format!(
        "{}({} {} {}..{} x{} {} v{})",
        b.id, r.customer_id, r.unit_id, r.start, r.end, r.people, b.status, b.version
    )
}

fn describe_changes(cs: &[Change]) -> String {
    if cs.is_empty() {
        return "no changes".to_string();
    }
    cs.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(", ")
}

pub fn diff(before: &[api::Booking], after: &[api::Booking]) -> Vec<Change> {
    let prev: HashMap<&str, &api::Booking> = before.iter().map(|b| (b.id.as_str(), b)).collect();
    let mut out = Vec::new();
    for b in after {
        match prev.get(b.id.as_str()) {
            None => out.push(Change { kind: ChangeKind::Created, b: b.clone() }),
            Some(p) if *p != b => out.push(Change { kind: ChangeKind::Changed, b: b.clone() }),
            _ => {}
        }
    }
    out
}

impl BookingExpect {
    pub fn matches(&self, c: &Change) -> bool {
        let b = &c.b;
        if !self.id.is_empty() && (self.id != b.id || c.kind != ChangeKind::Changed) {
            return false;
        }
        if self.id.is_empty() && c.kind != ChangeKind::Created {
            return false;
        }
        if !self.customer.is_empty() && self.customer != b.request.customer_id {
            return false;
        }
        if !self.unit.is_empty() && self.unit != b.request.unit_id {
            return false;
        }
        if !self.start.is_empty() && self.start != b.request.start {
            return false;
        }
        if !self.end.is_empty() && self.end != b.request.end {
            return false;
        }
        if let Some(p) = self.people {
            if p != b.request.people {
                return false;
            }
        }
        let status = if self.status.is_empty() { api::STATUS_BOOKED } else { self.status.as_str() };
        status == b.status
    }
}

impl fmt::Display for BookingExpect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts = Vec::new();
        if !self.id.is_empty() {
            parts.push(format!("id={}", self.id));
        }
        if !self.customer.is_empty() {
            parts.push(format!("customer={}", self.customer));
        }
        if !self.unit.is_empty() {
            parts.push(format!("unit={}", self.unit));
        }
        if !self.start.is_empty() {
            parts.push(format!("start={}", self.start));
        }
        if !self.end.is_empty() {
            parts.push(format!("end={}", self.end));
        }
        if let Some(p) = self.people {
            parts.push(format!("people={p}"));
        }
        if !self.status.is_empty() {
            parts.push(format!("status={}", self.status));
        }
        let kind = if self.id.is_empty() { ChangeKind::Created } else { ChangeKind::Changed };
        write!(f, "{}{{{}}}", kind, parts.join(" "))
    }
}

fn first_speech(effects: &[Effect]) -> String {
    effects
        .iter()
        .find(|ef| ef.type_() == "speech")
        .map(|ef| plain(ef.get("text").unwrap_or(&Value::Null)))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::api::{
        Booking, BookingRequest, Call, Site, CODE_CAPACITY_EXCEEDED, METHOD_BOOKINGS_CREATE, STATUS_CANCELLED,
    };

    fn booking(id: &str, customer: &str) -> Booking {
        Booking {
            request: BookingRequest {
                customer_id: customer.into(),
                unit_id: "unit_mazoji".into(),
                start: "2026-10-06".into(),
                end: "2026-10-07".into(),
                people: 4,
                site: Site { power_available: true },
                tow_capacity_kg: 2000,
            },
            id: id.into(),
            version: 1,
            status: "booked".into(),
            total: 240,
        }
    }

    fn speech(text: &str) -> Effect {
        Effect::new("speech").with("text", text)
    }

    fn expect(action: &str, bookings: Vec<BookingExpect>) -> Expect {
        Expect { action: action.into(), bookings }
    }

    /// A change to an existing booking is matched by id, with the status
    /// spelled out; left unexpected, it is reported with the booking's fields.
    #[test]
    fn changed_booking_matches_by_id() {
        let before = vec![booking("bk_1", "cus_07")];
        let mut after = before.clone();
        after[0].status = STATUS_CANCELLED.into();
        after[0].version = 2;
        let identified = HashSet::from(["cus_07".to_string()]);
        let effects = [speech("Cancelled.")];
        let w = World { effects: &effects, calls: &[], before: &before, after: &after, identified: &identified };

        let want = BookingExpect { id: "bk_1".into(), status: STATUS_CANCELLED.into(), ..Default::default() };
        assert_eq!(evaluate(&expect("say", vec![want]), &w), Vec::<String>::new());
        assert_eq!(
            evaluate(&expect("say", vec![]), &w),
            vec!["bookings: unexpected changed bk_1(cus_07 unit_mazoji 2026-10-06..2026-10-07 x4 cancelled v2)"]
        );
        let wrong = BookingExpect { id: "bk_1".into(), ..Default::default() }; // status defaults to booked
        let fails = evaluate(&expect("say", vec![wrong]), &w);
        assert_eq!(fails.len(), 2, "{fails:?}");
        assert_eq!(fails[0], "bookings: expected changed{id=bk_1}, got changed bk_1(cus_07 unit_mazoji 2026-10-06..2026-10-07 x4 cancelled v2)");
    }

    /// wait means silence and no speech; the two lines name what happened.
    #[test]
    fn wait_with_speech_fails_twice() {
        let effects = [speech("Take your time!")];
        let w = World { effects: &effects, calls: &[], before: &[], after: &[], identified: &HashSet::new() };
        assert_eq!(
            evaluate(&expect("wait", vec![]), &w),
            vec![
                "action: wanted wait, got effects [speech]",
                "action: wanted wait, but there was speech: \"Take your time!\"",
            ]
        );
        let none =
            evaluate(&expect("say", vec![BookingExpect { unit: "unit_mazoji".into(), ..Default::default() }]), &w);
        assert_eq!(none, vec!["bookings: expected created{unit=unit_mazoji}, got no changes"]);
    }

    /// Injected failures are the fixture's doing; real ones are the agent's.
    #[test]
    fn injected_failures_are_allowed() {
        let mut call = Call {
            method: METHOD_BOOKINGS_CREATE.into(),
            status: 409,
            code: "UNAVAILABLE".into(),
            detail: "injected fault".into(),
            injected: true,
            ..Default::default()
        };
        let effects = [speech("ok")];
        let identified = HashSet::from(["cus_07".to_string()]);
        let calls = [call.clone()];
        let w = World { effects: &effects, calls: &calls, before: &[], after: &[], identified: &identified };
        assert!(evaluate(&expect("say", vec![]), &w).is_empty());

        call.injected = false;
        call.status = 422;
        call.code = CODE_CAPACITY_EXCEEDED.into();
        call.detail = "too many".into();
        let calls = [call];
        let w = World { calls: &calls, ..w };
        assert_eq!(
            evaluate(&expect("say", vec![]), &w),
            vec!["api: bookings.create returned 422 CAPACITY_EXCEEDED: too many (rule violation: offered something the rules do not allow)"]
        );
    }
}
