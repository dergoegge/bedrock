#!/usr/bin/env python3
"""Find the divergence point between a reference and divergent bedrock run.

Given a test directory and a divergent run, this script:
1. Finds the last matching and first divergent deterministic exit
2. Extracts the TSC window between them
3. Shows non-deterministic exits from both runs within that window

Usage:
    python3 determ-divergence.py <test-dir> <run-dir>

Example:
    python3 determ-divergence.py ~/workspace/determ-tests/my_test run-231446
    python3 determ-divergence.py ~/workspace/determ-tests/my_test run-231446 --context 20
"""

import difflib
import json
import os
import argparse


EXIT_REASONS = {
    0: "EXCEPTION_NMI",
    1: "EXTERNAL_INTERRUPT",
    10: "CPUID",
    12: "HLT",
    16: "RDTSC",
    28: "CR_ACCESS",
    30: "IO_INSTRUCTION",
    31: "MSR_READ",
    32: "MSR_WRITE",
    36: "MWAIT",
    39: "MONITOR",
    48: "EPT_VIOLATION",
    51: "RDTSCP",
    55: "XSETBV",
    57: "RDRAND",
    61: "RDSEED",
    258: "VMCALL_SHUTDOWN",
    260: "VMCALL_SNAPSHOT",
}


def exit_name(code):
    return EXIT_REASONS.get(code, f"UNKNOWN({code})")


def fmt_entry(e, prefix=""):
    reason = exit_name(e["exit_reason"])
    rip = hex(e["rip"])
    parts = [
        f"{prefix}tsc={e['tsc']}  exit={e['exit_reason']}({reason})  rip={rip}",
        f"{prefix}  eq={e['exit_qualification']}  rflags={hex(e['rflags'])}",
    ]
    # Show registers
    regs = []
    for r in ["rax", "rcx", "rdx", "rbx", "rsp", "rbp", "rsi", "rdi",
              "r8", "r9", "r10", "r11", "r12", "r13", "r14", "r15"]:
        regs.append(f"{r}={hex(e[r])}")
    parts.append(f"{prefix}  {' '.join(regs[:8])}")
    parts.append(f"{prefix}  {' '.join(regs[8:])}")
    # Show segment/system state
    seg_fields = ["fs_base", "gs_base", "kernel_gs_base", "cr3",
                  "cs_base", "ds_base", "es_base", "ss_base"]
    segs = []
    for s in seg_fields:
        if s in e:
            segs.append(f"{s}={hex(e[s])}")
    if segs:
        parts.append(f"{prefix}  {' '.join(segs[:4])}")
        if len(segs) > 4:
            parts.append(f"{prefix}  {' '.join(segs[4:])}")
    misc = []
    for f in ["pending_dbg_exceptions", "interruptibility_state", "cow_page_count"]:
        if f in e:
            misc.append(f"{f}={hex(e[f]) if isinstance(e[f], int) and e[f] > 255 else e[f]}")
    if misc:
        parts.append(f"{prefix}  {' '.join(misc)}")
    # Show hashes
    hashes = []
    for h in ["memory_hash", "apic_hash", "serial_hash", "ioapic_hash",
              "rtc_hash", "mtrr_hash", "rdrand_hash"]:
        if h in e:
            hashes.append(f"{h}={hex(e[h])}")
    if hashes:
        parts.append(f"{prefix}  {' '.join(hashes)}")
    return "\n".join(parts)


def fmt_entry_short(e, prefix=""):
    reason = exit_name(e["exit_reason"])
    return f"{prefix}tsc={e['tsc']}  exit={e['exit_reason']}({reason})  rip={hex(e['rip'])}  eq={e['exit_qualification']}"


def load_jsonl(path):
    entries = []
    with open(path) as f:
        for line in f:
            line = line.strip()
            if line:
                entries.append(json.loads(line))
    return entries


def find_nondeterm_in_window(nondeterm_entries, tsc_start, tsc_end):
    """Find non-determ exits with TSC in [tsc_start, tsc_end]."""
    return [e for e in nondeterm_entries if tsc_start <= e["tsc"] <= tsc_end]


