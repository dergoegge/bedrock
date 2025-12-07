// SPDX-License-Identifier: GPL-2.0

//! Command-line interface for the bedrock hypervisor.
//!
//! Loads vmlinux ELF images and boots them using the Linux 64-bit boot protocol.

mod args;
mod elf;

use std::fs::File;
use std::io::{self, Read, Write};
use std::process;

use clap::Parser;
use log::{debug, info, trace, warn};

use bedrock_vm::{
    parse_line_tsc_entries, ExitKind, ExitStatsReport, LineTscEntry, LinuxBootConfig, LogConfig,
    LogEntry, RdrandConfig, Vm, VmBuilder, BEDROCK_DEVICE_PATH,
};


use args::{Args, RdrandMode};
use elf::load_kernel;

/// Line-buffered output that prefixes each line with a virtual time timestamp.
///
/// The timestamp format is `[vt x.xxx]` where x.xxx is the emulated TSC
/// converted to seconds since VM start. Uses per-line TSC values from
/// serial buffer metadata when available for accurate timestamps.
struct LineBufferedOutput {
    /// Partial line buffer (content before newline received).
    buffer: String,
    /// Optional file to write raw output (without timestamps).
    log_file: Option<File>,
}

impl LineBufferedOutput {
    fn new(log_file: Option<File>) -> Self {
        Self {
            buffer: String::new(),
            log_file,
        }
    }

    /// Process output from the guest, adding timestamps to each complete line.
    ///
    /// If `line_entries` is provided, uses per-line TSC values for accurate timestamps.
    /// Otherwise falls back to using `fallback_tsc` for all lines.
    fn write_with_line_tsc(
        &mut self,
        output: &str,
        line_entries: Option<&[LineTscEntry]>,
        fallback_tsc: u64,
        tsc_frequency: u64,
    ) {
        // Write raw output to log file if present
        if let Some(ref mut f) = self.log_file {
            let _ = f.write_all(output.as_bytes());
            let _ = f.flush();
        }

        // Track current byte offset to match with line entries
        let mut byte_offset: usize = 0;
        let mut line_idx: usize = 0;

        // Process output character by character
        for ch in output.chars() {
            if ch == '\n' {
                // Complete line - find the TSC for this line
                let tsc = if let Some(entries) = line_entries {
                    // Find the entry whose offset matches the start of this line
                    // The buffer contains the line content (excluding newline)
                    // The line started at (byte_offset - buffer.len())
                    let line_start = byte_offset.saturating_sub(self.buffer.len());
                    entries
                        .iter()
                        .skip(line_idx)
                        .find(|e| e.offset as usize == line_start)
                        .map(|e| {
                            line_idx += 1;
                            e.tsc
                        })
                        .unwrap_or(fallback_tsc)
                } else {
                    fallback_tsc
                };

                let secs = tsc as f64 / tsc_frequency as f64;
                println!("[vt {:>8.3}] {}", secs, self.buffer);
                self.buffer.clear();
            } else {
                self.buffer.push(ch);
            }
            byte_offset += ch.len_utf8();
        }

        let _ = std::io::stdout().flush();
    }

    /// Flush any remaining partial line (without timestamp since it's incomplete).
    fn flush_partial(&mut self) {
        if !self.buffer.is_empty() {
            print!("{}", self.buffer);
            let _ = std::io::stdout().flush();
            self.buffer.clear();
        }
    }
}

fn main() {
    if let Err(e) = run() {
        eprintln!("Error: {}", e);
        process::exit(1);
    }
}

/// Log a VM exit warning with RIP.
fn log_vm_exit(vm: &Vm, msg: &str) {
    let rip = vm.get_regs().map(|r| r.rip).unwrap_or(0);
    warn!("VM exit: {} at RIP {:#018x}", msg, rip);
}

