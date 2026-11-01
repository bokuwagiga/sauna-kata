//! Runs the fixtures against the agent.
//!
//! ```text
//! cargo run                          all fixtures
//! cargo run -- --id silence          one fixture
//! cargo run -- --runs 3 --need 2     three times each, pass on two, for a stability check
//! cargo run -- -v                    print actions, effects and verdicts per run
//! cargo run -- --trace runs          also write every LLM call and per-run results under runs/
//! ```

use std::collections::HashSet;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::Parser;

use pirtis::agent;
use pirtis::harness::{load_fixtures, Runner};
use pirtis::llm;

#[derive(Parser)]
#[command(name = "harness", about = "Runs the fixtures against the agent.", disable_help_subcommand = true)]
struct Args {
    /// directory with *.yaml fixtures and seeds/
    #[arg(long, default_value = "../fixtures")]
    fixtures: PathBuf,
    /// directory to write every LLM call and per-run results to; off by default
    #[arg(long)]
    trace: Option<PathBuf>,
    /// comma-separated fixture ids to run; default all
    #[arg(long)]
    id: Option<String>,
    /// runs per fixture
    #[arg(long, default_value_t = 1)]
    runs: usize,
    /// runs that must pass for the fixture to pass
    #[arg(long, default_value_t = 1)]
    need: usize,
    /// print actions, effects and verdicts per run
    #[arg(short = 'v', long)]
    verbose: bool,
}

fn main() -> ExitCode {
    let args = Args::parse();

    load_dotenv(".env");

    let client = match llm::OpenAI::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            eprintln!("set LLM_API_KEY, and optionally LLM_BASE_URL; see .env.example");
            return ExitCode::from(2);
        }
    };

    let mut fixtures = match load_fixtures(&args.fixtures) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    if let Some(ids) = &args.id {
        let want: HashSet<&str> = ids.split(',').map(str::trim).collect();
        fixtures.retain(|f| want.contains(f.id.as_str()));
    }
    if fixtures.is_empty() {
        eprintln!("no fixtures matched");
        return ExitCode::from(2);
    }

    let runner = Runner {
        llm: Box::new(client),
        judge: None,
        decide: Box::new(agent::decide),
        handlers: agent::handlers(),
        trace_dir: args.trace,
    };

    let mut passed = 0;
    println!(
        "{:<26} {:<4} {:<6} {:>6} {:>14} {:>8} {:>8}",
        "fixture", "run", "result", "calls", "tokens in/out", "llm", "wall"
    );
    for f in &fixtures {
        let fr = runner.run(f, args.runs, args.need);
        for r in &fr.runs {
            let result = if r.passed() { "PASS" } else { "FAIL" };
            println!(
                "{:<26} {:<4} {:<6} {:>6} {:>14} {:>8} {:>8}",
                f.id,
                r.index,
                result,
                r.stats.calls,
                format!("{}/{}", r.stats.prompt_tokens, r.stats.completion_tokens),
                fmt_duration(round_10ms(r.stats.duration)),
                fmt_duration(round_10ms(r.wall))
            );
            if let Some(e) = &r.err {
                println!("    error: {e}");
            }
            for fl in &r.failures {
                println!("    - {fl}");
            }
            if args.verbose {
                for a in &r.actions {
                    println!("    action: {a}");
                }
                for e in &r.effects {
                    println!("    effect: {e}");
                }
                for v in &r.verdicts {
                    println!("    judge: {}  {}: {}", v.pass, v.criterion, v.reason);
                }
            }
        }
        if fr.passed() {
            passed += 1;
            println!("{:<26}      pass  ({}/{}, need {})", "", fr.pass_count(), fr.runs.len(), fr.threshold);
        } else {
            println!("{:<26}      FAIL  ({}/{}, need {})", "", fr.pass_count(), fr.runs.len(), fr.threshold);
        }
    }
    println!("\n{}/{} fixtures passed", passed, fixtures.len());
    if passed < fixtures.len() {
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}

/// Sets KEY=VALUE lines from `path` into the environment when the variable is
/// not already set. A missing file is fine.
fn load_dotenv(path: &str) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let k = k.trim();
        let v = v.trim().trim_matches(|c| c == '"' || c == '\'');
        if std::env::var(k).map(|cur| cur.is_empty()).unwrap_or(true) {
            std::env::set_var(k, v);
        }
    }
}

fn round_10ms(d: Duration) -> Duration {
    let ms = d.as_secs_f64() * 100.0;
    Duration::from_millis(ms.round() as u64 * 10)
}

/// Formats a duration the way Go's time.Duration prints: 0s, 450ms, 1.23s, 1m2.5s.
fn fmt_duration(d: Duration) -> String {
    let ns = d.as_nanos();
    fn trim(f: f64) -> String {
        let s = format!("{f:.9}");
        let s = s.trim_end_matches('0');
        s.trim_end_matches('.').to_string()
    }
    if ns == 0 {
        return "0s".to_string();
    }
    if ns < 1_000 {
        return format!("{ns}ns");
    }
    if ns < 1_000_000 {
        return format!("{}µs", trim(ns as f64 / 1e3));
    }
    if ns < 1_000_000_000 {
        return format!("{}ms", trim(ns as f64 / 1e6));
    }
    let secs = ns / 1_000_000_000;
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = (ns % 60_000_000_000) as f64 / 1e9;
    let mut out = String::new();
    if h > 0 {
        out.push_str(&format!("{h}h"));
    }
    if h > 0 || m > 0 {
        out.push_str(&format!("{m}m"));
    }
    out.push_str(&format!("{}s", trim(s)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dotenv_fills_only_unset_variables() {
        let path = std::env::temp_dir().join(format!("pirtis-dotenv-{}", std::process::id()));
        std::fs::write(&path, "# comment\n\nPIRTIS_TEST_A = \"quoted\"\nPIRTIS_TEST_B='kept'\nnot a pair\n").unwrap();
        std::env::set_var("PIRTIS_TEST_B", "from env");
        load_dotenv(path.to_str().unwrap());
        std::fs::remove_file(&path).unwrap();
        assert_eq!(std::env::var("PIRTIS_TEST_A").unwrap(), "quoted");
        assert_eq!(std::env::var("PIRTIS_TEST_B").unwrap(), "from env");
        load_dotenv("/nonexistent/.env"); // a missing file is fine
    }

    #[test]
    fn durations_print_like_go() {
        assert_eq!(fmt_duration(Duration::ZERO), "0s");
        assert_eq!(fmt_duration(Duration::from_millis(450)), "450ms");
        assert_eq!(fmt_duration(Duration::from_millis(1230)), "1.23s");
        assert_eq!(fmt_duration(Duration::from_secs(2)), "2s");
        assert_eq!(fmt_duration(Duration::from_millis(62_500)), "1m2.5s");
        assert_eq!(fmt_duration(round_10ms(Duration::from_micros(1_234_567))), "1.23s");
    }
}
