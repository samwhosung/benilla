//! The CPU clocks the instruments are denominated in — process, main thread, and machine.
//!
//! Three different questions, three different calls. [`process_cpu_secs`] is the campaign's
//! currency: *work per frame*, immune to the present grant.
//! [`main_thread_cpu_secs`] narrows that to the serialized half — the part a hitch is actually made
//! of. [`system_cpu_ticks`] says how loaded the machine was while we measured, so two legs can be
//! told apart from two moods of the same leg (1157).

/// Whole-process CPU seconds consumed so far — **user + system, summed across every thread**
/// (`getrusage(RUSAGE_SELF)`).
///
/// The perf probes report wall-clock frame time, which on this machine is not a usable regression
/// instrument: parallel builds share the same cores, and two identical probe runs
/// of the same pin came back 49.6 ms and 28.6 ms apart purely on machine load. CPU-per-frame moves
/// with the work we actually do, not with who else is compiling — and it is the metric the Mac
/// report is written in ("250 % CPU at 59 fps" against 1.12.1's "100 % at 160"), so a probe that
/// prints it can be compared against a reporter's number directly.
///
/// Unix answers through `getrusage`, Windows through `GetProcessTimes`;
/// elsewhere `None`: the probes print the field only where the platform answers.
pub(crate) fn process_cpu_secs() -> Option<f64> {
    #[cfg(unix)]
    {
        // SAFETY: `getrusage` writes a fully-initialized `rusage` into the out-param and reads
        // nothing from it; zeroed is a valid starting value for a C struct of plain integers.
        unsafe {
            let mut ru: libc::rusage = std::mem::zeroed();
            if libc::getrusage(libc::RUSAGE_SELF, &mut ru) != 0 {
                return None;
            }
            let secs = |t: libc::timeval| t.tv_sec as f64 + t.tv_usec as f64 * 1e-6;
            Some(secs(ru.ru_utime) + secs(ru.ru_stime))
        }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
        // SAFETY: `GetProcessTimes` writes four fully-initialised `FILETIME`s into its out-params
        // and reads nothing from them; zeroed is a valid starting value for two plain integers,
        // and the pseudo-handle `GetCurrentProcess` returns is never closed.
        unsafe {
            let mut creation = std::mem::zeroed();
            let mut exit = std::mem::zeroed();
            let mut kernel = std::mem::zeroed();
            let mut user = std::mem::zeroed();
            if GetProcessTimes(
                GetCurrentProcess(),
                &mut creation,
                &mut exit,
                &mut kernel,
                &mut user,
            ) == 0
            {
                return None;
            }
            Some(filetime_secs(kernel) + filetime_secs(user))
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        None
    }
}

/// A `FILETIME` — 100-nanosecond ticks — as an integer, for the differences and sums the
/// Windows arms take before converting.
#[cfg(windows)]
fn filetime_ticks(t: windows_sys::Win32::Foundation::FILETIME) -> u64 {
    (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)
}

/// A `FILETIME` as seconds.
#[cfg(windows)]
fn filetime_secs(t: windows_sys::Win32::Foundation::FILETIME) -> f64 {
    filetime_ticks(t) as f64 * 1e-7
}

/// The process's page-fault counters so far — `(minor, major)` from `getrusage`. A minor fault
/// is a page the kernel had to map or zero-fill on first touch: memory the allocator handed back
/// to the OS and then asked for again. It is CPU time the process pays in the kernel with no
/// syscall in any user stack — invisible to a sampler, visible to [`thread_cpu_table`]'s `sys`
/// column. Per frame, it is the number that says whether that column is allocator churn.
pub(crate) fn process_faults() -> Option<(u64, u64)> {
    #[cfg(unix)]
    {
        // SAFETY: as in `process_cpu_secs` — `getrusage` fills the out-param completely.
        unsafe {
            let mut ru: libc::rusage = std::mem::zeroed();
            if libc::getrusage(libc::RUSAGE_SELF, &mut ru) != 0 {
                return None;
            }
            Some((ru.ru_minflt as u64, ru.ru_majflt as u64))
        }
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// CPU seconds consumed by **the calling thread** (`CLOCK_THREAD_CPUTIME_ID`).
///
/// The twin [`process_cpu_secs`] cannot answer the question a hitch actually poses. It sums every
/// thread, so a frame in which an asset worker spent 12 ms decompressing reads identically to a
/// frame in which the main thread blocked for 12 ms — the first costs nothing the player can see,
/// the second *is* the stutter. Narrowing to one thread separates them.
///
/// **Only meaningful when called from the thread you mean.** Bevy's executor runs systems across
/// the task pool, so a caller that wants the *main* thread's number must pin itself there with a
/// [`NonSendMarker`](bevy::ecs::system::NonSendMarker) param; without it the reading silently
/// becomes "whichever worker happened to run this system", which is noise shaped like a
/// measurement.
///
/// Unix answers through `clock_gettime`, Windows through `GetThreadTimes` on the calling
/// thread's pseudo-handle — the same "whichever thread calls" contract;
/// elsewhere `None`, like its twin.
pub(crate) fn main_thread_cpu_secs() -> Option<f64> {
    #[cfg(unix)]
    {
        // SAFETY: `clock_gettime` writes a fully-initialized `timespec` into the out-param and
        // reads nothing from it; zeroed is a valid starting value for two plain integers.
        unsafe {
            let mut ts: libc::timespec = std::mem::zeroed();
            if libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) != 0 {
                return None;
            }
            Some(ts.tv_sec as f64 + ts.tv_nsec as f64 * 1e-9)
        }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{GetCurrentThread, GetThreadTimes};
        // SAFETY: as in `process_cpu_secs` — four out-params fully written, nothing read, and
        // `GetCurrentThread` is a pseudo-handle valid on the thread that asked for it.
        unsafe {
            let mut creation = std::mem::zeroed();
            let mut exit = std::mem::zeroed();
            let mut kernel = std::mem::zeroed();
            let mut user = std::mem::zeroed();
            if GetThreadTimes(
                GetCurrentThread(),
                &mut creation,
                &mut exit,
                &mut kernel,
                &mut user,
            ) == 0
            {
                return None;
            }
            Some(filetime_secs(kernel) + filetime_secs(user))
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        None
    }
}

/// Cumulative **machine-wide** CPU ticks as `(busy, total)` — every core, every process, us
/// included (`host_statistics64(HOST_CPU_LOAD_INFO)`). Diff two samples and `busy/total` is the
/// fraction of all cores that were doing work between them.
///
/// Why the probes need it, when [`process_cpu_secs`] above already promises load-independence:
/// that promise is **weaker than it reads**, and 1157 caught it. `cpu_ms` is per-frame work rather
/// than wall time, so it shrugs off the scheduling delay that makes frame time useless under load
/// — but it is not immune. The same LBRS pin leg, same binary, same day, read **25.93** while two
/// other slots ran `cargo test --workspace` (load 20-32 on 14 cores) and **18.80-20.12** on a quiet
/// machine: a ~35 % inflation, far outside the ±0.4 band the campaign reasons in. Cache and
/// memory-bandwidth contention is work we genuinely do; `getrusage` counts it and cannot tell us
/// it was someone else's fault.
///
/// So the leg has to say how loaded the machine was, and the reader compares. Deliberately a
/// **stamp, not a gate** (the director's call): no threshold is invented here. Two legs at similar
/// `sys_busy_pct` are comparable; two legs far apart are not, whatever their `cpu_ms` says. That
/// is a relative rule, which is the only kind the four calibration points behind 1157 support.
///
/// **Do not reach for `uptime` instead.** Load average is a 1-minute *decaying* mean and lags
/// badly: a leg stamped 99 % here was taken at load 5.55, seconds after a build burst the average
/// had not caught up with. Cross-checked against an independent out-of-process sampler over the
/// same window — 44 % vs 44.5 % — so where the two disagree, this is the one that is right.
///
/// macOS answers through `host_statistics64`, Windows through `GetSystemTimes` (decision 2219
/// — its kernel time INCLUDES idle, so busy is kernel − idle + user); elsewhere `None`: the
/// probes print the field only where the platform answers.
///
/// The `deprecated` allow is `libc::mach_host_self`, whose deprecation note says "use the `mach2`
/// crate instead". Checked, and it does not apply here: `mach2` 0.5.0 carries `mach_host_self` and
/// **nothing else this call needs** — no `host_statistics64`, no `host_cpu_load_info`, no
/// `HOST_CPU_LOAD_INFO`. Taking the advice would split one call across two crates and add a direct
/// dependency to get the *port* while `libc` still supplies the call it is passed to. Revisit if
/// `mach2` ever grows the host-statistics surface.
#[allow(deprecated)]
pub(crate) fn system_cpu_ticks() -> Option<(u64, u64)> {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: `host_statistics64` fills at most `count` u32s into the out-param and reads
        // nothing from it; `host_cpu_load_info` is plain integers, so zeroed is a valid starting
        // value. `mach_host_self` returns the global host port — no send right to deallocate.
        unsafe {
            let mut info: libc::host_cpu_load_info = std::mem::zeroed();
            let mut count = libc::HOST_CPU_LOAD_INFO_COUNT;
            if libc::host_statistics64(
                libc::mach_host_self(),
                libc::HOST_CPU_LOAD_INFO,
                (&raw mut info).cast(),
                &mut count,
            ) != libc::KERN_SUCCESS
            {
                return None;
            }
            let total: u64 = info.cpu_ticks.iter().map(|&x| u64::from(x)).sum();
            let idle = u64::from(info.cpu_ticks[libc::CPU_STATE_IDLE as usize]);
            Some((total - idle, total))
        }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::GetSystemTimes;
        // SAFETY: `GetSystemTimes` writes three fully-initialised `FILETIME`s and reads nothing.
        unsafe {
            let mut idle = std::mem::zeroed();
            let mut kernel = std::mem::zeroed();
            let mut user = std::mem::zeroed();
            if GetSystemTimes(&mut idle, &mut kernel, &mut user) == 0 {
                return None;
            }
            // All three are summed across every processor, and kernel time includes idle.
            let total = filetime_ticks(kernel) + filetime_ticks(user);
            Some((total.saturating_sub(filetime_ticks(idle)), total))
        }
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        None
    }
}

/// CPU seconds consumed so far by **every thread of this process, by name** — the split of
/// [`process_cpu_secs`] that says *which* thread a tax landed on. A frame's `cpu_ms` rose by
/// ~1.8 ms under vsync with no system growing on a sampled profile and no thread spinning:
/// the number that resolves that is per-thread CPU across a window, which
/// no sampler reports and this call does.
///
/// macOS only (`task_threads` + `thread_info(THREAD_BASIC_INFO)`, names through
/// `pthread_getname_np`); elsewhere `None`, like the twins above. Threads are keyed by their
/// pthread identity so two snapshots subtract, user and system time apart — the vsync tax
/// turned out to be the main thread's kernel time, which a user-mode sampler never sees; a
/// thread without a name reports as `"?"`.
/// Cost: one kernel round-trip per thread — tens of microseconds, taken twice per probe
/// window, never per frame.
pub(crate) fn thread_cpu_table() -> Option<Vec<(usize, String, f64, f64)>> {
    // `libc` deprecates its mach port bindings in favour of a crate this module does not need
    // for two symbols; both are libSystem's, the import every mach caller links.
    #[cfg(target_os = "macos")]
    extern "C" {
        static mach_task_self_: libc::mach_port_t;
        fn mach_port_deallocate(
            task: libc::mach_port_t,
            name: libc::mach_port_t,
        ) -> libc::kern_return_t;
    }
    #[cfg(target_os = "macos")]
    {
        use std::ffi::CStr;
        let mut out = Vec::new();
        // SAFETY: mach's own out-param protocol — `task_threads` hands back a kernel-allocated
        // array of thread ports and its count, each port is inspected with a stack-allocated,
        // count-checked `thread_basic_info`, and every port and the array are released after.
        unsafe {
            let task = mach_task_self_;
            let mut list: libc::thread_act_array_t = std::ptr::null_mut();
            let mut count: libc::mach_msg_type_number_t = 0;
            if libc::task_threads(task, &mut list, &mut count) != libc::KERN_SUCCESS {
                return None;
            }
            for i in 0..count as usize {
                let port = *list.add(i);
                let mut info: libc::thread_basic_info = std::mem::zeroed();
                let mut n = libc::THREAD_BASIC_INFO_COUNT;
                let kr = libc::thread_info(
                    port,
                    libc::THREAD_BASIC_INFO as libc::thread_flavor_t,
                    (&mut info as *mut libc::thread_basic_info).cast(),
                    &mut n,
                );
                if kr == libc::KERN_SUCCESS {
                    let secs = |t: libc::time_value_t| {
                        f64::from(t.seconds) + f64::from(t.microseconds) * 1e-6
                    };
                    let (user, system) = (secs(info.user_time), secs(info.system_time));
                    let pthread = libc::pthread_from_mach_thread_np(port);
                    let mut name = [0 as libc::c_char; 64];
                    let label = if pthread != 0
                        && libc::pthread_getname_np(pthread, name.as_mut_ptr(), name.len()) == 0
                    {
                        let s = CStr::from_ptr(name.as_ptr()).to_string_lossy();
                        if s.is_empty() {
                            "?".to_string()
                        } else {
                            s.into_owned()
                        }
                    } else {
                        "?".to_string()
                    };
                    out.push((pthread as usize, label, user, system));
                }
                mach_port_deallocate(task, port);
            }
            libc::vm_deallocate(
                task,
                list as libc::vm_address_t,
                (count as usize * std::mem::size_of::<libc::thread_act_t>()) as libc::vm_size_t,
            );
        }
        Some(out)
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}
