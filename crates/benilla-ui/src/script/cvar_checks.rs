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
use super::binding_abi::s_str_to_int;

/// What a [`CvarCheck`] answers a write: whether it may be stored, and the lines the callback
/// prints to the console (`0x63cb50`), refused or not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CvarVerdict {
    pub accept: bool,
    pub lines: Vec<String>,
}

impl CvarVerdict {
    fn accept() -> Self {
        Self {
            accept: true,
            lines: Vec::new(),
        }
    }

    fn accept_saying(line: impl Into<String>) -> Self {
        Self {
            accept: true,
            lines: vec![line.into()],
        }
    }

    fn refuse(line: impl Into<String>) -> Self {
        Self {
            accept: false,
            lines: vec![line.into()],
        }
    }
}

/// Another row's applied value by name, for a callback that reads an engine global another
/// CVar's callback sets ([`check_horizon_farclip`]).
pub type CvarRows<'a> = &'a dyn Fn(&str) -> Option<String>;

/// A row's change callback as a gate: the value being written, the applied one and the other
/// rows in, the verdict out. It holds no state; the applied values stand in for what the
/// reference's callback keeps elsewhere.
pub type CvarCheck = fn(value: &str, applied: &str, rows: CvarRows<'_>) -> CvarVerdict;

/// `SmallCull`'s callback (`0x688b10`): `sscanf(value, "%f")` (`0x73fb5d`, format `0x835160`),
/// then a refusal below the double 0.001 (`[0x801608]`) or above the float 2.0 (`[0x801628]`),
/// with "SmallCull must be in range 0.001 - 2.0." (`0x869900`). A value with no number in it
/// leaves the callback comparing the bits of its own string pointer, read as a float far below
/// the lower bound.
pub fn check_small_cull(value: &str, _applied: &str, _rows: CvarRows<'_>) -> CvarVerdict {
    match scanf_f(value) {
        Some(v) if f64::from(v) >= 0.001 && v <= 2.0 => CvarVerdict::accept(),
        _ => CvarVerdict::refuse("SmallCull must be in range 0.001 - 2.0."),
    }
}

/// `shadowLevel`'s callback (`0x688c10`): `SStrToInt` (`0x64ac60`), refused above 1 with "Shadow
/// mip level must be in range 0 - 1." (`0x869998`), else accepted with "Shadow mip level changed
/// upon restart." (`0x869970`). The test is signed and has no lower bound.
pub fn check_shadow_level(value: &str, _applied: &str, _rows: CvarRows<'_>) -> CvarVerdict {
    if s_str_to_int(value) > 1 {
        CvarVerdict::refuse("Shadow mip level must be in range 0 - 1.")
    } else {
        CvarVerdict::accept_saying("Shadow mip level changed upon restart.")
    }
}

/// `texLodBias`'s callback (`0x688c90`): `SStrToFloat` (`0x64aaa0`), refused below -1.0
/// (`[0x8030a0]`) or above 1.0 (`[0x7ff9d8]`) with "TexLodBias must be in range -1.0 - 1.0."
/// (`0x869a14`). The lower test reads the parsed value as is; the upper one reloads it from the
/// float it was stored to (`0x688c9c`, `0x688cac`).
pub fn check_tex_lod_bias(value: &str, _applied: &str, _rows: CvarRows<'_>) -> CvarVerdict {
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
pub fn check_base_mip(value: &str, applied: &str, _rows: CvarRows<'_>) -> CvarVerdict {
    let level = s_str_to_int(value);
    if level == s_str_to_int(applied) {
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
pub fn check_triple_buffer(value: &str, _applied: &str, _rows: CvarRows<'_>) -> CvarVerdict {
    if matches!(s_str_to_int(value), 0 | 1) {
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
pub fn check_refresh(value: &str, _applied: &str, _rows: CvarRows<'_>) -> CvarVerdict {
    if REFRESH_RATES.contains(&s_str_to_int(value)) {
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
pub fn check_farclip(value: &str, _applied: &str, _rows: CvarRows<'_>) -> CvarVerdict {
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
pub fn check_frill_density(value: &str, _applied: &str, _rows: CvarRows<'_>) -> CvarVerdict {
    if (1..=256).contains(&(s_str_to_int(value) as u32)) {
        CvarVerdict::accept()
    } else {
        CvarVerdict::refuse("Frill Density must be in range 1 - 256.")
    }
}

/// `MaxLights`'s callback (`0x688bd0`): `SStrToInt` (`0x64ac60`), refused below 1 (signed, `jl`)
/// or above 4 (`ja`) with "MaxLights must be in range 1 - %i." of 4 (`0x86994c`, `0x688bf1`).
pub fn check_max_lights(value: &str, _applied: &str, _rows: CvarRows<'_>) -> CvarVerdict {
    if (1..=4).contains(&s_str_to_int(value)) {
        CvarVerdict::accept()
    } else {
        CvarVerdict::refuse("MaxLights must be in range 1 - 4.")
    }
}

/// `mapObjLightLOD`'s callback (`0x688f00`): `SStrToInt`, refused outside 0..=2 (signed) with
/// "MapObjLightLOD must be 0-2" (`0x869bc4`).
pub fn check_map_obj_light_lod(value: &str, _applied: &str, _rows: CvarRows<'_>) -> CvarVerdict {
    if (0..=2).contains(&s_str_to_int(value)) {
        CvarVerdict::accept()
    } else {
        CvarVerdict::refuse("MapObjLightLOD must be 0-2")
    }
}

/// `waterLOD`'s callback (`0x689050`): it zeroes the engine's water LOD first (`0x689057`), then
/// takes only an `SStrToInt` of 0 and refuses anything else with "waterLOD fixed to 0"
/// (`0x869c88`).
pub fn check_water_lod(value: &str, _applied: &str, _rows: CvarRows<'_>) -> CvarVerdict {
    if s_str_to_int(value) == 0 {
        CvarVerdict::accept()
    } else {
        CvarVerdict::refuse("waterLOD fixed to 0")
    }
}

/// A float callback's test on `SStrToFloat` (`0x64aaa0`) left on the x87 stack: below `min`
/// (`test ah,5; jnp`) or above `max` (`test ah,0x41; je`), each bound a float. A NaN is
/// unordered and passes both.
fn outside(v: f64, min: f32, max: f32) -> bool {
    v < f64::from(min) || v > f64::from(max)
}

/// `DistCull`'s callback (`0x688b70`): refused below 1.0 (`[0x7ff9d8]`) or above 888.888916
/// (`[0x810218]`), both against the parse as is, with "DistCull must be in range 1.0 - %f." of
/// that bound as a double (`0x869928`, `0x688ba2`).
pub fn check_dist_cull(value: &str, _applied: &str, _rows: CvarRows<'_>) -> CvarVerdict {
    // The float nearest 888.8889 is the bound, 888.888916015625.
    if outside(s_str_to_float(value), 1.0, 888.888_9) {
        CvarVerdict::refuse("DistCull must be in range 1.0 - 888.888916.")
    } else {
        CvarVerdict::accept()
    }
}

/// `particleDensity`'s callback (`0x688fb0`): refused below 0.25 (`[0x8029b0]`) against the
/// parse as is, or above 1.0 (`[0x7ff9d8]`) against the float it was stored to (`0x688fbc`,
/// `0x688fcc`), with "Value must be between 0.25 and 1.0." (`0x869c40`).
pub fn check_particle_density(value: &str, _applied: &str, _rows: CvarRows<'_>) -> CvarVerdict {
    let v = s_str_to_float(value);
    if v < 0.25 || v as f32 > 1.0 {
        CvarVerdict::refuse("Value must be between 0.25 and 1.0.")
    } else {
        CvarVerdict::accept()
    }
}

/// `unitDrawDist`'s callback (`0x689000`): refused below 100 (`[0x806b10]`) or above 300
/// (`[0x810220]`), both against the parse as is, with "Value must be between 100 and 300."
/// (`0x869c64`).
pub fn check_unit_draw_dist(value: &str, _applied: &str, _rows: CvarRows<'_>) -> CvarVerdict {
    if outside(s_str_to_float(value), 100.0, 300.0) {
        CvarVerdict::refuse("Value must be between 100 and 300.")
    } else {
        CvarVerdict::accept()
    }
}

/// The callback `targetNearestDistance` and `targetNearestDistanceRadius` share (`0x490720`,
/// registered at `0x48fda4` and `0x48fd60`): refused below 1.0 (`[0x7ff9d8]`) or above 50.0
/// (`[0x804514]`), against the parse as is, with "TargetNearestDistance must be between 1 and
/// %f" of 50.0 (`0x843154`, through `ConsolePrintf` at `0x49075f`).
pub fn check_target_nearest_distance(
    value: &str,
    _applied: &str,
    _rows: CvarRows<'_>,
) -> CvarVerdict {
    if outside(s_str_to_float(value), 1.0, 50.0) {
        CvarVerdict::refuse("TargetNearestDistance must be between 1 and 50.000000")
    } else {
        CvarVerdict::accept()
    }
}

/// `horizonfarclip`'s callback (`0x689250`): its lower bound is the world's far clip
/// (`[0xc7b498]`) plus 528 (`[0x810228]`), stored as a float (`0x689269`), and its upper 2112
/// (`[0x810224]`). The parse as is is refused below the lower bound, the float it was stored to
/// above the upper one, with "HorizonFarClip must be in range %f - %f." of both (`0x869db0`,
/// built in 0x100 bytes at `0x6892c0`). The far clip is the float `farclip`'s callback
/// (`0x688d40`) hands its setter (`0x6725d0`, the one writer of `[0xc7b498]`), so the `farclip`
/// row's applied value stands in for it; without one it is 0.
pub fn check_horizon_farclip(value: &str, _applied: &str, rows: CvarRows<'_>) -> CvarVerdict {
    let far_clip = rows("farclip").map_or(0.0, |v| s_str_to_float(&v) as f32);
    let lower = (f64::from(far_clip) + 528.0) as f32;
    let v = s_str_to_float(value);
    if v < f64::from(lower) || v as f32 > 2112.0 {
        CvarVerdict::refuse(format!(
            "HorizonFarClip must be in range {:.6} - {:.6}.",
            f64::from(lower),
            2112.0
        ))
    } else {
        CvarVerdict::accept()
    }
}

/// The two depth-bias callbacks, `footstepBias`'s (`0x689450`) and `shadowBias`'s (`0x6d8530`):
/// `sscanf(value, "%f")` (`0x73fb5d`) into the argument's own slot, then [`depth_bias_of`]. With
/// no number the slot keeps the string pointer's bits, which a heap address reads as a tiny
/// positive float, so such text is taken here as 0.
fn depth_bias(value: &str, refused: &str, taken: &str) -> CvarVerdict {
    depth_bias_of(scanf_f(value).unwrap_or(0.0), refused, taken)
}

/// The depth-bias test on the scanned float: refused below 0 or unordered, a NaN, at the first
/// compare (`fcomp 0.0; test ah,1; jne`, C0 set), and above 1 at the second (`test ah,0x41; jp`),
/// with `refused`; taken with `taken` and the value through the CRT's `%f` (`0x73fec4`).
fn depth_bias_of(v: f32, refused: &str, taken: &str) -> CvarVerdict {
    if (0.0..=1.0).contains(&v) {
        CvarVerdict::accept_saying(format!("{taken} {:.6}", f64::from(v)))
    } else {
        CvarVerdict::refuse(refused)
    }
}

/// `footstepBias`: "Footstep bias must be in the range (0, 1)" (`0x869f0c`), "Footstep bias set
/// to %f" (`0x869f38`).
pub fn check_footstep_bias(value: &str, _applied: &str, _rows: CvarRows<'_>) -> CvarVerdict {
    depth_bias(
        value,
        "Footstep bias must be in the range (0, 1)",
        "Footstep bias set to",
    )
}

/// `shadowBias`: "Shadow bias must be in the range (0, 1)" (`0x86f650`), "Shadow bias set to %f"
/// (`0x86f678`).
pub fn check_shadow_bias(value: &str, _applied: &str, _rows: CvarRows<'_>) -> CvarVerdict {
    depth_bias(
        value,
        "Shadow bias must be in the range (0, 1)",
        "Shadow bias set to",
    )
}

/// The two LOD distance callbacks never refuse. Out of range they print their line and hand the
/// setter a stand-in inside it, which the setter takes and answers 1 for: `lodDist`'s (`0x688ab0`)
/// takes 50 to 250 (`[0x80fed4]`, `[0x80fed0]`), prints "Lod distance must be in range 50.0f -
/// 250.0f." (`0x8698d0`) and substitutes 77.0 (`0x688af4`) for `0x672680`. So the typed text is
/// stored. With no number in the text, `sscanf` leaves the pointer's bits, read far below 50.
fn lod_distance(value: &str, min: f32, max: f32, line: &str) -> CvarVerdict {
    match scanf_f(value) {
        Some(v) if !(v < min || v > max) => CvarVerdict::accept(),
        _ => CvarVerdict::accept_saying(line),
    }
}

/// `lodDist` ([`lod_distance`]).
pub fn check_lod_dist(value: &str, _applied: &str, _rows: CvarRows<'_>) -> CvarVerdict {
    lod_distance(
        value,
        50.0,
        250.0,
        "Lod distance must be in range 50.0f - 250.0f.",
    )
}

/// `textureLodDist`'s callback (`0x6891f0`), as [`lod_distance`]: 80 to 777 (`[0x80fedc]`,
/// `[0x80fed8]`), "Texure Lod distance must be in range 80.0f - 777.0f." (`0x869d78`, the
/// reference's spelling), and the stand-in 80.0 (`0x689234`) for `0x6726c0`.
pub fn check_texture_lod_dist(value: &str, _applied: &str, _rows: CvarRows<'_>) -> CvarVerdict {
    lod_distance(
        value,
        80.0,
        777.0,
        "Texure Lod distance must be in range 80.0f - 777.0f.",
    )
}

/// The error filter's words and their bits, as `0x403200` sets them: the first letter picks a
/// case through the jump table at `0x403470` (indices `0x403494`, `A` to `w`), whose `SStrCmpI`
/// (`0x64a4c0`) must then match the whole token. `s` and `S` lead to the `objects` compare
/// (`0x4033de`), so no token starting with them matches, and bit `0x40` is never set.
const ERROR_FILTERS: [(&str, u32); 7] = [
    ("general", 0x1),
    ("world", 0x2),
    ("ui", 0x4),
    ("animation", 0x8),
    ("models", 0x10),
    ("objects", 0x20),
    ("movement", 0x80),
];

/// The names `0x403110` prints for a mask, each with its trailing space (`0x82ebc4`…`0x82eb8c`).
const ERROR_FILTER_NAMES: [(u32, &str); 7] = [
    (0x1, "general "),
    (0x2, "world "),
    (0x4, "ui "),
    (0x8, "animation "),
    (0x10, "models "),
    (0x20, "objects "),
    (0x80, "movement "),
];

/// `ErrorFilter`'s callback (`0x4030f0`). Its parser (`0x403200`) reads tokens split at tab,
/// CR, LF, `"` and space (`0x82ecc8`) into 0x40 bytes, from a mask of 0: `all` sets every bit, or
/// none after `except` (`0x403275`); `except` inverts every later word (`0x4032ce`); each filter
/// word sets its bit, or clears it after `except`. An empty value is the empty mask. Any other
/// word refuses with "Unknown filter %s" (`0x82ec70`) and three help lines (`0x403433`). A taken
/// mask prints "Now filtering: all messages" (`0x82ebd0`) when it is every bit, else "Now
/// filtering: %s" of its names (`0x82eb78`).
pub fn check_error_filter(value: &str, _applied: &str, _rows: CvarRows<'_>) -> CvarVerdict {
    let mut mask: u32 = 0;
    let mut except = false;
    let mut rest = value.as_bytes();
    loop {
        let token = super::console_exec::s_str_tokenize(&mut rest, 0x40, b"\t\r\n\" ");
        if token.is_empty() {
            break;
        }
        let token = String::from_utf8_lossy(&token);
        if token.eq_ignore_ascii_case("all") {
            mask = if except { 0 } else { u32::MAX };
        } else if token.eq_ignore_ascii_case("except") {
            except = true;
        } else if let Some((_, bit)) = ERROR_FILTERS
            .iter()
            .find(|(word, _)| token.eq_ignore_ascii_case(word))
        {
            if except {
                mask &= !bit;
            } else {
                mask |= bit;
            }
        } else {
            return CvarVerdict {
                accept: false,
                lines: vec![
                    format!("Unknown filter {token}"),
                    "Filters: general world ui animation models objects all".to_string(),
                    "         use \"except\" to invert mask".to_string(),
                    "         i.e.: all except objects".to_string(),
                ],
            };
        }
    }
    if mask == u32::MAX {
        return CvarVerdict::accept_saying("Now filtering: all messages");
    }
    let names: String = ERROR_FILTER_NAMES
        .iter()
        .filter(|(bit, _)| mask & bit != 0)
        .map(|(_, name)| *name)
        .collect();
    CvarVerdict::accept_saying(format!("Now filtering: {names}"))
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
pub fn check_camera_rate(value: &str, _applied: &str, _rows: CvarRows<'_>) -> CvarVerdict {
    const DEG_PER_RAD: f32 = 57.295_78; // [0x803a78]
    let min = (f64::from(0.001_745_329_2f32) * f64::from(DEG_PER_RAD)) as f32; // [0x8089ec]
    let max = (f64::from(6.283_185_5f32) * f64::from(DEG_PER_RAD)) as f32; // [0x8089e8]
    camera_range(value, min, max)
}

/// `0x50b380`, a saved view's pitch: `[0x8089dc]·[0x803a78]` to `[0x8089d8]·[0x803a78]` as
/// floats, ±89 degrees.
pub fn check_camera_pitch(value: &str, _applied: &str, _rows: CvarRows<'_>) -> CvarVerdict {
    const DEG_PER_RAD: f32 = 57.295_78; // [0x803a78]
    let max = (f64::from(1.553_343_f32) * f64::from(DEG_PER_RAD)) as f32; // [0x8089d8]
    let min = (f64::from(-1.553_343_f32) * f64::from(DEG_PER_RAD)) as f32; // [0x8089dc]
    camera_range(value, min, max)
}

/// `0x50b3c0`, a saved view's yaw: `[0x8089e4]·[0x803a78]` to `[0x8089e0]·[0x803a78]` as floats,
/// 0 to 360 degrees, so a negative offset is refused.
pub fn check_camera_yaw(value: &str, _applied: &str, _rows: CvarRows<'_>) -> CvarVerdict {
    const DEG_PER_RAD: f32 = 57.295_78; // [0x803a78]
    let max = (f64::from(6.283_185_5f32) * f64::from(DEG_PER_RAD)) as f32; // [0x8089e0]
    camera_range(value, 0.0, max)
}

/// `0x50bfe0`, the camera's durations and the bob frequency: `[0x8089fc]` 0.001 to `[0x8089f8]`
/// 300.
pub fn check_camera_time(value: &str, _applied: &str, _rows: CvarRows<'_>) -> CvarVerdict {
    camera_range(value, 0.001, 300.0)
}

/// `0x50c040`, the camera's decay rates (`cameraBobbingSmoothSpeed`, `cameraDistanceMoveSpeed`):
/// `[0x8089f4]` 1/360 to `[0x8089f0]` 50.
pub fn check_camera_decay(value: &str, _applied: &str, _rows: CvarRows<'_>) -> CvarVerdict {
    camera_range(value, 1.0 / 360.0, 50.0)
}

/// `0x50b310`, `cameraDistanceMax`: `[0x8089a8]` 0 to `[0x8089a4]` 50.
pub fn check_camera_distance_max(value: &str, _applied: &str, _rows: CvarRows<'_>) -> CvarVerdict {
    camera_range(value, 0.0, 50.0)
}

/// `0x50c060`, `cameraSmoothStyle` and `cameraSmoothTrackingStyle`: 0 to 3.
pub fn check_camera_smooth_style(value: &str, _applied: &str, _rows: CvarRows<'_>) -> CvarVerdict {
    camera_range(value, 0.0, 3.0)
}

/// `0x50bfc0`, `cameraView`: 0 to 4.
pub fn check_camera_view(value: &str, _applied: &str, _rows: CvarRows<'_>) -> CvarVerdict {
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
    fn no_rows(_: &str) -> Option<String> {
        None
    }

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
            assert!(check(lo, "", &no_rows).accept, "{line}: {lo}");
            assert!(check(hi, "", &no_rows).accept, "{line}: {hi}");
            for bad in ["-1000", "1000", "1e9"] {
                assert_eq!(check(bad, "", &no_rows), CvarVerdict::refuse(line), "{bad}");
            }
        }
        // `SStrToFloat`: no `+`, no leading space, so both read 0.
        assert!(!check_camera_rate("+90", "", &no_rows).accept);
        assert!(check_camera_smooth_style(" 9", "", &no_rows).accept);
        assert!(!check_camera_rate("0.0999", "", &no_rows).accept);
        assert!(
            !check_camera_yaw("-1", "", &no_rows).accept,
            "a negative yaw offset"
        );
        assert!(!check_camera_pitch("89.001", "", &no_rows).accept);
        // The bound is the float nearest 0.1, which is above the parse of "0.1".
        assert!(!check_camera_rate("0.1", "", &no_rows).accept);
        assert!(!check_camera_time("0.001", "", &no_rows).accept);
        assert!(
            !check_camera_time("0", "", &no_rows).accept,
            "the tilt time that panicked the clamp"
        );
    }

    #[test]
    fn the_gx_checks_take_only_their_values() {
        for ok in ["0", "1", "1.9"] {
            assert_eq!(
                check_triple_buffer(ok, "0", &no_rows),
                CvarVerdict::accept_saying("set pending gxRestart"),
                "{ok}"
            );
        }
        for bad in ["2", "-1", "3"] {
            assert_eq!(
                check_triple_buffer(bad, "0", &no_rows),
                CvarVerdict::refuse("TripleBuffer must be 0 or 1"),
                "{bad}"
            );
        }
        for ok in ["60", "75", "200", "85.5"] {
            assert!(check_refresh(ok, "75", &no_rows).accept, "{ok}");
        }
        for bad in ["59", "144", "0", "abc"] {
            assert_eq!(
                check_refresh(bad, "75", &no_rows),
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
            let v = check_farclip(ok, "350", &no_rows);
            assert_eq!(v.accept, ok != "abc177", "{ok}");
        }
        // `SStrToFloat`'s 0 * inf is NaN, which neither of the callback's compares refuses.
        assert!(s_str_to_float("0e5000").is_nan());
        assert!(check_farclip("0e5000", "350", &no_rows).accept);
        for bad in ["176.9", "1000", "-5", "0"] {
            assert_eq!(
                check_farclip(bad, "350", &no_rows),
                CvarVerdict::refuse("FarClip must be in range 177.0 - 777.0."),
                "{bad}"
            );
        }
        for ok in ["1", "16", "256", "48.9"] {
            assert!(check_frill_density(ok, "16", &no_rows).accept, "{ok}");
        }
        for bad in ["0", "257", "-1", "9000", "abc"] {
            assert_eq!(
                check_frill_density(bad, "16", &no_rows),
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
            assert!(check_small_cull(ok, "0.04", &no_rows).accept, "{ok}");
        }
        for bad in ["0.0009", "0", "-1", "2.01", "abc", "", "1e9"] {
            let v = check_small_cull(bad, "0.04", &no_rows);
            assert!(!v.accept, "{bad}");
            assert_eq!(
                v.lines.first().map(String::as_str),
                Some("SmallCull must be in range 0.001 - 2.0.")
            );
        }
    }

    #[test]
    fn shadow_level_refuses_only_above_one() {
        for ok in ["0", "1", "-5", "abc", "1.9"] {
            let v = check_shadow_level(ok, "1", &no_rows);
            assert!(v.accept, "{ok}");
            assert_eq!(
                v.lines.first().map(String::as_str),
                Some("Shadow mip level changed upon restart.")
            );
        }
        for bad in ["2", "10"] {
            assert!(!check_shadow_level(bad, "1", &no_rows).accept, "{bad}");
        }
    }

    #[test]
    fn tex_lod_bias_takes_minus_one_to_one() {
        for ok in ["-1", "0.0", "1", "1.00000001", "+5", " 5"] {
            assert!(check_tex_lod_bias(ok, "0.0", &no_rows).accept, "{ok}");
        }
        for bad in ["-1.00000001", "1.5", "-2", "5"] {
            let v = check_tex_lod_bias(bad, "0.0", &no_rows);
            assert!(!v.accept, "{bad}");
            assert_eq!(
                v.lines.first().map(String::as_str),
                Some("TexLodBias must be in range -1.0 - 1.0.")
            );
        }
    }

    /// Each world callback's range at its bounds and just past them, and its line verbatim.
    #[test]
    fn the_world_callbacks_refuse_outside_their_ranges() {
        for (check, ok, bad, line) in [
            (
                check_max_lights as CvarCheck,
                &["1", "4", "2.9", "3abc"][..],
                &["0", "5", "-1", "abc"][..],
                "MaxLights must be in range 1 - 4.",
            ),
            (
                check_map_obj_light_lod,
                &["0", "2", "abc"],
                &["-1", "3"],
                "MapObjLightLOD must be 0-2",
            ),
            (
                check_water_lod,
                &["0", "abc", "0.9"],
                &["1", "-1"],
                "waterLOD fixed to 0",
            ),
            (
                check_dist_cull,
                &["1", "500", "888.8889"],
                &["0.99", "888.89", "abc"],
                "DistCull must be in range 1.0 - 888.888916.",
            ),
            (
                check_particle_density,
                &["0.25", "1", "1.00000001"],
                &["0.2499", "1.01", "abc"],
                "Value must be between 0.25 and 1.0.",
            ),
            (
                check_unit_draw_dist,
                &["100", "300"],
                &["99.9", "300.01", "abc"],
                "Value must be between 100 and 300.",
            ),
            (
                check_target_nearest_distance,
                &["1", "41", "50"],
                &["0.5", "50.1", "abc"],
                "TargetNearestDistance must be between 1 and 50.000000",
            ),
        ] {
            for v in ok {
                assert_eq!(check(v, "", &no_rows), CvarVerdict::accept(), "{line}: {v}");
            }
            for v in bad {
                assert_eq!(check(v, "", &no_rows), CvarVerdict::refuse(line), "{v}");
            }
        }
    }

    /// The lower bound is the far clip plus 528, stored as a float, and follows the `farclip`
    /// row; the upper is 2112.
    #[test]
    fn horizonfarclip_is_bounded_by_the_far_clip() {
        let at = |far: &'static str| move |n: &str| (n == "farclip").then(|| far.to_string());
        let farclip_350 = at("350");
        assert_eq!(
            check_horizon_farclip("800", "2112", &farclip_350),
            CvarVerdict::refuse("HorizonFarClip must be in range 878.000000 - 2112.000000.")
        );
        assert!(check_horizon_farclip("900", "2112", &farclip_350).accept);
        assert!(check_horizon_farclip("878", "2112", &farclip_350).accept);
        assert!(!check_horizon_farclip("2112.01", "2112", &farclip_350).accept);
        assert!(!check_horizon_farclip("900", "2112", &at("400")).accept);
        assert!(check_horizon_farclip("600", "2112", &no_rows).accept);
    }

    /// `0x689450` and `0x6d8530`: 0 to 1 by `sscanf`, the taken value printed with `%f`.
    #[test]
    fn the_depth_biases_take_zero_to_one_and_say_so() {
        assert_eq!(
            check_footstep_bias(" 0.5", "", &no_rows),
            CvarVerdict::accept_saying("Footstep bias set to 0.500000")
        );
        assert_eq!(
            check_shadow_bias("1", "", &no_rows),
            CvarVerdict::accept_saying("Shadow bias set to 1.000000")
        );
        assert!(check_footstep_bias("0", "", &no_rows).accept);
        // No number: the slot's pointer bits read as a tiny float, taken as 0.
        for text in ["abc", "", "nan"] {
            assert_eq!(
                check_shadow_bias(text, "", &no_rows),
                CvarVerdict::accept_saying("Shadow bias set to 0.000000"),
                "{text:?}"
            );
        }
        assert_eq!(
            depth_bias_of(f32::NAN, "refused", "taken"),
            CvarVerdict::refuse("refused"),
            "a NaN is unordered at the first compare"
        );
        for bad in ["-0.1", "1.5", "+2"] {
            assert_eq!(
                check_footstep_bias(bad, "", &no_rows),
                CvarVerdict::refuse("Footstep bias must be in the range (0, 1)"),
                "{bad}"
            );
            assert_eq!(
                check_shadow_bias(bad, "", &no_rows),
                CvarVerdict::refuse("Shadow bias must be in the range (0, 1)"),
                "{bad}"
            );
        }
    }

    /// `0x688ab0` and `0x6891f0` never refuse: out of range they print their line and stand a
    /// value inside it in for the setter, which answers 1.
    #[test]
    fn the_lod_distances_take_anything_and_say_when_it_is_out_of_range() {
        assert_eq!(check_lod_dist("100", "", &no_rows), CvarVerdict::accept());
        assert_eq!(
            check_lod_dist("9999", "", &no_rows),
            CvarVerdict::accept_saying("Lod distance must be in range 50.0f - 250.0f.")
        );
        assert_eq!(
            check_texture_lod_dist("777", "", &no_rows),
            CvarVerdict::accept()
        );
        assert_eq!(
            check_texture_lod_dist("1", "", &no_rows),
            CvarVerdict::accept_saying("Texure Lod distance must be in range 80.0f - 777.0f.")
        );
    }

    /// `0x403200`'s words, case-blind, with `except` inverting what follows; anything else
    /// refuses with four lines.
    #[test]
    fn the_error_filter_takes_its_words_and_says_what_it_filters() {
        let said = |v: &str| check_error_filter(v, "all", &no_rows);
        assert_eq!(
            said("all"),
            CvarVerdict::accept_saying("Now filtering: all messages")
        );
        assert_eq!(
            said("ALL except objects"),
            CvarVerdict::accept_saying(
                "Now filtering: general world ui animation models movement "
            )
        );
        assert_eq!(
            said("ui\tWorld"),
            CvarVerdict::accept_saying("Now filtering: world ui ")
        );
        assert_eq!(said(""), CvarVerdict::accept_saying("Now filtering: "));
        assert_eq!(
            said("except all"),
            CvarVerdict::accept_saying("Now filtering: ")
        );
        assert_eq!(
            said("all,objects"),
            CvarVerdict {
                accept: false,
                lines: vec![
                    "Unknown filter all,objects".to_string(),
                    "Filters: general world ui animation models objects all".to_string(),
                    "         use \"except\" to invert mask".to_string(),
                    "         i.e.: all except objects".to_string(),
                ],
            },
            "a comma is no delimiter"
        );
        assert!(!said("sound").accept);
    }

    #[test]
    fn base_mip_takes_zero_or_one_and_says_so_when_it_moves() {
        assert_eq!(check_base_mip("0", "0", &no_rows), CvarVerdict::accept());
        assert_eq!(
            check_base_mip("1", "0", &no_rows),
            CvarVerdict::accept_saying("BaseMip level changed to 1")
        );
        assert_eq!(
            check_base_mip("0", "1", &no_rows),
            CvarVerdict::accept_saying("BaseMip level changed to 0")
        );
        for bad in ["2", "-1", "5"] {
            assert_eq!(
                check_base_mip(bad, "0", &no_rows),
                CvarVerdict::refuse("BaseMip must be 0 or 1"),
                "{bad}"
            );
        }
    }
}
