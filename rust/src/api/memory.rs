use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Mutex;

use chrono::{Datelike, Days, NaiveDate, Weekday};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::types::*;
use super::{
    Call, Client, Fault, METHOD_BOOKINGS_CREATE, METHOD_BOOKINGS_LIST, METHOD_BOOKINGS_UPDATE, METHOD_CUSTOMERS_GET,
    METHOD_CUSTOMERS_IDENTIFY, METHOD_UNITS_AVAILABILITY, METHOD_UNITS_LIST,
};

/// Seed is the database contents a fixture starts from.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Seed {
    pub units: Vec<Unit>,
    pub customers: Vec<Customer>,
    pub bookings: Vec<Booking>,
    pub blocked: Vec<Blocked>,
}

/// Reads a seed JSON file.
pub fn load_seed(path: impl AsRef<Path>) -> anyhow::Result<Seed> {
    let path = path.as_ref();
    let b = std::fs::read(path)?;
    let s = serde_json::from_slice(&b).map_err(|e| anyhow::anyhow!("{}: {}", path.display(), e))?;
    Ok(s)
}

/// Memory is the in-memory implementation of [`Client`].
pub struct Memory {
    state: Mutex<State>,
}

/// The tables, in insertion order, plus the call log.
struct State {
    units: IndexMap<String, Unit>,
    customers: IndexMap<String, Customer>,
    bookings: IndexMap<String, Booking>,
    blocked: Vec<Blocked>,

    faults: Vec<Fault>,
    counts: HashMap<String, i32>,
    calls: Vec<Call>,
    next_id: i32,
}

impl Memory {
    /// Builds a fresh database from a seed. Faults are optional.
    pub fn new(seed: Seed, faults: Vec<Fault>) -> Memory {
        Memory {
            state: Mutex::new(State {
                units: seed.units.into_iter().map(|u| (u.id.clone(), u)).collect(),
                customers: seed.customers.into_iter().map(|c| (c.id.clone(), c)).collect(),
                bookings: seed.bookings.into_iter().map(|b| (b.id.clone(), b)).collect(),
                blocked: seed.blocked,
                faults,
                counts: HashMap::new(),
                calls: Vec::new(),
                next_id: 100,
            }),
        }
    }

    /// Returns every booking, seed and new, in insertion order. The harness
    /// reads it after a run. It is not part of [`Client`] and not logged.
    pub fn bookings(&self) -> Vec<Booking> {
        self.state.lock().unwrap().bookings.values().cloned().collect()
    }
}

impl State {
    fn record<T: Serialize>(&mut self, method: &str, args: Value, res: &Result<T, Error>, injected: bool) {
        let mut c = Call { method: method.to_string(), status: 200, args, injected, ..Call::default() };
        match res {
            Err(e) => {
                c.status = e.status;
                c.code = e.code.clone();
                c.detail = e.detail.clone();
            }
            Ok(v) => c.result = serde_json::to_value(v).unwrap_or(Value::Null),
        }
        self.calls.push(c);
    }

    /// Returns the injected error for this call of `method`, if any.
    fn fault(&mut self, method: &str) -> Option<Error> {
        let n = self.counts.entry(method.to_string()).or_insert(0);
        *n += 1;
        let n = *n;
        let f = self.faults.iter().find(|f| f.on == method && f.nth == n)?;
        let code = if f.code.is_empty() {
            match f.status {
                409 => CODE_UNAVAILABLE,
                500 => "INTERNAL",
                _ => "FAULT",
            }
        } else {
            &f.code
        };
        Some(Error::new(f.status, code, "injected fault"))
    }

    fn identify(&self, phone: &str, name: &str) -> IdentifyResult {
        let want = digits(phone);
        let given = name_tokens(name);
        self.customers
            .values()
            .find(|c| digits(&c.phone) == want && names_match(&name_tokens(&c.name), &given))
            .map(|c| IdentifyResult { ok: true, customer_id: c.id.clone() })
            .unwrap_or_default()
    }

