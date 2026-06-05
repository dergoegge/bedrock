//! nftables fault primitives, applied over NETLINK_NETFILTER directly via
//! libnftnl — no `nft` binary, no `nsenter`.
//!
//! Each fault gets its own table (named by the server, e.g. `bedrock_fault_3`)
//! inside the target container's network namespace. A whole-table delete is the
//! unit of teardown, so an individual fault can expire without disturbing any
//! other fault sharing the same netns.
//!
//! We `setns(2)` into the target netns and open the netlink socket there, so a
//! fault costs one netlink round-trip instead of spawning processes per rule.

use std::ffi::CString;
use std::fs::File;
use std::net::Ipv4Addr;

use nftnl::expr::InterfaceName;
use nftnl::{nft_expr, Batch, Chain, FinalizedBatch, ProtoFamily, Rule, Table};
use nix::sched::{setns, CloneFlags};

/// Base chain name within each fault's table.
const NFT_CHAIN: &str = "input";
/// `nfproto` value for IPv4, used to guard the `ip saddr` payload match (the
/// `inet` family carries both v4 and v6). Matches `NFPROTO_IPV4`.
const NFPROTO_IPV4: u8 = 2;

fn cstr(s: &str) -> Result<CString, String> {
    CString::new(s).map_err(|e| format!("invalid nft name {s:?}: {e}"))
}

/// Install a "partition all" fault: drop every non-loopback inbound packet,
/// isolating the container from all peers.
pub fn apply_partition_all(sandbox: &str, table_name: &str) -> Result<(), String> {
    in_netns(sandbox, || send_batch(&partition_all_batch(table_name)?))
}

/// Install a "drop from peer" fault: drop inbound IPv4 packets sourced from
/// `peer`.
pub fn apply_drop_saddr(sandbox: &str, table_name: &str, peer: Ipv4Addr) -> Result<(), String> {
    in_netns(sandbox, || send_batch(&drop_saddr_batch(table_name, peer)?))
}

/// Tear down a fault's table. Deleting an absent table errors (ENOENT); callers
/// that are cleaning up best-effort should ignore the error.
pub fn delete_table(sandbox: &str, table_name: &str) -> Result<(), String> {
    in_netns(sandbox, || send_batch(&delete_table_batch(table_name)?))
}

/// Run `f` with the process's network namespace switched to the one named by
/// `sandbox`, restoring the original namespace afterward. The closure is where
/// any netlink socket must be opened so it binds to the target netns.
fn in_netns<T>(sandbox: &str, f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    let target = File::open(sandbox).map_err(|e| format!("open netns {sandbox}: {e}"))?;
    let current =
        File::open("/proc/self/ns/net").map_err(|e| format!("open current netns: {e}"))?;
    setns(&target, CloneFlags::CLONE_NEWNET).map_err(|e| format!("setns {sandbox}: {e}"))?;
    let result = f();
    // Always attempt to restore, even if `f` failed — leaving the process in
    // the container's netns would corrupt any subsequent operation.
    let restored =
        setns(&current, CloneFlags::CLONE_NEWNET).map_err(|e| format!("restore netns: {e}"));
    result.and_then(|v| restored.map(|()| v))
}

/// Send a finalized netlink batch on a fresh NETLINK_NETFILTER socket and drain
/// the kernel's acknowledgements, surfacing any error ACK as an `Err`. Must be
/// called inside the target netns so the socket binds there.
fn send_batch(batch: &FinalizedBatch) -> Result<(), String> {
    let socket =
        mnl::Socket::new(mnl::Bus::Netfilter).map_err(|e| format!("open netlink socket: {e}"))?;
    let portid = socket.portid();
    socket
        .send_all(batch)
        .map_err(|e| format!("send netlink batch: {e}"))?;

    let mut buf = vec![0u8; nftnl::nft_nlmsg_maxsize() as usize];
    // The kernel ACKs each message in the batch by sequence number; drain until
    // every expected sequence has been seen. cb_run turns an error ACK into an
    // Err, so a rejected rule (or a delete of an absent table) propagates here.
    let mut expected_seqs = batch.sequence_numbers();
    while !expected_seqs.is_empty() {
        for message in socket
            .recv(&mut buf[..])
            .map_err(|e| format!("recv netlink ack: {e}"))?
        {
            let message = message.map_err(|e| format!("malformed netlink ack: {e}"))?;
            let seq = expected_seqs.next().ok_or("unexpected extra netlink ACK")?;
            mnl::cb_run(message, seq, portid)
                .map_err(|e| format!("netlink batch rejected: {e}"))?;
        }
    }
    Ok(())
}

/// Add `table_name`'s table and its base input chain to `batch`, returning the
/// chain so rules can be hung off it.
fn add_table_and_chain<'a>(batch: &mut Batch, table: &'a Table) -> Result<Chain<'a>, String> {
    batch.add(table, nftnl::MsgType::Add);
    let mut chain = Chain::new(&cstr(NFT_CHAIN)?, table);
    // type filter hook input priority -100; policy accept.
    chain.set_hook(nftnl::Hook::In, -100);
    chain.set_policy(nftnl::Policy::Accept);
    batch.add(&chain, nftnl::MsgType::Add);
    Ok(chain)
}

