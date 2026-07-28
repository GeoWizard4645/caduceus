//! System status: what is running, what it costs, and how the machine is doing.
//!
//! Backed by one long-lived [`sysinfo::System`] rather than a fresh snapshot per
//! call, because **CPU percentages are a delta**: they are computed against the
//! previous refresh, so a newly-created `System` reports either zero or garbage
//! for every process. Keeping the instance alive across polls is what makes the
//! numbers mean anything.
//!
//! Everything here is read-only except [`kill`], which is deliberately a
//! separate command so "look at my system" and "terminate a process" are never
//! the same click.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use parking_lot::Mutex;
use serde::Serialize;
use sysinfo::{Disks, Networks, Pid, ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};

/// sysinfo requires at least this long between refreshes for CPU deltas to be
/// meaningful; polling faster returns noise.
pub const MIN_POLL_MS: u64 = 500;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessRow {
    pub pid: u32,
    pub name: String,
    /// Percent of *one* core, so this exceeds 100 on a multi-threaded process —
    /// same convention Activity Monitor uses.
    pub cpu: f32,
    pub memory_bytes: u64,
    /// True when the process belongs to the user running Caduceus. Killing
    /// anything else needs privileges we do not have, so the UI greys it out
    /// rather than offering a button that always fails.
    pub own: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessGroupRow {
    /// Human-readable app name (bundle name when inside a `.app`).
    pub name: String,
    /// Sum of child CPU usages — can exceed 100% on multi-core machines.
    pub cpu: f32,
    pub memory_bytes: u64,
    pub own: bool,
    /// A representative pid for the app (often the main executable in the bundle).
    pub root_pid: Option<u32>,
    pub processes: Vec<ProcessRow>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskRow {
    pub name: String,
    pub mount_point: String,
    pub total_bytes: u64,
    pub available_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemSnapshot {
    pub cpu_percent: f32,
    pub core_count: usize,
    pub memory_used_bytes: u64,
    pub memory_total_bytes: u64,
    pub swap_used_bytes: u64,
    pub swap_total_bytes: u64,
    /// Bytes since the previous snapshot, not since boot — a rate, which is
    /// what anyone reading a network row actually wants.
    pub net_down_bytes: u64,
    pub net_up_bytes: u64,
    pub uptime_secs: u64,
    pub load_average: [f64; 3],
    pub host_name: Option<String>,
    pub os_version: Option<String>,
    pub kernel_version: Option<String>,
    pub disks: Vec<DiskRow>,
    pub process_groups: Vec<ProcessGroupRow>,
    pub processes: Vec<ProcessRow>,
    /// How many processes existed before `limit` was applied, so the UI can say
    /// "showing 40 of 612" instead of implying that is everything.
    pub process_total: usize,
}

/// Live sampling state, managed by Tauri.
#[derive(Clone)]
pub struct SysMonitor {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    system: System,
    disks: Disks,
    networks: Networks,
    own_uid: Option<sysinfo::Uid>,
}

impl Default for SysMonitor {
    fn default() -> Self {
        Self::new()
    }
}

impl SysMonitor {
    pub fn new() -> Self {
        let system = System::new_with_specifics(
            RefreshKind::nothing()
                .with_cpu(sysinfo::CpuRefreshKind::everything())
                .with_memory(sysinfo::MemoryRefreshKind::everything()),
        );

        Self {
            inner: Arc::new(Mutex::new(Inner {
                system,
                disks: Disks::new_with_refreshed_list(),
                networks: Networks::new_with_refreshed_list(),
                own_uid: own_uid(),
            })),
        }
    }

    /// Refresh and return a snapshot.
    ///
    /// By default groups are sorted by CPU (or memory when `sort_by_memory`) and
    /// truncated to `limit` — for the system monitor's "what is hot" view.
    /// When `sort_by_name` is true, groups and their children are sorted
    /// alphabetically instead, which keeps the Processes tool list stable while
    /// stats refresh.
    pub fn snapshot(&self, limit: usize, sort_by_memory: bool, sort_by_name: bool) -> SystemSnapshot {
        let mut inner = self.inner.lock();
        let Inner {
            system,
            disks,
            networks,
            own_uid,
        } = &mut *inner;

        system.refresh_cpu_usage();
        system.refresh_memory();
        system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing()
                .with_cpu()
                .with_memory()
                .with_exe(sysinfo::UpdateKind::OnlyIfNotSet)
                .with_user(sysinfo::UpdateKind::OnlyIfNotSet),
        );
        disks.refresh(true);
        networks.refresh(true);

        let process_total = system.processes().len();

        let mut groups_map: HashMap<String, ProcessGroupRow> = HashMap::new();
        for (pid, p) in system.processes() {
            let row = ProcessRow {
                pid: pid.as_u32(),
                name: p.name().to_string_lossy().to_string(),
                cpu: p.cpu_usage(),
                memory_bytes: p.memory(),
                own: match (own_uid.as_ref(), p.user_id()) {
                    (Some(mine), Some(theirs)) => mine == theirs,
                    _ => false,
                },
            };
            let (group_name, root_pid) = resolve_app_group(*pid, system);
            let group = groups_map.entry(group_name.clone()).or_insert_with(|| ProcessGroupRow {
                name: group_name,
                cpu: 0.0,
                memory_bytes: 0,
                own: false,
                root_pid: Some(root_pid.as_u32()),
                processes: Vec::new(),
            });
            group.cpu += row.cpu;
            group.memory_bytes += row.memory_bytes;
            group.own |= row.own;
            group.processes.push(row);
        }

        let mut process_groups: Vec<ProcessGroupRow> = groups_map.into_values().collect();
        for group in &mut process_groups {
            if sort_by_name {
                group.processes.sort_by(|a, b| {
                    a.name
                        .to_lowercase()
                        .cmp(&b.name.to_lowercase())
                        .then_with(|| a.pid.cmp(&b.pid))
                });
            } else {
                group.processes.sort_by(|a, b| {
                    b.cpu
                        .partial_cmp(&a.cpu)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then_with(|| b.memory_bytes.cmp(&a.memory_bytes))
                });
            }
            if group.root_pid.is_none() {
                group.root_pid = group.processes.first().map(|p| p.pid);
            }
        }

        if sort_by_name {
            process_groups.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        } else if sort_by_memory {
            process_groups.sort_by(|a, b| b.memory_bytes.cmp(&a.memory_bytes));
        } else {
            process_groups.sort_by(|a, b| {
                b.cpu
                    .partial_cmp(&a.cpu)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| b.memory_bytes.cmp(&a.memory_bytes))
            });
        }
        process_groups.truncate(limit);

        let mut processes: Vec<ProcessRow> = process_groups
            .iter()
            .flat_map(|g| g.processes.iter().cloned())
            .collect();

        // Legacy flat sort (unused for display once grouped, kept for callers that
        // still read `processes` directly).
        if sort_by_name {
            processes.sort_by(|a, b| {
                a.name
                    .to_lowercase()
                    .cmp(&b.name.to_lowercase())
                    .then_with(|| a.pid.cmp(&b.pid))
            });
        } else if sort_by_memory {
            processes.sort_by(|a, b| b.memory_bytes.cmp(&a.memory_bytes));
        } else {
            processes.sort_by(|a, b| {
                b.cpu
                    .partial_cmp(&a.cpu)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| b.memory_bytes.cmp(&a.memory_bytes))
            });
        }

        let (net_down_bytes, net_up_bytes) = networks
            .iter()
            .fold((0, 0), |(down, up), (_, data)| {
                (down + data.received(), up + data.transmitted())
            });

        SystemSnapshot {
            cpu_percent: system.global_cpu_usage(),
            core_count: system.cpus().len(),
            memory_used_bytes: system.used_memory(),
            memory_total_bytes: system.total_memory(),
            swap_used_bytes: system.used_swap(),
            swap_total_bytes: system.total_swap(),
            net_down_bytes,
            net_up_bytes,
            uptime_secs: System::uptime(),
            load_average: {
                let avg = System::load_average();
                [avg.one, avg.five, avg.fifteen]
            },
            host_name: System::host_name(),
            os_version: System::long_os_version(),
            kernel_version: System::kernel_version(),
            disks: disks
                .iter()
                .map(|d| DiskRow {
                    name: d.name().to_string_lossy().to_string(),
                    mount_point: d.mount_point().to_string_lossy().to_string(),
                    total_bytes: d.total_space(),
                    available_bytes: d.available_space(),
                })
                .collect(),
            process_groups,
            processes,
            process_total,
        }
    }

    /// Ask a process to quit.
    ///
    /// `force` sends SIGKILL instead of SIGTERM. The polite signal is the
    /// default because an app that is given the chance to exit cleanly saves
    /// its work; force is there for the one that ignores it.
    pub fn kill(&self, pid: u32, force: bool) -> Result<(), String> {
        // Refusing to kill ourselves is not paranoia: Caduceus appears in its
        // own process list, and "quit" on that row would look like a crash.
        if pid == std::process::id() {
            return Err("That is Caduceus itself. Quit it from the menu-bar icon.".into());
        }

        let inner = self.inner.lock();
        let process = inner
            .system
            .process(Pid::from_u32(pid))
            .ok_or_else(|| format!("Process {pid} is no longer running."))?;

        let signalled = if force {
            process.kill_with(sysinfo::Signal::Kill).unwrap_or(false)
        } else {
            process.kill_with(sysinfo::Signal::Term).unwrap_or(false)
        };

        if signalled {
            Ok(())
        } else {
            Err(format!(
                "Could not quit {} — it is probably owned by another user or by the system.",
                process.name().to_string_lossy()
            ))
        }
    }
}

