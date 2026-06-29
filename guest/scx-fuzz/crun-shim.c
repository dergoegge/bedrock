// SPDX-License-Identifier: GPL-2.0
//
// OCI runtime wrapper that opts the workload's containers into sched_ext.
//
// Registered as podman's OCI runtime (see the guest containers.conf), this
// binary is invoked in crun's place for every container operation. For the
// subcommands that create a container payload (create/run/exec) it runs the
// real crun as a child and then switches the payload process directly into
// SCHED_EXT, by the pid crun records in its --pid-file.
//
// We switch the recorded pid rather than switching ourselves and relying on
// inheritance because crun resets the container process's scheduling policy
// while setting it up — a SCHED_EXT crun would not yield a SCHED_EXT init.
// Setting the policy on the pid AFTER crun records it (while the init is still
// paused on the start fifo, for create) is reliable and races nothing. From
// there the payload's own fork/exec descendants inherit SCHED_EXT normally.
//
// Everything the payload runs is therefore governed by the in-kernel fuzzing
// scheduler; crun stays SCHED_NORMAL on the stock scheduler, as does every
// other host process (the scheduler is attached with SCX_OPS_SWITCH_PARTIAL).

#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <sched.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <unistd.h>

#ifndef SCHED_EXT
#define SCHED_EXT 7
#endif

// Absolute path to the real crun, substituted by the build (-DREAL_CRUN). A
// wrapper must never resolve "crun" via PATH, or it would re-exec itself.
#ifndef REAL_CRUN
#define REAL_CRUN "@crun@"
#endif

// Debug log surfaced into the journal by guest/init (tag "crun-shim").
#define SHIM_LOG "/run/scx-crun-shim.log"

// How long to wait for crun to write the --pid-file. The pid appears within
// milliseconds; the bound just stops us hanging if crun never writes it.
#define PID_WAIT_TRIES 5000	// * 1ms = 5s
#define PID_WAIT_US 1000

static long set_ext(pid_t pid)
{
	struct sched_param p = { .sched_priority = 0 };

	// Raw syscall, not the glibc wrapper: some libc versions reject an
	// unknown policy value (SCHED_EXT == 7) before the syscall.
	return syscall(SYS_sched_setscheduler, pid, SCHED_EXT, &p);
}

static void shim_log(char **argv, const char *note, long val, long rc, int err)
{
	int fd = open(SHIM_LOG, O_WRONLY | O_CREAT | O_APPEND, 0644);
	char buf[1024];
	int n = 0;

	if (fd < 0)
		return;
	n += snprintf(buf + n, sizeof(buf) - n, "%s pid=%ld rc=%ld errno=%d argv:",
		      note, val, rc, err);
	for (int i = 0; argv[i] && n < (int)sizeof(buf) - 64; i++)
		n += snprintf(buf + n, sizeof(buf) - n, " %s", argv[i]);
	if (n < (int)sizeof(buf))
		buf[n++] = '\n';
	(void)write(fd, buf, n);
	close(fd);
}

static int switches_payload(const char *subcmd)
{
	return !strcmp(subcmd, "create") || !strcmp(subcmd, "run") ||
	       !strcmp(subcmd, "exec");
}

// The subcommand is the first arg that is one of crun's payload verbs. We scan
// for the verb itself rather than "first non-flag", because crun's global flags
// take values (e.g. "--root /run/crun") that would otherwise be mistaken for it.
static const char *find_subcmd(int argc, char **argv)
{
	for (int i = 1; i < argc; i++)
		if (switches_payload(argv[i]))
			return argv[i];
	return NULL;
}

static const char *find_pid_file(int argc, char **argv)
{
	for (int i = 1; i < argc - 1; i++)
		if (!strcmp(argv[i], "--pid-file"))
			return argv[i + 1];
	return NULL;
}

static pid_t read_pid(const char *path)
{
	FILE *f = fopen(path, "r");
	long pid = 0;

	if (!f)
		return 0;
	if (fscanf(f, "%ld", &pid) != 1)
		pid = 0;
	fclose(f);
	return (pid_t)pid;
}

int main(int argc, char **argv)
{
	const char *subcmd = find_subcmd(argc, argv);
	const char *pidfile = subcmd ? find_pid_file(argc, argv) : NULL;

	// No payload to switch (start/state/kill/delete/features/version, or a
	// payload verb without --pid-file): run crun transparently.
	if (!subcmd || !pidfile) {
		execv(REAL_CRUN, argv);
		_exit(127);
	}

	// Drop any stale pid-file so we read the pid crun is about to write.
	unlink(pidfile);

	pid_t child = fork();
	if (child == 0) {
		execv(REAL_CRUN, argv);
		_exit(127);
	}
	if (child < 0) {
		execv(REAL_CRUN, argv); // fork failed; fall back to plain crun
		_exit(127);
	}

	// Poll for the pid crun records, then switch that process into
	// SCHED_EXT. For create the init is paused on the start fifo so this
	// races nothing; for exec the target has just started.
	pid_t target = 0;
	long rc = -1;
	int err = 0;
	for (int i = 0; i < PID_WAIT_TRIES; i++) {
		target = read_pid(pidfile);
		if (target > 0)
			break;
		// Stop early if crun already exited without a pid (failure).
		if (waitpid(child, NULL, WNOHANG) == child) {
			child = -1;
			break;
		}
		usleep(PID_WAIT_US);
	}
	if (target > 0) {
		rc = set_ext(target);
		err = errno;
	}
	shim_log(argv, "switch", (long)target, rc, err);

	int status = 0;
	if (child > 0)
		waitpid(child, &status, 0);
	_exit(WIFEXITED(status) ? WEXITSTATUS(status) : 1);
}
