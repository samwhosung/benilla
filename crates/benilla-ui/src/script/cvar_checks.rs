//! CVar change callbacks that can refuse a write. The reference's `CVar::Set` (`0x63df50`) calls
//! a record's callback before it stores anything (`0x63df7d`) and keeps the old value when the
//! callback answers false (`0x63df85`), on every write that asks for the callback: `SetCVar`
//! (`0x488ca8`), a CVar's console command (`0x63de23`), `set` (`0x63d55b`) and the engine's own
//! writes. A row carries its callback as a [`CvarCheck`], which the script VM's mirror and the
//! host's registry both run, so a refused write never reaches `GetCVar`.
//!
//! A value the config file saved is not refused: the file is read before these CVars are
//! registered (`InitializeGlobal` loads it at `0x402375`, the video block registers from
//! `0x4027e7`), so `CVar::Register` finds a record already holding it, runs the callback and
//! ignores its answer (`0x63dc58`).

use super::binding_abi::s_str_to_float;
use super::cvars::sstr_to_int;

/// What a [`CvarCheck`] answers a write: whether it may be stored, and the line the callback
/// prints to the console (`0x63cb50`), refused or not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CvarVerdict {
    pub accept: bool,
    pub line: Option<String>,
}

impl CvarVerdict {
    fn accept() -> Self {
        Self {
            accept: true,
            line: None,
        }
    }

    fn accept_saying(line: impl Into<String>) -> Self {
        Self {
            accept: true,
            line: Some(line.into()),
        }
    }

    fn refuse(line: impl Into<String>) -> Self {
        Self {
            accept: false,
            line: Some(line.into()),
        }
    }
}

/// A row's change callback as a gate: the value being written and the applied one in, the
/// verdict out. It holds no state; the applied value stands in for what the reference's callback
/// keeps elsewhere.
pub type CvarCheck = fn(value: &str, applied: &str) -> CvarVerdict;

/// `SmallCull`'s callback (`0x688b10`): `sscanf(value, "%f")` (`0x73fb5d`, format `0x835160`),
/// then a refusal below the double 0.001 (`[0x801608]`) or above the float 2.0 (`[0x801628]`),
/// with "SmallCull must be in range 0.001 - 2.0." (`0x869900`). A value with no number in it
/// leaves the callback comparing the bits of its own string pointer, read as a float far below
/// the lower bound.
pub fn check_small_cull(value: &str, _applied: &str) -> CvarVerdict {
    match scanf_f(value) {
        Some(v) if f64::from(v) >= 0.001 && v <= 2.0 => CvarVerdict::accept(),
        _ => CvarVerdict::refuse("SmallCull must be in range 0.001 - 2.0."),
    }
}

/// `shadowLevel`'s callback (`0x688c10`): `SStrToInt` (`0x64ac60`), refused above 1 with "Shadow
/// mip level must be in range 0 - 1." (`0x869998`), else accepted with "Shadow mip level changed
/// upon restart." (`0x869970`). The test is signed and has no lower bound.
pub fn check_shadow_level(value: &str, _applied: &str) -> CvarVerdict {
    if sstr_to_int(value) > 1 {
        CvarVerdict::refuse("Shadow mip level must be in range 0 - 1.")
    } else {
        CvarVerdict::accept_saying("Shadow mip level changed upon restart.")
    }
}

/// `texLodBias`'s callback (`0x688c90`): `SStrToFloat` (`0x64aaa0`), refused below -1.0
/// (`[0x8030a0]`) or above 1.0 (`[0x7ff9d8]`) with "TexLodBias must be in range -1.0 - 1.0."
/// (`0x869a14`). The lower test reads the parsed value as is; the upper one reloads it from the
/// float it was stored to (`0x688c9c`, `0x688cac`).
pub fn check_tex_lod_bias(value: &str, _applied: &str) -> CvarVerdict {
    let v = s_str_to_float(value);
    if v < -1.0 || v as f32 > 1.0 {
        CvarVerdict::refuse("TexLodBias must be in range -1.0 - 1.0.")
    } else {
        CvarVerdict::accept()
    }
}

