use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub struct ProcFsSource {
    root: PathBuf,
    mem_cache: Option<(Instant, MemInfo)>,
}

impl ProcFsSource {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            mem_cache: None,
        }
    }

    pub fn read_stat(&mut self) -> Result<CpuStat, anyhow::Error> {
        let stat = fs::read_to_string(self.root.join("stat"))
            .map_err(|e| anyhow::anyhow!("read /proc/stat: {}", e))?;
        let line = stat
            .lines()
            .find(|l| l.starts_with("cpu "))
            .ok_or_else(|| anyhow::anyhow!("cpu line not found in /proc/stat"))?;
        let fields: Vec<u64> = line
            .split_whitespace()
            .skip(1)
            .filter_map(|s| s.parse().ok())
            .collect();

        if fields.len() < 8 {
            anyhow::bail!("not enough fields in /proc/stat cpu line");
        }

        Ok(CpuStat {
            user: fields[0],
            nice: fields[1],
            system: fields[2],
            idle: fields[3],
            iowait: fields[4],
            irq: fields[5],
            softirq: fields[6],
            steal: fields[7],
        })
    }

    pub fn read_meminfo(&mut self) -> Result<MemInfo, anyhow::Error> {
        if let Some((captured, value)) = self.mem_cache {
            if captured.elapsed() < Duration::from_millis(750) {
                return Ok(value);
            }
        }
        let meminfo = fs::read_to_string(self.root.join("meminfo"))
            .map_err(|e| anyhow::anyhow!("read /proc/meminfo: {}", e))?;
        let mut total_kb: u64 = 0;
        let mut available_kb: u64 = 0;
        let mut swap_total_kb: u64 = 0;
        let mut swap_free_kb: u64 = 0;

        for line in meminfo.lines() {
            if let Some(rest) = line.strip_prefix("MemTotal:") {
                total_kb = parse_kb_value(rest);
            } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
                available_kb = parse_kb_value(rest);
            } else if let Some(rest) = line.strip_prefix("SwapTotal:") {
                swap_total_kb = parse_kb_value(rest);
            } else if let Some(rest) = line.strip_prefix("SwapFree:") {
                swap_free_kb = parse_kb_value(rest);
            }
        }

        let value = MemInfo {
            total_kb,
            available_kb,
            swap_total_kb,
            swap_free_kb,
        };
        self.mem_cache = Some((Instant::now(), value));
        Ok(value)
    }

    pub fn read_uptime(&self) -> Result<f64, anyhow::Error> {
        let content = fs::read_to_string(self.root.join("uptime"))
            .map_err(|e| anyhow::anyhow!("read /proc/uptime: {}", e))?;
        content
            .split_whitespace()
            .next()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| anyhow::anyhow!("failed to parse /proc/uptime"))
    }
}

fn parse_kb_value(s: &str) -> u64 {
    s.trim()
        .split_whitespace()
        .next()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

#[derive(Debug, Clone, Copy)]
pub struct CpuStat {
    pub user: u64,
    pub nice: u64,
    pub system: u64,
    pub idle: u64,
    pub iowait: u64,
    pub irq: u64,
    pub softirq: u64,
    pub steal: u64,
}

impl CpuStat {
    pub fn total(&self) -> u64 {
        self.user
            .saturating_add(self.nice)
            .saturating_add(self.system)
            .saturating_add(self.idle)
            .saturating_add(self.iowait)
            .saturating_add(self.irq)
            .saturating_add(self.softirq)
            .saturating_add(self.steal)
    }

    pub fn idle_total(&self) -> u64 {
        self.idle.saturating_add(self.iowait)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct MemInfo {
    pub total_kb: u64,
    pub available_kb: u64,
    pub swap_total_kb: u64,
    pub swap_free_kb: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).ok();
        dir
    }

    #[test]
    fn test_read_stat() {
        let dir = setup_dir("pulsedeck_test_procfs_stat");
        fs::write(
            dir.join("stat"),
            "cpu  100 20 50 1000 30 5 10 5 0 0\ncpu0 100 20 50 1000 30 5 10 5 0 0\n",
        )
        .ok();

        let mut src = ProcFsSource::new(dir);
        let stat = src.read_stat().unwrap();
        assert_eq!(stat.user, 100);
        assert_eq!(stat.idle, 1000);
        assert_eq!(stat.iowait, 30);
        assert_eq!(stat.idle_total(), 1030);
        assert!(stat.total() > 1000);
    }

    #[test]
    fn test_read_meminfo() {
        let dir = setup_dir("pulsedeck_test_procfs_meminfo");
        fs::write(
            dir.join("meminfo"),
            "MemTotal:        8192000 kB\nMemFree:         2000000 kB\nMemAvailable:    4500000 kB\n",
        )
        .ok();

        let mut src = ProcFsSource::new(dir);
        let mem = src.read_meminfo().unwrap();
        assert_eq!(mem.total_kb, 8192000);
        assert_eq!(mem.available_kb, 4500000);
    }

    #[test]
    fn test_read_uptime() {
        let dir = setup_dir("pulsedeck_test_procfs_uptime");
        fs::write(dir.join("uptime"), "12345.67 98765.43\n").ok();

        let src = ProcFsSource::new(dir);
        let up = src.read_uptime().unwrap();
        assert!((up - 12345.67).abs() < 0.01);
    }
}
