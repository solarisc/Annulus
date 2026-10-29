//! Keeping this process's memory, which holds the open vault, out of reach: no core dump on
//! Unix, and on Linux no other process of the same user may attach a debugger or read
//! `/proc/<pid>/mem`.
//!
//! Windows keeps its own crash reports (Windows Error Reporting); a known gap, see
//! SECURITY.md.

/// Hardens the current process as described above. Called once, first thing in `main`.
pub fn harden() {
    #[cfg(unix)]
    unix::harden();
}

#[cfg(unix)]
#[allow(unsafe_code)]
mod unix {
    pub(super) fn harden() {
        let none = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        // SAFETY: setrlimit only reads the `rlimit` passed, which lives on this stack frame.
        unsafe {
            libc::setrlimit(libc::RLIMIT_CORE, &raw const none);
        }
        #[cfg(target_os = "linux")]
        {
            let off: libc::c_ulong = 0;
            // SAFETY: PR_SET_DUMPABLE takes integers and touches no memory of ours.
            unsafe {
                libc::prctl(libc::PR_SET_DUMPABLE, off, off, off, off);
            }
        }
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn a_hardened_process_leaves_no_core_and_refuses_debuggers() {
            super::harden();
            let mut limit = libc::rlimit {
                rlim_cur: 1,
                rlim_max: 1,
            };
            // SAFETY: getrlimit writes into the `rlimit` passed, which lives on this frame.
            unsafe {
                libc::getrlimit(libc::RLIMIT_CORE, &raw mut limit);
            }
            assert_eq!((limit.rlim_cur, limit.rlim_max), (0, 0));
            #[cfg(target_os = "linux")]
            {
                let none: libc::c_ulong = 0;
                // SAFETY: PR_GET_DUMPABLE takes integers and touches no memory of ours.
                let dumpable =
                    unsafe { libc::prctl(libc::PR_GET_DUMPABLE, none, none, none, none) };
                assert_eq!(dumpable, 0);
            }
        }
    }
}
