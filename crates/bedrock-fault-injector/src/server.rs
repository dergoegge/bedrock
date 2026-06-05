//! Fault server: owns the authoritative set of active faults, injects them on
//! request, expires them when their duration elapses, and clears them all on
//! demand.
//!
//! Single-threaded event loop built on `poll(2)`: it waits for either an
//! incoming client connection or the next fault's expiry deadline, whichever
//! comes first. Durations are measured with the guest's monotonic clock; the
//! hypervisor makes the whole guest deterministic, so expiry lands at a
//! reproducible point in the guest's execution.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::fd::AsFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::time::{Duration, Instant};

use nix::errno::Errno;
use nix::poll::{poll, PollFd, PollFlags, PollTimeout};

use crate::nft;
use crate::podman;
use crate::protocol::{Request, Response, SOCKET_PATH};
use crate::util::log;

/// One installed fault the server is tracking.
struct ActiveFault {
    /// nftables table name installed for this fault (same name in each netns).
    table: String,
    /// Netns sandbox paths the fault's table was installed into (one for a
    /// one-sided partition, two for a symmetric one).
    sandboxes: Vec<String>,
    /// When the fault should auto-expire; `None` means it persists until clear.
    deadline: Option<Instant>,
}

/// Server state: the fault registry plus the monotonic id counter that names
/// each fault's table.
struct Server {
    faults: HashMap<u64, ActiveFault>,
    next_id: u64,
}

impl Server {
    fn new() -> Self {
        Server {
            faults: HashMap::new(),
            next_id: 0,
        }
    }

    /// Apply `req`, mutating the registry. Returns a human error on failure.
    fn handle(&mut self, req: Request) -> Result<(), String> {
        match req {
            Request::Clear => {
                self.clear_all();
                Ok(())
            }
            Request::Partition {
                container,
                peer,
                duration_ms,
            } => self.apply_partition(&container, peer.as_deref(), duration_ms),
        }
    }

    fn apply_partition(
        &mut self,
        container: &str,
        peer: Option<&str>,
        duration_ms: Option<u64>,
    ) -> Result<(), String> {
        let id = self.next_id;
        let table = format!("bedrock_fault_{id}");
        let deadline = duration_ms.map(|ms| Instant::now() + Duration::from_millis(ms));
        let dur = duration_ms.map_or_else(String::new, |ms| format!(" for {ms}ms"));

        let sandboxes = match peer {
            Some(peer) if !peer.is_empty() => {
                let cs = podman::inspect(&[container, peer])?;
                let (a, b) = (&cs[0], &cs[1]);
                let a_sbx = a.sandbox_path()?.to_string();
                let b_sbx = b.sandbox_path()?.to_string();
                let (a_ip, b_ip) = (a.require_ip()?, b.require_ip()?);
                log(&format!(
                    "fault {id}: partition {} ({a_ip}) <-/-> {} ({b_ip}){dur}",
                    a.name, b.name
                ));
                nft::apply_drop_saddr(&a_sbx, &table, b_ip)?;
                // Roll back the first side if the second fails, so a failed
                // request leaves nothing behind.
                if let Err(e) = nft::apply_drop_saddr(&b_sbx, &table, a_ip) {
                    let _ = nft::delete_table(&a_sbx, &table);
                    return Err(e);
                }
                vec![a_sbx, b_sbx]
            }
            _ => {
                let cs = podman::inspect(&[container])?;
                let c = &cs[0];
                let sbx = c.sandbox_path()?.to_string();
                let ip = c.ip.map_or_else(|| "?".to_string(), |ip| ip.to_string());
                log(&format!("fault {id}: isolate {} (ip={ip}){dur}", c.name));
                nft::apply_partition_all(&sbx, &table)?;
                vec![sbx]
            }
        };

        self.next_id += 1;
        self.faults.insert(
            id,
            ActiveFault {
                table,
                sandboxes,
                deadline,
            },
        );
        Ok(())
    }

    /// Remove a single fault's nftables state (best-effort across its netns).
    fn teardown(id: u64, fault: &ActiveFault) {
        for sbx in &fault.sandboxes {
            if let Err(e) = nft::delete_table(sbx, &fault.table) {
                log(&format!(
                    "fault {id}: cleanup of {} in {sbx} failed: {e}",
                    fault.table
                ));
            }
        }
    }

    /// Remove every tracked fault.
    fn clear_all(&mut self) {
        let n = self.faults.len();
        for (id, fault) in self.faults.drain() {
            Self::teardown(id, &fault);
        }
        log(&format!("cleared {n} fault(s)"));
    }

    /// Tear down every fault whose deadline has passed.
    fn expire_due(&mut self, now: Instant) {
        let due: Vec<u64> = self
            .faults
            .iter()
            .filter(|(_, f)| f.deadline.is_some_and(|d| d <= now))
            .map(|(id, _)| *id)
            .collect();
        for id in due {
            if let Some(fault) = self.faults.remove(&id) {
                Self::teardown(id, &fault);
                log(&format!("fault {id} expired"));
            }
        }
    }

    /// Time until the soonest deadline, as a poll timeout. `NONE` (wait
    /// forever) when no fault has a deadline.
    fn next_timeout(&self, now: Instant) -> PollTimeout {
        match self.faults.values().filter_map(|f| f.deadline).min() {
            None => PollTimeout::NONE,
            Some(deadline) => {
                let remaining = deadline.saturating_duration_since(now);
                PollTimeout::try_from(remaining).unwrap_or(PollTimeout::MAX)
            }
        }
    }
}

/// Bind the socket and run the event loop forever.
pub fn run() -> Result<(), String> {
    // Clear any stale socket from a previous boot (the rootfs is ephemeral, but
    // be defensive) and bind.
    let _ = std::fs::remove_file(SOCKET_PATH);
    let listener =
        UnixListener::bind(SOCKET_PATH).map_err(|e| format!("bind {SOCKET_PATH}: {e}"))?;
    log(&format!("fault server listening on {SOCKET_PATH}"));

    let mut server = Server::new();
    loop {
        server.expire_due(Instant::now());
        let timeout = server.next_timeout(Instant::now());

        let mut fds = [PollFd::new(listener.as_fd(), PollFlags::POLLIN)];
        match poll(&mut fds, timeout) {
            Ok(0) => continue, // timed out — loop around to expire due faults
            Ok(_) => {}
            Err(Errno::EINTR) => continue,
            Err(e) => return Err(format!("poll: {e}")),
        }

        match listener.accept() {
            Ok((stream, _)) => {
                if let Err(e) = serve_one(&mut server, stream) {
                    log(&format!("client error: {e}"));
                }
            }
            Err(e) => log(&format!("accept failed: {e}")),
        }
    }
}

/// Read one request from `stream`, apply it, and write the response back.
fn serve_one(server: &mut Server, mut stream: UnixStream) -> Result<(), String> {
    let mut data = String::new();
    stream
        .read_to_string(&mut data)
        .map_err(|e| format!("read request: {e}"))?;
    let req: Request =
        serde_json::from_str(data.trim()).map_err(|e| format!("parse request {data:?}: {e}"))?;

    let response = match server.handle(req) {
        Ok(()) => Response::Ok,
        Err(e) => {
            log(&format!("request failed: {e}"));
            Response::Err(e)
        }
    };

    let mut out =
        serde_json::to_string(&response).map_err(|e| format!("serialize response: {e}"))?;
    out.push('\n');
    stream
        .write_all(out.as_bytes())
        .map_err(|e| format!("write response: {e}"))?;
    Ok(())
}
