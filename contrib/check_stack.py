#!/usr/bin/env python3
"""
Stack usage analyzer for bedrock kernel module.

Parses objdump output to compute per-function and worst-case call-chain stack usage.
Designed to ensure the kernel module stays within the Linux kernel's 8KB stack limit.

Usage:
    ./check_stack.py <path-to-bedrock.ko>
    ./check_stack.py --objdump-file <objdump-output.txt>
"""

import argparse
import re
import subprocess
import sys
from collections import defaultdict
from dataclasses import dataclass, field
from typing import Optional


def demangle_rust_symbol(symbol: str) -> str:
    """Attempt to demangle a Rust symbol name using rustfilt or c++filt."""
    # Skip if already looks demangled
    if not symbol.startswith('_R') and not symbol.startswith('_ZN'):
        return symbol

    # Try rustfilt first (best for Rust symbols)
    try:
        result = subprocess.run(
            ['rustfilt', symbol],
            capture_output=True,
            text=True,
            timeout=1,
        )
        if result.returncode == 0 and result.stdout.strip():
            demangled = result.stdout.strip()
            # Truncate very long names
            if len(demangled) > 80:
                return demangled[:77] + "..."
            return demangled
    except (FileNotFoundError, subprocess.TimeoutExpired):
        pass

    # Fall back to c++filt
    try:
        result = subprocess.run(
            ['c++filt', symbol],
            capture_output=True,
            text=True,
            timeout=1,
        )
        if result.returncode == 0 and result.stdout.strip() != symbol:
            demangled = result.stdout.strip()
            if len(demangled) > 80:
                return demangled[:77] + "..."
            return demangled
    except (FileNotFoundError, subprocess.TimeoutExpired):
        pass

    # Return truncated mangled name if demangling fails
    if len(symbol) > 60:
        return symbol[:57] + "..."
    return symbol


@dataclass
class FunctionInfo:
    """Information about a single function's stack usage."""
    name: str
    address: int
    stack_size: int = 0
    calls: set = field(default_factory=set)
    is_exported: bool = False
    has_indirect_calls: bool = False


