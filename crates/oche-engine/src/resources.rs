//! The daemon's own CPU and memory use, read from Linux `/proc`.
use oche_domain::Resources;
use std::{fs, time::Instant};

/// Linux reports CPU time in USER_HZ ticks, 100 per second on every
/// architecture it exposes to user space.
const TICKS_PER_SECOND: f64 = 100.0;

/// Samples the whole process, all threads included. CPU use is the average
/// since the previous sample, so the first sample reports none.
#[derive(Debug, Default)]
pub struct Sampler {
    last: Option<(Instant, u64)>,
}

impl Sampler {
    /// `None` where `/proc` is unavailable (not Linux) or unreadable.
    pub fn sample(&mut self) -> Option<Resources> {
        let ticks = cpu_ticks(&fs::read_to_string("/proc/self/stat").ok()?)?;
        let memory_bytes = resident_bytes(&fs::read_to_string("/proc/self/status").ok()?)?;
        let now = Instant::now();
        let cpu_percent = self.last.replace((now, ticks)).and_then(|(then, before)| {
            let seconds = now.duration_since(then).as_secs_f64();
            (seconds > 0.0)
                .then(|| ticks.saturating_sub(before) as f64 / TICKS_PER_SECOND / seconds * 100.0)
        });
        Some(Resources {
            cpu_percent,
            memory_bytes,
        })
    }
}

/// User plus system time, fields 14 and 15 of `/proc/self/stat`. The command
/// name (field 2) may contain spaces, so fields are counted after its `)`.
fn cpu_ticks(stat: &str) -> Option<u64> {
    let fields: Vec<&str> = stat
        .get(stat.rfind(')')? + 1..)?
        .split_whitespace()
        .collect();
    let user: u64 = fields.get(11)?.parse().ok()?;
    let system: u64 = fields.get(12)?.parse().ok()?;
    Some(user + system)
}

/// `VmRSS` from `/proc/self/status`, given in kibibytes.
fn resident_bytes(status: &str) -> Option<u64> {
    let line = status.lines().find_map(|l| l.strip_prefix("VmRSS:"))?;
    let kib: u64 = line.trim().strip_suffix("kB")?.trim().parse().ok()?;
    Some(kib * 1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_time_is_read_after_a_command_name_with_spaces() {
        let stat = "4242 (oche core) S 1 4242 4242 0 -1 4194560 830 0 0 0 157 43 0 0 20 0 9 0 \
                    123 456 789";
        assert_eq!(cpu_ticks(stat), Some(200));
        assert_eq!(cpu_ticks("4242 (oche"), None);
    }

    #[test]
    fn resident_memory_is_read_in_kibibytes() {
        let status = "Name:\toche-core\nVmPeak:\t  90000 kB\nVmRSS:\t   41236 kB\nThreads:\t9\n";
        assert_eq!(resident_bytes(status), Some(41236 * 1024));
        assert_eq!(resident_bytes("Name:\toche-core\n"), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn this_process_reports_memory_then_cpu() {
        let mut sampler = Sampler::default();
        let first = sampler.sample().unwrap();
        assert!(first.memory_bytes > 0);
        assert_eq!(first.cpu_percent, None);
        assert!(sampler.sample().unwrap().cpu_percent.is_some());
    }
}