    fn availability(&self, q: &AvailabilityQuery) -> Result<Vec<UnitAvailability>, Error> {
        let (from, to) = parse_range(&q.from, &q.to)?;
        let mut out = Vec::new();
        for u in self.units.values() {
            if q.people.is_some_and(|p| u.capacity < p)
                || q.max_tow_kg.is_some_and(|kg| u.gross_weight_kg > kg)
                || (q.power_available == Some(false) && u.needs_power)
            {
                continue;
            }
            let mut busy: HashSet<NaiveDate> = HashSet::new();
            for b in self.bookings.values().filter(|b| b.request.unit_id == u.id && b.status == STATUS_BOOKED) {
                let Ok((bs, be)) = parse_range(&b.request.start, &b.request.end) else { continue };
                // the turnaround day on either side is busy too
                busy.extend(days(bs - Days::new(1), be + Days::new(1)));
            }
            for bl in self.blocked.iter().filter(|bl| bl.unit_id == u.id) {
                let Ok((bs, be)) = parse_range(&bl.start, &bl.end) else { continue };
                busy.extend(days(bs, be));
            }
            let mut free = Vec::new();
            let mut cur: Option<(NaiveDate, NaiveDate)> = None;
            for d in days(from, to) {
                if busy.contains(&d) {
                    if let Some((s, e)) = cur.take() {
                        free.push(DateRange { start: fmt_date(s), end: fmt_date(e) });
                    }
                } else {
                    match &mut cur {
                        None => cur = Some((d, d)),
                        Some((_, e)) => *e = d,
                    }
                }
            }
            if let Some((s, e)) = cur {
                free.push(DateRange { start: fmt_date(s), end: fmt_date(e) });
            }
            out.push(UnitAvailability { unit_id: u.id.clone(), free });
        }
        Ok(out)
    }

    fn list_bookings(&self, q: &BookingQuery) -> Result<Vec<Booking>, Error> {
        let from = if q.from.is_empty() { None } else { Some(parse_date(&q.from)?) };
        let to = if q.to.is_empty() { None } else { Some(parse_date(&q.to)?) };
        let mut out = Vec::new();
        for b in self.bookings.values() {
            if !q.customer_id.is_empty() && b.request.customer_id != q.customer_id {
                continue;
            }
            if !q.unit_id.is_empty() && b.request.unit_id != q.unit_id {
                continue;
            }
            if from.is_some() || to.is_some() {
                let Ok((bs, be)) = parse_range(&b.request.start, &b.request.end) else { continue };
                if from.is_some_and(|from| be < from) || to.is_some_and(|to| bs > to) {
                    continue;
                }
            }
            out.push(b.clone());
        }
        Ok(out)
    }

    fn create_booking(&mut self, req: &BookingRequest) -> Result<Booking, Error> {
        self.validate(req, "")?;
        self.next_id += 1;
        let b = Booking {
            request: req.clone(),
            id: format!("bk_{}", self.next_id),
            version: 1,
            status: STATUS_BOOKED.to_string(),
            total: self.total(req),
        };
        self.bookings.insert(b.id.clone(), b.clone());
        Ok(b)
    }

    fn update_booking(&mut self, id: &str, version: i32, patch: &BookingPatch) -> Result<Booking, Error> {
        let Some(cur) = self.bookings.get(id).cloned() else {
            return Err(Error::new(404, CODE_NOT_FOUND, format!("no booking {id}")));
        };
        if cur.version != version {
            return Err(Error::new(
                409,
                CODE_VERSION_MISMATCH,
                format!("booking {id} is at version {}, got {version}", cur.version),
            ));
        }
        let mut next = cur;
        if let Some(status) = &patch.status {
            if status != STATUS_CANCELLED {
                return Err(Error::new(400, CODE_BAD_STATUS, "status can only be set to cancelled"));
            }
            next.status = STATUS_CANCELLED.to_string();
            next.version += 1;
            self.bookings.insert(id.to_string(), next.clone());
            return Ok(next);
        }
        if let Some(s) = &patch.start {
            next.request.start = s.clone();
        }
        if let Some(e) = &patch.end {
            next.request.end = e.clone();
        }
        if let Some(p) = patch.people {
            next.request.people = p;
        }
        if let Some(t) = patch.tow_capacity_kg {
            next.request.tow_capacity_kg = t;
        }
        self.validate(&next.request, id)?;
        next.total = self.total(&next.request);
        next.version += 1;
        self.bookings.insert(id.to_string(), next.clone());
        Ok(next)
    }

