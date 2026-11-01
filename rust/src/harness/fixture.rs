use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::api;
use crate::transcript::Transcript;

/// Fixture is one test case: the call so far, what the world must look like
/// after the agent's turn, and optionally what a judge should check in the
/// reply. One YAML file each, one turn each: `decide` is called once with the
/// transcript and the checks run on what it did.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Fixture {
    pub id: String,
    /// rows added to the base seed
    pub setup: Setup,
    /// make the nth call of a method fail
    pub faults: Vec<api::Fault>,
    pub expect: Expect,
    /// A list of statements about the reply, each true or false, for fixtures
    /// where the right answer is a sentence and not a booking. The same pinned
    /// model reads the call and the reply at temperature zero and marks each
    /// one with a one-sentence reason; a failed line prints as
    /// `judge: <statement>: <reason>`. Optional.
    pub judge: Vec<String>,
    pub transcript: Transcript,

    /// where the fixture file lives
    #[serde(skip)]
    pub dir: PathBuf,
}

/// Setup adds rows to the base seed before the run.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Setup {
    pub bookings: Vec<SetupBooking>,
}

/// SetupBooking is a booking that already exists when the turn starts.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SetupBooking {
    pub id: String,
    pub customer: String,
    pub unit: String,
    pub start: String,
    pub end: String,
    pub people: i32,
    pub power: bool,
    pub tow: i32,
    pub version: i32,
    pub total: i32,
}

impl SetupBooking {
    /// Converts a SetupBooking into an [`api::Booking`].
    pub fn booking(&self) -> api::Booking {
        api::Booking {
            request: api::BookingRequest {
                customer_id: self.customer.clone(),
                unit_id: self.unit.clone(),
                start: self.start.clone(),
                end: self.end.clone(),
                people: self.people,
                site: api::Site { power_available: self.power },
                tow_capacity_kg: self.tow,
            },
            id: self.id.clone(),
            version: if self.version == 0 { 1 } else { self.version },
            status: api::STATUS_BOOKED.to_string(),
            total: self.total,
        }
    }
}

/// Expect is what must be true after the last turn. Everything not listed
/// here is an invariant the harness always checks; see [`super::evaluate`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Expect {
    /// The effect type the last turn must produce: say, wait, transfer or
    /// end_call. wait also means no speech.
    pub action: String,
    /// What changed in the bookings table, as a diff against the seed. Empty
    /// means nothing changed. Listed fields are compared, the rest are
    /// ignored. `id` names an existing booking that must have changed.
    pub bookings: Vec<BookingExpect>,
}

/// BookingExpect is one expected created or changed booking.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BookingExpect {
    pub id: String,
    pub customer: String,
    pub unit: String,
    pub start: String,
    pub end: String,
    pub people: Option<i32>,
    pub status: String,
}

/// Reads every *.yaml in `dir`, sorted by file name.
pub fn load_fixtures(dir: impl AsRef<Path>) -> anyhow::Result<Vec<Fixture>> {
    let dir = dir.as_ref();
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| anyhow::anyhow!("{}: {}", dir.display(), e))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && p.extension().and_then(|x| x.to_str()) == Some("yaml"))
        .collect();
    paths.sort();
    let mut out = Vec::new();
    for p in paths {
        let b = std::fs::read_to_string(&p)?;
        let mut f: Fixture = serde_yaml::from_str(&b).map_err(|e| anyhow::anyhow!("{}: {}", p.display(), e))?;
        if f.id.is_empty() {
            anyhow::bail!("{}: missing id", p.display());
        }
        if f.transcript.items.is_empty() || f.transcript.today.is_empty() {
            anyhow::bail!("{}: transcript needs today and items", p.display());
        }
        match f.expect.action.as_str() {
            "say" | "wait" | "transfer" | "end_call" => {}
            _ => anyhow::bail!("{}: expect.action must be say, wait, transfer or end_call", p.display()),
        }
        f.dir = dir.to_path_buf();
        out.push(f);
    }
    Ok(out)
}

impl Fixture {
    /// The base seed every fixture starts from.
    pub fn seed_path(&self) -> PathBuf {
        self.dir.join("seeds").join("base.json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load_one(name: &str, yaml: &str) -> String {
        let dir = std::env::temp_dir().join(format!("pirtis-fixture-{}-{}", std::process::id(), name));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("00_case.yaml"), yaml).unwrap();
        let err = load_fixtures(&dir).unwrap_err().to_string();
        std::fs::remove_dir_all(&dir).unwrap();
        err
    }

    /// A fixture needs an id, a dated transcript with items, and a known action.
    #[test]
    fn rejects_incomplete_fixtures() {
        let ok_transcript = "transcript: {today: '2026-09-28', now: 3, items: [{type: message, role: user, start: 1, end: 2, content: hi}]}";
        let err = load_one("no-id", &format!("expect: {{action: say}}\n{ok_transcript}\n"));
        assert!(err.ends_with("00_case.yaml: missing id"), "{err}");
        let err = load_one("no-items", "id: x\nexpect: {action: say}\ntranscript: {today: '2026-09-28'}\n");
        assert!(err.ends_with("00_case.yaml: transcript needs today and items"), "{err}");
        let err = load_one("bad-action", &format!("id: x\nexpect: {{action: shout}}\n{ok_transcript}\n"));
        assert!(err.ends_with("00_case.yaml: expect.action must be say, wait, transfer or end_call"), "{err}");
        let err = load_one("not-yaml", "id: [\n");
        assert!(err.contains("00_case.yaml: "), "{err}");
    }

    /// Setup bookings default to version 1 and status booked.
    #[test]
    fn setup_booking_defaults() {
        let sb = SetupBooking {
            id: "bk_1".into(),
            customer: "cus_07".into(),
            unit: "unit_mazoji".into(),
            ..Default::default()
        };
        let b = sb.booking();
        assert_eq!((b.version, b.status.as_str(), b.request.customer_id.as_str()), (1, api::STATUS_BOOKED, "cus_07"));
        assert_eq!(SetupBooking { version: 3, ..sb }.booking().version, 3);
    }
}
