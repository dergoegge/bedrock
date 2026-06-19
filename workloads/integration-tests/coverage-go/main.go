// Integration-test driver: an Antithesis-instrumented Go program whose edge
// coverage is recorded through our libvoidstar -> libfeedback shim, for the
// host to read back (tests/integration/coverage.rs).
//
// antithesis-go-instrumentor rewrites this module; the generated notifier's
// init() calls init_coverage_module() in /usr/lib/libvoidstar.so (our shim),
// which registers a feedback buffer keyed by the instrumentor's symbol-table
// name. The branchy workload below then drives notify_coverage() on the edges
// it hits.
package main

import (
	"syscall"
	"time"
)

// Branchy work so the instrumentation has a spread of edges to report.
func classify(x int) int {
	switch x % 3 {
	case 0:
		return x * 2
	case 1:
		return x + 7
	default:
		return x - 1
	}
}

func workload() int {
	acc := 0
	for i := 0; i < 100; i++ {
		c := classify(i)
		if c > 50 {
			acc += c
		} else {
			acc -= c
		}
	}
	return acc
}

func main() {
	// Detach into our own session so the podman-exec teardown that follows the
	// launcher's return doesn't take us down with it (like the C drivers).
	// Best-effort: harmless if we aren't allowed to (the file-backed coverage
	// buffer survives our death anyway).
	_, _ = syscall.Setsid()

	sink := workload()
	_ = sink

	// Idle forever so the coverage buffer stays mapped for the host to read.
	for {
		time.Sleep(time.Hour)
	}
}