class StackAnalyzer:
    """Analyzes stack usage from objdump disassembly."""

    # x86-64 stack allocation patterns
    # sub $0x..., %rsp - direct stack allocation
    SUB_RSP_PATTERN = re.compile(r'sub\s+\$0x([0-9a-fA-F]+),\s*%rsp')
    # sub $0x..., %esp - 32-bit (shouldn't occur, but check)
    SUB_ESP_PATTERN = re.compile(r'sub\s+\$0x([0-9a-fA-F]+),\s*%esp')
    # push %reg - 8 bytes per push
    PUSH_PATTERN = re.compile(r'push\s+%r[a-z0-9]+')
    # pushq - also 8 bytes
    PUSHQ_PATTERN = re.compile(r'pushq?\s+')

    # Function boundary pattern
    # Format: 0000000000001234 <function_name>:
    FUNC_PATTERN = re.compile(r'^([0-9a-fA-F]+)\s+<([^>]+)>:')

    # Direct call pattern
    # Format: call 1234 <function_name>
    CALL_PATTERN = re.compile(r'call\s+[0-9a-fA-F]+\s+<([^>]+)>')

    # Indirect call patterns
    INDIRECT_CALL_PATTERN = re.compile(r'call\s+\*')

    def __init__(self, verbose: bool = False):
        self.verbose = verbose
        self.functions: dict[str, FunctionInfo] = {}
        self.exported_symbols: set[str] = set()

    def log(self, msg: str):
        """Print message if verbose mode is enabled."""
        if self.verbose:
            print(f"[DEBUG] {msg}", file=sys.stderr)

    def parse_objdump(self, objdump_output: str) -> dict[str, FunctionInfo]:
        """Parse objdump -d output to extract function info."""
        current_func: Optional[FunctionInfo] = None

        for line in objdump_output.splitlines():
            # Check for new function
            func_match = self.FUNC_PATTERN.match(line)
            if func_match:
                addr = int(func_match.group(1), 16)
                name = func_match.group(2)
                current_func = FunctionInfo(name=name, address=addr)
                self.functions[name] = current_func
                self.log(f"Found function: {name} at 0x{addr:x}")
                continue

            if current_func is None:
                continue

            # Check for stack allocations
            sub_rsp = self.SUB_RSP_PATTERN.search(line)
            if sub_rsp:
                size = int(sub_rsp.group(1), 16)
                current_func.stack_size += size
                self.log(f"  {current_func.name}: sub rsp, {size}")

            sub_esp = self.SUB_ESP_PATTERN.search(line)
            if sub_esp:
                size = int(sub_esp.group(1), 16)
                current_func.stack_size += size
                self.log(f"  {current_func.name}: sub esp, {size}")

            # Count pushes (8 bytes each on x86-64)
            if self.PUSH_PATTERN.search(line) or self.PUSHQ_PATTERN.search(line):
                current_func.stack_size += 8
                self.log(f"  {current_func.name}: push (+8)")

            # Check for direct calls
            call_match = self.CALL_PATTERN.search(line)
            if call_match:
                callee = call_match.group(1)
                # Filter out PLT stubs and special symbols
                if not callee.endswith('@plt') and not callee.startswith('.'):
                    current_func.calls.add(callee)
                    self.log(f"  {current_func.name} calls {callee}")

            # Check for indirect calls
            if self.INDIRECT_CALL_PATTERN.search(line):
                current_func.has_indirect_calls = True
                self.log(f"  {current_func.name}: has indirect call")

        return self.functions

    def parse_exported_symbols(self, nm_output: str):
        """Parse nm output to find exported symbols."""
        # Look for symbols marked as exported (T = text section, global)
        for line in nm_output.splitlines():
            parts = line.split()
            if len(parts) >= 3 and parts[1] == 'T':
                symbol = parts[2]
                self.exported_symbols.add(symbol)
                if symbol in self.functions:
                    self.functions[symbol].is_exported = True

    def build_call_graph(self) -> dict[str, set[str]]:
        """Build adjacency list from function call information."""
        graph = {}
        for name, func in self.functions.items():
            # Only include calls to functions we know about
            known_calls = {c for c in func.calls if c in self.functions}
            graph[name] = known_calls
        return graph

    def find_entry_points(self) -> list[str]:
        """Find all entry points (exported symbols and known entry functions)."""
        entry_points = []

        # Known kernel module entry points
        known_entries = [
            'init_module',
            'cleanup_module',
            '__bedrock_init',
            '__bedrock_exit',
            'bedrock_init',
            'bedrock_exit',
            # VM exit handler (called from assembly)
            'handle_vmexit',
            'rust_handle_vmexit',
        ]

        for name in known_entries:
            if name in self.functions:
                entry_points.append(name)

        # Add all exported symbols
        for name in self.exported_symbols:
            if name in self.functions and name not in entry_points:
                entry_points.append(name)

        # If no entry points found, use all functions that aren't called by anyone
        if not entry_points:
            called_funcs = set()
            for func in self.functions.values():
                called_funcs.update(func.calls)
            for name in self.functions:
                if name not in called_funcs:
                    entry_points.append(name)

        return entry_points

    def detect_cycles(self, graph: dict[str, set[str]]) -> list[list[str]]:
        """Detect cycles in the call graph using DFS."""
        cycles = []
        visited = set()
        rec_stack = set()
        path = []

        def dfs(node):
            visited.add(node)
            rec_stack.add(node)
            path.append(node)

            for neighbor in graph.get(node, set()):
                if neighbor not in visited:
                    dfs(neighbor)
                elif neighbor in rec_stack:
                    # Found a cycle
                    cycle_start = path.index(neighbor)
                    cycles.append(path[cycle_start:] + [neighbor])

            path.pop()
            rec_stack.remove(node)

        for node in graph:
            if node not in visited:
                dfs(node)

        return cycles

    def compute_worst_case(self) -> dict[str, tuple[int, list[str]]]:
        """
        Compute worst-case stack usage for each function.
        Returns dict of function name -> (max_stack, call_path)
        """
        graph = self.build_call_graph()

        # Detect cycles (recursion)
        cycles = self.detect_cycles(graph)
        recursive_funcs = set()
        for cycle in cycles:
            recursive_funcs.update(cycle)

        # Memoization for computed values
        memo: dict[str, tuple[int, list[str]]] = {}

        def compute(name: str, visited: set[str]) -> tuple[int, list[str]]:
            if name in memo:
                return memo[name]

            if name not in self.functions:
                return (0, [])

            if name in visited:
                # Recursion detected, return current function's stack only
                return (self.functions[name].stack_size, [name + " (RECURSIVE)"])

            func = self.functions[name]
            visited = visited | {name}

            max_callee_stack = 0
            max_path = []

            for callee in graph.get(name, set()):
                callee_stack, callee_path = compute(callee, visited)
                if callee_stack > max_callee_stack:
                    max_callee_stack = callee_stack
                    max_path = callee_path

            total = func.stack_size + max_callee_stack
            path = [f"{name} ({func.stack_size})"] + max_path

            memo[name] = (total, path)
            return (total, path)

        results = {}
        for name in self.functions:
            results[name] = compute(name, set())

        return results

    def report(
        self,
        per_func_warn: int = 2048,
        per_func_error: int = 8192,
        chain_warn: int = 4096,
        chain_error: int = 8192,  # 2*PAGE_SIZE
        top_n: int = 10,
        demangle: bool = True,
    ) -> bool:
        """
        Generate report and return True if all checks pass.
        """
        worst_case = self.compute_worst_case()
        entry_points = self.find_entry_points()
        cycles = self.detect_cycles(self.build_call_graph())

        def fmt_name(name: str) -> str:
            return demangle_rust_symbol(name) if demangle else name

        print("Stack Usage Analysis for bedrock.ko")
        print("=" * 60)
        print()

        # Per-function analysis
        print(f"Per-Function Stack Usage (top {top_n}):")
        print("-" * 40)

        sorted_funcs = sorted(
            self.functions.items(),
            key=lambda x: x[1].stack_size,
            reverse=True
        )

        per_func_violations = []
        per_func_warnings = []

        for i, (name, func) in enumerate(sorted_funcs[:top_n]):
            marker = ""
            if func.stack_size >= per_func_error:
                marker = " [ERROR]"
                per_func_violations.append((name, func.stack_size))
            elif func.stack_size >= per_func_warn:
                marker = " [WARN]"
                per_func_warnings.append((name, func.stack_size))

            indirect = " (has indirect calls)" if func.has_indirect_calls else ""
            display_name = fmt_name(name)
            print(f"  {i+1:3}. {display_name}: {func.stack_size:>6} bytes{marker}{indirect}")

        print()

        # Call chain analysis - only show entries with significant stack usage
        print("Worst-Case Call Chain Analysis (entries with >512 bytes):")
        print("-" * 40)

        chain_violations = []
        chain_warnings = []

        # Sort entry points by worst-case stack usage
        sorted_entries = sorted(
            [(e, worst_case[e]) for e in entry_points if e in worst_case],
            key=lambda x: x[1][0],
            reverse=True
        )

        shown = 0
        for entry, (total, path) in sorted_entries:
            if total < 512 and shown >= 5:
                continue  # Skip trivial entries after showing top 10

            marker = ""
            if total >= chain_error:
                marker = " [ERROR]"
                chain_violations.append((entry, total, path))
            elif total >= chain_warn:
                marker = " [WARN]"
                chain_warnings.append((entry, total, path))

            if total >= 512 or marker:  # Only show significant entries
                print(f"  Entry: {fmt_name(entry)}")
                print(f"    Maximum: {total} bytes{marker}")
                if len(path) > 1:
                    formatted_path = [fmt_name(p.split(' (')[0]) + (' (' + p.split(' (')[1] if ' (' in p else '') for p in path[:5]]
                    print(f"    Path: {' -> '.join(formatted_path)}")
                    if len(path) > 5:
                        print(f"          ... ({len(path) - 5} more)")
                print()
                shown += 1

        # Recursion detection
        if cycles:
            print("Recursive Call Chains Detected:")
            print("-" * 40)
            for cycle in cycles[:5]:
                print(f"  {' -> '.join(fmt_name(c) for c in cycle)}")
            if len(cycles) > 5:
                print(f"  ... and {len(cycles) - 5} more cycles")
            print()

        # Indirect calls summary
        indirect_funcs = [f for f in self.functions.values() if f.has_indirect_calls]
        if indirect_funcs:
            print(f"Functions with indirect calls: {len(indirect_funcs)}")
            for func in indirect_funcs[:10]:
                print(f"  - {fmt_name(func.name)}")
            if len(indirect_funcs) > 10:
                print(f"  ... and {len(indirect_funcs) - 10} more")
            print()

        # Summary
        print("Summary:")
        print("-" * 40)
        print(f"  Functions analyzed: {len(self.functions)}")
        print(f"  Entry points: {len(entry_points)}")
        print(f"  Recursive functions: {len(set().union(*[set(c) for c in cycles])) if cycles else 0}")
        print()

        # Thresholds
        print(f"  Per-function thresholds: warn={per_func_warn}, error={per_func_error}")
        print(f"  Call-chain thresholds:   warn={chain_warn}, error={chain_error}")
        print()

        # Violations
        has_errors = False

        if per_func_violations:
            print(f"  Per-function ERRORS ({len(per_func_violations)}):")
            for name, size in per_func_violations:
                print(f"    - {fmt_name(name)}: {size} bytes (limit: {per_func_error})")
            has_errors = True

        if chain_violations:
            print(f"  Call-chain ERRORS ({len(chain_violations)}):")
            for name, size, path in chain_violations:
                print(f"    - {fmt_name(name)}: {size} bytes (limit: {chain_error})")
            has_errors = True

        if per_func_warnings:
            print(f"  Per-function warnings ({len(per_func_warnings)}):")
            for name, size in per_func_warnings:
                print(f"    - {fmt_name(name)}: {size} bytes")

        if chain_warnings:
            print(f"  Call-chain warnings ({len(chain_warnings)}):")
            for name, size, path in chain_warnings:
                print(f"    - {fmt_name(name)}: {size} bytes")

        print()
        if has_errors:
            print("FAILED: Stack usage exceeds limits")
            return False
        else:
            print("PASSED: All checks passed")
            return True


