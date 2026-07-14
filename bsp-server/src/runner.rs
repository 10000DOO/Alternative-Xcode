//! Bounded subprocess execution.
//!
//! Callers may hold the state lock across these (Round B enumeration runs
//! `plutil` under the lock), so a stalled tool must NOT freeze the server: the
//! child is spawned, both pipes are drained on a worker thread (no pipe-fill
//! deadlock), and the parent waits with a timeout — killing the child on expiry.

use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// Run `program args…` with a bounded wait, returning stdout on success.
/// Non-zero exit → Err (with stderr); timeout → kill the child → Err.
pub fn run_bounded(program: &str, args: &[&str], timeout: Duration) -> Result<String, String> {
    let child = Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn {program}: {e}"))?;
    let pid = child.id();

    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });

    match rx.recv_timeout(timeout) {
        Ok(Ok(output)) => {
            if !output.status.success() {
                return Err(format!(
                    "{program} {} failed ({}): {}",
                    args.join(" "),
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        }
        Ok(Err(e)) => Err(format!("{program} wait failed: {e}")),
        Err(_) => {
            // Timed out — kill the child by pid so it can't linger (the worker
            // thread then unblocks and exits on its own).
            let _ = Command::new("/bin/kill").arg("-9").arg(pid.to_string()).status();
            Err(format!("{program} timed out after {}s", timeout.as_secs()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_stdout_on_success() {
        let out = run_bounded("echo", &["hello"], Duration::from_secs(5)).unwrap();
        assert_eq!(out.trim(), "hello");
    }

    #[test]
    fn times_out_and_kills_slow_child() {
        // `sleep 5` with a 150ms budget must return Err quickly (child killed).
        let start = std::time::Instant::now();
        let res = run_bounded("sleep", &["5"], Duration::from_millis(150));
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("timed out"));
        assert!(start.elapsed() < Duration::from_secs(3), "should not wait for the child");
    }

    #[test]
    fn nonzero_exit_is_err() {
        assert!(run_bounded("false", &[], Duration::from_secs(5)).is_err());
    }
}
