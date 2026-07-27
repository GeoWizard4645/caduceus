//! Developer environment inspection: ports, repositories, SSH hosts, containers.
//!
//! Everything here reads what is already on the machine. Nothing is installed,
//! nothing is started, and the two things that can act — killing a process on a
//! port and stopping a container — take an explicit identifier rather than a
//! pattern, so there is no way to phrase a request that matches more than one.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

use super::ToolOutcome;

fn run_tool(program: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| format!("could not run {program}: {e}"))?;
    // `lsof` and `docker` both exit non-zero simply because nothing matched, so
    // stdout is worth reading either way.
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if out.status.success() || !stdout.is_empty() {
        Ok(stdout)
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn tool_exists(program: &str) -> bool {
    Command::new("which")
        .arg(program)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Ports
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortUser {
    pub port: u16,
    pub pid: u32,
    pub process: String,
}

/// Which processes are listening, optionally filtered to one port.
///
/// `-sTCP:LISTEN` matters: without it an idle browser tab with a websocket open
/// shows up as "using" port 3000, and killing it does not free anything.
pub fn listening_ports(port: Option<u16>) -> Vec<PortUser> {
    let spec = match port {
        Some(p) => format!("-i:{p}"),
        None => "-i".to_string(),
    };
    let Ok(output) = run_tool("lsof", &[&spec, "-P", "-n", "-sTCP:LISTEN", "-F", "pcn"]) else {
        return Vec::new();
    };

    // `-F pcn` emits one field per line, prefixed by its letter, grouped per
    // process. Parsing that is far more robust than splitting the human table,
    // whose columns shift with long process names.
    let mut found: Vec<PortUser> = Vec::new();
    let mut pid = 0u32;
    let mut name = String::new();

    for line in output.lines() {
        let (tag, value) = line.split_at(1);
        match tag {
            "p" => pid = value.parse().unwrap_or(0),
            "c" => name = value.to_string(),
            "n" => {
                // Looks like "*:3000" or "127.0.0.1:3000" or "[::1]:3000".
                if let Some(port_text) = value.rsplit(':').next() {
                    if let Ok(parsed) = port_text.parse::<u16>() {
                        if !found.iter().any(|u| u.port == parsed && u.pid == pid) {
                            found.push(PortUser { port: parsed, pid, process: name.clone() });
                        }
                    }
                }
            }
            _ => {}
        }
    }

    found.sort_by_key(|u| (u.port, u.pid));
    found
}

/// Free a port by ending whatever is listening on it.
pub fn free_port(port: u16) -> ToolOutcome {
    let users = listening_ports(Some(port));
    if users.is_empty() {
        return ToolOutcome::ok(format!("Nothing is listening on port {port}."));
    }

    let mut ended = Vec::new();
    let mut refused = Vec::new();
    for user in &users {
        // SIGTERM, never SIGKILL: a dev server that is asked politely flushes
        // its logs and removes its socket file.
        match run_tool("kill", &["-TERM", &user.pid.to_string()]) {
            Ok(_) => ended.push(format!("{} ({})", user.process, user.pid)),
            Err(_) => refused.push(format!("{} ({})", user.process, user.pid)),
        }
    }

    if ended.is_empty() {
        ToolOutcome::err(format!("Could not stop {}", refused.join(", ")))
    } else {
        ToolOutcome::ok(format!("Port {port} freed — stopped {}", ended.join(", ")))
    }
}

// ---------------------------------------------------------------------------
// Git repositories
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitRepo {
    pub name: String,
    pub path: String,
    pub branch: String,
    /// Number of changed files, or `None` when git could not be asked.
    pub dirty: Option<usize>,
}

/// Directories scanned for repositories, in the order most people keep them.
fn repo_roots() -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    ["Developer", "Projects", "Code", "src", "dev", "repos", "Documents/GitHub"]
        .iter()
        .map(|name| home.join(name))
        .filter(|path| path.is_dir())
        .collect()
}

/// Find git repositories under the usual project directories.
///
/// Only descends two levels. A recursive walk of a home directory takes seconds
/// and finds every `node_modules` fixture repository on the disk, which is not
/// what "switch to a project" means.
pub fn git_repos(limit: usize) -> Vec<GitRepo> {
    let mut repos = Vec::new();

    for root in repo_roots() {
        collect_repos(&root, 2, &mut repos, limit);
        if repos.len() >= limit {
            break;
        }
    }

    repos.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    repos
}

fn collect_repos(dir: &Path, depth: usize, out: &mut Vec<GitRepo>, limit: usize) {
    if out.len() >= limit || depth == 0 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        if out.len() >= limit {
            return;
        }
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name == "node_modules" || name == "target" {
            continue;
        }

        if path.join(".git").exists() {
            out.push(describe_repo(&path, &name));
        } else {
            collect_repos(&path, depth - 1, out, limit);
        }
    }
}

