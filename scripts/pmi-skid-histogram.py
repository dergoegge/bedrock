#!/usr/bin/env python3
"""Plot a histogram of PMU-induced VM-exit skids from a determ exit log.

Skid is the difference (in retired guest instructions) between where the
PMI sampling counter actually overflowed and the position it was armed for
(`next_periodic_exit_count - PERIODIC_EXIT_MARGIN`). The hypervisor records
this on the external-interrupt exit that first enters the margin window;
MTF then steps the remaining `MARGIN - skid` instructions to land precisely
on the boundary. This script reads the `pmi_skid` field on those entries
and renders the distribution.

Usage:
    python3 pmi-skid-histogram.py <exit-log.jsonl> [<exit-log.jsonl> ...]
    python3 pmi-skid-histogram.py <run-dir>          # auto-finds exit-log.jsonl
    python3 pmi-skid-histogram.py <log> --output skid.png
    python3 pmi-skid-histogram.py <log> --bins 50 --range 0 200

By default writes an ASCII histogram to stdout (no matplotlib needed). With
--output, writes a PNG via matplotlib.
"""

import argparse
import json
import os
import sys
from collections import Counter


# Exit reason 1 = EXTERNAL_INTERRUPT. The PMI is delivered as a host
# interrupt while the guest runs, so it surfaces as an external-interrupt
# VM-exit. External interrupts are non-deterministic, which puts these
# entries in the `-nondeterm.jsonl` half of the log.
EXTERNAL_INTERRUPT = 1


def load_skids(paths):
    """Return (skids, stats) where stats is a dict with diagnostic counts.

    Walks every JSONL file, counting total entries, external-interrupt
    entries, and entries that carry a non-zero `pmi_skid` field. The
    distinction surfaces logs produced by a kernel module built before
    `pmi_skid` was added — entries are external-interrupt exits but no skid
    is available.
    """
    skids = []
    stats = {
        "total": 0,
        "extint": 0,
        "extint_with_skid_field": 0,
        "extint_with_nonzero_skid": 0,
    }
    for path in paths:
        with open(path) as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                e = json.loads(line)
                stats["total"] += 1
                if e.get("exit_reason") != EXTERNAL_INTERRUPT:
                    continue
                stats["extint"] += 1
                if "pmi_skid" not in e:
                    continue
                stats["extint_with_skid_field"] += 1
                skid = e["pmi_skid"]
                if skid == 0:
                    continue
                stats["extint_with_nonzero_skid"] += 1
                skids.append(skid)
    return skids, stats


def warn_no_skids(stats, paths):
    """Print a useful explanation when load_skids returned nothing."""
    print(f"loaded {stats['total']} log entries from:")
    for p in paths:
        print(f"  {p}")
    print(
        f"  exit_reason=EXTERNAL_INTERRUPT (1):    {stats['extint']}\n"
        f"  ...with pmi_skid field present:        {stats['extint_with_skid_field']}\n"
        f"  ...with non-zero pmi_skid:             {stats['extint_with_nonzero_skid']}"
    )
    if stats["extint"] > 0 and stats["extint_with_skid_field"] == 0:
        print(
            "\nfound external-interrupt exits but none carry a `pmi_skid` "
            "field — the kernel module that produced this log was built "
            "before pmi_skid was added. Rebuild with `just remote` and re-run."
        )
    elif stats["extint_with_skid_field"] > 0 and stats["extint_with_nonzero_skid"] == 0:
        print(
            "\nfound external-interrupt exits with the `pmi_skid` field, but "
            "all are zero. Either the PMU is firing exactly on target (very "
            "unlikely) or no external-interrupt exit was a PMI margin entry "
            "in this log."
        )
    elif stats["extint"] == 0:
        print(
            "\nno external-interrupt exits found. PMI exits are "
            "non-deterministic, so check the non-deterministic log "
            "(`<stem>-nondeterm.jsonl`, not the un-suffixed one)."
        )


def companion_path(p):
    """Given a JSONL log path, return its deterministic/non-deterministic pair.

    bedrock writes deterministic exits to `<stem>.jsonl` and non-deterministic
    ones to `<stem>-nondeterm.jsonl`. PMI exits are classified as
    non-deterministic, so they live in the `-nondeterm` file. We auto-load
    both halves so the caller doesn't have to remember which.
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


def ascii_histogram(skids, bins, lo, hi, width=60):
    """Render a histogram to stdout. Bins are equal-width over [lo, hi]; values
    outside that range fall into underflow/overflow buckets shown separately.
    """
    if lo is None:
        lo = min(skids)
    if hi is None:
        hi = max(skids)
    if hi <= lo:
        hi = lo + 1

    counts = [0] * bins
    underflow = overflow = 0
    bin_width = (hi - lo) / bins
    for s in skids:
        if s < lo:
            underflow += 1
        elif s >= hi:
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
    print(f"PMI exits: {n}")
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

        ax.set_xlabel("skid (instructions past target)")
        ax.set_ylabel("PMI exit count")
        ax.tick_params(axis="both", which="both", top=False, right=False)

        n = len(skids)
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
    print(f"wrote {output}  ({n} PMI exits)")


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
    skids, stats = load_skids(paths)
    if not skids:
        warn_no_skids(stats, paths)
        sys.exit(1)
    lo, hi = (args.range[0], args.range[1]) if args.range else (None, None)

    if args.output:
        png_histogram(skids, args.output, args.bins, lo, hi)
    else:
        ascii_histogram(skids, args.bins, lo, hi)


if __name__ == "__main__":
    main()
