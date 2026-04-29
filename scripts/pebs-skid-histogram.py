#!/usr/bin/env python3
"""Plot a histogram of PEBS-induced EPT-violation skids.

Skid is the difference (in TSC ticks = retired guest instructions) between
where a PEBS-induced EPT violation actually fired and the target it was armed
for. With architecturally-precise PEBS the skid should be 0 or 1; anything
else is hardware imprecision or an arming bug. This script reads the
`pebs_skid` field on EPT_VIOLATION_PEBS entries and renders the distribution.

PEBS-induced EPT violations are non-deterministic (asynchronous + write), so
they live in `exit-log-nondeterm.jsonl`. Pass either the determ or non-
determ path (or a run directory) — the script auto-loads the companion so
the caller doesn't have to remember which file holds them.

Usage:
    python3 pebs-skid-histogram.py <exit-log.jsonl> [<exit-log.jsonl> ...]
    python3 pebs-skid-histogram.py <run-dir>          # auto-finds exit-log.jsonl
    python3 pebs-skid-histogram.py <log> --output skid.png
    python3 pebs-skid-histogram.py <log> --bins 50 --range -5 50

By default writes an ASCII histogram to stdout (no matplotlib needed). With
--output, writes a PNG via matplotlib.
"""

import argparse
import json
import os
import sys
from collections import Counter


# Exit reason 48 = EPT_VIOLATION. Bit 16 of the qualification = asynchronous
# (set for PEBS record writes on EPT-friendly processors). Both required to
# distinguish a PEBS-induced violation from a CoW/MMIO/etc. one.
EPT_VIOLATION = 48
EPT_QUAL_ASYNCHRONOUS = 1 << 16


def load_skids(paths):
    """Return (records, stats).

    `records` is a list of dicts with the PEBS-diagnostic fields the kernel
    module records on EPT_VIOLATION_PEBS entries (skid, arm_delta,
    iters_since_arm, inst_delta, tsc_offset_delta). Missing fields are
    None on logs from older kernel modules.

    Walks every JSONL file, counting total entries, EPT-violation entries,
    PEBS-induced ones (EPT-violation with the asynchronous bit set), and
    PEBS-induced entries that actually carry a `pebs_skid` field. The
    distinction between the last two surfaces the case where the kernel
    module was built before the `pebs_skid` field was added — entries are
    classified as PEBS-induced but no skid is available.
    """
    records = []
    stats = {
        "total": 0,
        "ept": 0,
        "ept_pebs": 0,
        "ept_pebs_with_skid": 0,
    }
    for path in paths:
        with open(path) as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                e = json.loads(line)
                stats["total"] += 1
                if e.get("exit_reason") != EPT_VIOLATION:
                    continue
                stats["ept"] += 1
                if not (e.get("exit_qualification", 0) & EPT_QUAL_ASYNCHRONOUS):
                    continue
                stats["ept_pebs"] += 1
                if "pebs_skid" not in e:
                    continue
                stats["ept_pebs_with_skid"] += 1
                records.append({
                    "skid": e["pebs_skid"],
                    "arm_delta": e.get("pebs_arm_delta"),
                    "iters_since_arm": e.get("pebs_iters_since_arm"),
                    "inst_delta": e.get("pebs_inst_delta"),
                    "tsc_offset_delta": e.get("pebs_tsc_offset_delta"),
                })
    return records, stats


def warn_no_skids(stats, paths):
    """Print a useful explanation when load_skids returned nothing."""
    print(f"loaded {stats['total']} log entries from:")
    for p in paths:
        print(f"  {p}")
    print(
        f"  exit_reason=EPT_VIOLATION (48): {stats['ept']}\n"
        f"  ...with PEBS asynchronous bit set: {stats['ept_pebs']}\n"
        f"  ...with pebs_skid field present:   {stats['ept_pebs_with_skid']}"
    )
    if stats["ept_pebs"] > 0 and stats["ept_pebs_with_skid"] == 0:
        print(
            "\nfound PEBS-induced exits but none carry a `pebs_skid` field — "
            "the kernel module that produced this log was built before "
            "pebs_skid was added. Rebuild with `just remote` and re-run."
        )
    elif stats["ept_pebs"] == 0 and stats["ept"] > 0:
        print(
            "\nfound EPT violations but none with the asynchronous bit (PEBS) "
            "set. Either PEBS isn't being used, or the EPT violations in this "
            "log are CoW/MMIO faults."
        )
    elif stats["ept"] == 0:
        print(
            "\nno EPT violations found at all. PEBS-induced exits are "
            "non-deterministic, so check the non-deterministic log "
            "(`<stem>-nondeterm.jsonl`)."
        )


