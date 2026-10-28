//! Opt-in startup timing, for answering "why did the popup take so long?".
//!
//! Set `COPYCAT_TRACE=1` and every phase of startup is timed to stderr. It is
//! off by default and costs one environment lookup when it is.
//!
//! Timings are measured from process *exec*, not from the top of `main`. That
//! distinction is the whole point: copycat links 100-odd shared libraries, so a
//! meaningful slice of the wait can be spent in the dynamic linker before our
//! first instruction runs. A trace anchored at `main` would hide exactly the
//! cost we are hunting for and make the program look faster than it feels.

use std::cell::Cell;
use std::time::{Duration, Instant};

/// Clock ticks per second, the unit `/proc/self/stat` reports process start in.
///
/// This is `USER_HZ`, which the kernel fixes at 100 on Linux regardless of the
/// configured scheduler tick. It is deliberately not `sysconf(_SC_CLK_TCK)`:
/// that would mean a libc dependency for a diagnostic, and the value has been
/// 100 on every Linux ABI copycat targets.
///
/// The consequence is that the *first* line of a trace — time spent before
/// `main` — is quantised to 10 ms, so it prints as a suspiciously round number.
/// Every later line is measured with [`Instant`] and is precise; only the
/// anchor is coarse. It is accurate enough to answer "is the linker costing us
/// tens or hundreds of milliseconds", which is all it is there for.
const USER_HZ: f64 = 100.0;

/// A startup stopwatch. Construct once, then [`mark`](Trace::mark) each phase.
///
/// `mark` takes `&self` so the trace can live inside GTK's `Fn` signal
/// closures, which cannot hold anything mutable.
pub struct Trace {
    /// When this process was exec'd, as best we can reconstruct it.
    exec: Option<Instant>,
    /// The previous mark, so each phase reports its own cost.
    last: Cell<Instant>,
    enabled: bool,
}

impl Trace {
    /// Start tracing if `COPYCAT_TRACE` is set to anything non-empty.
    pub fn new() -> Self {
        let enabled = std::env::var_os("COPYCAT_TRACE").is_some_and(|v| !v.is_empty());
        let now = Instant::now();
        let exec = enabled
            .then(process_age)
            .flatten()
            .and_then(|age| now.checked_sub(age));

        if enabled {
            match exec {
                Some(exec) => eprintln!(
                    "copycat trace: {:>6.1} ms  reached main (dynamic linking + libc startup)",
                    now.duration_since(exec).as_secs_f64() * 1000.0
                ),
                None => eprintln!(
                    "copycat trace: process age unavailable; timings are relative to main"
                ),
            }
        }

        Self {
            exec,
            last: Cell::new(now),
            enabled,
        }
    }

    /// Record that `label` just finished.
    pub fn mark(&self, label: &str) {
        if !self.enabled {
            return;
        }
        let now = Instant::now();
        let phase = now.duration_since(self.last.replace(now));
        match self.exec {
            Some(exec) => eprintln!(
                "copycat trace: {:>6.1} ms  {label} (+{:.1} ms)",
                now.duration_since(exec).as_secs_f64() * 1000.0,
                phase.as_secs_f64() * 1000.0
            ),
            None => eprintln!(
                "copycat trace:  +{:.1} ms  {label}",
                phase.as_secs_f64() * 1000.0
            ),
        }
    }
}

impl Default for Trace {
    fn default() -> Self {
        Self::new()
    }
}

/// How long ago this process was exec'd, from `/proc`.
///
/// Returns `None` anywhere `/proc` is not the Linux one we expect, in which
/// case the caller falls back to timing from `main`.
fn process_age() -> Option<Duration> {
    let uptime = std::fs::read_to_string("/proc/uptime").ok()?;
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    age_from(&uptime, &stat, USER_HZ)
}

/// Compute process age from raw `/proc/uptime` and `/proc/self/stat` contents.
///
/// Split out from the file reads so the parsing — which has a genuinely nasty
/// edge case — is testable without a live process.
fn age_from(uptime: &str, stat: &str, hz: f64) -> Option<Duration> {
    let booted_secs: f64 = uptime.split_whitespace().next()?.parse().ok()?;
    let started_ticks = starttime_ticks(stat)?;
    let age = booted_secs - (started_ticks as f64 / hz);
    // A negative age means the two readings disagree (or the clock moved);
    // reporting a nonsense number is worse than reporting none.
    (age >= 0.0).then(|| Duration::from_secs_f64(age))
}

/// Field 22 of `/proc/self/stat`: process start time, in clock ticks since boot.
///
/// Parsing cannot simply split on whitespace. Field 2 is the executable name in
/// parentheses, and that name can itself contain spaces *and* parentheses —
/// `comm` is whatever the binary was called. The kernel's own documented
/// workaround is to scan from the *last* `)`, which is what this does.
fn starttime_ticks(stat: &str) -> Option<u64> {
    let tail = &stat[stat.rfind(')')? + 1..];
    // After the closing paren the next field is `state` (field 3), so the start
    // time at field 22 is the 20th whitespace-separated token from here.
    tail.split_whitespace().nth(19)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A realistic stat line: pid, comm, state, then fields 4..=22.
    fn stat_line(comm: &str, starttime: u64) -> String {
        let mut fields = vec!["1234".to_string(), format!("({comm})"), "S".to_string()];
        // Fields 4..=21 are filler; field 22 is the start time.
        for n in 4..=21 {
            fields.push(n.to_string());
        }
        fields.push(starttime.to_string());
        fields.push("99999".to_string()); // field 23, proving we stop at 22
        fields.join(" ")
    }

    #[test]
    fn reads_the_start_time_field() {
        assert_eq!(starttime_ticks(&stat_line("copycat", 54321)), Some(54321));
    }

    #[test]
    fn survives_a_process_name_containing_spaces_and_parens() {
        // The case that breaks naive whitespace splitting.
        let stat = stat_line("evil ) name (x", 777);
        assert_eq!(starttime_ticks(&stat), Some(777));
    }

    #[test]
    fn malformed_stat_yields_no_timing_rather_than_a_wrong_one() {
        assert_eq!(starttime_ticks("no parens here at all"), None);
        assert_eq!(starttime_ticks("1234 (short) S 1 2 3"), None);
    }

    #[test]
    fn age_is_uptime_minus_start_time() {
        // Booted 500s ago, process started at tick 49000 == 490s => 10s old.
        let age = age_from("500.0 1000.0\n", &stat_line("copycat", 49_000), 100.0);
        assert_eq!(age, Some(Duration::from_secs(10)));
    }

    #[test]
    fn a_process_starting_after_boot_time_reports_nothing() {
        // Clock skew between the two reads must not produce a negative age.
        let age = age_from("100.0 1000.0\n", &stat_line("copycat", 50_000), 100.0);
        assert_eq!(age, None);
    }

    #[test]
    fn unparseable_uptime_yields_no_timing() {
        assert_eq!(age_from("", &stat_line("copycat", 1), 100.0), None);
    }
}