def main():
    parser = argparse.ArgumentParser(
        description="Find divergence point between reference and divergent bedrock runs"
    )
    parser.add_argument("test_dir", help="Path to the test directory")
    parser.add_argument("run_dir", help="Divergent run directory name (e.g. run-231446)")
    parser.add_argument("--ref", default="run-001", dest="ref_dir",
                        help="Reference run directory name (default: run-001)")
    parser.add_argument("--context", type=int, default=5,
                        help="Number of matching exits to show before divergence (default: 5)")
    parser.add_argument("--nondeterm-window", type=int, default=0,
                        help="Extra TSC range to search for non-determ exits beyond the "
                             "divergence window (default: 0)")
    args = parser.parse_args()

    test_dir = args.test_dir
    ref_name = args.ref_dir
    bad_name = args.run_dir

    ref_dir = os.path.join(test_dir, ref_name)
    bad_dir = os.path.join(test_dir, bad_name)

    # Load deterministic exit logs
    ref_log = os.path.join(ref_dir, "exit-log.jsonl")
    bad_log = os.path.join(bad_dir, "exit-log.jsonl")

    print(f"Loading {ref_log}...")
    ref_entries = load_jsonl(ref_log)
    print(f"Loading {bad_log}...")
    bad_entries = load_jsonl(bad_log)

    print(f"REF: {len(ref_entries)} deterministic exits")
    print(f"BAD: {len(bad_entries)} deterministic exits")
    print()

    # Find first divergence
    min_len = min(len(ref_entries), len(bad_entries))
    diverge_idx = None

    for i in range(min_len):
        r, b = ref_entries[i], bad_entries[i]
        diffs = [k for k in r if k in b and r[k] != b[k]]
        if diffs:
            diverge_idx = i
            break

    if diverge_idx is None:
        if len(ref_entries) == len(bad_entries):
            print("Deterministic exit logs are identical.")
            return
        else:
            print(f"Logs match for {min_len} entries but differ in length: "
                  f"REF={len(ref_entries)} BAD={len(bad_entries)}")
            diverge_idx = min_len

    print(f"=== DIVERGENCE at deterministic exit {diverge_idx} ===")
    print()

    # Show context: last N matching exits
    ctx_start = max(0, diverge_idx - args.context)
    if ctx_start < diverge_idx:
        print(f"--- Last {diverge_idx - ctx_start} matching exits (before divergence) ---")
        for i in range(ctx_start, diverge_idx):
            print(f"  [{i}] {fmt_entry_short(ref_entries[i])}")
        print()

    # Show the last matching exit in detail
    if diverge_idx > 0:
        last_match = diverge_idx - 1
        print(f"--- Last matching exit [{last_match}] ---")
        print(fmt_entry(ref_entries[last_match], prefix="  "))
        print()

    # Show the divergent exit
    if diverge_idx < min_len:
        r, b = ref_entries[diverge_idx], bad_entries[diverge_idx]
        diffs = [k for k in r if k in b and r[k] != b[k]]

        print(f"--- First divergent exit [{diverge_idx}] ---")
        print(f"  Differing fields: {', '.join(diffs)}")
        print()
        print(f"  REF:")
        print(fmt_entry(r, prefix="    "))
        print(f"  BAD:")
        print(fmt_entry(b, prefix="    "))
        print()

        # TSC delta analysis
        if diverge_idx > 0:
            prev = ref_entries[diverge_idx - 1]
            ref_delta = r["tsc"] - prev["tsc"]
            bad_delta = b["tsc"] - prev["tsc"]
            print(f"  TSC delta from previous exit:")
            print(f"    REF: +{ref_delta} instructions")
            print(f"    BAD: +{bad_delta} instructions")
            print(f"    Shift: {bad_delta - ref_delta:+d} instructions")
            print()

    # Determine TSC window for non-determ exit search
    if diverge_idx > 0 and diverge_idx < min_len:
        tsc_start = ref_entries[diverge_idx - 1]["tsc"]
        tsc_end = max(ref_entries[diverge_idx]["tsc"],
                      bad_entries[diverge_idx]["tsc"])
    elif diverge_idx == 0 and min_len > 0:
        tsc_start = 0
        tsc_end = max(ref_entries[0]["tsc"], bad_entries[0]["tsc"])
    else:
        tsc_start = ref_entries[-1]["tsc"] if ref_entries else 0
        tsc_end = tsc_start

    tsc_end += args.nondeterm_window

    # Load and show non-determ exits in the window
    ref_nd_path = os.path.join(ref_dir, "exit-log-nondeterm.jsonl")
    bad_nd_path = os.path.join(bad_dir, "exit-log-nondeterm.jsonl")

    has_nondeterm = os.path.exists(ref_nd_path) or os.path.exists(bad_nd_path)
    if not has_nondeterm:
        print("No non-deterministic exit logs found.")
        return

    print(f"=== NON-DETERMINISTIC EXITS in TSC window [{tsc_start}, {tsc_end}] ===")
    print()

    for label, nd_path in [("REF", ref_nd_path), ("BAD", bad_nd_path)]:
        if not os.path.exists(nd_path):
            print(f"  {label}: no non-determ log")
            continue

        nd_entries = load_jsonl(nd_path)
        window = find_nondeterm_in_window(nd_entries, tsc_start, tsc_end)

        print(f"  {label}: {len(window)} non-determ exits in window "
              f"({len(nd_entries)} total)")
        for e in window:
            print(f"    {fmt_entry_short(e)}")
        print()

    # Summary of non-determ exit type counts
    print("=== NON-DETERM EXIT SUMMARY (full run) ===")
    print()
    for label, nd_path in [("REF", ref_nd_path), ("BAD", bad_nd_path)]:
        if not os.path.exists(nd_path):
            continue
        nd_entries = load_jsonl(nd_path)
        counts = {}
        for e in nd_entries:
            reason = exit_name(e["exit_reason"])
            counts[reason] = counts.get(reason, 0) + 1
        print(f"  {label} ({len(nd_entries)} total):")
        for reason, count in sorted(counts.items(), key=lambda x: -x[1]):
            print(f"    {reason}: {count}")
        print()

    # Compare guest console output (stdout.txt)
    ref_stdout = os.path.join(ref_dir, "stdout.txt")
    bad_stdout = os.path.join(bad_dir, "stdout.txt")

    if os.path.exists(ref_stdout) and os.path.exists(bad_stdout):
        with open(ref_stdout) as f:
            ref_lines = f.readlines()
        with open(bad_stdout) as f:
            bad_lines = f.readlines()

        if ref_lines == bad_lines:
            print("=== GUEST OUTPUT (stdout.txt) ===")
            print()
            print("  Identical.")
            print()
        else:
            diff = list(difflib.unified_diff(
                ref_lines, bad_lines,
                fromfile=f"{ref_name}/stdout.txt",
                tofile=f"{bad_name}/stdout.txt",
                lineterm="",
            ))
            print("=== GUEST OUTPUT DIFF (stdout.txt) ===")
            print()
            # Show up to 50 diff lines to avoid flooding
            for line in diff[:50]:
                print(f"  {line}")
            if len(diff) > 50:
                print(f"  ... ({len(diff) - 50} more diff lines)")
            print()


if __name__ == "__main__":
    main()
