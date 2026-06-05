//! Bedrock workload-agnostic fault injector.
//!
//! A client/server tool that runs in the guest's host namespace (outside the
//! workload containers). The long-lived **server** (`fault-injector serve`)
//! owns the authoritative set of active faults: it injects them, tracks them,
//! expires duration-limited ones, and clears them all on request. Every other
//! subcommand is a thin **client** that connects over a unix socket, sends one
//! request, and reports the result.
//!
//! Splitting injection out into a server lets a fault outlive the brief client
//! invocation (so `--duration` doesn't block the caller), keeps an accurate
//! registry for precise teardown, and avoids re-discovering container topology
//! on every command.
//!
//! Commands:
//!   serve                                    run the fault server
//!   partition <container> [--duration D]     isolate <container> from all peers
//!   partition <container> <peer> [...]       drop traffic between the two
//!   clear                                    revert every tracked fault
//!
//! The command set is meant to grow: each fault kind is a request variant the
//! server handles, and `clear` reverts whatever state the active faults left
//! behind.
//!
//! The partition fault drops traffic with nftables rules installed inside the
//! target container's network namespace; see `nft.rs`. The control socket lives
//! in the host netns, so it is unaffected by the network faults being injected.

mod client;
mod nft;
mod podman;
mod protocol;
mod server;
mod util;

use clap::{Parser, Subcommand};

use protocol::{parse_duration_ms, Request};

#[derive(Parser)]
#[command(
    name = "fault-injector",
    about = "Workload-agnostic fault injector for the bedrock guest",
    disable_help_subcommand = true
)]
struct Cli {
    #[command(subcommand)]
    command: FaultCommand,
}

#[derive(Subcommand)]
enum FaultCommand {
    /// Run the fault server: inject faults, track them, and expire them by
    /// duration. Started once at guest boot; clients talk to it over the socket.
    Serve,
    /// Drop traffic to/from a container. With only <container>, isolate it from
    /// every peer; with <peer>, drop traffic symmetrically between the two.
    Partition {
        /// Container to apply the fault to.
        container: String,
        /// Optional peer; when given, only traffic between the two is dropped.
        peer: Option<String>,
        /// Auto-expire the fault after this long (e.g. `500ms`, `5s`, `2m`; a
        /// bare number is seconds). Omit to keep it until `clear`.
        #[arg(long, value_parser = parse_duration_ms, value_name = "DURATION")]
        duration: Option<u64>,
    },
    /// Revert every fault the server is tracking (idempotent).
    Clear,
}

fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        FaultCommand::Serve => server::run(),
        FaultCommand::Partition {
            container,
            peer,
            duration,
        } => client::run(&Request::Partition {
            container,
            peer,
            duration_ms: duration,
        }),
        FaultCommand::Clear => client::run(&Request::Clear),
    };
    if let Err(e) = result {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
