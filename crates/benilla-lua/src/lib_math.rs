//! The math library (`lmathlib.c`, radians) and 1.12's `bit` library (`0x7fadc0`).

use crate::api::R;
use crate::value::Value;
use crate::vm::Vm;

/// The math function benilla has under `name`, for a state that registers its own list.
pub(crate) fn native(name: &str) -> Option<crate::heap::NativeFn> {
    Some(match name {
        "abs" => |vm, n| un(vm, n, f64::abs),
        "sin" => |vm, n| un(vm, n, f64::sin),
        "cos" => |vm, n| un(vm, n, f64::cos),
        "tan" => |vm, n| un(vm, n, f64::tan),
        "asin" => |vm, n| un(vm, n, f64::asin),
        "acos" => |vm, n| un(vm, n, f64::acos),
        "atan" => |vm, n| un(vm, n, f64::atan),
        "atan2" => |vm, n| bin_rev(vm, n, f64::atan2),
        "ceil" => |vm, n| un(vm, n, f64::ceil),
        "floor" => |vm, n| un(vm, n, f64::floor),
        // C's `fmod`: the sign of the dividend.
        "fmod" | "mod" => |vm, n| bin_rev(vm, n, |a, b| a % b),
        "frexp" => frexp,
        "ldexp" => ldexp,
        "sqrt" => |vm, n| un(vm, n, f64::sqrt),
        "min" => min,
        "max" => max,
        "log" => |vm, n| un(vm, n, f64::ln),
        "log10" => |vm, n| un(vm, n, f64::log10),
        "exp" => |vm, n| un(vm, n, f64::exp),
        "deg" => |vm, n| un(vm, n, deg),
        "pow" => |vm, n| bin_rev(vm, n, f64::powf),
        "rad" => |vm, n| un(vm, n, rad),
        "random" => random,
        "randomseed" => randomseed,
        _ => return None,
    })
}

