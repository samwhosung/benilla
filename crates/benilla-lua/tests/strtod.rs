//! The client's `strtod` (`0x741e39`) and `_atoflt` (`0x40fecc`) against benilla-re's
//! transcription of them (`crates/crt/src/strtod.rs` at benilla-re 4a83799), which its
//! `crt_strtod_difftest` checks bit for bit against the exe. `strtod.tsv` is that transcription's
//! answer, run with the exe's tables of powers of ten, over the difftest's edge numerals, its fixed
//! cases, its numerals at the doubles' boundaries and 2000 of its random numerals: a line per
//! numeral, `\\` and `\xHH` escaped, then the `double`'s bits, the end index, `ERANGE` and the
//! bits `_atoflt` gives the same text.

use benilla_lua::strtod::{atoflt, strtod};

fn unescape(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match (b[i], b.get(i + 1)) {
            (b'\\', Some(b'x')) => {
                out.push(u8::from_str_radix(&s[i + 2..i + 4], 16).expect("hex escape"));
                i += 4;
            }
            (b'\\', Some(b'\\')) => {
                out.push(b'\\');
                i += 2;
            }
            (c, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

#[test]
fn strtod_and_atoflt_are_the_clients() {
    let table = include_str!("strtod.tsv");
    let mut n = 0;
    for line in table.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        let [text, bits, end, erange, float] = f[..] else {
            panic!("a line of five fields: {line:?}");
        };
        let s = unescape(text);
        let r = strtod(&s);
        let want = u64::from_str_radix(bits, 16).unwrap();
        assert_eq!(r.value.to_bits(), want, "strtod({text:?}) value");
        assert_eq!(r.end, end.parse::<usize>().unwrap(), "strtod({text:?}) end");
        assert_eq!(r.erange, erange == "1", "strtod({text:?}) ERANGE");
        let want = u32::from_str_radix(float, 16).unwrap();
        assert_eq!(atoflt(&s).to_bits(), want, "atoflt({text:?})");
        n += 1;
    }
    assert_eq!(n, 3487);
}

/// The difftest's fixed cases, the exe's answers: a round to the double below the nearest, a
/// result just under and just at 2^-1022, the 12-byte real's flush to a signed zero and the
/// underflows either side of it, and 30 digits read as 24.
#[test]
fn strtod_fixed_cases() {
    let fixed: &[(&str, u64, bool)] = &[
        ("6e23", 0x44df_c384_2bd1_f071, false),
        ("2.2250738585072011e-308", 0, true),
        ("2.2250738585072013e-308", 0x0010_0000_0000_0000, false),
        ("1e-4950", 0, true),
        ("-1e-5000", 0x8000_0000_0000_0000, false),
        ("1e-5201", 0, true),
        ("-1e-400", 0, true),
        (
            "123456789012345678901234567890",
            0x45f8_ee90_ff6c_373e,
            false,
        ),
    ];
    for &(text, bits, erange) in fixed {
        let r = strtod(text.as_bytes());
        assert_eq!(
            (r.value.to_bits(), r.erange),
            (bits, erange),
            "strtod({text:?})"
        );
    }
}

/// `_atoflt` rounds the 12-byte real once, straight to the `float`: `460e8` is 45999996928, where
/// a `double` rounded again to a `float` is 46000001024.
#[test]
fn atoflt_rounds_once() {
    assert_eq!(atoflt(b"460e8"), 45_999_996_928.0);
    assert_ne!(atoflt(b"460e8"), 460e8_f64 as f32);
}
