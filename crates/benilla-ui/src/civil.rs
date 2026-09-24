//! **The tree's one civil-time conversion** — broken-down UTC from an epoch second.
//!
//! It lives here because this crate already had to have one: 1.12's `date()` global is Lua 5.0's
//! `os.date` hoisted, and the sandbox strips `os` ([`crate::script`]'s stdlib), so the calendar
//! had to be written by hand. The screenshot writer is its second caller, and one
//! correct closed form with two callers beats two hand-rolled loops.
//!
//! **UTC, deliberately, and it is a stated divergence wherever a caller wanted local time.** The
//! workspace has no date dependency at all — every format crate in it is in-repo — and resolving a
//! local offset needs a timezone source, not a different algorithm. `date()` records the same
//! divergence for the same reason.

/// Broken-down UTC time.
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

/// Break an epoch second down into UTC fields.
///
/// The civil-from-days conversion is Howard Hinnant's, shifted to a March-based year so the leap
/// day lands at the end and no month-length table is needed. Chosen over a hand-rolled loop
/// because it is a known-correct closed form with no accumulation error, which matters for the
/// "persisted last session, compared this session" use every corpus `date()` caller has.
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

    // Day of the year, from the same conversion applied to Jan 1.
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

/// Days since the epoch for a civil date — the inverse of the above, used only for `yearday`.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Wall-clock seconds since the Unix epoch. Before the epoch is not a state a game client is in; a
/// clock that somehow reports it clamps to 0 rather than raising inside an addon's file scope.
pub fn unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Four instants that between them cover the epoch itself, a leap day, a century boundary and
    /// an ordinary afternoon — the cases a closed-form conversion gets wrong when it is wrong.
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

    /// Before 1970 is not a state a client is in, but the arithmetic must not wrap into nonsense
    /// if a clock reports it — `div_euclid`/`rem_euclid` are load-bearing here, not stylistic.
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
