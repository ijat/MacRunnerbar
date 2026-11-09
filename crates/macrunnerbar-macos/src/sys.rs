//! The only `unsafe` in the workspace: signals, a non-reaping child check, and a liveness probe.

use std::io;
use std::process::{Command, Stdio};

pub(crate) const SIGTERM: i32 = libc::SIGTERM;
pub(crate) const SIGKILL: i32 = libc::SIGKILL;

/// Signal every process in group `pgid`. `Ok(false)` when the group no longer exists. Refuses
/// `pgid <= 1` so a bad value can never address "my own group", "all my processes" or init.
pub(crate) fn killpg(pgid: i32, sig: i32) -> io::Result<bool> {
    if pgid <= 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "refusing to signal process group <= 1",
        ));
    }
    // SAFETY: `killpg` only sends a signal to the group id validated above; it reads and writes no
    // memory of ours.
    let r = unsafe { libc::killpg(pgid, sig) };
    if r == 0 {
        return Ok(true);
    }
    let err = io::Error::last_os_error();
    if err.raw_os_error() == Some(libc::ESRCH) {
        Ok(false)
    } else {
        Err(err)
    }
}

/// Whether child `pid` has exited, without reaping it (so its pid stays reserved until the caller
/// waits, which keeps a following group signal from hitting a recycled pid). A child that is not
/// ours any more (`ECHILD`) counts as exited.
pub(crate) fn leader_exited(pid: u32) -> bool {
    // SAFETY: `siginfo_t` is plain old data for which the all-zero bit pattern is valid.
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    // SAFETY: `waitid` writes only into `info`, a live, properly aligned `siginfo_t` we own; `pid` is
    // a plain integer. `WNOWAIT` leaves the child waitable.
    let r = unsafe {
        libc::waitid(
            libc::P_PID,
            libc::id_t::from(pid),
            &raw mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if r == 0 {
        return info.si_pid != 0;
    }
    io::Error::last_os_error().raw_os_error() == Some(libc::ECHILD)
}

/// Whether some process with this pid exists (a zombie counts).
#[cfg(test)]
pub(crate) fn pid_alive(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    if pid <= 1 {
        return false;
    }
    // SAFETY: signal 0 performs only the existence and permission check; nothing is delivered.
    let r = unsafe { libc::kill(pid, 0) };
    r == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// The process start time as `ps` prints it (`lstart`), `None` if there is no such process. Together
/// with the pid it identifies a process across pid reuse.
pub(crate) fn start_time(pid: u32) -> Option<String> {
    let out = Command::new("/bin/ps")
        .args(["-o", "lstart=", "-p", &pid.to_string()])
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn killpg_refuses_dangerous_group_ids() {
        for bad in [-1, 0, 1] {
            assert!(killpg(bad, SIGTERM).is_err(), "{bad}");
        }
    }

    #[test]
    fn own_process_is_alive_with_a_start_time_and_a_bogus_pid_is_not() {
        let me = std::process::id();
        assert!(pid_alive(me));
        assert!(start_time(me).is_some());
        // A pid above the macOS maximum (99999) cannot exist.
        assert!(!pid_alive(4_000_000));
        assert!(start_time(4_000_000).is_none());
    }

    #[test]
    fn killpg_on_a_missing_group_is_ok_false() {
        assert!(!killpg(4_000_000, SIGTERM).expect("ESRCH is Ok(false)"));
    }

    #[test]
    fn leader_exited_peeks_without_reaping() {
        let mut child = Command::new("/bin/sh")
            .args(["-c", "exit 7"])
            .spawn()
            .expect("spawn");
        let pid = child.id();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !leader_exited(pid) {
            assert!(std::time::Instant::now() < deadline, "child never exited");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        // Still waitable: the peek did not reap it.
        assert!(leader_exited(pid));
        assert_eq!(child.wait().expect("wait").code(), Some(7));
    }
}