/// Display name from a path like `/Applications/Google Chrome.app/Contents/...`.
fn app_name_from_exe(path: &Path) -> Option<String> {
    let s = path.to_str()?;
    let lower = s.to_ascii_lowercase();
    let idx = lower.find(".app/")?;
    let before = &s[..idx];
    let bundle = before.rsplit('/').next()?;
    Some(
        bundle
            .strip_suffix(".app")
            .unwrap_or(bundle)
            .to_string(),
    )
}

/// Walk toward the root process until we find a `.app` bundle or run out of parents.
fn resolve_app_group(pid: Pid, system: &System) -> (String, Pid) {
    let mut current = pid;
    for _ in 0..48 {
        let Some(proc) = system.process(current) else {
            break;
        };

        if let Some(exe) = proc.exe() {
            if let Some(name) = app_name_from_exe(exe) {
                return (name, current);
            }
        }

        let name = proc.name().to_string_lossy();
        match proc.parent() {
            Some(parent) if parent.as_u32() > 1 => {
                let walk = name.starts_with("com.apple.WebKit")
                    || name.contains("Helper")
                    || name.contains("helper")
                    || (name.contains('.') && !name.contains(' ') && name.len() > 20);
                if walk {
                    current = parent;
                    continue;
                }
                return (name.to_string(), current);
            }
            _ => return (name.to_string(), current),
        }
    }

    let fallback = system
        .process(pid)
        .map(|p| p.name().to_string_lossy().to_string())
        .unwrap_or_else(|| "Unknown".into());
    (fallback, pid)
}