fn describe_repo(path: &Path, name: &str) -> GitRepo {
    // Reading .git/HEAD directly avoids spawning a git process per repository,
    // which is the difference between an instant list and a visible pause.
    let branch = std::fs::read_to_string(path.join(".git/HEAD"))
        .ok()
        .and_then(|head| {
            head.trim()
                .strip_prefix("ref: refs/heads/")
                .map(str::to_string)
                // A detached HEAD holds a bare hash.
                .or_else(|| Some(format!("detached at {}", &head.trim()[..7.min(head.trim().len())])))
        })
        .unwrap_or_else(|| "unknown".into());

    GitRepo {
        name: name.to_string(),
        path: path.to_string_lossy().to_string(),
        branch,
        dirty: None,
    }
}

/// Count uncommitted changes in one repository.
///
/// Separate from [`git_repos`] on purpose: this shells out to git, so it is run
/// for the highlighted row only rather than for every repository on the disk.
pub fn git_status(path: &str) -> Option<usize> {
    let out = Command::new("git")
        .args(["-C", path, "status", "--porcelain"])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).lines().count())
}

// ---------------------------------------------------------------------------
// SSH hosts
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SshHost {
    pub alias: String,
    pub hostname: String,
    pub user: String,
}

/// Parse `~/.ssh/config` into the hosts it defines.
pub fn ssh_hosts() -> Vec<SshHost> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let Ok(config) = std::fs::read_to_string(home.join(".ssh/config")) else {
        return Vec::new();
    };
    parse_ssh_config(&config)
}

/// Split out from [`ssh_hosts`] so the parser is testable without a real file.
fn parse_ssh_config(config: &str) -> Vec<SshHost> {
    let mut hosts: Vec<SshHost> = Vec::new();

    for line in config.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((keyword, value)) = line.split_once(|c: char| c.is_whitespace()) else {
            continue;
        };
        let value = value.trim();

        // Keywords are case-insensitive per ssh_config(5).
        match keyword.to_lowercase().as_str() {
            "host" => {
                for alias in value.split_whitespace() {
                    // A pattern is a rule, not a host you can connect to.
                    if alias.contains('*') || alias.contains('?') || alias == "!" {
                        continue;
                    }
                    hosts.push(SshHost {
                        alias: alias.to_string(),
                        hostname: String::new(),
                        user: String::new(),
                    });
                }
            }
            "hostname" => {
                if let Some(last) = hosts.last_mut() {
                    last.hostname = value.to_string();
                }
            }
            "user" => {
                if let Some(last) = hosts.last_mut() {
                    last.user = value.to_string();
                }
            }
            _ => {}
        }
    }

    for host in &mut hosts {
        if host.hostname.is_empty() {
            host.hostname = host.alias.clone();
        }
    }
    hosts
}

// ---------------------------------------------------------------------------
// Docker
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Container {
    pub id: String,
    pub name: String,
    pub image: String,
    pub status: String,
    pub running: bool,
}

/// Every container, running or not.
///
/// Uses the `docker` CLI rather than the daemon socket: the socket path moved
/// with Docker Desktop, moves again with Colima and OrbStack, and the CLI knows
/// where its own daemon is.
pub fn containers() -> Result<Vec<Container>, String> {
    if !tool_exists("docker") {
        return Err("Docker is not installed.".into());
    }
    let output = run_tool(
        "docker",
        &["ps", "-a", "--format", "{{.ID}}\t{{.Names}}\t{{.Image}}\t{{.State}}\t{{.Status}}"],
    )
    .map_err(|e| {
        if e.contains("Cannot connect") || e.contains("daemon") {
            "Docker is installed but not running.".to_string()
        } else {
            e
        }
    })?;

    Ok(output
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split('\t').collect();
            if fields.len() < 5 {
                return None;
            }
            Some(Container {
                id: fields[0].to_string(),
                name: fields[1].to_string(),
                image: fields[2].to_string(),
                running: fields[3] == "running",
                status: fields[4].to_string(),
            })
        })
        .collect())
}

