//! Container discovery via the `podman` CLI.
//!
//! Lookups are batched: a single `podman inspect` resolves every container a
//! request touches, so applying a fault costs at most one podman spawn.

use std::net::Ipv4Addr;
use std::path::Path;

use crate::util::run;

/// A container's network identity, as resolved from `podman inspect`.
pub struct Container {
    pub name: String,
    /// `SandboxKey` — a path under /run/netns/ naming the container's network
    /// namespace. Used instead of /proc/PID/ns/net because it's not subject to
    /// PID reuse and survives the container init process exiting.
    pub sandbox: String,
    /// Primary IPv4 address, if the container has one (host-networked
    /// containers have none).
    pub ip: Option<Ipv4Addr>,
}

impl Container {
    /// Validate that the sandbox path is present, returning it. A missing
    /// sandbox means the container isn't network-namespaced as expected.
    pub fn sandbox_path(&self) -> Result<&str, String> {
        if self.sandbox.is_empty() || !Path::new(&self.sandbox).exists() {
            return Err(format!(
                "container {} has no sandbox file ({})",
                self.name, self.sandbox
            ));
        }
        Ok(&self.sandbox)
    }

    pub fn require_ip(&self) -> Result<Ipv4Addr, String> {
        self.ip.ok_or_else(|| {
            format!(
                "container {} has no IPv4 address (host networking?)",
                self.name
            )
        })
    }
}

/// Resolve every named container in a single `podman inspect` call. The
/// `--format` template emits one tab-separated `sandbox<TAB>ip…` line per
/// container, in the order requested, so the results line up with `names`.
pub fn inspect(names: &[&str]) -> Result<Vec<Container>, String> {
    if names.is_empty() {
        return Ok(Vec::new());
    }
    // SandboxKey, then every network's IPAddress space-separated.
    const FMT: &str =
        "{{.NetworkSettings.SandboxKey}}\t{{range .NetworkSettings.Networks}}{{.IPAddress}} {{end}}";
    let mut args: Vec<&str> = vec!["inspect", "--format", FMT];
    args.extend_from_slice(names);
    let out = run("podman", &args)?;

    let mut containers = Vec::with_capacity(names.len());
    for (name, line) in names.iter().zip(out.lines()) {
        let mut fields = line.splitn(2, '\t');
        let sandbox = fields.next().unwrap_or("").trim().to_string();
        // First non-empty token of the IP field, parsed as IPv4. podman's
        // default bridge hands out IPv4; anything else (IPv6, empty) is treated
        // as "no usable IP".
        let ip = fields
            .next()
            .unwrap_or("")
            .split_whitespace()
            .next()
            .and_then(|s| s.parse::<Ipv4Addr>().ok());
        containers.push(Container {
            name: (*name).to_string(),
            sandbox,
            ip,
        });
    }
    if containers.len() != names.len() {
        return Err(format!(
            "podman inspect returned {} records for {} containers",
            containers.len(),
            names.len()
        ));
    }
    Ok(containers)
}
