//! Small shared helpers.

use std::process::Command;

/// Log a line to stderr with the tool's prefix.
pub fn log(msg: &str) {
    eprintln!("fault-injector: {msg}");
}

/// Run a command to completion, capturing stdout. Returns an error describing
/// the failure (including stderr) on a non-zero exit or spawn failure.
pub fn run(cmd: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(cmd)
        .args(args)
        .output()
        .map_err(|e| format!("failed to spawn {cmd}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{cmd} {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}
