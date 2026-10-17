//! Allocations the allocator refuses: under an address-space limit, a concatenation whose result
//! cannot be built raises `not enough memory` where Rust's allocator would abort the process.
//! Its own test binary: the limit is the whole process's.

#[cfg(target_os = "linux")]
#[test]
fn a_concatenation_the_allocator_refuses_is_not_enough_memory() {
    #[repr(C)]
    struct Rlimit {
        cur: u64,
        max: u64,
    }
    extern "C" {
        fn setrlimit(resource: i32, rlim: *const Rlimit) -> i32;
    }
    /// `RLIMIT_AS` on Linux.
    const RLIMIT_AS: i32 = 9;
    let gib = 1u64 << 30;
    // SAFETY: a plain libc call with a valid struct; it only lowers this process's limit.
    assert_eq!(
        unsafe {
            setrlimit(
                RLIMIT_AS,
                &Rlimit {
                    cur: 3 * gib / 2,
                    max: u64::MAX,
                },
            )
        },
        0
    );
    let got = std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(|| {
            let mut vm = benilla_lua::new_script_host_state();
            let src = br#"local s = string.rep("x", 268435456)
                local ok1, e1 = pcall(function() return s .. s .. s .. s .. s .. s end)
                local ok2, e2 = pcall(function() return s .. s .. s .. s end)
                return ok1, e1, ok2, e2, string.len(s .. "y")"#;
            match vm.exec(src, b"=t") {
                Ok(vs) => Ok(vs.into_iter().map(|v| vm.display(v)).collect::<Vec<_>>()),
                Err(e) => Err(vm.display(e)),
            }
        })
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(
        got,
        Ok(vec![
            "false".into(),
            "not enough memory".into(),
            "false".into(),
            "not enough memory".into(),
            "268435457".into()
        ])
    );
}