/// Start, stop or restart one container by id.
pub fn container_action(id: &str, action: &str) -> ToolOutcome {
    if !matches!(action, "start" | "stop" | "restart") {
        return ToolOutcome::err("Unknown container action.");
    }
    match run_tool("docker", &[action, id]) {
        Ok(_) => ToolOutcome::ok(format!("Container {action}ed")),
        Err(e) => ToolOutcome::err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- ssh config --------------------------------------------------------

    #[test]
    fn ssh_hosts_are_read_with_their_hostname_and_user() {
        let config = "\
Host staging
    HostName staging.example.com
    User deploy
    Port 2222

Host db
    HostName 10.0.0.4
    User postgres
";
        let hosts = parse_ssh_config(config);
        assert_eq!(hosts.len(), 2);
        assert_eq!(hosts[0].alias, "staging");
        assert_eq!(hosts[0].hostname, "staging.example.com");
        assert_eq!(hosts[0].user, "deploy");
        assert_eq!(hosts[1].hostname, "10.0.0.4");
    }

    #[test]
    fn wildcard_patterns_are_rules_not_hosts() {
        let hosts = parse_ssh_config("Host *\n  ForwardAgent yes\n\nHost real\n  HostName r.example.com\n");
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].alias, "real");
    }

    #[test]
    fn a_host_with_no_hostname_connects_to_its_own_alias() {
        let hosts = parse_ssh_config("Host box\n    User me\n");
        assert_eq!(hosts[0].hostname, "box");
    }

    #[test]
    fn keywords_are_case_insensitive_and_comments_are_skipped() {
        let hosts = parse_ssh_config("# a comment\nHOST web\n    hostname web.example.com\n    USER root\n");
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].hostname, "web.example.com");
        assert_eq!(hosts[0].user, "root");
    }

    #[test]
    fn several_aliases_on_one_line_all_become_hosts() {
        let hosts = parse_ssh_config("Host web1 web2\n    HostName shared.example.com\n");
        assert_eq!(hosts.len(), 2);
        // The HostName applies to the entry it follows, which is the last one.
        assert_eq!(hosts[1].hostname, "shared.example.com");
        assert_eq!(hosts[0].hostname, "web1");
    }

    #[test]
    fn an_empty_config_yields_no_hosts() {
        assert!(parse_ssh_config("").is_empty());
        assert!(parse_ssh_config("\n\n# only comments\n").is_empty());
    }

    // --- ports -------------------------------------------------------------

    #[test]
    fn listening_ports_are_reported_with_a_pid_and_a_name() {
        // Any Mac running these tests has something listening; if not, an empty
        // list is still correct and this asserts on shape, not on presence.
        for user in listening_ports(None) {
            assert!(user.pid > 0, "{user:?}");
            assert!(!user.process.is_empty(), "{user:?}");
            assert!(user.port > 0, "{user:?}");
        }
    }

    #[test]
    fn freeing_an_unused_port_says_so_rather_than_failing() {
        // 1 is reserved and nothing binds it without root.
        let outcome = free_port(1);
        assert!(outcome.ok);
        assert!(outcome.message.contains("Nothing is listening"), "{}", outcome.message);
    }

    // --- docker ------------------------------------------------------------

    #[test]
    fn container_actions_are_a_closed_set() {
        let outcome = container_action("abc123", "rm -rf");
        assert!(!outcome.ok);
        assert!(outcome.message.contains("Unknown"), "{}", outcome.message);
    }

    #[test]
    fn a_machine_without_docker_is_told_so_plainly() {
        // Passes either way: the point is that it never panics and the error is
        // a sentence rather than a raw exec failure.
        match containers() {
            Ok(list) => {
                for container in list {
                    assert!(!container.id.is_empty());
                }
            }
            Err(e) => assert!(
                e.contains("not installed") || e.contains("not running"),
                "unhelpful error: {e}"
            ),
        }
    }

    // --- repositories ------------------------------------------------------

    #[test]
    fn repository_scanning_stops_at_the_limit() {
        let repos = git_repos(3);
        assert!(repos.len() <= 3);
        for repo in repos {
            assert!(!repo.branch.is_empty());
            assert!(Path::new(&repo.path).join(".git").exists());
        }
    }

    #[test]
    fn this_very_repository_reports_its_branch() {
        let here = env!("CARGO_MANIFEST_DIR");
        let root = Path::new(here).parent().unwrap();
        let repo = describe_repo(root, "Caduceus");
        assert!(!repo.branch.is_empty());
        assert_ne!(repo.branch, "unknown", "could not read .git/HEAD");
    }
}

/// Open a Terminal window connected to a configured SSH host.
///
/// The alias is checked against `~/.ssh/config` before anything is run, so this
/// cannot be used to execute an arbitrary command: the only string that reaches
/// the shell is one the user's own config file already contains.
pub fn ssh_connect(alias: &str) -> ToolOutcome {
    let alias = alias.trim();
    if !ssh_hosts().iter().any(|host| host.alias == alias) {
        return ToolOutcome::err(format!("{alias} is not a host in your SSH config."));
    }

    let script = format!(
        "tell application \"Terminal\"\n\
         activate\n\
         do script \"ssh {alias}\"\n\
         end tell"
    );
    match run_tool("osascript", &["-e", &script]) {
        Ok(_) => ToolOutcome::ok(format!("Connecting to {alias}")),
        Err(e) if e.contains("-1743") => ToolOutcome::err(
            "Caduceus is not allowed to control Terminal yet. Grant it in System Settings → \
             Privacy & Security → Automation.",
        ),
        Err(e) => ToolOutcome::err(e),
    }
}
