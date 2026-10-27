//! Broken-down time from an epoch second, the calendar behind `date()` and screenshot names: UTC by
//! arithmetic, and local time through the platform's own runtime, as the reference's `localtime`
//! (`0x73f125`) and `_mktime` (`0x74066b`) read the system's zone.

/// Broken-down time.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Civil {
    pub year: i64,
    /// 1–12.
    pub month: u32,
    /// 1–31.
    pub day: u32,
    pub hour: u32,
    pub min: u32,
    pub sec: u32,
    /// 0 = Sunday.
    pub weekday: u32,
    /// 1–366.
    pub yearday: u32,
}

/// Break an epoch second down into UTC fields, by Howard Hinnant's civil-from-days: years count
/// from month 3, so the leap day falls last and no month-length table is needed.
pub fn from_unix(secs: i64) -> Civil {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (hour, min, sec) = (
        (rem / 3600) as u32,
        ((rem % 3600) / 60) as u32,
        (rem % 60) as u32,
    );
    // 1970-01-01 was a Thursday (4).
    let weekday = (days + 4).rem_euclid(7) as u32;

    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if month <= 2 { y + 1 } else { y };

    let yearday = (days - days_from_civil(year, 1, 1) + 1) as u32;
    Civil {
        year,
        month,
        day,
        hour,
        min,
        sec,
        weekday,
        yearday,
    }
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// An epoch second from 0 to 2^31 broken down in the system's zone, with whether daylight saving
/// is in force: the runtime's `localtime` (`0x73f125`), in the zone as the process first read it.
pub fn local(secs: i64) -> (Civil, bool) {
    let z = zone();
    let i = z
        .spans
        .partition_point(|s| s.start <= secs)
        .saturating_sub(1);
    let span = z.spans[i];
    (from_unix(secs + span.east), span.dst)
}

/// The epoch second for fields the caller has summed as UTC (`utc`, range-checked as the
/// runtime does), in the system's zone: `_mktime`'s local half, which adds `_timezone`, the
/// zone's standard offset west of UTC as it was first read, refuses a negative time (`localtime`,
/// `0x740825`), then adds `_dstbias` only when `isdst` is set (`0x740811`, `0x740836`). The sum is
/// a 32-bit `time_t`'s, so a wrap past it is negative and refused.
///
/// Deviation: a `_dstbias` that takes the time below 0 faults the client, which copies the second
/// `localtime`'s null answer unchecked (`0x740842` → `0x740860`); here it is refused.
pub fn local_seconds(utc: i32, isdst: bool) -> Option<i32> {
    let (zone, bias) = zone().biases;
    let mut t = utc.wrapping_add(zone);
    if t < 0 {
        return None;
    }
    if isdst {
        t = t.wrapping_add(bias);
    }
    (t >= 0).then_some(t)
}

/// The system zone's name for daylight time or standard, `_tzname[isdst]`.
pub fn zone_name(isdst: bool) -> Vec<u8> {
    zone().names[usize::from(isdst)].clone()
}

/// From `start` on, the zone is `east` seconds ahead of UTC, in daylight time or not.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Span {
    start: i64,
    east: i64,
    dst: bool,
}

/// The zone as the process read it, once: the reference's runtime reads it on the first
/// `localtime`, `mktime`, `strftime` `%Z` or `___loctotime_t`, and its latch (`0x883c60`, tested at
/// `0x414568`, set at `0x414580`) stops every later read, so a zone changed while the client runs
/// is not seen. The platform's own `localtime` may reread the zone at every call (macOS's does),
/// so it is asked once, over every day of the 32-bit `time_t` from 0, and each change of offset
/// found to the second.
///
/// Deviation: the zone database's offset for the date, where the reference's CRT applies the
/// rules it read for every date, so a date in a zone whose offset has since changed does not
/// round-trip through `time{}`.
struct Zone {
    spans: Vec<Span>,
    /// `_timezone` and `_dstbias`, in seconds.
    biases: (i32, i32),
    /// `_tzname`.
    names: [Vec<u8>; 2],
}