def main():
    parser = argparse.ArgumentParser(
        description="Analyze stack usage in bedrock kernel module"
    )
    parser.add_argument(
        "ko_path",
        nargs="?",
        help="Path to bedrock.ko file",
    )
    parser.add_argument(
        "--objdump-file",
        help="Use pre-generated objdump output file instead of running objdump",
    )
    parser.add_argument(
        "--nm-file",
        help="Use pre-generated nm output file for exported symbols",
    )
    parser.add_argument(
        "--per-func-warn",
        type=int,
        default=2048,
        help="Per-function warning threshold in bytes (default: 2048)",
    )
    parser.add_argument(
        "--per-func-error",
        type=int,
        default=8192,
        help="Per-function error threshold in bytes (default: 4096)",
    )
    parser.add_argument(
        "--chain-warn",
        type=int,
        default=4096,
        help="Call-chain warning threshold in bytes (default: 6144)",
    )
    parser.add_argument(
        "--chain-error",
        type=int,
        default=8192,
        help="Call-chain error threshold in bytes (default: 8192, i.e. 2*PAGE_SIZE)",
    )
    parser.add_argument(
        "--top",
        type=int,
        default=10,
        help="Number of top functions to display (default: 20)",
    )
    parser.add_argument(
        "-v", "--verbose",
        action="store_true",
        help="Enable verbose output",
    )
    parser.add_argument(
        "--no-demangle",
        action="store_true",
        help="Don't demangle Rust symbol names",
    )

    args = parser.parse_args()

    if not args.ko_path and not args.objdump_file:
        parser.error("Either ko_path or --objdump-file is required")

    analyzer = StackAnalyzer(verbose=args.verbose)

    # Get objdump output
    if args.objdump_file:
        with open(args.objdump_file) as f:
            objdump_output = f.read()
    else:
        try:
            result = subprocess.run(
                ["objdump", "-d", args.ko_path],
                capture_output=True,
                text=True,
                check=True,
            )
            objdump_output = result.stdout
        except subprocess.CalledProcessError as e:
            print(f"Error running objdump: {e}", file=sys.stderr)
            sys.exit(1)
        except FileNotFoundError:
            print("objdump not found. Please install binutils.", file=sys.stderr)
            sys.exit(1)

    analyzer.parse_objdump(objdump_output)

    # Get exported symbols if available
    if args.nm_file:
        with open(args.nm_file) as f:
            analyzer.parse_exported_symbols(f.read())
    elif args.ko_path:
        try:
            result = subprocess.run(
                ["nm", args.ko_path],
                capture_output=True,
                text=True,
            )
            if result.returncode == 0:
                analyzer.parse_exported_symbols(result.stdout)
        except FileNotFoundError:
            pass  # nm not available, continue without exported symbols

    # Generate report
    success = analyzer.report(
        per_func_warn=args.per_func_warn,
        per_func_error=args.per_func_error,
        chain_warn=args.chain_warn,
        chain_error=args.chain_error,
        top_n=args.top,
        demangle=not args.no_demangle,
    )

    sys.exit(0 if success else 1)


if __name__ == "__main__":
    main()