/// Dump the feedback buffer to a file.
fn dump_feedback_buffer(vm: &mut Vm, path: &str) -> io::Result<()> {
    // Check if feedback buffer is registered
    let info = match vm.get_feedback_buffer_info()? {
        Some(info) => info,
        None => {
            warn!("No feedback buffer registered, skipping dump");
            return Ok(());
        }
    };

    // Map the feedback buffer if not already mapped
    let buffer = match vm.feedback_buffer() {
        Some(buf) => buf,
        None => vm.map_feedback_buffer()?,
    };

    // Write to file
    let mut file = File::create(path)?;
    file.write_all(buffer)?;

    info!(
        "Dumped feedback buffer to {} ({} bytes, {} pages)",
        path,
        buffer.len(),
        info.num_pages
    );

    Ok(())
}

/// Wait for Ctrl-C if wait flag is set.
fn maybe_wait_for_ctrl_c(wait: bool) {
    if wait {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        info!("Press Ctrl-C to exit...");

        let running = Arc::new(AtomicBool::new(true));
        let r = running.clone();

        ctrlc::set_handler(move || {
            r.store(false, Ordering::SeqCst);
        })
        .expect("Error setting Ctrl-C handler");

        while running.load(Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
}

/// Log an optional configuration value at debug level.
macro_rules! debug_opt {
    ($label:expr, $value:expr) => {
        if let Some(ref v) = $value {
            debug!("  {:<14}{}", $label, v);
        }
    };
    ($label:expr, $value:expr, $fmt:expr) => {
        if let Some(ref v) = $value {
            debug!("  {:<14}{}", $label, format!($fmt, v));
        }
    };
}

/// Build RDRAND config from command-line arguments.
fn build_rdrand_config(args: &Args) -> RdrandConfig {
    match args.rdrand_mode {
        RdrandMode::Constant => RdrandConfig::constant(args.rdrand_seed),
        RdrandMode::Seeded => RdrandConfig::seeded_rng(args.rdrand_seed),
        RdrandMode::Userspace => RdrandConfig::exit_to_userspace(),
    }
}

/// Build log config from command-line arguments.
/// Returns None if logging is not enabled.
fn build_log_config(args: &Args) -> Option<LogConfig> {
    let log_start_tsc = args.log_after_tsc.unwrap_or(0);

    let config = if args.single_step.is_some() {
        // Single-step mode uses TscRange logging
        Some(LogConfig::tsc_range().with_start_tsc(log_start_tsc))
    } else if args.log_at_shutdown {
        Some(LogConfig::at_shutdown().with_start_tsc(log_start_tsc))
    } else if let Some(target_tsc) = args.log_at_tsc {
        Some(LogConfig::at_tsc(target_tsc).with_start_tsc(log_start_tsc))
    } else if let Some(interval) = args.log_checkpoints {
        Some(LogConfig::checkpoints(interval).with_start_tsc(log_start_tsc))
    } else if args.should_enable_log() {
        Some(LogConfig::all_exits(0).with_start_tsc(log_start_tsc))
    } else {
        None
    };

    let config = if args.no_memory_hash {
        config.map(|c| c.with_no_memory_hash())
    } else {
        config
    };

    if args.intercept_pf {
        config.map(|c| c.with_intercept_pf())
    } else {
        config
    }
}

fn run() -> io::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args = Args::parse();
    let memory_size = args.memory * 1024 * 1024;

    debug!("Configuration:");
    debug!("  {:<14}{}", "Kernel:", args.vmlinux);
    debug!("  {:<14}{} MB", "Memory:", args.memory);
    debug!("  {:<14}\"{}\"", "Command line:", args.cmdline);
    debug_opt!("Initramfs:", args.initramfs);
    debug_opt!("Log file:", args.log);
    debug_opt!(
        "Serial input:",
        args.input.as_ref().map(|i| format!("{} bytes", i.len()))
    );
    debug!(
        "  {:<14}{}",
        "RDRAND mode:",
        match args.rdrand_mode {
            RdrandMode::Constant => format!("constant (value: {:#x})", args.rdrand_seed),
            RdrandMode::Seeded => format!("seeded (seed: {:#x})", args.rdrand_seed),
            RdrandMode::Userspace => "userspace (exit to userspace)".to_string(),
        }
    );
    if args.should_enable_log() {
        debug!("  {:<14}enabled", "Exit logging:");
        debug_opt!("Log JSONL:", args.log_jsonl);
    }
    debug_opt!(
        "Single-step:",
        args.single_step
            .map(|(s, e)| format!("TSC range [{}, {})", s, e))
    );
    debug_opt!("Stop at TSC:", args.stop_at_tsc);

    // Open log file if specified and create line-buffered output
    let log_file: Option<File> = args.log.as_ref().map(File::create).transpose()?;
    let mut output = LineBufferedOutput::new(log_file);

    // Build configs from args
    let rdrand_config = build_rdrand_config(&args);
    let log_config = build_log_config(&args);

    // Build VM configuration
    let mut builder = VmBuilder::new().rdrand(rdrand_config);

    if let Some(parent_id) = args.parent_id {
        debug!("  Parent VM ID: {}", parent_id);
        builder = builder.forked_from(parent_id);
    } else {
        builder = builder.memory_size(memory_size);
    }

    if let Some(config) = log_config {
        builder = builder.logging(config);
    }
    if let Some((start, end)) = args.single_step {
        builder = builder.single_step(start, end);
    }
    let stop_at_tsc = args.stop_at_tsc.or_else(|| {
        args.stop_at_vt
            .map(|vt| (vt * 2_995_200_000.0) as u64)
    });
    if let Some(tsc) = stop_at_tsc {
        builder = builder.stop_at_tsc(tsc);
    }

    // Create VM
    let mut vm: Vm = builder.build().map_err(|e| {
        io::Error::new(
            io::ErrorKind::Other,
            format!(
                "Failed to create VM: {}\nMake sure the bedrock kernel module is loaded:\n  sudo insmod bedrock.ko\nDevice path: {}",
                e, BEDROCK_DEVICE_PATH
            ),
        )
    })?;

    if let Some(parent_id) = args.parent_id {
        info!("Created forked VM (from parent {})", parent_id);

        // For forked VMs, map feedback buffer immediately if it exists and dump is requested
        if args.dump_feedback.is_some() {
            if let Ok(Some(_info)) = vm.get_feedback_buffer_info() {
                if let Err(e) = vm.map_feedback_buffer() {
                    warn!("Failed to map feedback buffer: {}", e);
                } else {
                    info!("Feedback buffer mapped (inherited from parent)");
                }
            }
        }
    } else {
        info!(
            "Created VM with {} MB guest memory",
            memory_size / (1024 * 1024)
        );
    }

    // Open JSONL files for logging (deterministic + non-deterministic)
    let mut log_jsonl_file: Option<std::io::BufWriter<File>> =
        if let Some(ref path) = args.log_jsonl {
            let f = File::create(path)?;
            Some(std::io::BufWriter::new(f))
        } else {
            None
        };
    let mut log_jsonl_nondeterm_file: Option<std::io::BufWriter<File>> =
        if let Some(ref path) = args.log_jsonl {
            let nondeterm_path = if let Some(stem) = path.strip_suffix(".jsonl") {
                format!("{}-nondeterm.jsonl", stem)
            } else {
                format!("{}-nondeterm", path)
            };
            let f = File::create(&nondeterm_path)?;
            Some(std::io::BufWriter::new(f))
        } else {
            None
        };
    let mut total_log_count: usize = 0;
    let mut total_nondeterm_log_count: usize = 0;

    // Setup for new VMs (not forked)
    if vm.is_root() {
        // Read kernel file
        let kernel_data = read_file(&args.vmlinux)?;

        // Read initramfs if provided
        let initramfs_data = args.initramfs.as_ref().map(|p| read_file(p)).transpose()?;

        // Load kernel into guest memory
        info!("Loading kernel from {}", args.vmlinux);
        let memory = vm.memory_mut().expect("Root VM must have memory");
        let (kernel_entry, kernel_end) = load_kernel(memory, &kernel_data)?;
        debug!("  Kernel entry point: {:#x}", kernel_entry);
        debug!("  Kernel ends at: {:#x}", kernel_end);

        // Build Linux boot configuration
        let mut boot_config = LinuxBootConfig::new(kernel_entry, kernel_end).cmdline(&args.cmdline);

        if let Some(ref data) = initramfs_data {
            info!("Loading initramfs ({} bytes)", data.len());
            boot_config = boot_config.initramfs(data);
        }

        if let Some(ref input) = args.input {
            debug!("Serial input: {} bytes queued", input.len());
            boot_config = boot_config.serial_input(input.as_bytes());
        }

        // Setup Linux boot (GDT, page tables, MP tables, boot_params, registers)
        debug!("Setting up Linux boot structures...");
        let boot_info = vm.setup_linux_boot(&boot_config).map_err(io_error)?;
        trace!(
            "  GDT at {:#x}, limit {:#x}",
            boot_info.gdt_base,
            boot_info.gdt_limit
        );
        if let Some(addr) = boot_info.initramfs_addr {
            debug!("  Initramfs at {:#x}", addr);
        }
    }

    // Run VM
    info!("Starting VM...");
    let wall_clock_start = std::time::Instant::now();
    let timeout_duration = args
        .timeout
        .map(std::time::Duration::from_secs_f64);

    loop {
        // Check wall-clock timeout
        if let Some(timeout) = timeout_duration {
            if wall_clock_start.elapsed() >= timeout {
                info!(
                    "Wall-clock timeout reached ({:.1}s)",
                    timeout.as_secs_f64()
                );
                break;
            }
        }

        match vm.run() {
            Ok(exit) => {
                // Print serial output with timestamps
                if exit.serial_len > 0 {
                    let serial_str = vm.serial_output_str(exit.serial_len as usize);
                    // Parse line TSC entries from the TSC metadata page for accurate per-line timestamps
                    let line_entries = parse_line_tsc_entries(vm.serial_tsc_buffer());
                    output.write_with_line_tsc(
                        serial_str,
                        line_entries.as_deref(),
                        exit.emulated_tsc,
                        exit.tsc_frequency,
                    );
                }

                // Write log entries to JSONL (split by deterministic flag)
                if exit.log_entry_count > 0 {
                    if let Some(buffer) = vm.log_buffer() {
                        let entries = LogEntry::from_buffer(buffer, exit.log_entry_count as usize);
                        for entry in entries {
                            if entry.is_deterministic() {
                                if let Some(ref mut w) = log_jsonl_file {
                                    let _ = serde_json::to_writer(&mut *w, entry);
                                    let _ = writeln!(w);
                                }
                                total_log_count += 1;
                            } else {
                                if let Some(ref mut w) = log_jsonl_nondeterm_file {
                                    let _ = serde_json::to_writer(&mut *w, entry);
                                    let _ = writeln!(w);
                                }
                                total_nondeterm_log_count += 1;
                            }
                        }
                    }
                }

                // Use the new ExitKind enum for cleaner matching
                match exit.kind() {
                    ExitKind::Hlt => {
                        info!("VM halted (HLT instruction)");
                        break;
                    }
                    ExitKind::VmcallShutdown => {
                        info!("VM shutdown (VMCALL hypercall)");
                        break;
                    }
                    ExitKind::VmcallSnapshot { tag } => {
                        let vm_id = vm.get_vm_id().unwrap_or(0);
                        info!(
                            "VM snapshot: vm_id={}, tag={}, tsc={}",
                            vm_id, tag, exit.emulated_tsc
                        );
                        maybe_wait_for_ctrl_c(args.wait);
                        break;
                    }
                    ExitKind::StopTscReached => {
                        let vm_id = vm.get_vm_id().unwrap_or(0);
                        let vt = exit.emulated_tsc as f64 / exit.tsc_frequency as f64;
                        info!(
                            "VM stopped at TSC {} (vt {:.3}s, stop-at-tsc), vm_id={}",
                            exit.emulated_tsc, vt, vm_id
                        );

                        // Dump feedback buffer if requested
                        if let Some(ref path) = args.dump_feedback {
                            dump_feedback_buffer(&mut vm, path)?;
                        }

                        maybe_wait_for_ctrl_c(args.wait);
                        break;
                    }
                    ExitKind::FeedbackBufferRegistered => {
                        // Map the feedback buffer for later dumping
                        if args.dump_feedback.is_some() {
                            if let Err(e) = vm.map_feedback_buffer() {
                                warn!("Failed to map feedback buffer: {}", e);
                            } else {
                                info!("Feedback buffer registered and mapped");
                            }
                        } else {
                            debug!(
                                "Feedback buffer registered (not mapping, --dump-feedback not set)"
                            );
                        }
                        continue;
                    }
                    ExitKind::Continue | ExitKind::LogBufferFull => continue,
                    ExitKind::Rdrand | ExitKind::Rdseed => {
                        warn!("VM exit: RDRAND/RDSEED in userspace mode not supported by CLI");
                        break;
                    }
                    ExitKind::UnhandledExit { reason } => {
                        log_vm_exit(&vm, &format!("{} ({})", reason, exit.reason_str()));
                        break;
                    }
                }
            }
            Err(e) => {
                log::error!("VM run failed: {}", e);
                if let Ok(regs) = vm.get_regs() {
                    log::error!("  RIP: {:#018x}, RFLAGS: {:#018x}", regs.rip, regs.rflags);
                }
                return Err(io::Error::new(io::ErrorKind::Other, e.to_string()));
            }
        }
    }

    // Flush any partial line from guest output
    output.flush_partial();

    // Flush JSONL files
    if let Some(ref mut jsonl_writer) = log_jsonl_file {
        let _ = jsonl_writer.flush();
    }
    if let Some(ref mut jsonl_writer) = log_jsonl_nondeterm_file {
        let _ = jsonl_writer.flush();
    }
    if let Some(ref path) = args.log_jsonl {
        if total_log_count > 0 {
            info!("Wrote {} deterministic log entries to {}", total_log_count, path);
        } else {
            debug!("No deterministic log entries written to {}", path);
        }
        if total_nondeterm_log_count > 0 {
            let nondeterm_path = if let Some(stem) = path.strip_suffix(".jsonl") {
                format!("{}-nondeterm.jsonl", stem)
            } else {
                format!("{}-nondeterm", path)
            };
            info!(
                "Wrote {} non-deterministic log entries to {}",
                total_nondeterm_log_count, nondeterm_path
            );
        }
    }

    // Display exit statistics after VM shutdown
    let wall_clock_elapsed = wall_clock_start.elapsed();
    if let Ok(stats) = vm.get_exit_stats() {
        // Write exit stats JSON if requested
        if let Some(ref path) = args.exit_stats_json {
            let json = serde_json::to_string_pretty(&stats).map_err(io_error)?;
            std::fs::write(path, json)?;
            debug!("Wrote exit stats to {}", path);
        }

        println!(
            "{}",
            ExitStatsReport {
                stats: &stats,
                wall_clock: wall_clock_elapsed
            }
        );
    } else {
        warn!("Failed to retrieve exit statistics");
    }

    // Display userspace ioctl timing statistics
    println!("{}", vm.get_ioctl_stats());

    Ok(())
}

fn read_file(path: &str) -> io::Result<Vec<u8>> {
    let mut file = File::open(path)?;
    let mut data = Vec::new();
    file.read_to_end(&mut data)?;
    Ok(data)
}

fn io_error<E: std::fmt::Display>(e: E) -> io::Error {
    io::Error::new(io::ErrorKind::Other, e.to_string())
}