static ZONE: std::sync::RwLock<Option<std::sync::Arc<Zone>>> = std::sync::RwLock::new(None);

fn zone() -> std::sync::Arc<Zone> {
    if let Some(z) = ZONE.read().unwrap_or_else(|e| e.into_inner()).as_ref() {
        return z.clone();
    }
    let mut held = ZONE.write().unwrap_or_else(|e| e.into_inner());
    held.get_or_insert_with(|| std::sync::Arc::new(Zone::read()))
        .clone()
}

impl Zone {
    fn read() -> Zone {
        platform::tzset();
        let names = [platform::name(false), platform::name(true)];
        let spans = spans_of(
            |t| platform::localtime(t).unwrap_or((0, false)),
            i64::from(i32::MAX),
        );
        let biases = platform::biases(&spans);
        Zone {
            spans,
            biases,
            names,
        }
    }
}

/// The spans of `at`, a second's offset east of UTC and daylight flag, from 0 to `end`: sampled
/// each day, and each change between two samples bisected to its second, again from there while
/// the day holds another.
fn spans_of(at: impl Fn(i64) -> (i64, bool), end: i64) -> Vec<Span> {
    let span = |t: i64| {
        let (east, dst) = at(t);
        Span {
            start: t,
            east,
            dst,
        }
    };
    let same = |a: Span, b: Span| (a.east, a.dst) == (b.east, b.dst);
    let mut spans = vec![Span {
        start: i64::MIN,
        ..span(0)
    }];
    // The last second known to hold the last span's offset.
    let mut lo = 0;
    for t in (86_400..end).step_by(86_400).chain([end]) {
        loop {
            let last = *spans.last().expect("a span");
            if same(span(t), last) {
                break;
            }
            let mut hi = t;
            while hi - lo > 1 {
                let mid = lo + (hi - lo) / 2;
                if same(span(mid), last) {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            spans.push(span(hi));
            lo = hi;
        }
        lo = t;
    }
    spans
}

/// Reads the zone now, on a thread of its own, so the first `date` does not wait for it.
pub fn warm() {
    let _ = std::thread::Builder::new()
        .name("zone".into())
        .spawn(|| drop(zone()));
}

/// The epoch second a broken-down time names, read as UTC.
#[cfg(any(unix, windows))]
fn tm_seconds(tm: &libc::tm) -> i64 {
    days_from_civil(
        i64::from(tm.tm_year) + 1900,
        (tm.tm_mon + 1) as u32,
        tm.tm_mday as u32,
    ) * 86_400
        + i64::from(tm.tm_hour) * 3600
        + i64::from(tm.tm_min) * 60
        + i64::from(tm.tm_sec)
}

/// The platform's half: the C runtime's zone, its `localtime` as an offset east of UTC.
#[cfg(unix)]
mod platform {
    use std::ffi::{c_char, CStr};

    extern "C" {
        #[link_name = "tzset"]
        fn c_tzset();
        static tzname: [*const c_char; 2];
    }

    pub(super) fn tzset() {
        // SAFETY: `tzset` takes nothing and only reads the zone.
        unsafe { c_tzset() };
    }

    pub(super) fn localtime(secs: i64) -> Option<(i64, bool)> {
        let time = secs as libc::time_t;
        // SAFETY: `localtime_r` fully writes the out-param and reads nothing from it; zeroed is
        // valid for its integers and its nullable zone-name pointer.
        let mut tm = unsafe { std::mem::zeroed::<libc::tm>() };
        // SAFETY: both pointers are to live locals of the types it takes.
        let got = unsafe { libc::localtime_r(&time, &mut tm) };
        (!got.is_null()).then(|| (super::tm_seconds(&tm) - secs, tm.tm_isdst > 0))
    }

    /// `_timezone` and `_dstbias` as the Windows runtime derives them from the zone's rules:
    /// the standard offset west of UTC, and what daylight time adds to it (0 for a zone with
    /// none), read off this year's January and July.
    pub(super) fn biases(spans: &[super::Span]) -> (i32, i32) {
        let year = super::from_unix(super::unix_seconds()).year;
        let offsets = [1, 7].map(|month| {
            let t = super::days_from_civil(year, month, 1) * 86_400;
            let i = spans.partition_point(|s| s.start <= t).saturating_sub(1);
            (spans[i].east, spans[i].dst)
        });
        let east = |dst: bool| offsets.iter().find(|o| o.1 == dst).map(|o| o.0);
        let standard = east(false)
            .or_else(|| offsets.iter().map(|o| o.0).min())
            .unwrap_or(0);
        let daylight = east(true).map_or(0, |d| d - standard);
        (-standard as i32, -daylight as i32)
    }

    pub(super) fn name(isdst: bool) -> Vec<u8> {
        // SAFETY: `tzset` filled both names; each is a NUL-terminated string the runtime owns.
        unsafe {
            let names = std::ptr::addr_of!(tzname).read();
            let p = names[usize::from(isdst)];
            if p.is_null() {
                return Vec::new();
            }
            CStr::from_ptr(p).to_bytes().to_vec()
        }
    }
}

/// The platform's half on Windows: the CRT's own zone, the one the reference's statically linked
/// runtime reads (`__tzset`, `0x4145a5`).
#[cfg(windows)]
mod platform {
    pub(super) fn tzset() {
        // SAFETY: `_tzset` takes nothing and only reads the zone.
        unsafe { libc::tzset() };
    }

    pub(super) fn localtime(secs: i64) -> Option<(i64, bool)> {
        let time = secs as libc::time_t;
        // SAFETY: zeroed is valid for `tm`'s integers; `localtime_s` writes it whole on success.
        let mut tm = unsafe { std::mem::zeroed::<libc::tm>() };
        (unsafe { libc::localtime_s(&mut tm, &time) } == 0)
            .then(|| (super::tm_seconds(&tm) - secs, tm.tm_isdst > 0))
    }

    /// The runtime's own `_timezone` and `_dstbias`.
    pub(super) fn biases(_: &[super::Span]) -> (i32, i32) {
        // `long` is 32 bits on Windows.
        let (mut zone, mut bias): (libc::c_long, libc::c_long) = (0, 0);
        // SAFETY: each writes the one long it is handed.
        unsafe {
            libc::get_timezone(&mut zone);
            libc::get_dstbias(&mut bias);
        }
        (zone, bias)
    }

    pub(super) fn name(isdst: bool) -> Vec<u8> {
        let mut buf = [0u8; 64];
        let mut len: libc::size_t = 0;
        // SAFETY: the runtime writes at most `buf.len()` bytes, NUL included, and the length.
        let err = unsafe {
            libc::get_tzname(
                &mut len,
                buf.as_mut_ptr().cast(),
                buf.len(),
                i32::from(isdst),
            )
        };
        if err != 0 {
            return Vec::new();
        }
        buf[..len.saturating_sub(1).min(buf.len())].to_vec()
    }
}

/// Wall-clock seconds since the Unix epoch; a clock set before 1970 reads 0 rather than raising.
pub fn unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Holds the process's time zone at a `TZ` value for a test, so a local-time answer is the same on
/// every machine: the lock keeps two zone tests apart, the zone is read at once (so `tzset` has
/// run for the platform's own `localtime_r` too), and the drop puts the old `TZ` and zone back.
/// The process reads its zone once (`0x414568`), so a test sets it here and never through `TZ`
/// alone. Behind `test-support` for the tests of crates that read local time through [`local`].
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub struct ZoneGuard {
    old: Option<std::ffi::OsString>,
    _lock: std::sync::MutexGuard<'static, ()>,
}

#[cfg(any(test, feature = "test-support"))]
impl ZoneGuard {
    pub fn set(tz: &str) -> Self {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let old = std::env::var_os("TZ");
        std::env::set_var("TZ", tz);
        reread_zone();
        drop(zone());
        Self { old, _lock: lock }
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Drop for ZoneGuard {
    fn drop(&mut self) {
        match self.old.take() {
            Some(v) => std::env::set_var("TZ", v),
            None => std::env::remove_var("TZ"),
        }
        reread_zone();
        drop(zone());
    }
}

/// Drops the zone the process read, so the next use reads the one a test set.
#[cfg(any(test, feature = "test-support"))]
fn reread_zone() {
    *ZONE.write().unwrap_or_else(|e| e.into_inner()) = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The epoch, an ordinary afternoon, a leap day and the eve of 2000.
    #[test]
    fn known_instants_break_down() {
        let cases = [
            (0_i64, 1970, 1, 1, 0, 0, 0, 4, 1),
            (1_787_319_907, 2026, 8, 21, 13, 45, 7, 5, 233),
            (1_709_251_199, 2024, 2, 29, 23, 59, 59, 4, 60),
            (946_598_400, 1999, 12, 31, 0, 0, 0, 5, 365),
        ];
        for (secs, year, month, day, hour, min, sec, weekday, yearday) in cases {
            assert_eq!(
                from_unix(secs),
                Civil {
                    year,
                    month,
                    day,
                    hour,
                    min,
                    sec,
                    weekday,
                    yearday
                },
                "epoch second {secs}"
            );
        }
    }

    /// The zone is read once (`0x414568`): a `TZ` changed after the first local time is not seen,
    /// in the offset, the biases or the names.
    #[test]
    fn the_zone_is_read_once() {
        let _zone = ZoneGuard::set("UTC0");
        assert_eq!(local(0).0.hour, 0);
        std::env::set_var("TZ", "JST-9");
        assert_eq!(local(0), (from_unix(0), false));
        assert_eq!(local_seconds(0, false), Some(0));
        assert_eq!(zone_name(false), b"UTC");
        reread_zone();
        assert_eq!(local(0).0.hour, 9);
        assert_eq!(zone_name(false), b"JST");
    }

    /// A zone with daylight time: each change of offset found to the second.
    #[test]
    fn the_zone_changes_at_its_second() {
        let _zone = ZoneGuard::set("CET-1CEST,M3.5.0,M10.5.0/3");
        // 2024-03-31 01:00 UTC, when 02:00 CET becomes 03:00 CEST.
        let at = 1_711_846_800;
        assert_eq!(local(at - 1).0.hour, 1);
        assert!(!local(at - 1).1);
        assert_eq!(local(at).0.hour, 3);
        assert!(local(at).1);
        // `_timezone` -3600 and `_dstbias` -3600.
        assert_eq!(local_seconds(7200, false), Some(3600));
        assert_eq!(local_seconds(7200, true), Some(0));
        assert_eq!(local_seconds(0, false), None);
    }

    /// Two changes in one sampled day, and one back the next day, each found at its second.
    #[test]
    fn every_change_lands_at_its_second() {
        let at = |t: i64| match t {
            ..100_000 => (0, false),
            100_000..100_500 => (3600, true),
            100_500..200_000 => (7200, true),
            _ => (0, false),
        };
        let starts: Vec<_> = spans_of(at, 1_000_000)
            .iter()
            .map(|s| (s.start, s.east, s.dst))
            .collect();
        assert_eq!(
            starts,
            [
                (i64::MIN, 0, false),
                (100_000, 3600, true),
                (100_500, 7200, true),
                (200_000, 0, false)
            ]
        );
    }

    /// `div_euclid`/`rem_euclid` keep a second before 1970 from wrapping into nonsense.
    #[test]
    fn negative_seconds_stay_civil() {
        assert_eq!(
            from_unix(-1),
            Civil {
                year: 1969,
                month: 12,
                day: 31,
                hour: 23,
                min: 59,
                sec: 59,
                weekday: 3,
                yearday: 365
            }
        );
    }
}