#[cfg(unix)]
fn own_uid() -> Option<sysinfo::Uid> {
    // sysinfo has no "current user" helper, so read our own process's uid via
    // a throwaway system that only looks at this pid.
    let pid = Pid::from_u32(std::process::id());
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing().with_user(sysinfo::UpdateKind::Always),
    );
    system.process(pid).and_then(|p| p.user_id().cloned())
}

#[cfg(not(unix))]
fn own_uid() -> Option<sysinfo::Uid> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_snapshot_describes_a_plausible_machine() {
        let monitor = SysMonitor::new();
        // Two passes: CPU usage is a delta and the first refresh has nothing to
        // compare against.
        let _ = monitor.snapshot(10, false, false);
        std::thread::sleep(std::time::Duration::from_millis(MIN_POLL_MS));
        let snap = monitor.snapshot(10, false, false);

        assert!(snap.core_count > 0, "a machine with no cores is not running this test");
        assert!(snap.memory_total_bytes > 0);
        assert!(
            snap.memory_used_bytes <= snap.memory_total_bytes,
            "used {} > total {}",
            snap.memory_used_bytes,
            snap.memory_total_bytes
        );
        assert!(snap.process_total > 0);
        assert!(snap.process_groups.len() <= 10, "group limit was not applied");
    }

    #[test]
    fn processes_come_back_heaviest_first() {
        let monitor = SysMonitor::new();
        let _ = monitor.snapshot(40, false, false);
        std::thread::sleep(std::time::Duration::from_millis(MIN_POLL_MS));
        let snap = monitor.snapshot(40, false, false);

        for pair in snap.process_groups.windows(2) {
            assert!(
                pair[0].cpu >= pair[1].cpu || pair[0].memory_bytes >= pair[1].memory_bytes,
                "{} then {} is out of order",
                pair[0].name,
                pair[1].name
            );
        }
    }

    #[test]
    fn sorting_by_memory_is_monotonic() {
        let monitor = SysMonitor::new();
        let snap = monitor.snapshot(40, true, false);
        for pair in snap.process_groups.windows(2) {
            assert!(pair[0].memory_bytes >= pair[1].memory_bytes);
        }
    }

    #[test]
    fn caduceus_refuses_to_kill_itself() {
        let monitor = SysMonitor::new();
        let _ = monitor.snapshot(200, false, false);
        let err = monitor
            .kill(std::process::id(), false)
            .expect_err("killing our own pid must be refused");
        assert!(err.contains("Caduceus"), "unhelpful message: {err}");
    }

    #[test]
    fn killing_a_dead_pid_reports_rather_than_panics() {
        let monitor = SysMonitor::new();
        let _ = monitor.snapshot(10, false, false);
        // Above the default pid_max on macOS and Linux, so it cannot exist.
        assert!(monitor.kill(4_000_000, false).is_err());
    }
}