/// `baseMip`'s callback (`0x689090`): `SStrToInt` (`0x64ac60`) of the value against the device's
/// current first mip level (`0x589c50`, the device's `+0x31c`, which only this callback sets,
/// through `0x589bf0`): the same level is accepted silently; else anything outside 0..=1 is refused
/// with "BaseMip must be 0 or 1" (`0x869c9c`), and 0 or 1 is applied with "BaseMip level changed
/// to %d" (`0x869cb4`).
///
/// benilla's device uploads every level, so the applied value stands for the device's level: the
/// two differ only after a saved out-of-range value, which the reference keeps without applying,
/// and then only in the console line.
pub fn check_base_mip(value: &str, applied: &str) -> CvarVerdict {
    let level = sstr_to_int(value);
    if level == sstr_to_int(applied) {
        CvarVerdict::accept()
    } else if !(0..=1).contains(&level) {
        CvarVerdict::refuse("BaseMip must be 0 or 1")
    } else {
        CvarVerdict::accept_saying(format!("BaseMip level changed to {level}"))
    }
}

/// What the `gx*` callbacks print when they take a value (`0x865008`): the device record changed
/// and waits for the next `gxRestart`.
const GX_PENDING: &str = "set pending gxRestart";

/// `gxTripleBuffer`'s callback (`0x63afe0`): `SStrToInt` (`0x64ac60`), refused unless 0 or 1 with
/// "TripleBuffer must be 0 or 1" (`0x8650b8`), else taken with [`GX_PENDING`].
pub fn check_triple_buffer(value: &str, _applied: &str) -> CvarVerdict {
    if matches!(sstr_to_int(value), 0 | 1) {
        CvarVerdict::accept_saying(GX_PENDING)
    } else {
        CvarVerdict::refuse("TripleBuffer must be 0 or 1")
    }
}

/// The eleven rates `gxRefresh`'s callback takes, the table at `0x8645e0`.
const REFRESH_RATES: [i32; 11] = [60, 70, 72, 75, 85, 90, 100, 120, 160, 180, 200];

/// `gxRefresh`'s callback (`0x63aed0`): `SStrToInt` (`0x64ac60`) looked up in [`REFRESH_RATES`]
/// (`0x63aee4`-`0x63aef6`). A miss is refused with "unsupported refresh rate, must be one of "
/// (`0x86508c`) and the rates as `%d` joined by ", " (`0x84480c`); a hit is taken with
/// [`GX_PENDING`].
pub fn check_refresh(value: &str, _applied: &str) -> CvarVerdict {
    if REFRESH_RATES.contains(&sstr_to_int(value)) {
        return CvarVerdict::accept_saying(GX_PENDING);
    }
    let rates: Vec<String> = REFRESH_RATES.iter().map(i32::to_string).collect();
    CvarVerdict::refuse(format!(
        "unsupported refresh rate, must be one of {}",
        rates.join(", ")
    ))
}

/// `farclip`'s callback (`0x688d40`): `SStrToFloat` (`0x64aaa0`), refused below 177
/// (`[0x81021c]`) or above 777 (`[0x80fed8]`) with "FarClip must be in range 177.0 - 777.0."
/// (`0x869a94`). As in `texLodBias`'s, the lower test reads the parse as is and the upper one the
/// float it was stored to (`0x688d4c`, `0x688d5c`), and a NaN passes both, unordered
/// (`0x688d57`, `0x688d67`). Neither bound clamps.
pub fn check_farclip(value: &str, _applied: &str) -> CvarVerdict {
    let v = s_str_to_float(value);
    if v < 177.0 || v as f32 > 777.0 {
        CvarVerdict::refuse("FarClip must be in range 177.0 - 777.0.")
    } else {
        CvarVerdict::accept()
    }
}

/// `frillDensity`'s callback (`0x688de0`): `SStrToInt` (`0x64ac60`) compared unsigned to 1 and
/// 256 (`0x688dec`-`0x688df6`), so a negative is refused too, with "Frill Density must be in
/// range 1 - 256." (`0x869ae4`).
pub fn check_frill_density(value: &str, _applied: &str) -> CvarVerdict {
    if (1..=256).contains(&(sstr_to_int(value) as u32)) {
        CvarVerdict::accept()
    } else {
        CvarVerdict::refuse("Frill Density must be in range 1 - 256.")
    }
}