    fn total(&self, req: &BookingRequest) -> i32 {
        let Some(u) = self.units.get(&req.unit_id) else { return 0 };
        match parse_range(&req.start, &req.end) {
            Ok((s, e)) => u.price_per_day * days_inclusive(s, e),
            Err(_) => 0,
        }
    }

    /// Every business rule. `exclude_id` skips one booking, for updates.
    fn validate(&self, req: &BookingRequest, exclude_id: &str) -> Result<(), Error> {
        if !self.customers.contains_key(&req.customer_id) {
            return Err(Error::new(400, CODE_CUSTOMER_UNKNOWN, format!("no customer {}", req.customer_id)));
        }
        let Some(u) = self.units.get(&req.unit_id) else {
            return Err(Error::new(400, CODE_UNIT_UNKNOWN, format!("no unit {}", req.unit_id)));
        };
        let (start, end) = parse_range(&req.start, &req.end)?;
        if req.people < 1 {
            return Err(Error::new(400, CODE_BAD_PEOPLE, "people must be at least 1"));
        }
        if req.people > u.capacity {
            return Err(Error::new(
                422,
                CODE_CAPACITY_EXCEEDED,
                format!("{} takes {} people, asked for {}", u.name, u.capacity, req.people),
            ));
        }
        if u.gross_weight_kg > req.tow_capacity_kg {
            return Err(Error::new(
                422,
                CODE_TOW_WEIGHT_EXCEEDED,
                format!("{} is {} kg, stated towing capacity is {} kg", u.name, u.gross_weight_kg, req.tow_capacity_kg),
            ));
        }
        if u.needs_power && !req.site.power_available {
            return Err(Error::new(
                422,
                CODE_POWER_REQUIRED,
                format!("{} is electric and the site has no mains power", u.name),
            ));
        }
        if touches_weekend(start, end) && days_inclusive(start, end) < 2 {
            return Err(Error::new(
                422,
                CODE_MIN_DAYS_WEEKEND,
                "rentals that touch a Saturday or Sunday are two days minimum",
            ));
        }
        let live = || self.bookings.values().filter(|b| b.id != exclude_id && b.status == STATUS_BOOKED);
        for b in live() {
            let Ok((bs, be)) = parse_range(&b.request.start, &b.request.end) else { continue };
            if b.request.customer_id == req.customer_id && overlaps(start, end, bs, be) {
                return Err(Error::new(
                    422,
                    CODE_OVERLAPS_EXISTING,
                    format!("customer already has {} from {} to {}", b.id, b.request.start, b.request.end),
                ));
            }
        }
        for b in live().filter(|b| b.request.unit_id == req.unit_id) {
            let Ok((bs, be)) = parse_range(&b.request.start, &b.request.end) else { continue };
            if overlaps(start, end, bs, be) {
                return Err(Error::new(
                    409,
                    CODE_UNAVAILABLE,
                    format!("{} is booked from {} to {}", u.name, b.request.start, b.request.end),
                ));
            }
            if adjacent(start, end, bs, be) {
                return Err(Error::new(
                    422,
                    CODE_TURNAROUND_CONFLICT,
                    format!(
                        "{} needs a clear day after the booking ending {} or before the one starting {}",
                        u.name, b.request.end, b.request.start
                    ),
                ));
            }
        }
        for bl in self.blocked.iter().filter(|bl| bl.unit_id == req.unit_id) {
            let Ok((bs, be)) = parse_range(&bl.start, &bl.end) else { continue };
            if overlaps(start, end, bs, be) {
                return Err(Error::new(
                    409,
                    CODE_UNAVAILABLE,
                    format!("{} is out for {} from {} to {}", u.name, bl.reason, bl.start, bl.end),
                ));
            }
        }
        Ok(())
    }
}

impl Client for Memory {
    fn identify_customer(&self, phone: &str, name: &str) -> Result<IdentifyResult, Error> {
        let mut s = self.state.lock().unwrap();
        let args = json!({"phone": phone, "name": name});
        let (res, injected) = match s.fault(METHOD_CUSTOMERS_IDENTIFY) {
            Some(e) => (Err(e), true),
            None => (Ok(s.identify(phone, name)), false),
        };
        s.record(METHOD_CUSTOMERS_IDENTIFY, args, &res, injected);
        res
    }

