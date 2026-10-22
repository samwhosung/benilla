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

/// An epoch second broken down in the system's zone, with whether daylight saving is in force:
/// the platform's `localtime`. `None` where it refuses the time.
pub fn local(secs: i64) -> Option<(Civil, bool)> {
    let tm = zone::localtime(secs)?;
    let c = Civil {
        year: i64::from(tm.tm_year) + 1900,
        month: (tm.tm_mon + 1) as u32,
        day: tm.tm_mday as u32,
        hour: tm.tm_hour as u32,
        min: tm.tm_min as u32,
        sec: tm.tm_sec as u32,
        weekday: tm.tm_wday as u32,
        yearday: (tm.tm_yday + 1) as u32,
    };
    Some((c, tm.tm_isdst > 0))
}

/// The epoch second for fields the caller has summed as UTC (`utc`, range-checked as the
/// runtime does), in the system's zone: `_mktime`'s local half, which adds `_timezone`, the
/// zone's standard offset west of UTC as it stands now, refuses a negative time (`localtime`,
/// `0x740825`), then adds `_dstbias` only when `isdst` is set (`0x740811`, `0x740836`). The sum is
/// a 32-bit `time_t`'s, so a wrap past it is negative and refused.
pub fn local_seconds(utc: i32, isdst: bool) -> Option<i32> {
    let (zone, bias) = zone::biases();
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
    zone::name(isdst)
}

/// The platform's half: the C runtime's zone, read afresh (`tzset`) at each call so a change of
/// `TZ` reaches it.
#[cfg(unix)]
mod zone {
    use std::ffi::{c_char, CStr};

    extern "C" {
        fn tzset();
        static tzname: [*const c_char; 2];
    }

    pub(super) fn reread() {
        // SAFETY: `tzset` takes nothing and only rereads the zone.
        unsafe { tzset() };
    }

    /// Deviation: the zone database's offset for the date, where the reference's CRT applies
    /// today's rules, so a date in a zone whose offset has since changed does not round-trip
    /// through `time{}`.
    pub(super) fn localtime(secs: i64) -> Option<libc::tm> {
        reread();
        let time = secs as libc::time_t;
        // SAFETY: `localtime_r` fully writes the out-param and reads nothing from it; zeroed is
        // valid for its integers and its nullable zone-name pointer.
        let mut tm = unsafe { std::mem::zeroed::<libc::tm>() };
        // SAFETY: both pointers are to live locals of the types it takes.
        let got = unsafe { libc::localtime_r(&time, &mut tm) };
        (!got.is_null()).then_some(tm)
    }

    /// `_timezone` and `_dstbias` as the Windows runtime derives them from the zone's rules for
    /// now: the standard offset west of UTC, and what daylight time adds to it (0 for a zone with
    /// none), read off the year's January and July.
    pub(super) fn biases() -> (i32, i32) {
        let year = super::from_unix(super::unix_seconds()).year;
        let offsets = [1, 7].map(|month| {
            let t = super::days_from_civil(year, month, 1) * 86_400;
            localtime(t).map(|tm| (gmtoff(&tm), tm.tm_isdst > 0))
        });
        let east = |dst: bool| offsets.iter().flatten().find(|o| o.1 == dst).map(|o| o.0);
        let standard = east(false)
            .or_else(|| offsets.iter().flatten().map(|o| o.0).min())
            .unwrap_or(0);
        let daylight = east(true).map_or(0, |d| d - standard);
        (-standard as i32, -daylight as i32)
    }

    /// Seconds east of UTC; `long` is 32 bits on some unix targets.
    fn gmtoff(tm: &libc::tm) -> i64 {
        #[allow(clippy::useless_conversion)]
        let off = i64::from(tm.tm_gmtoff);
        off
    }

    pub(super) fn name(isdst: bool) -> Vec<u8> {
        reread();
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
mod zone {
    pub(super) fn reread() {
        // SAFETY: `_tzset` takes nothing and only rereads the zone.
        unsafe { libc::tzset() };
    }

    pub(super) fn localtime(secs: i64) -> Option<libc::tm> {
        reread();
        let time = secs as libc::time_t;
        // SAFETY: zeroed is valid for `tm`'s integers; `localtime_s` writes it whole on success.
        let mut tm = unsafe { std::mem::zeroed::<libc::tm>() };
        (unsafe { libc::localtime_s(&mut tm, &time) } == 0).then_some(tm)
    }

    /// The runtime's own `_timezone` and `_dstbias`.
    pub(super) fn biases() -> (i32, i32) {
        reread();
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
        reread();
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
/// every machine: the lock keeps two zone tests apart, and the drop puts the old `TZ` back.
#[cfg(test)]
pub(crate) struct ZoneGuard {
    old: Option<std::ffi::OsString>,
    _lock: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl ZoneGuard {
    pub(crate) fn set(tz: &str) -> Self {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let old = std::env::var_os("TZ");
        std::env::set_var("TZ", tz);
        reread_zone();
        Self { old, _lock: lock }
    }
}

#[cfg(test)]
impl Drop for ZoneGuard {
    fn drop(&mut self) {
        match self.old.take() {
            Some(v) => std::env::set_var("TZ", v),
            None => std::env::remove_var("TZ"),
        }
        reread_zone();
    }
}

#[cfg(test)]
fn reread_zone() {
    zone::reread();
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