def companion_path(p):
    """Given a JSONL log path, return its deterministic/non-deterministic pair.

    bedrock writes deterministic exits to `<stem>.jsonl` and non-deterministic
    ones to `<stem>-nondeterm.jsonl`. PEBS-induced EPT violations are
    classified as non-deterministic (asynchronous + write), so they live in
    the `-nondeterm.jsonl` file. We auto-load both halves so the caller can
    pass either path — or just a run directory — without remembering which.
    """
    base, ext = os.path.splitext(p)
    if ext != ".jsonl":
        return None
    if base.endswith("-nondeterm"):
        return base[: -len("-nondeterm")] + ".jsonl"
    return base + "-nondeterm.jsonl"


def resolve_inputs(inputs):
    """Expand directories to <dir>/exit-log.jsonl and pair each JSONL file
    with its deterministic/non-deterministic companion if present.
    """
    out = []
    seen = set()

    def add(path):
        if path in seen:
            return
        seen.add(path)
        out.append(path)

    for p in inputs:
        if os.path.isdir(p):
            candidate = os.path.join(p, "exit-log.jsonl")
            if not os.path.exists(candidate):
                sys.exit(f"error: {candidate} not found")
            add(candidate)
            companion = companion_path(candidate)
            if companion and os.path.exists(companion):
                add(companion)
        else:
            if not os.path.exists(p):
                sys.exit(f"error: {p} not found")
            add(p)
            companion = companion_path(p)
            if companion and os.path.exists(companion):
                add(companion)
    return out


