//! Network lookups.
//!
//! # A note on "local-first"
//!
//! Two of these leave the machine: [`public_address`] and [`ping`] obviously
//! have to. Both run **only** when the user picks that row, both are named so it
//! is clear what they do, and neither reports anything about the user to the
//! endpoint beyond the request itself. Nothing in this file runs on a timer, at
//! launch, or in the background.

use std::process::Command;

use serde::Serialize;

use super::ToolOutcome;

fn run_tool(program: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| format!("could not run {program}: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if out.status.success() {
        Ok(stdout)
    } else if stdout.is_empty() {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    } else {
        // `ping` exits non-zero on packet loss but its output is the answer.
        Ok(stdout)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Interface {
    pub name: String,
    pub address: String,
    /// The friendly name macOS shows, e.g. "Wi-Fi" or "Thunderbolt Bridge".
    pub label: String,
}

/// Every interface with an IPv4 address, labelled the way Network settings does.
pub fn interfaces() -> Vec<Interface> {
    let Ok(ports) = run_tool("networksetup", &["-listallhardwareports"]) else {
        return Vec::new();
    };

    let mut result = Vec::new();
    let mut label = String::new();
    for line in ports.lines() {
        if let Some(name) = line.strip_prefix("Hardware Port: ") {
            label = name.trim().to_string();
        } else if let Some(device) = line.strip_prefix("Device: ") {
            let device = device.trim();
            if let Ok(address) = run_tool("ipconfig", &["getifaddr", device]) {
                if !address.is_empty() {
                    result.push(Interface {
                        name: device.to_string(),
                        address,
                        label: label.clone(),
                    });
                }
            }
        }
    }
    result
}

/// A summary of local addressing, for the output panel.
pub fn local_summary() -> ToolOutcome {
    let found = interfaces();
    if found.is_empty() {
        return ToolOutcome::err("No interface has an IP address right now.");
    }

    let primary = found[0].address.clone();
    let router = run_tool("sh", &["-c", "route -n get default 2>/dev/null | awk '/gateway/{print $2}'"])
        .unwrap_or_default();

    let mut lines: Vec<String> = found
        .iter()
        .map(|i| format!("{:<22} {:<8} {}", i.label, i.name, i.address))
        .collect();
    if !router.is_empty() {
        lines.push(String::new());
        lines.push(format!("{:<22} {:<8} {}", "Router", "", router));
    }

    ToolOutcome::copied(primary, lines.join("\n"))
}

/// This machine's address as the internet sees it.
///
/// Deliberately a plain-text endpoint that returns nothing but the address, and
/// called only when asked for.
pub async fn public_address() -> ToolOutcome {
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(6))
        .build()
    {
        Ok(client) => client,
        Err(e) => return ToolOutcome::err(format!("Could not start the request: {e}")),
    };

    match client.get("https://api.ipify.org").send().await {
        Ok(response) => match response.text().await {
            Ok(address) => {
                let address = address.trim().to_string();
                if address.is_empty() {
                    ToolOutcome::err("The lookup returned nothing.")
                } else {
                    ToolOutcome::copied(address.clone(), format!("Public address {address}"))
                }
            }
            Err(e) => ToolOutcome::err(format!("Could not read the reply: {e}")),
        },
        Err(e) if e.is_timeout() => ToolOutcome::err("The lookup timed out."),
        Err(_) => ToolOutcome::err("Could not reach the lookup service. Are you online?"),
    }
}

/// Resolve a hostname.
pub fn dns_lookup(host: &str) -> ToolOutcome {
    let host = host.trim();
    if host.is_empty() {
        return ToolOutcome::err("Type a hostname to look up.");
    }
    // `dig +short` gives the answer with none of the surrounding report.
    match run_tool("dig", &["+short", host]) {
        Ok(answer) if !answer.is_empty() => {
            ToolOutcome::copied(answer.clone(), format!("{host} → {}", answer.replace('\n', ", ")))
        }
        Ok(_) => ToolOutcome::err(format!("{host} did not resolve.")),
        Err(e) => ToolOutcome::err(format!("Lookup failed: {e}")),
    }
}

/// Five pings, summarised.
pub fn ping(host: &str) -> ToolOutcome {
    let host = host.trim();
    if host.is_empty() {
        return ToolOutcome::err("Type a host to ping.");
    }
    match run_tool("ping", &["-c", "5", "-t", "10", host]) {
        Ok(output) => {
            // The last two lines carry the loss percentage and the timings.
            let summary: Vec<&str> = output
                .lines()
                .filter(|l| l.contains("packet loss") || l.contains("round-trip") || l.contains("min/avg"))
                .collect();
            if summary.is_empty() {
                ToolOutcome::err(format!("{host} did not answer."))
            } else {
                ToolOutcome::ok(summary.join(" · "))
            }
        }
        Err(e) => ToolOutcome::err(format!("Could not ping {host}: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_host_is_refused_before_anything_is_run() {
        assert!(!dns_lookup("   ").ok);
        assert!(!ping("").ok);
    }

    #[test]
    fn every_reported_interface_has_an_address_and_a_label() {
        for interface in interfaces() {
            assert!(!interface.address.is_empty(), "{interface:?}");
            assert!(!interface.name.is_empty(), "{interface:?}");
            assert!(!interface.label.is_empty(), "{interface:?}");
            // `ipconfig getifaddr` only ever returns IPv4.
            assert_eq!(interface.address.split('.').count(), 4, "{interface:?}");
        }
    }

    #[test]
    fn a_hostname_that_does_not_exist_is_reported_not_faked() {
        let outcome = dns_lookup("this-host-does-not-exist.caduceus-test.invalid");
        assert!(!outcome.ok, "{}", outcome.message);
    }

    #[test]
    fn localhost_resolves() {
        // Uses the resolver, not the network, so it works offline.
        let outcome = dns_lookup("localhost");
        assert!(outcome.ok || outcome.message.contains("did not resolve"));
    }
}
