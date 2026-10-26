use std::collections::HashMap;
use std::fmt;

use super::InspectionError;

#[derive(Clone, Default)]
pub struct SystemSnapshot {
    pub os: Option<String>,
    pub architecture: Option<String>,
    pub hostname: Option<String>,
    pub cpu_model: Option<String>,
    pub logical_cores: Option<usize>,
    pub uptime_seconds: Option<f64>,
    pub cpu_usage_percent: Option<f32>,
    pub memory_total_bytes: Option<u64>,
    pub memory_used_bytes: Option<u64>,
    pub interfaces: Vec<InterfaceSample>,
    pub notes: Vec<String>,
    pub(crate) cpu_ticks: Option<(u64, u64)>,
    pub(crate) sample_seconds: Option<f64>,
}

impl fmt::Debug for SystemSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SystemSnapshot")
            .field("interfaces", &self.interfaces.len())
            .field("notes", &self.notes.len())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Default)]
pub struct InterfaceSample {
    pub name: String,
    pub received_bytes: u64,
    pub transmitted_bytes: u64,
    pub receive_bytes_per_second: Option<f64>,
    pub transmit_bytes_per_second: Option<f64>,
}

impl fmt::Debug for InterfaceSample {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("InterfaceSample([redacted])")
    }
}

#[derive(Default, Debug)]
pub struct SystemSampler {
    previous: Option<SystemSnapshot>,
}

impl SystemSampler {
    pub fn reset(&mut self) {
        self.previous = None;
    }

    pub fn sample(&mut self, mut current: SystemSnapshot) -> SystemSnapshot {
        current
            .interfaces
            .sort_by(|left, right| left.name.cmp(&right.name));
        let time = current.sample_seconds.or(current.uptime_seconds);
        let elapsed = self.previous.as_ref().and_then(|old| {
            let previous_time = old.sample_seconds.or(old.uptime_seconds)?;
            let elapsed = time? - previous_time;
            (elapsed.is_finite()
                && elapsed > 0.0
                && current
                    .uptime_seconds
                    .zip(old.uptime_seconds)
                    .is_none_or(|(now, before)| now >= before))
            .then_some(elapsed)
        });
        let previous = self.previous.as_ref();
        let cpu_rolled_back = previous
            .and_then(|old| old.cpu_ticks)
            .zip(current.cpu_ticks)
            .is_some_and(|((before_total, before_idle), (total, idle))| {
                total < before_total || idle < before_idle
            });
        let topology_unchanged = previous.is_some_and(|old| {
            old.interfaces.len() == current.interfaces.len()
                && current.interfaces.iter().all(|interface| {
                    old.interfaces
                        .iter()
                        .any(|before| before.name == interface.name)
                })
        });
        let network_elapsed = (!cpu_rolled_back && topology_unchanged)
            .then_some(elapsed)
            .flatten();
        if current
            .cpu_usage_percent
            .is_some_and(|usage| !usage.is_finite() || !(0.0..=100.0).contains(&usage))
        {
            current.cpu_usage_percent = None;
        }
        if elapsed.is_none() {
            current.cpu_usage_percent = None;
        } else if let Some((total, idle)) = current.cpu_ticks {
            current.cpu_usage_percent = previous.and_then(|old| {
                let (old_total, old_idle) = old.cpu_ticks?;
                let total_delta = total.checked_sub(old_total)?;
                let idle_delta = idle.checked_sub(old_idle)?;
                (total_delta > 0 && idle_delta <= total_delta).then(|| {
                    (100.0 * (total_delta - idle_delta) as f64 / total_delta as f64) as f32
                })
            });
        }
        let previous_interfaces: HashMap<_, _> = previous
            .into_iter()
            .flat_map(|old| &old.interfaces)
            .map(|interface| (interface.name.as_str(), interface))
            .collect();
        for interface in &mut current.interfaces {
            interface.receive_bytes_per_second = None;
            interface.transmit_bytes_per_second = None;
            if let (Some(elapsed), Some(old)) = (
                network_elapsed,
                previous_interfaces.get(interface.name.as_str()),
            ) {
                if let (Some(received), Some(transmitted)) = (
                    interface.received_bytes.checked_sub(old.received_bytes),
                    interface
                        .transmitted_bytes
                        .checked_sub(old.transmitted_bytes),
                ) {
                    interface.receive_bytes_per_second = Some(received as f64 / elapsed);
                    interface.transmit_bytes_per_second = Some(transmitted as f64 / elapsed);
                }
            }
        }
        self.previous = Some(current.clone());
        current
    }
}