/// The camera block's range validator (`0x50b330(value, min, max)`): `SStrToFloat` (`0x64aaa0`),
/// refused below `min` or above `max` (unordered refuses too), each bound a float widened to the
/// parse's precision, with `Value out of range (%f - %f)` (`0x84fb00`) printed through `0x63cd00`.
/// So a lower bound stored as a float above its decimal refuses that decimal: "0.1" parses under
/// the float 0.1, and `cameraYawMoveSpeed 0.1` is refused.
fn camera_range(value: &str, min: f32, max: f32) -> CvarVerdict {
    let v = s_str_to_float(value);
    if v >= f64::from(min) && v <= f64::from(max) {
        CvarVerdict::accept()
    } else {
        CvarVerdict::refuse(format!(
            "Value out of range ({:.6} - {:.6})",
            f64::from(min),
            f64::from(max)
        ))
    }
}

/// `0x50c000`, the camera's angular rates (`camera*MoveSpeed`, `camera*SmoothSpeed`): its bounds
/// are degrees made from radians, `[0x8089ec]·[0x803a78]` and `[0x8089e8]·[0x803a78]` stored as
/// floats, 0.1 and 360.
pub fn check_camera_rate(value: &str, _applied: &str) -> CvarVerdict {
    const DEG_PER_RAD: f32 = 57.295_78; // [0x803a78]
    let min = (f64::from(0.001_745_329_2f32) * f64::from(DEG_PER_RAD)) as f32; // [0x8089ec]
    let max = (f64::from(6.283_185_5f32) * f64::from(DEG_PER_RAD)) as f32; // [0x8089e8]
    camera_range(value, min, max)
}

/// `0x50b380`, a saved view's pitch: `[0x8089dc]·[0x803a78]` to `[0x8089d8]·[0x803a78]` as
/// floats, ±89 degrees.
pub fn check_camera_pitch(value: &str, _applied: &str) -> CvarVerdict {
    const DEG_PER_RAD: f32 = 57.295_78; // [0x803a78]
    let max = (f64::from(1.553_343_f32) * f64::from(DEG_PER_RAD)) as f32; // [0x8089d8]
    let min = (f64::from(-1.553_343_f32) * f64::from(DEG_PER_RAD)) as f32; // [0x8089dc]
    camera_range(value, min, max)
}

/// `0x50b3c0`, a saved view's yaw: `[0x8089e4]·[0x803a78]` to `[0x8089e0]·[0x803a78]` as floats,
/// 0 to 360 degrees, so a negative offset is refused.
pub fn check_camera_yaw(value: &str, _applied: &str) -> CvarVerdict {
    const DEG_PER_RAD: f32 = 57.295_78; // [0x803a78]
    let max = (f64::from(6.283_185_5f32) * f64::from(DEG_PER_RAD)) as f32; // [0x8089e0]
    camera_range(value, 0.0, max)
}

/// `0x50bfe0`, the camera's durations and the bob frequency: `[0x8089fc]` 0.001 to `[0x8089f8]`
/// 300.
pub fn check_camera_time(value: &str, _applied: &str) -> CvarVerdict {
    camera_range(value, 0.001, 300.0)
}

/// `0x50c040`, the camera's decay rates (`cameraBobbingSmoothSpeed`, `cameraDistanceMoveSpeed`):
/// `[0x8089f4]` 1/360 to `[0x8089f0]` 50.
pub fn check_camera_decay(value: &str, _applied: &str) -> CvarVerdict {
    camera_range(value, 1.0 / 360.0, 50.0)
}

/// `0x50b310`, `cameraDistanceMax`: `[0x8089a8]` 0 to `[0x8089a4]` 50.
pub fn check_camera_distance_max(value: &str, _applied: &str) -> CvarVerdict {
    camera_range(value, 0.0, 50.0)
}

/// `0x50c060`, `cameraSmoothStyle` and `cameraSmoothTrackingStyle`: 0 to 3.
pub fn check_camera_smooth_style(value: &str, _applied: &str) -> CvarVerdict {
    camera_range(value, 0.0, 3.0)
}