fn make_table(table_name: &str) -> Result<Table, String> {
    Ok(Table::new(&cstr(table_name)?, ProtoFamily::Inet))
}

/// Batch isolating a container: drop all non-loopback inbound traffic
/// (`iifname != "lo"`). More general than enumerating peer IPs.
fn partition_all_batch(table_name: &str) -> Result<FinalizedBatch, String> {
    let table = make_table(table_name)?;
    let mut batch = Batch::new();
    let chain = add_table_and_chain(&mut batch, &table)?;
    let mut rule = Rule::new(&chain);
    rule.add_expr(&nft_expr!(meta iifname));
    rule.add_expr(&nft_expr!(cmp != InterfaceName::Exact(cstr("lo")?)));
    rule.add_expr(&nft_expr!(verdict drop));
    batch.add(&rule, nftnl::MsgType::Add);
    Ok(batch.finalize())
}

/// Batch dropping inbound IPv4 packets whose source address is `peer`. The
/// `meta nfproto ipv4` guard mirrors what `nft` adds implicitly for an
/// `ip saddr` match in an inet-family chain.
fn drop_saddr_batch(table_name: &str, peer: Ipv4Addr) -> Result<FinalizedBatch, String> {
    let table = make_table(table_name)?;
    let mut batch = Batch::new();
    let chain = add_table_and_chain(&mut batch, &table)?;
    let mut rule = Rule::new(&chain);
    rule.add_expr(&nft_expr!(meta nfproto));
    rule.add_expr(&nft_expr!(cmp == NFPROTO_IPV4));
    rule.add_expr(&nft_expr!(payload ipv4 saddr));
    rule.add_expr(&nft_expr!(cmp == peer));
    rule.add_expr(&nft_expr!(verdict drop));
    batch.add(&rule, nftnl::MsgType::Add);
    Ok(batch.finalize())
}

/// Batch removing a fault's table entirely (the unit of teardown).
fn delete_table_batch(table_name: &str) -> Result<FinalizedBatch, String> {
    let table = make_table(table_name)?;
    let mut batch = Batch::new();
    batch.add(&table, nftnl::MsgType::Del);
    Ok(batch.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::run;
    use nix::sched::unshare;

    /// Apply each fault primitive against a real kernel in a throwaway network
    /// namespace, confirming via the `nft` CLI that the kernel accepted the
    /// rules we built and that per-fault tables are independent.
    /// `unshare(CLONE_NEWNET)` affects only this test's thread, so it doesn't
    /// disturb the rest of the suite. The current netns ("/proc/self/ns/net")
    /// is used as the sandbox, so `in_netns` round-trips to it and back.
    ///
    /// Requires root, nf_tables, and the `nft` binary; where any is missing
    /// (e.g. an unprivileged build sandbox) it skips rather than fails.
    #[test]
    fn faults_apply_in_a_fresh_netns() {
        if unshare(CloneFlags::CLONE_NEWNET).is_err() {
            eprintln!("skipping: cannot unshare a network namespace (need root)");
            return;
        }
        if run("nft", &["--version"]).is_err() {
            eprintln!("skipping: `nft` binary unavailable");
            return;
        }
        let ns = "/proc/self/ns/net";

        // Fault 0: one-sided isolation (iifname != "lo" => drop).
        apply_partition_all(ns, "bedrock_fault_0").unwrap();
        let t0 = run("nft", &["list", "table", "inet", "bedrock_fault_0"]).expect("list t0");
        assert!(t0.contains("iifname"), "no iifname match:\n{t0}");
        assert!(t0.contains("\"lo\""), "no lo operand:\n{t0}");
        assert!(t0.contains("drop"), "no drop verdict:\n{t0}");

        // Fault 1: drop from a specific source address, in its own table.
        let peer: Ipv4Addr = "10.88.0.7".parse().unwrap();
        apply_drop_saddr(ns, "bedrock_fault_1", peer).unwrap();
        let t1 = run("nft", &["list", "table", "inet", "bedrock_fault_1"]).expect("list t1");
        assert!(t1.contains("10.88.0.7"), "no saddr operand:\n{t1}");

        // Expiring fault 0 removes only its table; fault 1 survives.
        delete_table(ns, "bedrock_fault_0").unwrap();
        assert!(
            run("nft", &["list", "table", "inet", "bedrock_fault_0"]).is_err(),
            "fault 0 table unexpectedly still present"
        );
        run("nft", &["list", "table", "inet", "bedrock_fault_1"]).expect("fault 1 should remain");

        // Deleting an absent table errors (ENOENT) — why clear swallows errors.
        assert!(
            delete_table(ns, "bedrock_fault_0").is_err(),
            "deleting an absent table unexpectedly succeeded"
        );
    }
}