pub fn parse_linux_system(bytes: &[u8]) -> Result<SystemSnapshot, InspectionError> {
    let text = std::str::from_utf8(bytes).map_err(|_| InspectionError::InvalidData)?;
    let mut lines = text.lines();
    if lines.next() != Some("RSTERM_SYSTEM_1") || lines.next() != Some("Linux") {
        return Err(InspectionError::Unsupported);
    }
    let architecture = lines
        .next()
        .filter(|line| !line.is_empty())
        .map(str::to_owned);
    let mut sections: HashMap<&str, String> = HashMap::new();
    let mut section = "";
    for line in lines {
        if line.starts_with("@@") {
            if !matches!(
                line,
                "@@os" | "@@stat" | "@@mem" | "@@uptime" | "@@cpu" | "@@net"
            ) || sections.contains_key(line)
            {
                return Err(InspectionError::InvalidData);
            }
            section = line;
            sections.entry(section).or_default();
        } else if !section.is_empty() {
            let value = sections.entry(section).or_default();
            value.push_str(line);
            value.push('\n');
        } else {
            return Err(InspectionError::InvalidData);
        }
    }
    let mut snapshot = SystemSnapshot {
        os: Some("Linux".into()),
        architecture,
        ..Default::default()
    };
    if let Some(release) = sections.get("@@os") {
        snapshot.os = release
            .lines()
            .find_map(|line| {
                line.strip_prefix("PRETTY_NAME=")
                    .map(|value| value.trim_matches('"').trim_matches('\'').to_owned())
            })
            .or(snapshot.os);
    }
    snapshot.uptime_seconds = sections
        .get("@@uptime")
        .and_then(|value| value.split_whitespace().next()?.parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value >= 0.0);
    snapshot.sample_seconds = snapshot.uptime_seconds;
    if let Some(stat) = sections.get("@@stat") {
        snapshot.logical_cores = Some(
            stat.lines()
                .filter(|line| {
                    line.strip_prefix("cpu")
                        .is_some_and(|suffix| suffix.starts_with(|ch: char| ch.is_ascii_digit()))
                })
                .count(),
        )
        .filter(|count| *count > 0);
        snapshot.cpu_ticks = stat.lines().find_map(|line| {
            let mut fields = line.split_whitespace();
            if fields.next()? != "cpu" {
                return None;
            }
            let ticks = fields
                .take(8)
                .map(str::parse::<u64>)
                .collect::<Result<Vec<_>, _>>()
                .ok()?;
            if ticks.len() < 4 {
                return None;
            }
            let total = ticks
                .iter()
                .try_fold(0_u64, |sum, value| sum.checked_add(*value))?;
            let idle = ticks[3].checked_add(*ticks.get(4).unwrap_or(&0))?;
            Some((total, idle))
        });
    }
    if let Some(memory) = sections.get("@@mem") {
        let values: HashMap<_, _> = memory
            .lines()
            .filter_map(|line| {
                let (name, value) = line.split_once(':')?;
                let mut fields = value.split_whitespace();
                let amount = fields.next()?.parse::<u64>().ok()?;
                (fields.next() == Some("kB")).then_some((name, amount.checked_mul(1024)?))
            })
            .collect();
        snapshot.memory_total_bytes = values.get("MemTotal").copied().filter(|total| *total > 0);
        snapshot.memory_used_bytes = snapshot
            .memory_total_bytes
            .zip(values.get("MemAvailable").copied())
            .and_then(|(total, available)| total.checked_sub(available));
    }
    snapshot.cpu_model = sections.get("@@cpu").and_then(|cpu| {
        cpu.lines().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            matches!(key.trim(), "model name" | "Hardware").then(|| value.trim().to_owned())
        })
    });
    if let Some(network) = sections.get("@@net") {
        snapshot.interfaces = network
            .lines()
            .filter_map(|line| {
                let (name, counters) = line.rsplit_once(':')?;
                let fields: Vec<_> = counters.split_whitespace().collect();
                Some(InterfaceSample {
                    name: name.trim().to_owned(),
                    received_bytes: fields.first()?.parse().ok()?,
                    transmitted_bytes: fields.get(8)?.parse().ok()?,
                    ..Default::default()
                })
            })
            .collect();
    }
    snapshot
        .interfaces
        .sort_by(|left, right| left.name.cmp(&right.name));
    if snapshot.cpu_ticks.is_none()
        && snapshot.memory_total_bytes.is_none()
        && snapshot.uptime_seconds.is_none()
        && snapshot.interfaces.is_empty()
    {
        return Err(InspectionError::Unavailable);
    }
    if snapshot.cpu_ticks.is_none() {
        snapshot.notes.push("CPU counters are unavailable".into());
    }
    if snapshot.memory_total_bytes.is_none() {
        snapshot
            .notes
            .push("Memory counters are unavailable".into());
    }
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(time: f64, total: u64, idle: u64, rx: u64) -> SystemSnapshot {
        SystemSnapshot {
            uptime_seconds: Some(time),
            cpu_ticks: Some((total, idle)),
            interfaces: vec![InterfaceSample {
                name: "en0".into(),
                received_bytes: rx,
                transmitted_bytes: rx,
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn deltas_warm_reset_and_handle_counter_rollback() {
        let mut sampler = SystemSampler::default();
        assert_eq!(
            sampler.sample(sample(1.0, 100, 20, 5)).cpu_usage_percent,
            None
        );
        let next = sampler.sample(sample(3.0, 200, 60, 105));
        assert_eq!(next.cpu_usage_percent, Some(60.0));
        assert_eq!(next.interfaces[0].receive_bytes_per_second, Some(50.0));
        let rollback = sampler.sample(sample(4.0, 10, 5, 1));
        assert_eq!(rollback.cpu_usage_percent, None);
        assert_eq!(rollback.interfaces[0].receive_bytes_per_second, None);
        let cpu_reset_only = sampler.sample(sample(4.5, 5, 2, 100));
        assert_eq!(cpu_reset_only.interfaces[0].receive_bytes_per_second, None);
        sampler.reset();
        assert_eq!(
            sampler.sample(sample(5.0, 20, 10, 2)).cpu_usage_percent,
            None
        );
    }

    #[test]
    fn interface_order_is_stable_across_unordered_samples() {
        let mut sampler = SystemSampler::default();
        for names in [["utun4", "lo0", "en2"], ["en2", "utun4", "lo0"]] {
            let mut snapshot = sample(1.0, 100, 50, 100);
            snapshot.interfaces = names
                .into_iter()
                .map(|name| InterfaceSample {
                    name: name.into(),
                    ..Default::default()
                })
                .collect();
            let snapshot = sampler.sample(snapshot);
            assert_eq!(
                snapshot
                    .interfaces
                    .iter()
                    .map(|interface| interface.name.as_str())
                    .collect::<Vec<_>>(),
                ["en2", "lo0", "utun4"]
            );
        }
        let snapshot = parse_linux_system(b"RSTERM_SYSTEM_1\nLinux\nx86_64\n@@net\n z0: 1 0 0 0 0 0 0 0 2\n a0: 3 0 0 0 0 0 0 0 4\n").unwrap();
        assert_eq!(
            snapshot
                .interfaces
                .iter()
                .map(|interface| interface.name.as_str())
                .collect::<Vec<_>>(),
            ["a0", "z0"]
        );
    }

    #[test]
    fn interface_set_changes_rewarm_surviving_interfaces() {
        let mut sampler = SystemSampler::default();
        sampler.sample(sample(1.0, 100, 50, 100));
        let mut added = sample(2.0, 200, 100, 200);
        added.interfaces.push(InterfaceSample {
            name: "utun0".into(),
            ..Default::default()
        });
        let added = sampler.sample(added);
        assert!(added
            .interfaces
            .iter()
            .all(|interface| interface.receive_bytes_per_second.is_none()));
        let removed = sampler.sample(sample(3.0, 300, 150, 300));
        assert_eq!(removed.interfaces[0].receive_bytes_per_second, None);
        let stable = sampler.sample(sample(4.0, 400, 200, 400));
        assert_eq!(stable.interfaces[0].receive_bytes_per_second, Some(100.0));
    }

    #[test]
    fn linux_ignores_guest_and_preserves_missing_fields() {
        let snapshot = parse_linux_system(b"RSTERM_SYSTEM_1\nLinux\naarch64\n@@stat\ncpu 10 0 10 50 5 1 1 1 999 999\ncpu0 0\n@@mem\nMemTotal: 100 kB\nMemAvailable: 40 kB\n@@uptime\n2.5 0\n@@net\n eth0: 10 0 0 0 0 0 0 0 30 0 0 0 0 0 0 0\n").unwrap();
        assert_eq!(snapshot.cpu_ticks, Some((78, 55)));
        assert_eq!(snapshot.memory_used_bytes, Some(60 * 1024));
        assert_eq!(snapshot.logical_cores, Some(1));
        assert_eq!(snapshot.interfaces[0].transmitted_bytes, 30);
        assert_eq!(
            parse_linux_system(b"banner\nLinux\n").unwrap_err(),
            InspectionError::Unsupported
        );
    }
}
