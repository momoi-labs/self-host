//! The daemon's open file limit. Every proxied connection holds a descriptor,
//! and a service manager often starts processes with a soft limit of 1024
//! (systemd) or 256 (launchd). Behind a CDN that pools connections, 1024 runs
//! out: accept fails with EMFILE and every site on the Host goes down.

/// `OPEN_MAX` from macOS `<sys/syslimits.h>`. The libc crate does not export it.
#[cfg(target_os = "macos")]
const OPEN_MAX: libc::rlim_t = 10240;

/// Raises the soft open file limit to the hard limit, and returns the soft
/// limit in effect afterwards. Never lowers the soft limit and never changes
/// the hard limit. macOS rejects values above `OPEN_MAX`, so there the raise
/// falls back to `min(OPEN_MAX, hard)`.
pub fn raise() -> std::io::Result<u64> {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    if limit.rlim_cur >= limit.rlim_max {
        return Ok(limit.rlim_cur);
    }
    #[cfg(target_os = "macos")]
    let fallback = Some(OPEN_MAX.min(limit.rlim_max)).filter(|open_max| *open_max > limit.rlim_cur);
    #[cfg(not(target_os = "macos"))]
    let fallback = None;
    let mut error = None;
    for target in std::iter::once(limit.rlim_max).chain(fallback) {
        let raised = libc::rlimit {
            rlim_cur: target,
            rlim_max: limit.rlim_max,
        };
        if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &raised) } == 0 {
            return Ok(target);
        }
        error = Some(std::io::Error::last_os_error());
    }
    Err(error.unwrap_or_else(std::io::Error::last_os_error))
}

#[cfg(test)]
mod tests {
    const CHILD: &str = "SELF_HOST_FD_LIMIT_TEST_CHILD";

    fn current() -> libc::rlimit {
        let mut limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        assert_eq!(
            unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) },
            0
        );
        limit
    }

    /// Lowering the limit would starve the other tests in this process, so
    /// the test runs itself again in a child process and checks it there.
    #[test]
    fn raises_a_lowered_soft_limit() {
        if std::env::var_os(CHILD).is_some() {
            let before = current();
            let lowered = libc::rlimit {
                rlim_cur: 64,
                rlim_max: before.rlim_max,
            };
            assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &lowered) }, 0);

            let soft = super::raise().unwrap();

            let after = current();
            assert_eq!(after.rlim_max, before.rlim_max, "hard limit changed");
            assert_eq!(after.rlim_cur, soft);
            #[cfg(target_os = "macos")]
            assert!(
                after.rlim_cur == before.rlim_max
                    || after.rlim_cur == super::OPEN_MAX.min(before.rlim_max),
                "soft limit {} not raised",
                after.rlim_cur
            );
            #[cfg(not(target_os = "macos"))]
            assert_eq!(after.rlim_cur, before.rlim_max, "soft limit not raised");
            assert!(
                super::raise().unwrap() >= soft,
                "second call lowered the limit"
            );
            return;
        }
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "fd_limit::tests::raises_a_lowered_soft_limit"])
            .env(CHILD, "1")
            .status()
            .unwrap();
        assert!(status.success(), "child test failed: {status}");
    }
}