    fn get_customer(&self, id: &str) -> Result<Customer, Error> {
        let mut s = self.state.lock().unwrap();
        let (res, injected) = match s.fault(METHOD_CUSTOMERS_GET) {
            Some(e) => (Err(e), true),
            None => (
                s.customers
                    .get(id)
                    .cloned()
                    .ok_or_else(|| Error::new(404, CODE_NOT_FOUND, format!("no customer {id}"))),
                false,
            ),
        };
        s.record(METHOD_CUSTOMERS_GET, json!(id), &res, injected);
        res
    }

    fn list_units(&self) -> Result<Vec<Unit>, Error> {
        let mut s = self.state.lock().unwrap();
        let (res, injected) = match s.fault(METHOD_UNITS_LIST) {
            Some(e) => (Err(e), true),
            None => (Ok(s.units.values().cloned().collect()), false),
        };
        s.record(METHOD_UNITS_LIST, Value::Null, &res, injected);
        res
    }

    fn availability(&self, q: &AvailabilityQuery) -> Result<Vec<UnitAvailability>, Error> {
        let mut s = self.state.lock().unwrap();
        let (res, injected) = match s.fault(METHOD_UNITS_AVAILABILITY) {
            Some(e) => (Err(e), true),
            None => (s.availability(q), false),
        };
        s.record(METHOD_UNITS_AVAILABILITY, json!(q), &res, injected);
        res
    }

    fn list_bookings(&self, q: &BookingQuery) -> Result<Vec<Booking>, Error> {
        let mut s = self.state.lock().unwrap();
        let (res, injected) = match s.fault(METHOD_BOOKINGS_LIST) {
            Some(e) => (Err(e), true),
            None => (s.list_bookings(q), false),
        };
        s.record(METHOD_BOOKINGS_LIST, json!(q), &res, injected);
        res
    }

    fn create_booking(&self, req: &BookingRequest) -> Result<Booking, Error> {
        let mut s = self.state.lock().unwrap();
        let (res, injected) = match s.fault(METHOD_BOOKINGS_CREATE) {
            Some(e) => (Err(e), true),
            None => (s.create_booking(req), false),
        };
        s.record(METHOD_BOOKINGS_CREATE, json!(req), &res, injected);
        res
    }

    fn update_booking(&self, id: &str, version: i32, patch: &BookingPatch) -> Result<Booking, Error> {
        let mut s = self.state.lock().unwrap();
        let args = json!({"id": id, "version": version, "patch": patch});
        let (res, injected) = match s.fault(METHOD_BOOKINGS_UPDATE) {
            Some(e) => (Err(e), true),
            None => (s.update_booking(id, version, patch), false),
        };
        s.record(METHOD_BOOKINGS_UPDATE, args, &res, injected);
        res
    }

    fn log(&self) -> Vec<Call> {
        self.state.lock().unwrap().calls.clone()
    }
}

// dates

const DATE_LAYOUT: &str = "%Y-%m-%d";

fn parse_date(s: &str) -> Result<NaiveDate, Error> {
    NaiveDate::parse_from_str(s, DATE_LAYOUT)
        .map_err(|_| Error::new(400, CODE_BAD_DATES, format!("bad date {s:?}, want YYYY-MM-DD")))
}

fn parse_range(from: &str, to: &str) -> Result<(NaiveDate, NaiveDate), Error> {
    let f = parse_date(from)?;
    let t = parse_date(to)?;
    if t < f {
        return Err(Error::new(400, CODE_BAD_DATES, "end is before start"));
    }
    Ok((f, t))
}

fn fmt_date(d: NaiveDate) -> String {
    d.format(DATE_LAYOUT).to_string()
}

/// Every day from `from` to `to`, inclusive.
fn days(from: NaiveDate, to: NaiveDate) -> impl Iterator<Item = NaiveDate> {
    from.iter_days().take_while(move |d| *d <= to)
}

fn days_inclusive(s: NaiveDate, e: NaiveDate) -> i32 {
    (e - s).num_days() as i32 + 1
}

fn overlaps(a_s: NaiveDate, a_e: NaiveDate, b_s: NaiveDate, b_e: NaiveDate) -> bool {
    a_e >= b_s && b_e >= a_s
}