pub fn open(vm: &mut Vm) {
    let t = vm.heap.new_table(0, 5);
    let fns: [(&'static str, crate::heap::NativeFn); 24] = [
        ("abs", |vm, n| un(vm, n, f64::abs)),
        ("sin", |vm, n| un(vm, n, f64::sin)),
        ("cos", |vm, n| un(vm, n, f64::cos)),
        ("tan", |vm, n| un(vm, n, f64::tan)),
        ("asin", |vm, n| un(vm, n, f64::asin)),
        ("acos", |vm, n| un(vm, n, f64::acos)),
        ("atan", |vm, n| un(vm, n, f64::atan)),
        ("atan2", |vm, n| bin_rev(vm, n, f64::atan2)),
        ("ceil", |vm, n| un(vm, n, f64::ceil)),
        ("floor", |vm, n| un(vm, n, f64::floor)),
        // C's `fmod`: the sign of the dividend.
        ("mod", |vm, n| bin_rev(vm, n, |a, b| a % b)),
        ("frexp", frexp),
        ("ldexp", ldexp),
        ("sqrt", |vm, n| un(vm, n, f64::sqrt)),
        ("min", min),
        ("max", max),
        ("log", |vm, n| un(vm, n, f64::ln)),
        ("log10", |vm, n| un(vm, n, f64::log10)),
        ("exp", |vm, n| un(vm, n, f64::exp)),
        ("deg", |vm, n| un(vm, n, deg)),
        ("pow", |vm, n| bin_rev(vm, n, f64::powf)),
        ("rad", |vm, n| un(vm, n, rad)),
        ("random", random),
        ("randomseed", randomseed),
    ];
    for (name, f) in fns {
        let v = vm.new_native(name, f);
        let k = vm.str(name.as_bytes());
        vm.heap.table_mut(t).set(k, v).unwrap();
    }
    let k = vm.str(b"pi");
    vm.heap
        .table_mut(t)
        .set(k, Value::Num(std::f64::consts::PI))
        .unwrap();
    vm.set_global("math", Value::Table(t));
    // The client's `luaopen_math` registers `math_pow` as the global `__pow` (`0x7fb4c4`), which
    // its `^` calls.
    let pow = vm.new_native("__pow", |vm, n| bin(vm, n, f64::powf));
    vm.set_global("__pow", pow);
    open_bit(vm);
}

/// 5.1's `RADIANS_PER_DEGREE`, `PI/180.0`.
const RADIANS_PER_DEGREE: f64 = std::f64::consts::PI / 180.0;

/// `math_deg`: a division by `RADIANS_PER_DEGREE`, which rounds otherwise than Rust's
/// `to_degrees` (a product with `180/PI`) for about one value in nine.
fn deg(x: f64) -> f64 {
    x / RADIANS_PER_DEGREE
}

/// `math_rad`: a product with `RADIANS_PER_DEGREE`.
fn rad(x: f64) -> f64 {
    x * RADIANS_PER_DEGREE
}

fn un(vm: &mut Vm, nargs: usize, f: fn(f64) -> f64) -> R<usize> {
    let x = vm.check_number(nargs, 1)?;
    Ok(vm.ret1(Value::Num(f(x))))
}

/// A two-argument C call such as `fmod(luaL_checknumber(L, 1), luaL_checknumber(L, 2))`: gcc
/// evaluated the arguments right to left, so the second is checked first. The order is the
/// x86-64 Linux C build's on every platform (clang on arm64 checks the first); the client's MSVC
/// x86-32 order is later work, as are its `rand` (#1258) and number text (#1376).
fn bin_rev(vm: &mut Vm, nargs: usize, f: fn(f64, f64) -> f64) -> R<usize> {
    let b = vm.check_number(nargs, 2)?;
    let a = vm.check_number(nargs, 1)?;
    Ok(vm.ret1(Value::Num(f(a, b))))
}

fn bin(vm: &mut Vm, nargs: usize, f: fn(f64, f64) -> f64) -> R<usize> {
    let x = vm.check_number(nargs, 1)?;
    let y = vm.check_number(nargs, 2)?;
    Ok(vm.ret1(Value::Num(f(x, y))))
}

fn frexp(vm: &mut Vm, nargs: usize) -> R<usize> {
    let x = vm.check_number(nargs, 1)?;
    let (m, e) = if x == 0.0 || !x.is_finite() {
        (x, 0)
    } else {
        let bits = x.to_bits();
        let exp = ((bits >> 52) & 0x7ff) as i32;
        if exp == 0 {
            // Subnormal: scale into the normal range first.
            let y = x * 2f64.powi(54);
            let e = ((y.to_bits() >> 52) & 0x7ff) as i32 - 1022 - 54;
            (y / 2f64.powi(e + 54), e)
        } else {
            let e = exp - 1022;
            (f64::from_bits((bits & !(0x7ff << 52)) | (1022 << 52)), e)
        }
    };
    vm.push(Value::Num(m));
    vm.push(Value::Num(e as f64));
    Ok(2)
}

/// `ldexp(luaL_checknumber(L, 1), luaL_checkint(L, 2))`: gcc evaluated the arguments right to
/// left, so the second is checked first.
fn ldexp(vm: &mut Vm, nargs: usize) -> R<usize> {
    let e = vm.check_int(nargs, 2)?;
    let x = vm.check_number(nargs, 1)?;
    Ok(vm.ret1(Value::Num(crate::number::ldexp(x, e))))
}

fn min(vm: &mut Vm, nargs: usize) -> R<usize> {
    let mut m = vm.check_number(nargs, 1)?;
    for i in 2..=nargs {
        let d = vm.check_number(nargs, i)?;
        if d < m {
            m = d;
        }
    }
    Ok(vm.ret1(Value::Num(m)))
}

fn max(vm: &mut Vm, nargs: usize) -> R<usize> {
    let mut m = vm.check_number(nargs, 1)?;
    for i in 2..=nargs {
        let d = vm.check_number(nargs, i)?;
        if d > m {
            m = d;
        }
    }
    Ok(vm.ret1(Value::Num(m)))
}

/// glibc's `rand()`: `random_r`'s additive feedback generator (`TYPE_3`, `r[i] = r[i-3] +
/// r[i-31]`), as the script host's 5.1 build called it on Linux, on every platform. The client
/// runs MSVC's clock-seeded `rand`, which is later work (#1258). Its state is the process's in C,
/// shared by every VM; here it is the VM's own, so a fork or a clone draws its own sequence.
#[derive(Clone)]
pub(crate) struct GlibcRand {
    r: [i32; 34],
    i: usize,
}

impl GlibcRand {
    /// `srandom_r`: the 31 words from `seed` by `16807 * w % (2^31 - 1)`, then 310 outputs dropped.
    pub(crate) fn seeded(seed: u32) -> GlibcRand {
        let mut r = [0i32; 34];
        let mut word = if seed == 0 { 1 } else { seed as i32 };
        r[0] = word;
        for slot in r.iter_mut().take(31).skip(1) {
            let (hi, lo) = (word / 127_773, word % 127_773);
            word = 16807i32
                .wrapping_mul(lo)
                .wrapping_sub(2836i32.wrapping_mul(hi));
            if word < 0 {
                word = word.wrapping_add(2_147_483_647);
            }
            *slot = word;
        }
        for k in 31..34 {
            r[k] = r[k - 31];
        }
        let mut g = GlibcRand { r, i: 34 };
        for _ in 0..310 {
            g.next();
        }
        g
    }

    fn next(&mut self) -> u32 {
        let v = self.r[(self.i + 34 - 31) % 34].wrapping_add(self.r[(self.i + 34 - 3) % 34]);
        self.r[self.i % 34] = v;
        self.i += 1;
        (v as u32) >> 1
    }
}

fn rand(vm: &mut Vm) -> u32 {
    vm.rand.next()
}

/// glibc's `RAND_MAX`.
const RAND_MAX: u32 = 0x7fff_ffff;

fn random(vm: &mut Vm, nargs: usize) -> R<usize> {
    let r = (rand(vm) % RAND_MAX) as f64 / RAND_MAX as f64;
    let v = match nargs {
        0 => r,
        1 => {
            let u = vm.check_int(nargs, 1)?;
            if u < 1 {
                return Err(vm.arg_error(1, "interval is empty"));
            }
            (r * u as f64).floor() + 1.0
        }
        2 => {
            let l = vm.check_int(nargs, 1)?;
            let u = vm.check_int(nargs, 2)?;
            if l > u {
                return Err(vm.arg_error(2, "interval is empty"));
            }
            // `floor(r*(u-l+1))+l`: the width in C's `int`, which wraps.
            (r * u.wrapping_sub(l).wrapping_add(1) as f64).floor() + l as f64
        }
        _ => return Err(vm.error_at(1, "wrong number of arguments")),
    };
    Ok(vm.ret1(Value::Num(v)))
}

fn randomseed(vm: &mut Vm, nargs: usize) -> R<usize> {
    let s = vm.check_int(nargs, 1)?;
    vm.rand = GlibcRand::seeded(s as u32);
    Ok(0)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::GlibcRand;

    extern "C" {
        fn srand(seed: u32);
        fn rand() -> i32;
    }

    /// The emulation against the C library the build links.
    #[test]
    fn rand_is_glibcs() {
        for seed in [1u32, 0, 42, 7, 123_456_789, u32::MAX, 0x8000_0000] {
            let mut g = GlibcRand::seeded(seed);
            // SAFETY: plain libc calls; no other test in this binary uses `rand`.
            unsafe { srand(seed) };
            for _ in 0..1000 {
                assert_eq!(g.next() as i32, unsafe { rand() }, "seed {seed}");
            }
        }
    }
}

// ── bit ──────────────────────────────────────────────────────────────────────────────────────

/// The client truncates to a 32-bit int and answers it signed.
fn u32_of(v: f64) -> u32 {
    v as i64 as u32
}

fn out(v: u32) -> Value {
    Value::Num(v as i32 as f64)
}

fn open_bit(vm: &mut Vm) {
    let t = vm.heap.new_table(0, 3);
    let fns: [(&'static str, crate::heap::NativeFn); 8] = [
        ("bnot", |vm, n| {
            let a = vm.check_number(n, 1)?;
            Ok(vm.ret1(out(!u32_of(a))))
        }),
        ("band", |vm, n| bit2(vm, n, |a, b| a & b)),
        ("bor", |vm, n| bit2(vm, n, |a, b| a | b)),
        ("bxor", |vm, n| bit2(vm, n, |a, b| a ^ b)),
        // Shift counts mask to 5 bits, as x86 shifts do.
        ("lshift", |vm, n| bit2(vm, n, |a, b| a << (b & 31))),
        ("rshift", |vm, n| bit2(vm, n, |a, b| a >> (b & 31))),
        ("arshift", |vm, n| {
            bit2(vm, n, |a, b| ((a as i32) >> (b & 31)) as u32)
        }),
        ("mod", |vm, n| {
            let a = vm.check_number(n, 1)?;
            let b = u32_of(vm.check_number(n, 2)?) as i32;
            if b == 0 {
                return Err(vm.error_at(1, "bit.mod: division by zero"));
            }
            Ok(vm.ret1(Value::Num(((u32_of(a) as i32).wrapping_rem(b)) as f64)))
        }),
    ];
    for (name, f) in fns {
        let v = vm.new_native(name, f);
        let k = vm.str(name.as_bytes());
        vm.heap.table_mut(t).set(k, v).unwrap();
    }
    vm.set_global("bit", Value::Table(t));
}

fn bit2(vm: &mut Vm, nargs: usize, f: fn(u32, u32) -> u32) -> R<usize> {
    let a = vm.check_number(nargs, 1)?;
    let b = vm.check_number(nargs, 2)?;
    Ok(vm.ret1(out(f(u32_of(a), u32_of(b)))))
}
