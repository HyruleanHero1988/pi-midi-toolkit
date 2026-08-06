//! Optional Linux realtime hardening (no-ops on Windows / macOS hosts).

/// Try `mlockall` + `SCHED_FIFO`. Soft-fails with a log line if privileges are missing.
pub fn apply_rt_hints(enable: bool) {
    if !enable {
        return;
    }
    #[cfg(target_os = "linux")]
    {
        linux::apply();
    }
    #[cfg(not(target_os = "linux"))]
    {
        tracing::info!("RT hints skipped (not Linux)");
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use tracing::{info, warn};

    pub fn apply() {
        // Lock current and future pages to avoid page-fault jitter on the MIDI path.
        let lock = unsafe { libc::mlockall(libc::MCL_CURRENT | libc::MCL_FUTURE) };
        if lock != 0 {
            warn!(
                "mlockall failed (errno {}); continuing without memory lock",
                std::io::Error::last_os_error()
            );
        } else {
            info!("mlockall ok");
        }

        let mut param = libc::sched_param {
            sched_priority: 70,
        };
        let rc = unsafe { libc::sched_setscheduler(0, libc::SCHED_FIFO, &mut param) };
        if rc != 0 {
            warn!(
                "SCHED_FIFO failed (errno {}); run via systemd with LimitRTPRIO or as root",
                std::io::Error::last_os_error()
            );
        } else {
            info!(priority = 70, "SCHED_FIFO ok");
        }
    }
}
