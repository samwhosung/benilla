//! The math library (`lmathlib.c`, radians) and 1.12's `bit` library (`0x7fadc0`).

use crate::api::R;
use crate::value::Value;
use crate::vm::Vm;

pub fn open(vm: &mut Vm) {
    // `luaL_openlib` makes a library's table empty, by `lua_newtable` (`0x6f4dff`).
    let t = vm.heap.new_table(0, 0);
    let fns: [(&'static str, crate::heap::NativeFn); 24] = [
        ("abs", |vm, n| un(vm, n, f64::abs)),
        ("sin", |vm, n| un(vm, n, sin)),
        ("cos", |vm, n| un(vm, n, cos)),
        ("tan", |vm, n| un(vm, n, tan)),
        ("asin", |vm, n| un(vm, n, f64::asin)),
        ("acos", |vm, n| un(vm, n, f64::acos)),
        ("atan", |vm, n| un(vm, n, f64::atan)),
        ("atan2", |vm, n| bin(vm, n, f64::atan2)),
        ("ceil", |vm, n| un(vm, n, f64::ceil)),
        ("floor", |vm, n| un(vm, n, f64::floor)),
        ("mod", |vm, n| bin(vm, n, fmod)),
        ("frexp", frexp),
        ("ldexp", ldexp),
        ("sqrt", |vm, n| un(vm, n, f64::sqrt)),
        ("min", min),
        ("max", max),
        ("log", |vm, n| un(vm, n, f64::ln)),
        ("log10", |vm, n| un(vm, n, f64::log10)),
        ("exp", |vm, n| un(vm, n, exp)),
        ("deg", |vm, n| un(vm, n, deg)),
        ("pow", |vm, n| bin(vm, n, pow)),
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
    let pow = vm.new_native("__pow", math_pow());
    vm.set_global("__pow", pow);
    open_bit(vm);
}

/// `math_pow` (`0x7fb050`), which the client's `luaopen_math` also registers as the global
/// `__pow` (`0x7fb4c4`), the function its `^` on two numbers calls (`0x6f9b52`).
pub fn math_pow() -> crate::heap::NativeFn {
    |vm, n| bin(vm, n, pow)
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

/// 2^63: `fsin`, `fcos` and `fptan` reduce only an operand below it in magnitude, and leave a
/// finite one at or past it unchanged, with C2 set (Intel SDM, `FSIN`).
const X87_TRIG_RANGE: f64 = 9_223_372_036_854_775_808.0;

/// `math_sin` (`0x7fae10`) is a bare `fsin` (`0x7fae1d`): past the x87's range the operand is the
/// answer.
fn sin(x: f64) -> f64 {
    if x.is_finite() && x.abs() >= X87_TRIG_RANGE {
        x
    } else {
        x.sin()
    }
}

/// `math_cos` (`0x7fae40`) is a bare `fcos` (`0x7fae4d`): past the x87's range the operand is the
/// answer.
fn cos(x: f64) -> f64 {
    if x.is_finite() && x.abs() >= X87_TRIG_RANGE {
        x
    } else {
        x.cos()
    }
}

/// `math_tan` (`0x7fae70`): `fptan` (`0x7fae7d`) pushes 1.0 over the tangent and `0x7fae84` pops
/// it. Past the x87's range `fptan` pushes nothing, so that pop takes the operand and the `fstp`
/// at `0x7fae86` stores from an empty stack, the masked underflow's indefinite.
fn tan(x: f64) -> f64 {
    if x.is_finite() && x.abs() >= X87_TRIG_RANGE {
        f64::from_bits(crate::number::INDEFINITE)
    } else {
        x.tan()
    }
}

/// `math_exp` (`0x7fb100`) computes inline on the x87, `2^(x·log2 e)` split at `frndint`; for an
/// infinite argument its `fsub` (`0x7fb11c`) is `±inf − ±inf`, the indefinite, where C's is
/// `inf` or `0`.
fn exp(x: f64) -> f64 {
    if x.is_infinite() {
        f64::NAN
    } else {
        x.exp()
    }
}

/// The client's `pow` (`__CIpow`, `0x73f940`): a NaN operand is the answer (its `faddp` at
/// `0x73fa0c`), so neither C99's `1^NaN = 1` nor `NaN^0 = 1`, and `__powhlp` (`0x744c8c`)
/// answers the indefinite for `±1^±inf` (`0x744cdc`), where C's is `1`.
fn pow(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() || (b.is_infinite() && a.abs() == 1.0) {
        f64::NAN
    } else {
        a.powf(b)
    }
}

/// The client's `fmod` (`__CIfmod`, `0x73f90a`), `fprem` with the sign of the dividend, behind
/// the operand classes of `__cintrindisp2` (`0x7436c0`, its table at `0x875eb0`): an infinite
/// divisor is a domain error, the indefinite (`0x743ad6`), where C's answers the dividend, and a
/// zero dividend over a finite divisor is `fldz` (`0x743a38`), so `fmod(-0, 2)` is `+0`.
fn fmod(a: f64, b: f64) -> f64 {
    if b.is_infinite() {
        f64::NAN
    } else if a == 0.0 && b != 0.0 && !b.is_nan() {
        0.0
    } else {
        a % b
    }
}

fn un(vm: &mut Vm, nargs: usize, f: fn(f64) -> f64) -> R<usize> {
    let x = vm.check_number(nargs, 1)?;
    Ok(vm.ret1(num(f(x), x, 0.0)))
}

/// A C library result as the client's runtime answers it: a domain error is the x87 indefinite.
fn num(r: f64, a: f64, b: f64) -> Value {
    Value::Num(if r.is_nan() {
        crate::number::x87_fn(r, a, b)
    } else {
        r
    })
}

/// A two-argument function: argument 1 checked first, then 2, as the client's `math_atan2`,
/// `math_mod` and `math_pow` check them (`0x7faf30`, `0x7fafd0`, `0x7fb050`: `luaL_checknumber`
/// on 1 at `0x7fb057`, then on 2 at `0x7fb066`).
fn bin(vm: &mut Vm, nargs: usize, f: fn(f64, f64) -> f64) -> R<usize> {
    let x = vm.check_number(nargs, 1)?;
    let y = vm.check_number(nargs, 2)?;
    Ok(vm.ret1(num(f(x, y), x, y)))
}

fn frexp(vm: &mut Vm, nargs: usize) -> R<usize> {
    let x = vm.check_number(nargs, 1)?;
    // The client's `_frexp` (`0x7fd8db`): `0` for either zero (`fldz`, `0x40c6fa`), and for an
    // infinity or NaN an exponent of -1 (`0x7fd908`) beside the indefinite (`0x7fd939`) or the NaN.
    let (m, e) = if x == 0.0 {
        (0.0, 0)
    } else if x.is_nan() {
        (x, -1)
    } else if x.is_infinite() {
        (f64::from_bits(crate::number::INDEFINITE), -1)
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

/// `math_ldexp` (`0x7fb1f0`): the client checks argument 2 first (`0x7fb1f8`), then 1
/// (`0x7fb20a`).
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

/// MSVC's `rand` (`0x7400e5`): `s = s·214013 + 2531011`, answering `(s >> 16) & 0x7fff`, so
/// `RAND_MAX` is 32767. The client's state is the main thread's, seeded with the millisecond
/// clock before `WinMain` (`0x5b800b` → `0x5d1c70`, `srand` at `0x5d1c8b`). Deviation: the
/// client's one process-wide stream is shared with its engine draws; here each VM holds its own,
/// seeded from the wall clock when it is built, so a fork or a clone draws its own sequence.
#[derive(Clone)]
pub(crate) struct CrtRand(u32);

impl CrtRand {
    /// `srand` (`0x7400d8`): the state is the seed.
    pub(crate) fn seeded(seed: u32) -> CrtRand {
        CrtRand(seed)
    }

    /// The start-up seed, `OsGetAsyncTimeMs` (`0x42c010`): milliseconds, as a 32-bit integer.
    pub(crate) fn from_clock() -> CrtRand {
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(1, |d| d.as_millis() as u32);
        CrtRand(ms)
    }

    fn next(&mut self) -> i32 {
        self.0 = self.0.wrapping_mul(214_013).wrapping_add(2_531_011);
        ((self.0 >> 16) & 0x7fff) as i32
    }
}

/// MSVC's `RAND_MAX`.
const RAND_MAX: i32 = 0x7fff;

/// `math_random` (`0x7fb310`): `rand() % 32767` (`0x7fb324`) times the stored `1/32767`
/// (`0x822d38`), a multiply; an interval's `floor` is taken to an `int` by `__ftol` and its low
/// end added as an `int` (`0x7fb3bb`, `0x7fb419`).
fn random(vm: &mut Vm, nargs: usize) -> R<usize> {
    const RECIP: f64 = 1.0 / RAND_MAX as f64;
    let r = f64::from(vm.rand.next() % RAND_MAX) * RECIP;
    let v = match nargs {
        0 => r,
        1 => {
            let u = vm.check_int(nargs, 1)?;
            if u < 1 {
                return Err(vm.arg_error(1, "interval is empty"));
            }
            f64::from(crate::api::c_int((r * f64::from(u)).floor()).wrapping_add(1))
        }
        2 => {
            let l = vm.check_int(nargs, 1)?;
            let u = vm.check_int(nargs, 2)?;
            if l > u {
                return Err(vm.arg_error(2, "interval is empty"));
            }
            // The width in C's `int`, which wraps.
            let w = u.wrapping_sub(l).wrapping_add(1);
            f64::from(crate::api::c_int((r * f64::from(w)).floor()).wrapping_add(l))
        }
        _ => return Err(vm.error_at(1, "wrong number of arguments")),
    };
    Ok(vm.ret1(Value::Num(v)))
}

/// `math_randomseed` (`0x7fb450`): `srand` of argument 1 through `__ftol`.
fn randomseed(vm: &mut Vm, nargs: usize) -> R<usize> {
    let s = vm.check_int(nargs, 1)?;
    vm.srand(s as u32);
    Ok(0)
}

impl Vm {
    /// `srand` (`0x7400d8`) from the host: `math.random`'s state becomes `seed`.
    pub fn srand(&mut self, seed: u32) {
        self.rand = CrtRand::seeded(seed);
    }
}

// ── bit ──────────────────────────────────────────────────────────────────────────────────────

/// `__ftol` (`0x40a2b0`) and its low word, which every `bit` function reads: a truncation to 64
/// bits whose `edx` is dropped, so a value past 64 bits or a NaN is 0.
fn ftol(x: f64) -> u32 {
    crate::api::c_int(x) as u32
}

/// `luaL_checknumber` and `__ftol`: argument `n`'s low word.
fn arg_word(vm: &mut Vm, nargs: usize, n: usize) -> R<u32> {
    Ok(ftol(vm.check_number(nargs, n)?))
}

/// A word stored with its high word zeroed and loaded by `fild qword`: unsigned.
fn unsigned(vm: &mut Vm, v: u32) -> usize {
    vm.ret1(Value::Num(f64::from(v)))
}

/// 1.12's `bit` library (`luaopen_bit` `0x7fadc0`, array `0x822c18`), in the array's order.
pub fn bit_library() -> [(&'static str, crate::heap::NativeFn); 8] {
    [
        // `bit_bnot` (`0x7faa50`): `not eax`, unsigned (`0x7faa6d`).
        ("bnot", |vm, n| {
            let a = arg_word(vm, n, 1)?;
            Ok(unsigned(vm, !a))
        }),
        // `bit_band`, `bit_bor`, `bit_bxor` (`0x7faa90`, `0x7fab10`, `0x7fab90`): every argument
        // from 2 to `lua_gettop` folded into the first (`0x7faac0`..`0x7faad9`), unsigned
        // (`0x7faae1`).
        ("band", |vm, n| fold(vm, n, |a, b| a & b)),
        ("bor", |vm, n| fold(vm, n, |a, b| a | b)),
        ("bxor", |vm, n| fold(vm, n, |a, b| a ^ b)),
        // `bit_lshift`, `bit_rshift` (`0x7fac10`, `0x7fac70`): the count, argument 2, read first
        // (`0x7fac18`), masked to 5 bits by `shl`/`shr eax,cl`, unsigned (`0x7fac40`, `0x7faca0`).
        ("lshift", |vm, n| shift(vm, n, |a, c| a << c)),
        ("rshift", |vm, n| shift(vm, n, |a, c| a >> c)),
        // `bit_arshift` (`0x7facd0`): `sar eax,cl`, loaded signed by `fild dword` (`0x7fad06`).
        ("arshift", |vm, n| {
            let c = arg_word(vm, n, 2)?;
            let a = arg_word(vm, n, 1)?;
            Ok(vm.ret1(Value::Num(f64::from((a as i32) >> (c & 31)))))
        }),
        // `bit_mod` (`0x7fad20`) and its remainder (`0x7fad70`): the divisor, argument 2, read
        // first; `cdq; idiv`, the remainder signed (`0x7fadab`).
        ("mod", |vm, n| {
            let d = vm.check_number(n, 2)?;
            let b = ftol(d) as i32;
            // A divisor whose low word is 0 (`0x7fad32`) answers 1 divided by it (`fdivr` with
            // 1.0 at `0x7fad42`), argument 1 unread: `mod(7, 0.5)` is 2, `mod(7, 0)` infinity.
            if b == 0 {
                return Ok(vm.ret1(Value::Num(1.0 / d)));
            }
            let a = arg_word(vm, n, 1)? as i32;
            // `wrapping_rem`: `INT32_MIN % -1` is 0, not a Rust panic.
            Ok(vm.ret1(Value::Num(f64::from(a.wrapping_rem(b)))))
        }),
    ]
}

fn open_bit(vm: &mut Vm) {
    // `luaL_openlib` makes a library's table empty, by `lua_newtable` (`0x6f4dff`).
    let t = vm.heap.new_table(0, 0);
    for (name, f) in bit_library() {
        let v = vm.new_native(name, f);
        let k = vm.str(name.as_bytes());
        vm.heap.table_mut(t).set(k, v).unwrap();
    }
    vm.set_global("bit", Value::Table(t));
}

/// Argument 1, then each argument after it folded in: one argument answers itself.
fn fold(vm: &mut Vm, nargs: usize, f: fn(u32, u32) -> u32) -> R<usize> {
    let mut acc = arg_word(vm, nargs, 1)?;
    for i in 2..=nargs {
        acc = f(acc, arg_word(vm, nargs, i)?);
    }
    Ok(unsigned(vm, acc))
}

/// A logical shift: the count first, then the value; x86 masks the count to 5 bits.
fn shift(vm: &mut Vm, nargs: usize, f: fn(u32, u32) -> u32) -> R<usize> {
    let c = arg_word(vm, nargs, 2)?;
    let a = arg_word(vm, nargs, 1)?;
    Ok(unsigned(vm, f(a, c & 31)))
}