/// `0x50bfc0`, `cameraView`: 0 to 4.
pub fn check_camera_view(value: &str, _applied: &str) -> CvarVerdict {
    camera_range(value, 0.0, 4.0)
}

/// `sscanf(s, "%f")` as the reference's CRT reads it: leading white space, an optional sign, digits
/// with at most one `.` (at least one digit), then `e`/`E` with an optional sign and digits,
/// converted as a double and stored as a float. `None` when no number starts the string.
fn scanf_f(s: &str) -> Option<f32> {
    let b = s.as_bytes();
    let mut i = b
        .iter()
        .take_while(|c| matches!(c, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r'))
        .count();
    let start = i;
    if matches!(b.get(i), Some(b'+' | b'-')) {
        i += 1;
    }
    let digits = |i: &mut usize| {
        let from = *i;
        while b.get(*i).is_some_and(u8::is_ascii_digit) {
            *i += 1;
        }
        *i - from
    };
    let mut n = digits(&mut i);
    if b.get(i) == Some(&b'.') {
        i += 1;
        n += digits(&mut i);
    }
    if n == 0 {
        return None;
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        let mut e = i + 1;
        if matches!(b.get(e), Some(b'+' | b'-')) {
            e += 1;
        }
        if digits(&mut e) > 0 {
            i = e;
        }
    }
    s[start..i].parse::<f64>().ok().map(|v| v as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `0x50b330` through each wrapper: values at the float bounds pass, values past them do not,
    /// and the line prints the float bounds with `%f`.
    #[test]
    fn the_camera_ranges_are_the_validators() {
        for (check, lo, hi, line) in [
            (
                check_camera_rate as CvarCheck,
                "0.10000001",
                "360",
                "Value out of range (0.100000 - 360.000000)",
            ),
            (
                check_camera_time,
                "0.0010000001",
                "300",
                "Value out of range (0.001000 - 300.000000)",
            ),
            (
                check_camera_decay,
                "0.002777778",
                "50",
                "Value out of range (0.002778 - 50.000000)",
            ),
            (
                check_camera_distance_max,
                "0",
                "50",
                "Value out of range (0.000000 - 50.000000)",
            ),
            (
                check_camera_smooth_style,
                "0",
                "3",
                "Value out of range (0.000000 - 3.000000)",
            ),
            (
                check_camera_view,
                "0",
                "4",
                "Value out of range (0.000000 - 4.000000)",
            ),
            (
                check_camera_pitch,
                "-89",
                "89",
                "Value out of range (-89.000000 - 89.000000)",
            ),
            (
                check_camera_yaw,
                "0",
                "360",
                "Value out of range (0.000000 - 360.000000)",
            ),
        ] {
            assert!(check(lo, "").accept, "{line}: {lo}");
            assert!(check(hi, "").accept, "{line}: {hi}");
            for bad in ["-1000", "1000", "1e9"] {
                assert_eq!(check(bad, ""), CvarVerdict::refuse(line), "{bad}");
            }
        }
        // `SStrToFloat`: no `+`, no leading space, so both read 0.
        assert!(!check_camera_rate("+90", "").accept);
        assert!(check_camera_smooth_style(" 9", "").accept);
        assert!(!check_camera_rate("0.0999", "").accept);
        assert!(!check_camera_yaw("-1", "").accept, "a negative yaw offset");
        assert!(!check_camera_pitch("89.001", "").accept);
        // The bound is the float nearest 0.1, which is above the parse of "0.1".
        assert!(!check_camera_rate("0.1", "").accept);
        assert!(!check_camera_time("0.001", "").accept);
        assert!(
            !check_camera_time("0", "").accept,
            "the tilt time that panicked the clamp"
        );
    }

    #[test]
    fn the_gx_checks_take_only_their_values() {
        for ok in ["0", "1", "1.9"] {
            assert_eq!(
                check_triple_buffer(ok, "0"),
                CvarVerdict::accept_saying("set pending gxRestart"),
                "{ok}"
            );
        }
        for bad in ["2", "-1", "3"] {
            assert_eq!(
                check_triple_buffer(bad, "0"),
                CvarVerdict::refuse("TripleBuffer must be 0 or 1"),
                "{bad}"
            );
        }
        for ok in ["60", "75", "200", "85.5"] {
            assert!(check_refresh(ok, "75").accept, "{ok}");
        }
        for bad in ["59", "144", "0", "abc"] {
            assert_eq!(
                check_refresh(bad, "75"),
                CvarVerdict::refuse(
                    "unsupported refresh rate, must be one of \
                     60, 70, 72, 75, 85, 90, 100, 120, 160, 180, 200"
                ),
                "{bad}"
            );
        }
    }

    #[test]
    fn farclip_and_frill_density_refuse_without_clamping() {
        for ok in ["177", "350", "777", "777.00001", "abc177"] {
            let v = check_farclip(ok, "350");
            assert_eq!(v.accept, ok != "abc177", "{ok}");
        }
        // `SStrToFloat`'s 0 * inf is NaN, which neither of the callback's compares refuses.
        assert!(s_str_to_float("0e5000").is_nan());
        assert!(check_farclip("0e5000", "350").accept);
        for bad in ["176.9", "1000", "-5", "0"] {
            assert_eq!(
                check_farclip(bad, "350"),
                CvarVerdict::refuse("FarClip must be in range 177.0 - 777.0."),
                "{bad}"
            );
        }
        for ok in ["1", "16", "256", "48.9"] {
            assert!(check_frill_density(ok, "16").accept, "{ok}");
        }
        for bad in ["0", "257", "-1", "9000", "abc"] {
            assert_eq!(
                check_frill_density(bad, "16"),
                CvarVerdict::refuse("Frill Density must be in range 1 - 256."),
                "{bad}"
            );
        }
    }

    #[test]
    fn small_cull_takes_0_001_to_2() {
        for ok in [
            "0.001", "0.04", "2", "2.0", " 1.5", "1.5junk", "+0.5", "1e-3",
        ] {
            assert!(check_small_cull(ok, "0.04").accept, "{ok}");
        }
        for bad in ["0.0009", "0", "-1", "2.01", "abc", "", "1e9"] {
            let v = check_small_cull(bad, "0.04");
            assert!(!v.accept, "{bad}");
            assert_eq!(
                v.line.as_deref(),
                Some("SmallCull must be in range 0.001 - 2.0.")
            );
        }
    }

    #[test]
    fn shadow_level_refuses_only_above_one() {
        for ok in ["0", "1", "-5", "abc", "1.9"] {
            let v = check_shadow_level(ok, "1");
            assert!(v.accept, "{ok}");
            assert_eq!(
                v.line.as_deref(),
                Some("Shadow mip level changed upon restart.")
            );
        }
        for bad in ["2", "10"] {
            assert!(!check_shadow_level(bad, "1").accept, "{bad}");
        }
    }

    #[test]
    fn tex_lod_bias_takes_minus_one_to_one() {
        for ok in ["-1", "0.0", "1", "1.00000001", "+5", " 5"] {
            assert!(check_tex_lod_bias(ok, "0.0").accept, "{ok}");
        }
        for bad in ["-1.00000001", "1.5", "-2", "5"] {
            let v = check_tex_lod_bias(bad, "0.0");
            assert!(!v.accept, "{bad}");
            assert_eq!(
                v.line.as_deref(),
                Some("TexLodBias must be in range -1.0 - 1.0.")
            );
        }
    }

    #[test]
    fn base_mip_takes_zero_or_one_and_says_so_when_it_moves() {
        assert_eq!(check_base_mip("0", "0"), CvarVerdict::accept());
        assert_eq!(
            check_base_mip("1", "0"),
            CvarVerdict::accept_saying("BaseMip level changed to 1")
        );
        assert_eq!(
            check_base_mip("0", "1"),
            CvarVerdict::accept_saying("BaseMip level changed to 0")
        );
        for bad in ["2", "-1", "5"] {
            assert_eq!(
                check_base_mip(bad, "0"),
                CvarVerdict::refuse("BaseMip must be 0 or 1"),
                "{bad}"
            );
        }
    }
}
