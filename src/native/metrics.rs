//! Read the counters for the whole native Application cgroup, including terminals.

use crate::metrics::AppSample;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Default)]
pub struct Sampler {
    previous: HashMap<String, (u64, Instant, u64)>,
}

impl Sampler {
    pub fn forget(&mut self, id: &str) {
        self.previous.remove(id);
    }

    pub fn sample(&mut self, id: &str, path: &Path) -> Result<Option<AppSample>> {
        self.read_at(id, path, Instant::now())
    }

    fn read_at(&mut self, id: &str, path: &Path, clock: Instant) -> Result<Option<AppSample>> {
        use std::os::unix::fs::MetadataExt;
        let generation = std::fs::metadata(path)?.ino();
        let tasks = number(&std::fs::read_to_string(path.join("pids.current"))?)?;
        if tasks == 0 {
            self.forget(id);
            return Ok(None);
        }
        let cpu = std::fs::read_to_string(path.join("cpu.stat"))?;
        let used = cpu
            .lines()
            .find_map(|line| line.strip_prefix("usage_usec "))
            .context("cgroup cpu.stat lacks usage_usec")?;
        let used = number(used)?;
        let previous = self.previous.insert(id.into(), (used, clock, generation));
        let cpu_percent = previous
            .filter(|(_, _, old_generation)| *old_generation == generation)
            .map(|(old_used, old_clock, _)| {
                let elapsed = clock.duration_since(old_clock).as_secs_f64();
                if elapsed > 0.0 {
                    used.saturating_sub(old_used) as f64 / 1_000_000.0 / elapsed * 100.0
                } else {
                    0.0
                }
            })
            .unwrap_or(0.0);
        let memory_bytes = number(&std::fs::read_to_string(path.join("memory.current"))?)?;
        let limit = std::fs::read_to_string(path.join("memory.max"))?;
        let memory_limit_bytes = if limit.trim() == "max" {
            std::fs::read_to_string("/proc/meminfo")
                .ok()
                .and_then(|text| {
                    text.lines()
                        .find_map(|line| {
                            line.strip_prefix("MemTotal:")
                                .and_then(|value| value.split_whitespace().next())
                                .and_then(|value| value.parse::<u64>().ok())
                        })
                        .map(|kb| kb.saturating_mul(1024))
                })
                .unwrap_or(0)
        } else {
            number(&limit)?
        };
        Ok(Some(AppSample {
            at: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
            cpu_percent,
            memory_bytes,
            memory_limit_bytes,
            // cgroup v2 does not supply per-cgroup network counters. Native
            // views omit network usage rather than display these as measured.
            rx_bytes: 0,
            tx_bytes: 0,
            tasks: Some(tasks),
        }))
    }
}

fn number(text: &str) -> Result<u64> {
    text.trim().parse().context("invalid cgroup counter")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn measures_cpu_memory_and_tasks_and_resets_after_stop() {
        struct Directory(std::path::PathBuf);
        impl Directory {
            fn path(&self) -> &Path {
                &self.0
            }
        }
        impl Drop for Directory {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let dir = Directory(
            std::env::temp_dir().join(format!("native-metrics-{:032x}", rand::random::<u128>())),
        );
        std::fs::create_dir(&dir.0).unwrap();
        for (name, value) in [
            ("pids.current", "3"),
            ("cpu.stat", "usage_usec 1000000\nuser_usec 900000\n"),
            ("memory.current", "1024"),
            ("memory.max", "4096"),
        ] {
            std::fs::write(dir.path().join(name), value).unwrap();
        }
        let mut sampler = Sampler::default();
        let now = Instant::now();
        let first = sampler.read_at("sample", dir.path(), now).unwrap().unwrap();
        assert_eq!(first.cpu_percent, 0.0);
        assert_eq!(first.memory_bytes, 1024);
        assert_eq!(first.memory_limit_bytes, 4096);
        assert_eq!(first.tasks, Some(3));
        std::fs::write(dir.path().join("cpu.stat"), "usage_usec 1500000\n").unwrap();
        assert_eq!(
            sampler
                .read_at("sample", dir.path(), now + Duration::from_secs(1))
                .unwrap()
                .unwrap()
                .cpu_percent,
            50.0
        );
        std::fs::write(dir.path().join("pids.current"), "0").unwrap();
        assert!(
            sampler
                .read_at("sample", dir.path(), now)
                .unwrap()
                .is_none()
        );
        std::fs::write(dir.path().join("pids.current"), "1").unwrap();
        assert_eq!(
            sampler
                .read_at("sample", dir.path(), now)
                .unwrap()
                .unwrap()
                .cpu_percent,
            0.0
        );
        std::fs::write(dir.path().join("memory.current"), "invalid").unwrap();
        assert!(sampler.read_at("sample", dir.path(), now).is_err());
    }
}