/// True when the ranges do not overlap but no clear day separates them.
fn adjacent(a_s: NaiveDate, a_e: NaiveDate, b_s: NaiveDate, b_e: NaiveDate) -> bool {
    a_s == b_e + Days::new(1) || b_s == a_e + Days::new(1)
}

fn touches_weekend(s: NaiveDate, e: NaiveDate) -> bool {
    days(s, e).any(|d| matches!(d.weekday(), Weekday::Sat | Weekday::Sun))
}

// names and phones

/// The digits of a phone number, however it was spaced or punctuated.
fn digits(s: &str) -> String {
    s.chars().filter(char::is_ascii_digit).collect()
}

/// The words of a name, lower-cased and sorted, so punctuation and order do
/// not matter.
fn name_tokens(s: &str) -> Vec<String> {
    let mut out: Vec<String> =
        s.to_lowercase().split(|c: char| !c.is_alphabetic()).filter(|t| !t.is_empty()).map(str::to_string).collect();
    out.sort();
    out
}

/// Accepts the stored name exactly, or every given token present in the
/// stored name when at least two distinct tokens were given. "Tomas.
/// Petrauskas." from a transcript matches "Tomas Petrauskas"; "Tomas" alone
/// does not, and neither does "Tomas Tomas".
fn names_match(stored: &[String], given: &[String]) -> bool {
    if given.is_empty() {
        return false;
    }
    if stored == given {
        return true;
    }
    let distinct: HashSet<&String> = given.iter().collect();
    if distinct.len() < 2 {
        return false;
    }
    let have: HashSet<&String> = stored.iter().collect();
    distinct.is_subset(&have)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed_for_test() -> Seed {
        load_seed(concat!(env!("CARGO_MANIFEST_DIR"), "/../fixtures/seeds/base.json")).unwrap()
    }

    fn req(cust: &str, unit: &str, start: &str, end: &str, people: i32, tow: i32, power: bool) -> BookingRequest {
        BookingRequest {
            customer_id: cust.into(),
            unit_id: unit.into(),
            start: start.into(),
            end: end.into(),
            people,
            site: Site { power_available: power },
            tow_capacity_kg: tow,
        }
    }

    fn want_code<T: std::fmt::Debug>(res: Result<T, Error>, status: u16, code: &str) {
        match res {
            Err(e) => assert!(
                e.status == status && e.code == code,
                "want {} {}, got {} {} ({})",
                status,
                code,
                e.status,
                e.code,
                e.detail
            ),
            Ok(v) => panic!("want Error {status} {code}, got {v:?}"),
        }
    }

    #[test]
    fn identify() {
        let m = Memory::new(seed_for_test(), vec![]);
        let cases = [
            ("+37061234567", "Tomas Petrauskas", true),
            ("+37061234567", "Tomas. Petrauskas.", true),
            ("+370 612 34567", "petrauskas tomas", true),
            ("+37061234567", "Tomas", false),
            ("+37061234567", "Tomas Tomas", false),
            ("+37061234567", "Jonas Jonaitis", false),
            ("+37000000000", "Tomas Petrauskas", false),
            ("+37061234567", "", false),
        ];
        for (phone, name, ok) in cases {
            let r = m.identify_customer(phone, name).unwrap();
            assert_eq!(r.ok, ok, "identify({:?}, {:?}) ok={} want {}", phone, name, r.ok, ok);
        }
    }

    #[test]
    fn rules() {
        let cases: Vec<(&str, BookingRequest, u16, &str)> = vec![
            ("ok weekday", req("cus_12", "unit_mazoji", "2026-10-06", "2026-10-07", 4, 1500, false), 0, ""),
            ("ok weekend two days", req("cus_12", "unit_mazoji", "2026-10-10", "2026-10-11", 4, 1500, false), 0, ""),
            (
                "capacity",
                req("cus_12", "unit_mazoji", "2026-10-06", "2026-10-07", 5, 1500, false),
                422,
                CODE_CAPACITY_EXCEEDED,
            ),
            (
                "tow weight",
                req("cus_12", "unit_didzioji", "2026-10-10", "2026-10-11", 6, 1500, false),
                422,
                CODE_TOW_WEIGHT_EXCEEDED,
            ),
            (
                "power",
                req("cus_12", "unit_vidutine", "2026-10-10", "2026-10-11", 6, 1500, false),
                422,
                CODE_POWER_REQUIRED,
            ),
            (
                "weekend one day",
                req("cus_12", "unit_mazoji", "2026-10-10", "2026-10-10", 4, 1500, false),
                422,
                CODE_MIN_DAYS_WEEKEND,
            ),
            ("weekday one day ok", req("cus_12", "unit_mazoji", "2026-10-06", "2026-10-06", 4, 1500, false), 0, ""),
            (
                "turnaround after",
                req("cus_12", "unit_vidutine", "2026-10-03", "2026-10-04", 6, 1500, true),
                422,
                CODE_TURNAROUND_CONFLICT,
            ),
            (
                "turnaround before",
                req("cus_12", "unit_didzioji", "2026-10-04", "2026-10-05", 8, 2500, true),
                422,
                CODE_TURNAROUND_CONFLICT,
            ),
            ("clear day ok", req("cus_12", "unit_vidutine", "2026-10-04", "2026-10-05", 6, 1500, true), 0, ""),
            (
                "unit taken",
                req("cus_12", "unit_didzioji", "2026-10-06", "2026-10-07", 8, 2500, true),
                409,
                CODE_UNAVAILABLE,
            ),
            (
                "unit blocked",
                req("cus_12", "unit_statine", "2026-10-08", "2026-10-09", 4, 1500, true),
                409,
                CODE_UNAVAILABLE,
            ),
            (
                "overlaps own",
                req("cus_07", "unit_didzioji", "2026-10-17", "2026-10-18", 8, 2500, true),
                422,
                CODE_OVERLAPS_EXISTING,
            ),
            (
                "unknown customer",
                req("cus_99", "unit_mazoji", "2026-10-06", "2026-10-07", 4, 1500, false),
                400,
                CODE_CUSTOMER_UNKNOWN,
            ),
            (
                "unknown unit",
                req("cus_12", "unit_x", "2026-10-06", "2026-10-07", 4, 1500, false),
                400,
                CODE_UNIT_UNKNOWN,
            ),
            (
                "bad dates",
                req("cus_12", "unit_mazoji", "2026-10-07", "2026-10-06", 4, 1500, false),
                400,
                CODE_BAD_DATES,
            ),
            (
                "bad date format",
                req("cus_12", "unit_mazoji", "6 October", "2026-10-07", 4, 1500, false),
                400,
                CODE_BAD_DATES,
            ),
        ];
        for (name, r, status, code) in cases {
            let m = Memory::new(seed_for_test(), vec![]);
            let res = m.create_booking(&r);
            if status == 0 {
                let b = res.unwrap_or_else(|e| panic!("{name}: want ok, got {e}"));
                assert!(b.status == STATUS_BOOKED && b.version == 1 && b.total != 0, "{name}: bad booking {b:?}");
                continue;
            }
            want_code(res, status, code);
        }
    }

    #[test]
    fn availability_turnaround_and_filters() {
        let m = Memory::new(seed_for_test(), vec![]);

        let out = m
            .availability(&AvailabilityQuery {
                from: "2026-10-01".into(),
                to: "2026-10-11".into(),
                ..Default::default()
            })
            .unwrap();
        let by_unit: HashMap<String, UnitAvailability> = out.into_iter().map(|u| (u.unit_id.clone(), u)).collect();
        // vidutine booked Sep 30 - Oct 2, so Oct 3 is turnaround; free from Oct 4
        let got = &by_unit["unit_vidutine"].free;
        assert!(
            got.len() == 1 && got[0].start == "2026-10-04" && got[0].end == "2026-10-11",
            "vidutine free = {got:?}"
        );
        // didzioji booked Oct 6-7: free Oct 1-4 and Oct 9-11
        let got = &by_unit["unit_didzioji"].free;
        assert!(
            got.len() == 2 && got[0].end == "2026-10-04" && got[1].start == "2026-10-09",
            "didzioji free = {got:?}"
        );
        // statine blocked Oct 9
        let got = &by_unit["unit_statine"].free;
        assert!(got.len() == 2 && got[0].end == "2026-10-08" && got[1].start == "2026-10-10", "statine free = {got:?}");

        let out = m
            .availability(&AvailabilityQuery {
                from: "2026-10-10".into(),
                to: "2026-10-11".into(),
                people: Some(6),
                power_available: Some(false),
                ..Default::default()
            })
            .unwrap();
        assert!(
            out.len() == 1 && out[0].unit_id == "unit_didzioji",
            "six people, no power: want only didzioji, got {out:?}"
        );

        let out = m
            .availability(&AvailabilityQuery {
                from: "2026-10-10".into(),
                to: "2026-10-11".into(),
                people: Some(6),
                max_tow_kg: Some(1500),
                ..Default::default()
            })
            .unwrap();
        assert!(
            out.len() == 1 && out[0].unit_id == "unit_vidutine",
            "six people, 1500 kg: want only vidutine, got {out:?}"
        );
    }

    #[test]
    fn update_and_version() {
        let m = Memory::new(seed_for_test(), vec![]);
        let b = m.create_booking(&req("cus_12", "unit_mazoji", "2026-10-03", "2026-10-04", 4, 1500, false)).unwrap();
        let patch =
            BookingPatch { start: Some("2026-10-10".into()), end: Some("2026-10-11".into()), ..Default::default() };
        want_code(m.update_booking(&b.id, 7, &patch), 409, CODE_VERSION_MISMATCH);

        let b2 = m.update_booking(&b.id, b.version, &patch).unwrap();
        assert!(b2.version == 2 && b2.request.start == "2026-10-10" && b2.total == 240, "bad update {b2:?}");

        let bad = BookingPatch { status: Some("booked".into()), ..Default::default() };
        want_code(m.update_booking(&b.id, b2.version, &bad), 400, CODE_BAD_STATUS);

        let cancel = BookingPatch { status: Some(STATUS_CANCELLED.into()), ..Default::default() };
        let b3 = m.update_booking(&b.id, b2.version, &cancel).unwrap();
        assert!(b3.status == STATUS_CANCELLED && b3.version == 3, "bad cancel {b3:?}");
        // cancelled bookings free the unit
        m.create_booking(&req("cus_07", "unit_mazoji", "2026-10-10", "2026-10-11", 4, 2000, false))
            .unwrap_or_else(|e| panic!("after cancel: {e}"));
    }

    #[test]
    fn faults_and_log() {
        let m = Memory::new(
            seed_for_test(),
            vec![Fault { on: METHOD_BOOKINGS_CREATE.into(), nth: 1, status: 409, code: String::new() }],
        );
        let r = req("cus_12", "unit_mazoji", "2026-10-06", "2026-10-07", 4, 1500, false);
        want_code(m.create_booking(&r), 409, CODE_UNAVAILABLE);
        m.create_booking(&r).unwrap_or_else(|e| panic!("second create: {e}"));
        let log = m.log();
        assert!(
            log.len() == 2 && log[0].status == 409 && log[1].status == 200 && log[0].method == METHOD_BOOKINGS_CREATE,
            "log = {log:?}"
        );
        assert!(log[0].injected && !log[1].injected);
        assert_eq!(m.bookings().len(), 4, "want 4 bookings (3 seed + 1)");
    }

    #[test]
    fn list_bookings_filters() {
        let m = Memory::new(seed_for_test(), vec![]);
        let out = m
            .list_bookings(&BookingQuery {
                unit_id: "unit_didzioji".into(),
                from: "2026-10-06".into(),
                to: "2026-10-07".into(),
                ..Default::default()
            })
            .unwrap();
        assert!(out.len() == 1 && out[0].request.customer_id == "cus_31", "want bk_88 for cus_31, got {out:?}");
        let out = m.list_bookings(&BookingQuery { customer_id: "cus_12".into(), ..Default::default() }).unwrap();
        assert!(out.is_empty(), "cus_12 has no bookings in the seed, got {out:?}");
    }

    #[test]
    fn booking_json_is_flat() {
        let seed = seed_for_test();
        let v = serde_json::to_value(&seed.bookings[0]).unwrap();
        assert_eq!(v["customer_id"], "cus_31");
        assert_eq!(v["site"]["power_available"], true);
        assert_eq!(v["id"], "bk_77");
    }
}