def _percentiles(values):
    """Return (min, p10, p50, p90, max, mean) for a non-empty list."""
    s = sorted(values)
    n = len(s)
    return (
        s[0],
        s[n // 10],
        s[n // 2],
        s[(9 * n) // 10],
        s[-1],
        sum(s) / n,
    )


def report_skid_zero_vs_one(records):
    """Compare the arm_delta distribution between skid=0 and skid=1 exits.

    The skid=1 cluster persists even when delta >= 257 (the documented PDist
    cutoff). To confirm whether it's a threshold-shifted-higher effect or
    architectural noise, contrast the arm_delta percentiles for the two
    skid values: a threshold issue would put skid=1 at systematically
    lower arm_deltas; uniform distributions would point at noise.
    """
    skid0 = [r["arm_delta"] for r in records if r["skid"] == 0 and r["arm_delta"] is not None]
    skid1 = [r["arm_delta"] for r in records if r["skid"] == 1 and r["arm_delta"] is not None]
    if not skid0 or not skid1:
        return
    print()
    print("arm_delta by skid value (PDist-engagement test):")
    for label, vals in [("skid=0", skid0), ("skid=1", skid1)]:
        lo, p10, p50, p90, hi, mean = _percentiles(vals)
        print(
            f"  {label}: n={len(vals):>6d}  min={lo} max={hi} "
            f"mean={mean:.1f} p10={p10} p50={p50} p90={p90}"
        )
    # Bucket counts at small arm_delta values to see if skid=1 is
    # concentrated near the documented PDist threshold (256).
    print("  arm_delta buckets:")
    edges = [256, 512, 1024, 2048, 4096, 8192]
    for label, vals in [("skid=0", skid0), ("skid=1", skid1)]:
        n = len(vals)
        s = sorted(vals)
        cuts = [sum(1 for v in s if v < e) for e in edges]
        line = "    " + label + ":"
        prev = 0
        for e, c in zip(edges, cuts):
            in_bucket = c - prev
            line += f"  [<{e}] {100.0 * in_bucket / n:5.1f}%"
            prev = c
        line += f"  [>={edges[-1]}] {100.0 * (n - prev) / n:5.1f}%"
        print(line)


def report_outlier_breakdown(records, skid_threshold=1):
    """Print PEBS-diagnostic stats for skid outliers (skid > threshold) so we
    can pin them on a specific mechanism (multi-iter arming, HLT/MWAIT
    clamp, instruction-vs-tsc divergence) instead of guessing.

    Each section reports outlier rate vs baseline (all exits) so the
    reader can see whether outliers are over-represented on a given
    signal. Sections silently skip when the relevant field isn't present
    in the log.
    """
    PDIST_MIN_DELTA = 257
    outliers = [r for r in records if r["skid"] > skid_threshold]
    if not outliers:
        return

    print()
    print(
        f"skid > {skid_threshold}: {len(outliers)} exits "
        f"({100.0 * len(outliers) / len(records):.2f}%)"
    )

    # arm_delta: separates the Reduced-Skid regime from the PDist regime.
    out_deltas = [r["arm_delta"] for r in outliers if r["arm_delta"] is not None]
    if out_deltas:
        lo, p10, p50, p90, hi, mean = _percentiles(out_deltas)
        print(
            f"  arm_delta:  min={lo} max={hi} mean={mean:.1f} "
            f"p10={p10} p50={p50} p90={p90}"
        )
        out_reduced = sum(1 for d in out_deltas if d < PDIST_MIN_DELTA)
        out_pdist = len(out_deltas) - out_reduced
        all_deltas = [r["arm_delta"] for r in records if r["arm_delta"] is not None]
        all_reduced = sum(1 for d in all_deltas if d < PDIST_MIN_DELTA)
        all_pdist = len(all_deltas) - all_reduced
        print(
            f"    Reduced-Skid  (delta <  {PDIST_MIN_DELTA}):  "
            f"{100.0 * out_reduced / len(out_deltas):5.1f}% of outliers   "
            f"vs baseline {100.0 * all_reduced / max(len(all_deltas), 1):5.1f}%"
        )
        print(
            f"    PDist         (delta >= {PDIST_MIN_DELTA}):  "
            f"{100.0 * out_pdist / len(out_deltas):5.1f}% of outliers   "
            f"vs baseline {100.0 * all_pdist / max(len(all_deltas), 1):5.1f}%"
        )

    # iters_since_arm: tests the SDM 21.9.5 hypothesis (PEBS deferred
    # past a non-PEBS VM-exit within the armed window).
    out_iters = [r["iters_since_arm"] for r in outliers if r["iters_since_arm"] is not None]
    if out_iters:
        out_multi = sum(1 for v in out_iters if v > 0)
        all_iters = [r["iters_since_arm"] for r in records if r["iters_since_arm"] is not None]
        all_multi = sum(1 for v in all_iters if v > 0)
        print(
            f"  iters_since_arm > 0 (arming carried across non-PEBS exits): "
            f"{100.0 * out_multi / len(out_iters):5.1f}% of outliers   "
            f"vs baseline {100.0 * all_multi / max(len(all_iters), 1):5.1f}%"
        )
        if out_multi:
            lo, p10, p50, p90, hi, mean = _percentiles([v for v in out_iters if v > 0])
            print(
                f"    when > 0:  min={lo} max={hi} mean={mean:.1f} "
                f"p10={p10} p50={p50} p90={p90}"
            )

    # tsc_offset_delta: HLT/MWAIT clamps that advanced emulated_tsc
    # without retiring guest instructions inside the armed window.
    out_offsets = [r["tsc_offset_delta"] for r in outliers if r["tsc_offset_delta"] is not None]
    if out_offsets:
        out_clamped = sum(1 for v in out_offsets if v != 0)
        all_offsets = [r["tsc_offset_delta"] for r in records if r["tsc_offset_delta"] is not None]
        all_clamped = sum(1 for v in all_offsets if v != 0)
        print(
            f"  tsc_offset_delta != 0 (HLT/MWAIT clamp inside window):       "
            f"{100.0 * out_clamped / len(out_offsets):5.1f}% of outliers   "
            f"vs baseline {100.0 * all_clamped / max(len(all_offsets), 1):5.1f}%"
        )

    # inst_delta vs arm_delta: does INST_RETIRED gain match the requested
    # distance? Mismatch means the counter ticked further than we asked
    # (positive divergence) — typically 1 for Reduced Skid, 0 for PDist,
    # but larger values point at deferred record-write effects.
    div = []
    for r in outliers:
        if r["inst_delta"] is not None and r["arm_delta"] is not None:
            div.append(r["inst_delta"] - r["arm_delta"])
    if div:
        lo, p10, p50, p90, hi, mean = _percentiles(div)
        print(
            f"  inst_delta - arm_delta (INST_RETIRED gain past requested):"
            f"  min={lo} max={hi} mean={mean:.1f} p10={p10} p50={p50} p90={p90}"
        )


def ascii_histogram(skids, bins, lo, hi, width=60):
    """Render a histogram to stdout. Bins are equal-width over [lo, hi]; values
    outside that range fall into underflow/overflow buckets shown separately.
    """
    if lo is None:
        lo = min(skids)
    if hi is None:
        hi = max(skids)
    if hi <= lo:
        # Single value — degenerate but render something.
        hi = lo + 1

    counts = [0] * bins
    underflow = overflow = 0
    bin_width = (hi - lo) / bins
    for s in skids:
        if s < lo:
            underflow += 1
        elif s >= hi:
            # Final bin closed on the right so the max value falls in.
            if s == hi and bins > 0:
                counts[-1] += 1
            else:
                overflow += 1
        else:
            idx = int((s - lo) / bin_width)
            if idx >= bins:
                idx = bins - 1
            counts[idx] += 1

    peak = max(counts) if counts else 0
    if underflow:
        peak = max(peak, underflow)
    if overflow:
        peak = max(peak, overflow)
    if peak == 0:
        peak = 1

    n = len(skids)
    print(f"PEBS exits: {n}")
    print(
        f"skid:  min={min(skids)}  max={max(skids)}  "
        f"mean={sum(skids)/n:.2f}  unique={len(set(skids))}"
    )
    print(f"range: [{lo}, {hi})  bins={bins}  bin_width={bin_width:g}")
    print()

    def bar(count):
        return "#" * int(round(width * count / peak)) if count else ""

    if underflow:
        print(f"      < {lo:>10g} | {underflow:>8d} | {bar(underflow)}")
    for i, c in enumerate(counts):
        edge_lo = lo + i * bin_width
        edge_hi = edge_lo + bin_width
        label = f"[{edge_lo:>10g}, {edge_hi:>10g})"
        print(f"{label} | {c:>8d} | {bar(c)}")
    if overflow:
        print(f"     >= {hi:>10g} | {overflow:>8d} | {bar(overflow)}")

    # Most-common exact values — useful when the distribution is heavily
    # concentrated on a few skid values (e.g., all 0 or all 1).
    print()
    print("top exact skid values:")
    for value, count in Counter(skids).most_common(10):
        pct = 100.0 * count / n
        print(f"  skid={value:>6d}  {count:>8d}  ({pct:5.1f}%)")


def png_histogram(skids, output, bins, lo, hi):
    try:
        import matplotlib

        matplotlib.use("Agg")
        import matplotlib.pyplot as plt
    except ImportError:
        sys.exit(
            "error: matplotlib not installed. Install with `pip install matplotlib` "
            "or omit --output for an ASCII histogram."
        )

    # Paper-style monochrome rendering: serif body font, no color, hairline
    # axes only on the left and bottom, no top/right spines, no fill color
    # (just a black outline on the bars). The result reads like a journal
    # figure rather than a slide deck.
    rc = {
        "font.family": "serif",
        "font.serif": ["DejaVu Serif", "Liberation Serif", "Times New Roman", "Times"],
        "font.size": 10,
        "axes.labelsize": 10,
        "axes.titlesize": 10,
        "axes.linewidth": 0.6,
        "axes.edgecolor": "black",
        "axes.facecolor": "white",
        "axes.spines.top": False,
        "axes.spines.right": False,
        "xtick.direction": "out",
        "ytick.direction": "out",
        "xtick.major.width": 0.6,
        "ytick.major.width": 0.6,
        "xtick.major.size": 3,
        "ytick.major.size": 3,
        "figure.facecolor": "white",
        "savefig.facecolor": "white",
        "savefig.bbox": "tight",
        "savefig.pad_inches": 0.02,
    }

    with plt.rc_context(rc):
        fig, ax = plt.subplots(figsize=(5.5, 3.2))
        kw = dict(
            bins=bins,
            color="white",
            edgecolor="black",
            linewidth=0.6,
            histtype="stepfilled",
        )
        if lo is not None and hi is not None:
            ax.hist(skids, range=(lo, hi), **kw)
        else:
            ax.hist(skids, **kw)

        ax.set_xlabel("skid (TSC ticks past target)")
        ax.set_ylabel("PEBS exit count")
        ax.tick_params(axis="both", which="both", top=False, right=False)

        n = len(skids)
        # Caption-style annotation in the upper right; a bare textual stat
        # block reads more naturally on a paper figure than a chart title.
        ax.text(
            0.98,
            0.95,
            f"$n={n:,}$\n"
            f"min $={min(skids):,}$\n"
            f"max $={max(skids):,}$\n"
            f"mean $={sum(skids)/n:.2f}$",
            transform=ax.transAxes,
            ha="right",
            va="top",
            fontsize=8,
            family="serif",
        )

        fig.savefig(output, dpi=300)
    print(f"wrote {output}  ({n} PEBS exits)")


def main():
    p = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    p.add_argument(
        "inputs",
        nargs="+",
        help="exit-log.jsonl path(s) or run directory containing exit-log.jsonl",
    )
    p.add_argument(
        "--bins", type=int, default=20, help="number of histogram bins (default: 20)"
    )
    p.add_argument(
        "--range",
        type=int,
        nargs=2,
        metavar=("LO", "HI"),
        help="restrict histogram to [LO, HI) (default: data min..max)",
    )
    p.add_argument(
        "--output",
        "-o",
        help="write PNG histogram via matplotlib instead of ASCII",
    )
    args = p.parse_args()

    paths = resolve_inputs(args.inputs)
    records, stats = load_skids(paths)
    if not records:
        warn_no_skids(stats, paths)
        sys.exit(1)
    skids = [r["skid"] for r in records]
    lo, hi = (args.range[0], args.range[1]) if args.range else (None, None)

    if args.output:
        png_histogram(skids, args.output, args.bins, lo, hi)
    else:
        ascii_histogram(skids, args.bins, lo, hi)
        report_skid_zero_vs_one(records)
        report_outlier_breakdown(records)


if __name__ == "__main__":
    main()
