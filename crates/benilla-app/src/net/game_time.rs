//! The game clock: the one object the reference builds at start-up (`0xce8538`, `gametime/overview`
//! in benilla-re) and the server's four time messages drive. It holds a date and time of day, a
//! rate in game minutes per real second, the fraction of a minute the frame time has built, the
//! minutes still to skip and the day phase's anchor. `GetGameTime` reads its hour and minute, and
//! the sky its minute of day and day phase.
//!
//! Every method that reads the millisecond clock (`0x42c010`) takes it as `tick`, so a test fixes
//! it. The reference's minute and day offsets (`+0x20`, `+0x24`), its count of minutes advanced
//! (`+0x28`) and its per-minute callbacks (`+0x48`) stay out: the offsets are 0 and the table is
//! empty in the live client, and nothing reads the count.

use benilla_protocol::{SessionEvent, WowTime};
use benilla_world::frame_pace::FrameStep;
use benilla_world::lighting::WorldTime;
use benilla_world::schedule::WorldStage;
use bevy::prelude::*;

/// The rate the constructor stores and the lowest the clamp takes, the `f32` 1/60 at `0x80e45c`.
const MIN_SCALE: f32 = f32::from_bits(0x3c88_8889);
/// The highest rate the clamp takes (`0x80e448`).
const MAX_SCALE: f32 = 60.0;
/// The float 0.001 the tick difference is scaled by (`0x801360`).
const MILLISECOND: f32 = f32::from_bits(0x3a83_126f);
/// The float 1/1440 the minute of day is scaled to a fraction by (`0x804480`).
const ONE_OVER_1440: f32 = f32::from_bits(0x3a36_0b61);

/// The game clock (`0xce8538`).
#[derive(Debug, Clone, Copy)]
pub(crate) struct GameTime {
    /// `+0`: minute, hour, weekday, day, month, year, with -1 for a field not set.
    time: WowTime,
    /// `+0x2c`: game minutes per real second.
    scale: f32,
    /// `+0x30`: the part of the next minute the frame time has built.
    fraction: f32,
    /// `+0x34`: minutes of frame time still to throw away, banked by a sync that found the server
    /// behind.
    skip: u32,
    /// `+0x38`: the tick of the last minute step.
    tick: u32,
    /// `+0x3c`: the minute of day after the last step plus the fraction then, the day phase's anchor.
    phase_base: f32,
    /// The day count moon02's phase reads, `year·372 + month·31 + day` of the date last taken from
    /// the wire, stepped with each midnight the clock passes; the reference reads the date's day
    /// number instead (`0x482bea`).
    day_count: i64,
    /// Whether a server has given the clock a time; until then the sky stays at noon.
    live: bool,
}

impl Default for GameTime {
    fn default() -> Self {
        Self::new(0)
    }
}

impl GameTime {
    /// The constructor (`0x643300`): the time unset (`0x642300`), the rate a sixtieth, nothing built
    /// or banked, and the anchor at the tick it runs.
    pub(crate) fn new(tick: u32) -> Self {
        Self {
            time: WowTime::UNSET,
            scale: MIN_SCALE,
            fraction: 0.0,
            skip: 0,
            tick,
            phase_base: 0.0,
            day_count: 0,
            live: false,
        }
    }

    /// The hour `GetGameTime` returns (`0x515ee0`), -1 until the first message or minute step.
    pub(crate) fn hour(&self) -> i32 {
        self.time.hour
    }

    /// The minute `GetGameTime` returns, -1 until the first message or minute step.
    pub(crate) fn minute(&self) -> i32 {
        self.time.minute
    }

    /// The date and time of day as the clock holds them.
    #[cfg(test)]
    pub(crate) fn time(&self) -> WowTime {
        self.time
    }

    /// The rate in game minutes per real second.
    #[cfg(test)]
    pub(crate) fn scale(&self) -> f32 {
        self.scale
    }

    /// The minutes still to skip.
    #[cfg(test)]
    pub(crate) fn skip(&self) -> u32 {
        self.skip
    }

    /// Whether a server has given the clock a time ([`WorldTime::live`]).
    pub(crate) fn live(&self) -> bool {
        self.live
    }

    /// `0x6424c0`: the minute of day the light state is stamped with, hour · 60 + minute, 0 when
    /// either is unset.
    pub(crate) fn minute_of_day(&self) -> u32 {
        minute_of_day(&self.time) as u32
    }

    /// `0x643a70` stored as a float (`0x6c5f77`, `0x482be2`): the time of day as a fraction. The tick
    /// difference since the anchor is unsigned, at the rate, and `x` loses 1440 while it exceeds
    /// it; the intermediate steps are the x87's 53 bits, rounded once to the float on the store.
    pub(crate) fn phase(&self, tick: u32) -> f32 {
        let elapsed = tick.wrapping_sub(self.tick);
        let mut x = f64::from(elapsed) * f64::from(MILLISECOND);
        x *= f64::from(self.scale);
        x += f64::from(self.phase_base);
        while x > 1440.0 {
            x -= 1440.0;
        }
        (x * f64::from(ONE_OVER_1440)) as f32
    }

    /// The day count moon02's phase reads (see the field).
    pub(crate) fn day_count(&self) -> i64 {
        self.day_count
    }

    /// `0x6435a0(dt)`, each frame (`0x6c5d50`): `dt` at the rate builds the fraction of a minute.
    /// Banked skips take their whole minutes of it first, and each whole minute left steps the
    /// clock. The sum is the x87's 53 bits rounded to the float on the store; a NaN rate makes the
    /// fraction NaN for good, and no minute steps after it.
    pub(crate) fn tick_frame(&mut self, dt: f32, tick: u32) {
        let sum = (f64::from(dt) * f64::from(self.scale) + f64::from(self.fraction)) as f32;
        self.fraction = sum;
        if self.skip != 0 && sum >= 1.0 {
            // `__ftol` (`0x40a2b0`), then the lesser of the two as unsigned (`0x6435db`).
            let whole = (sum as i64) as u32;
            let taken = self.skip.min(whole);
            self.skip -= taken;
            self.fraction = (f64::from(sum) - f64::from(taken)) as f32;
        }
        while self.fraction >= 1.0 {
            self.fraction -= 1.0;
            self.advance_minute(tick);
        }
    }

    /// `0x643ad0`: the clock steps one minute. The minute of day becomes `(minute of day + 1) mod
    /// 1440` (an unset hour or minute counting 0, so the first step lands at 00:01), a new day
    /// adds one to the date, and the anchor is the new minute plus the fraction at this tick.
    fn advance_minute(&mut self, tick: u32) {
        let next = (minute_of_day(&self.time) + 1) % 1440;
        // `0x6424e0`.
        self.time.hour = next / 60;
        self.time.minute = next % 60;
        if next == 0 {
            self.step_days(1);
        }
        self.tick = tick;
        self.phase_base = (f64::from(next) + f64::from(self.fraction)) as f32;
    }

    /// `0x6423c0` on the clock's date, and the day count with it.
    fn step_days(&mut self, days: i32) {
        add_days(&mut self.time, days);
        self.day_count += i64::from(days);
    }

    /// The clock takes a time as sent: the copy at `0x6434eb`, `0x6437be`.
    fn take(&mut self, time: WowTime) {
        self.time = time;
        self.day_count =
            i64::from(time.year) * 372 + i64::from(time.month) * 31 + i64::from(time.day);
        self.live = true;
    }

    /// `0x6434d0(time, 1)`, the set: the clock takes the time, steps it back one minute (minute 0
    /// is 59 of the hour before, hour 0 of the day before) and advances one, so the callbacks, the
    /// anchor and the tick restart from it. The result is the sent hour and minute taken modulo a
    /// day, and 00:01 when either is unset (the step back leaves the unset field negative).
    pub(crate) fn set(&mut self, time: WowTime, tick: u32) {
        self.take(time);
        if self.time.minute != 0 {
            self.time.minute -= 1;
        } else {
            self.time.minute = 59;
            if self.time.hour != 0 {
                self.time.hour -= 1;
            } else {
                self.time.hour = 23;
                self.step_days(-1);
            }
        }
        self.advance_minute(tick);
    }

    /// `0x6436c0(time, 0)`, the sync of the clock to a server time. `d` is the minutes the sent time
    /// leads the clock by, modulo a day and signed with the dividend. A lead steps the clock `d`
    /// minutes, then takes the sent time as sent. A tie takes it. A lag takes it, banks `-d` minutes
    /// to skip and steps `-d` minutes, back to the minute the clock showed.
    pub(crate) fn sync(&mut self, time: WowTime, tick: u32) {
        let days = day_number(&time).wrapping_sub(day_number(&self.time));
        let behind = minute_of_day(&self.time);
        let ahead = minute_of_day(&time);
        // `idiv`: the remainder takes the dividend's sign.
        let d = days
            .wrapping_mul(1440)
            .wrapping_sub(behind)
            .wrapping_add(ahead)
            % 1440;
        if d > 0 {
            for _ in 0..d {
                self.advance_minute(tick);
            }
            self.take(time);
        } else {
            self.take(time);
            self.skip += d.unsigned_abs();
            for _ in 0..d.unsigned_abs() {
                self.advance_minute(tick);
            }
        }
    }

    /// `0x643820`: the rate, held to a sixtieth through sixty. A NaN passes both compares and is
    /// stored through `fld`, which sets its quiet bit. Returns the rate the clock held.
    pub(crate) fn set_scale(&mut self, rate: f32) -> f32 {
        let held = self.scale;
        self.scale = if rate > MAX_SCALE {
            MAX_SCALE
        } else if rate.is_nan() {
            f32::from_bits(rate.to_bits() | 0x0040_0000)
        } else if rate >= MIN_SCALE {
            rate
        } else {
            MIN_SCALE
        };
        held
    }

    /// `0x6c5de0` after the read: the rate is stored and `Gamespeed set from %.03f to %.03f`
    /// (`0x86c4ac`) names the rate held and the rate as received, not as clamped.
    pub(crate) fn game_speed_set(&mut self, rate: f32) -> Vec<String> {
        vec![self.set_rate_line(rate)]
    }

    /// `0x6c5e80` after the read: the set (`0x6434d0`), the bridge's `Time set to` line, then the
    /// rate.
    pub(crate) fn login_time(&mut self, time: WowTime, rate: f32, tick: u32) -> Vec<String> {
        let mut lines = self.game_time_set(time, tick);
        lines.push(self.set_rate_line(rate));
        lines
    }

    /// `0x6c6120` after the read: the set, then the time-changed bridge (`0x6c5f50`). The bridge
    /// stamps the light state with the minute of day, the day phase and the day number, commits
    /// the light in full and calls four callbacks that stay empty; here the sky reads the clock
    /// every frame ([`publish_world_time`]), so what is left of it is the line it writes,
    /// `Time set to %s` (`0x86c50c`).
    pub(crate) fn game_time_set(&mut self, time: WowTime, tick: u32) -> Vec<String> {
        self.set(time, tick);
        vec![format!("Time set to {}", format_time(&self.time))]
    }

    /// `0x6c6010` after the read: the sync, and no line of its own (the two it would log go to a
    /// bare `ret`, `0x6dd240`).
    pub(crate) fn game_time_update(&mut self, time: WowTime, tick: u32) {
        self.sync(time, tick);
    }

    fn set_rate_line(&mut self, rate: f32) -> String {
        let held = self.set_scale(rate);
        format!(
            "Gamespeed set from {:.3} to {:.3}",
            f64::from(held),
            f64::from(rate)
        )
    }

    /// The handlers of the four time messages, `None` for any other event; the lines are the
    /// console's, which go to the log.
    pub(crate) fn on_message(&mut self, ev: &SessionEvent, tick: u32) -> Option<Vec<String>> {
        match *ev {
            SessionEvent::TimeSpeed { time, speed } => Some(self.login_time(time, speed, tick)),
            SessionEvent::GameSpeedSet { speed } => Some(self.game_speed_set(speed)),
            SessionEvent::GameTimeUpdate { time } => {
                self.game_time_update(time, tick);
                Some(Vec::new())
            }
            SessionEvent::GameTimeSet { time } => Some(self.game_time_set(time, tick)),
            _ => None,
        }
    }
}

/// `0x6424c0`.
fn minute_of_day(time: &WowTime) -> i32 {
    if time.hour < 0 || time.minute < 0 {
        0
    } else {
        time.hour * 60 + time.minute
    }
}

/// `WowTime::GetDayNumber` (`0x642320`): 0 for a date with the year, month or day unset, else the
/// days from the epoch of the date's midnight. The tm the reference builds takes the year modulo
/// 100, the month as it is and the day plus one, and `mktime` carries a month past eleven or a day
/// past the month's end into the next.
///
/// Deviation: UTC days, where `mktime` reads the host's local time zone, because the app has no
/// time zone source (`benilla_ui::civil` has the same). A zone east of UTC moves every date's number
/// down one together, which no difference between two dates sees, and a daylight-saving change
/// can give two dates one number, which the sync (`0x6436c0`) would see.
fn day_number(time: &WowTime) -> i32 {
    if time.year < 0 || time.month < 0 || time.day < 0 {
        return 0;
    }
    days_from_civil(2000 + time.year % 100, time.month, time.day) as i32
}

/// `0x6423c0(days, 0)`: moves the date by `days` through `mktime` and `localtime`, which write back
/// the year (less 2000), the month, the day (less one) and the weekday of the result. A date with
/// the year, month or day unset stays as it is. The reference adds the days to the date's local
/// midnight plus an hour, and the UTC days of [`day_number`] stand in for it.
fn add_days(time: &mut WowTime, days: i32) {
    if time.year < 0 || time.month < 0 || time.day < 0 {
        return;
    }
    let n = days_from_civil(2000 + time.year, time.month, time.day) + i64::from(days);
    let (year, month, day) = civil_from_days(n);
    time.year = (year - 2000) as i32;
    time.month = month - 1;
    time.day = day - 1;
    // 1970-01-01 was a Thursday; `tm_wday` counts from Sunday.
    time.weekday = (n + 4).rem_euclid(7) as i32;
}

/// Days from 1970-01-01 to the first of a month plus `day` days (`mktime`'s `tm_mday` less one),
/// for a month field that may run past eleven.
fn days_from_civil(year: i32, month: i32, day: i32) -> i64 {
    let year = i64::from(year) + i64::from(month / 12);
    let month = i64::from(month % 12) + 1;
    // Howard Hinnant's `days_from_civil`.
    let y = year - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468 + i64::from(day)
}

/// The year, month (1 to 12) and day of the month (1 to 31) of a day count from 1970-01-01.
fn civil_from_days(days: i64) -> (i64, i32, i32) {
    // Howard Hinnant's `civil_from_days`.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as i32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as i32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// `WowTime::FormatTime` (`0x642c80`): `Not Set` for a time every field of which packs to 0
/// (`0x642c10`), else `%s/%s/%s (%s) %s:%s` of the month plus one, the day plus one, the year plus
/// 2000, the weekday's `Sun` to `Sat` (`Any` when negative), the hour and the minute to two digits.
/// Any other negative field prints `A`.
fn format_time(time: &WowTime) -> String {
    // `0x642ae0` masks each field to its width; the pack is 0 only when every masked field is.
    let masked = [
        (time.minute, 0x3f),
        (time.hour, 0x1f),
        (time.weekday, 0x7),
        (time.day, 0x3f),
        (time.month, 0xf),
        (time.year, 0x1f),
        (time.spare, 0x3),
    ];
    if masked.iter().all(|&(field, mask)| field & mask == 0) {
        return "Not Set".to_string();
    }
    let number = |field: i32, add: i32| match field {
        f if f < 0 => "A".to_string(),
        f => (f + add).to_string(),
    };
    let weekday = match time.weekday {
        w if w < 0 => "Any",
        w => ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"][w as usize % 7],
    };
    let minute = match time.minute {
        m if m < 0 => "A".to_string(),
        m => format!("{m:02}"),
    };
    format!(
        "{}/{}/{} ({weekday}) {}:{minute}",
        number(time.month, 1),
        number(time.day, 1),
        number(time.year, 2000),
        number(time.hour, 0),
    )
}

/// The in-game clock (`SMSG_LOGIN_SETTIMESPEED` and the other time messages), advanced each frame
/// in the world. It is built once and no logout resets it (`0x6c61a0` clears only the handlers).
#[derive(Resource, Default)]
pub(crate) struct ServerTime(pub(crate) GameTime);

/// The clock's per-frame systems. In the net stage, after the drain that may have set the clock:
/// the lighting resolve runs after it and reads this frame's clock. The frame advance runs only
/// in the world: the reference registers it with `ClientInitializeGame` at world entry
/// (`0x401684`) and `ClientDestroyGame` removes it (`0x401f04`), while the clock, built once at
/// start-up, outlives both, so it stands where the world left it.
pub(super) fn register(app: &mut App) {
    app.add_systems(
        Update,
        (
            tick_game_time.in_set(crate::client_state::InWorldGated),
            publish_world_time,
        )
            .chain()
            .after(super::apply_net_updates)
            .in_set(WorldStage::Net),
    );
}

/// The millisecond clock `0x42c010` reads, here the paced frame clock the frame time is taken from.
pub(crate) fn clock_tick(step: &FrameStep) -> u32 {
    step.elapsed.as_millis() as u32
}

/// The client tick handler (`0x401ec0` → `0x6c5d50`): the clock takes the frame time. The reference
/// hands it unclamped (`0x420f70`), as [`FrameStep::delta`] is.
pub(crate) fn tick_game_time(step: Res<FrameStep>, mut server: ResMut<ServerTime>) {
    server
        .0
        .tick_frame(step.delta.as_secs_f32(), clock_tick(&step));
}

/// Publish the session clock into [`WorldTime`] each frame, as `StampLightState` (`0x482bcc`) stamps
/// the light state: the minute of day, the day phase and the day count. With no server time yet,
/// its default (noon).
pub(crate) fn publish_world_time(
    server: Res<ServerTime>,
    step: Res<FrameStep>,
    mut world_time: ResMut<WorldTime>,
) {
    let clock = &server.0;
    *world_time = if clock.live() {
        let phase = clock.phase(clock_tick(&step));
        WorldTime {
            minute: clock.minute_of_day(),
            minute_f: phase * 1440.0,
            day: clock.day_count() as f64 + f64::from(phase),
            live: true,
        }
    } else {
        WorldTime::default()
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A date at `hour:minute`, 2026-10-18 (a Sunday).
    fn sunday(hour: i32, minute: i32) -> WowTime {
        WowTime {
            minute,
            hour,
            weekday: 0,
            day: 17,
            month: 9,
            year: 26,
            spare: 0,
        }
    }

    /// A clock set to `hour:minute` on 2026-10-18 with the fraction at 0, at tick 0.
    fn at(hour: i32, minute: i32) -> GameTime {
        let mut clock = GameTime::new(0);
        clock.set(sunday(hour, minute), 0);
        clock
    }

    fn hm(clock: &GameTime) -> (i32, i32) {
        (clock.hour(), clock.minute())
    }

    /// `0x643300`, `0x642300`: no time, a sixtieth, nothing built; the sky is not the server's.
    #[test]
    fn a_new_clock_is_as_the_constructor_leaves_it() {
        let clock = GameTime::new(7);
        assert_eq!(clock.time(), WowTime::UNSET);
        assert_eq!(hm(&clock), (-1, -1));
        assert_eq!(clock.scale().to_bits(), 0x3c88_8889);
        assert_eq!(
            (clock.skip(), clock.minute_of_day(), clock.live()),
            (0, 0, false)
        );
        // The anchor is the construction tick and the base 0: a minute of frame time at a
        // sixtieth is a minute into the day.
        assert_eq!(clock.phase(7), 0.0);
        assert!((clock.phase(60_007) * 1440.0 - 1.0).abs() < 1e-5);
    }

    /// `0x6434d0`: the sent hour and minute come back; the step back and the step forward cancel.
    #[test]
    fn a_set_lands_on_the_sent_hour_and_minute() {
        assert_eq!(hm(&at(14, 30)), (14, 30));
        assert_eq!(
            hm(&at(14, 0)),
            (14, 0),
            "minute 0 steps back to 59 of the hour before"
        );
        assert_eq!(
            hm(&at(0, 0)),
            (0, 0),
            "hour 0 minute 0 steps back into the day before"
        );
        assert_eq!(hm(&at(23, 59)), (23, 59));
        assert!(at(14, 30).live());
    }

    /// `0x64353f` to `0x64355a`, `0x643afc`: a set at midnight steps the date back (the weekday
    /// with it, from `localtime`) and the advance steps it forward to the sent day.
    #[test]
    fn a_set_at_midnight_keeps_the_sent_date_and_takes_its_weekday_from_the_calendar() {
        let mut clock = GameTime::new(0);
        // Sent with a wrong weekday (Friday) on 2026-10-18.
        clock.set(
            WowTime {
                weekday: 5,
                ..sunday(0, 0)
            },
            0,
        );
        assert_eq!(clock.time(), sunday(0, 0));
        // Not at midnight, the weekday is kept as sent.
        clock.set(
            WowTime {
                weekday: 5,
                ..sunday(0, 5)
            },
            0,
        );
        assert_eq!(clock.time().weekday, 5);
    }

    /// `0x6424c0`, `0x643ad0`: an unset hour or minute counts 0, so the advance after the step
    /// back lands at 00:01; the date is left as sent.
    #[test]
    fn a_set_with_an_unset_hour_or_minute_lands_at_one_past_midnight() {
        for (hour, minute) in [(-1, -1), (-1, 30), (14, -1), (0, -1), (-1, 0)] {
            let mut clock = GameTime::new(0);
            clock.set(
                WowTime {
                    hour,
                    minute,
                    ..sunday(0, 0)
                },
                0,
            );
            assert_eq!(hm(&clock), (0, 1), "hour {hour} minute {minute}");
            assert_eq!(clock.time().day, 17, "the date as sent");
        }
        // The login packet with every field set to ones.
        let mut clock = GameTime::new(0);
        clock.set(WowTime::decode(0xffff_ffff), 0);
        assert_eq!(hm(&clock), (0, 1));
    }

    /// `0x6424c0`, `0x6424e0`: an out-of-range field is taken modulo a day by the advance.
    #[test]
    fn a_set_takes_an_out_of_range_hour_or_minute_modulo_a_day() {
        // Hour 25 minute 10: 25 · 60 + 9 + 1 = 1510, 70 past midnight.
        assert_eq!(hm(&at(25, 10)), (1, 10));
        // Hour 3 minute 62: 180 + 61 + 1 = 242.
        assert_eq!(hm(&at(3, 62)), (4, 2));
        // Hour 30 minute 0 steps back to 29:59 and lands at 30 · 60 mod 1440.
        assert_eq!(hm(&at(30, 0)), (6, 0));
    }

    /// `0x643b1b`: the set anchors the day phase at the new minute plus the fraction held.
    #[test]
    fn a_set_anchors_the_day_phase_at_the_sent_minute() {
        let mut clock = GameTime::new(0);
        clock.set(sunday(14, 30), 5_000);
        // At the tick of the set the phase is the minute of day over 1440.
        assert!((clock.phase(5_000) - 870.0 / 1440.0).abs() < 1e-6);
        // A minute of frame time later, at a sixtieth: one minute on.
        assert!((clock.phase(65_000) * 1440.0 - 871.0).abs() < 1e-3);
        // The fraction a frame built is part of the anchor.
        let mut clock = GameTime::new(0);
        clock.tick_frame(30.0, 0);
        clock.set(sunday(14, 30), 5_000);
        assert!((clock.phase(5_000) * 1440.0 - 870.5).abs() < 1e-3);
    }

    /// `0x643a70`: `x` loses 1440 while it exceeds it.
    #[test]
    fn the_day_phase_wraps_past_midnight() {
        let mut clock = at(23, 59);
        // 1439 + two minutes of frame time at a sixtieth = 1441 → 1.
        assert!((clock.phase(120_000) * 1440.0 - 1.0).abs() < 1e-3);
        // At a rate of 60, one second is 60 minutes: 1439 + 60 - 1440 = 59.
        clock.set_scale(60.0);
        assert!((clock.phase(1_000) * 1440.0 - 59.0).abs() < 1e-3);
    }

    /// `0x6436c0`, a server ahead of the clock: it steps forward to the sent minute and then takes
    /// the sent time, as sent.
    #[test]
    fn a_sync_ahead_steps_forward_and_takes_the_time_as_sent() {
        let mut clock = at(14, 30);
        clock.sync(sunday(14, 35), 9_000);
        assert_eq!((hm(&clock), clock.skip()), ((14, 35), 0));
        assert!(
            (clock.phase(9_000) * 1440.0 - 875.0).abs() < 1e-3,
            "the steps restamped the anchor"
        );
        // Across midnight: 23:50 to 00:10 of the next day is 20 minutes ahead.
        let mut clock = at(23, 50);
        clock.sync(
            WowTime {
                day: 18,
                weekday: 1,
                ..sunday(0, 10)
            },
            0,
        );
        assert_eq!(hm(&clock), (0, 10));
        assert_eq!(clock.time().day, 18);
        // An out-of-range sent time is taken as it is: 25:10 leads 14:30 by 640 minutes.
        let mut clock = at(14, 30);
        clock.sync(sunday(25, 10), 0);
        assert_eq!(hm(&clock), (25, 10));
    }

    /// `0x6436c0`, a server behind: it takes the sent time, banks the lag to skip and steps back to
    /// the minute the clock showed.
    #[test]
    fn a_sync_behind_banks_the_lag_and_shows_the_minute_it_had() {
        let mut clock = at(14, 30);
        clock.sync(sunday(14, 20), 0);
        assert_eq!((hm(&clock), clock.skip()), ((14, 30), 10));
        // A day behind: the clock was at 00:10 on the 18th, the server is at 23:50 on the 17th.
        let mut clock = GameTime::new(0);
        clock.set(sunday(0, 10), 0);
        clock.sync(
            WowTime {
                day: 16,
                weekday: 6,
                ..sunday(23, 50)
            },
            0,
        );
        assert_eq!((hm(&clock), clock.skip()), ((0, 10), 20));
        assert_eq!(
            clock.time().day,
            17,
            "stepped over midnight back to the 18th"
        );
    }

    /// `0x64375a`: a tie sets and banks nothing, and so does a whole number of days ahead.
    #[test]
    fn a_sync_to_the_same_minute_takes_the_time_and_banks_nothing() {
        let mut clock = at(14, 30);
        clock.sync(sunday(14, 30), 9_000);
        assert_eq!((hm(&clock), clock.skip()), ((14, 30), 0));
        // No step, so the anchor stays where the set put it: nine seconds on at a sixtieth.
        assert!((clock.phase(9_000) * 1440.0 - 870.15).abs() < 1e-3);
        // 24 hours ahead is a lead of 1440, which the remainder takes to 0.
        clock.sync(
            WowTime {
                day: 18,
                weekday: 1,
                ..sunday(14, 30)
            },
            9_000,
        );
        assert_eq!(
            (hm(&clock), clock.skip(), clock.time().day),
            ((14, 30), 0, 18)
        );
        assert!((clock.phase(9_000) * 1440.0 - 870.15).abs() < 1e-3);
        // Two days and five minutes ahead leads by 5.
        clock.sync(
            WowTime {
                day: 20,
                weekday: 3,
                ..sunday(14, 35)
            },
            0,
        );
        assert_eq!((hm(&clock), clock.skip()), ((14, 35), 0));
    }

    /// `0x64371d`: a clock never given a date has day number 0, so the first sync is ahead by the
    /// minute of day, whatever the date.
    #[test]
    fn the_first_sync_of_an_unset_clock_lands_on_the_sent_minute() {
        let mut clock = GameTime::new(0);
        clock.sync(sunday(14, 30), 0);
        assert_eq!(
            (hm(&clock), clock.skip(), clock.live()),
            ((14, 30), 0, true)
        );
    }

    /// `0x6435a0`: whole minutes of frame time step the clock, at the rate.
    #[test]
    fn frame_time_builds_minutes_at_the_rate() {
        let mut clock = at(14, 30);
        clock.tick_frame(30.0, 30_000);
        assert_eq!(hm(&clock), (14, 30));
        clock.tick_frame(30.0, 60_000);
        assert_eq!(hm(&clock), (14, 31), "a sixtieth of a minute a second");
        assert!((clock.phase(60_000) * 1440.0 - 871.0).abs() < 1e-3);
        let mut clock = at(14, 30);
        clock.set_scale(2.0);
        clock.tick_frame(1.5, 1_500);
        assert_eq!(
            hm(&clock),
            (14, 33),
            "three minutes in a second and a half at 2"
        );
    }

    /// `0x6435db`: banked minutes are thrown away before they step the clock.
    #[test]
    fn frame_time_pays_the_skip_before_it_steps() {
        let mut clock = at(14, 30);
        clock.sync(sunday(14, 28), 0);
        assert_eq!((hm(&clock), clock.skip()), ((14, 30), 2));
        // Two minutes of frame time: both skipped, the clock holds.
        clock.tick_frame(120.0, 0);
        assert_eq!((hm(&clock), clock.skip()), ((14, 30), 0));
        // With three banked, three minutes of frame time step none; a fourth steps one.
        clock.sync(sunday(14, 27), 0);
        assert_eq!(clock.skip(), 3);
        clock.tick_frame(180.0, 0);
        assert_eq!((hm(&clock), clock.skip()), ((14, 30), 0));
        clock.tick_frame(60.0, 0);
        assert_eq!(hm(&clock), (14, 31));
    }

    /// `0x6435a0` with a NaN rate: the fraction is NaN, both guards fall through, and it stays so.
    #[test]
    fn a_nan_rate_stops_the_clock_for_good() {
        let mut clock = at(14, 30);
        clock.tick_frame(60.0, 60_000);
        assert_eq!(hm(&clock), (14, 31), "the clock runs until the rate is NaN");
        clock.set_scale(f32::NAN);
        clock.tick_frame(10.0, 70_000);
        clock.set_scale(60.0);
        clock.tick_frame(10.0, 80_000);
        assert_eq!(hm(&clock), (14, 31));
    }

    /// `0x643820`: held to a sixtieth through sixty, the old rate returned, a NaN kept.
    #[test]
    fn the_rate_is_held_to_a_sixtieth_to_sixty() {
        let sixtieth = f32::from_bits(0x3c88_8889);
        let rate = |s: f32| {
            let mut clock = GameTime::new(0);
            clock.set_scale(s);
            clock.scale()
        };
        assert_eq!(rate(0.0), sixtieth);
        assert_eq!(rate(-0.0), sixtieth);
        assert_eq!(rate(-5.0), sixtieth);
        assert_eq!(rate(f32::NEG_INFINITY), sixtieth);
        assert_eq!(rate(1.0 / 60.0), sixtieth);
        assert_eq!(rate(0.5), 0.5);
        assert_eq!(rate(60.0), 60.0);
        assert_eq!(rate(60.5), 60.0);
        assert_eq!(rate(f32::INFINITY), 60.0);
        assert!(rate(f32::NAN).is_nan());
        // A signalling NaN is stored quiet.
        assert_eq!(rate(f32::from_bits(0x7f80_0001)).to_bits(), 0x7fc0_0001);
        let mut clock = GameTime::new(0);
        assert_eq!(clock.set_scale(2.0), sixtieth, "the rate held comes back");
        assert_eq!(clock.set_scale(3.0), 2.0);
    }

    /// `0x642320`: 0 for an unset date; days from the epoch otherwise; a month past eleven carries.
    #[test]
    fn the_day_number_counts_days_from_the_epoch() {
        assert_eq!(day_number(&WowTime::UNSET), 0);
        assert_eq!(
            day_number(&WowTime {
                day: -1,
                ..sunday(0, 0)
            }),
            0
        );
        assert_eq!(
            day_number(&WowTime {
                year: 0,
                month: 0,
                day: 0,
                ..sunday(0, 0)
            }),
            10_957
        );
        assert_eq!(day_number(&sunday(0, 0)), 20_744);
        // Month field 12 is January of the next year.
        assert_eq!(
            day_number(&WowTime {
                month: 12,
                day: 0,
                ..sunday(0, 0)
            }),
            day_number(&WowTime {
                year: 27,
                month: 0,
                day: 0,
                ..sunday(0, 0)
            })
        );
    }

    /// `0x6423c0`: the date moves through the calendar and takes its weekday from it.
    #[test]
    fn a_day_step_moves_the_date_through_the_calendar() {
        let mut time = WowTime {
            month: 11,
            day: 30,
            weekday: 0,
            ..sunday(0, 0)
        };
        add_days(&mut time, 1);
        assert_eq!(
            (time.year, time.month, time.day, time.weekday),
            (27, 0, 0, 5),
            "2027-01-01 a Friday"
        );
        add_days(&mut time, -1);
        assert_eq!(
            (time.year, time.month, time.day, time.weekday),
            (26, 11, 30, 4)
        );
        let mut leap = WowTime {
            year: 28,
            month: 1,
            day: 27,
            ..sunday(0, 0)
        };
        add_days(&mut leap, 1);
        assert_eq!(
            (leap.month, leap.day, leap.weekday),
            (1, 28, 2),
            "2028-02-29 a Tuesday"
        );
        // Before 2000 the year field goes to -1 and reads as unset.
        let mut early = WowTime {
            year: 0,
            month: 0,
            day: 0,
            ..sunday(0, 0)
        };
        add_days(&mut early, -1);
        assert_eq!(
            (early.year, early.month, early.day, early.weekday),
            (-1, 11, 30, 5)
        );
        // An unset date does not move.
        let mut unset = WowTime::UNSET;
        add_days(&mut unset, 1);
        assert_eq!(unset, WowTime::UNSET);
    }

    /// The day count moon02 reads keeps counting one a midnight, over a month's end.
    #[test]
    fn the_moon_day_count_steps_one_a_midnight() {
        let mut clock = GameTime::new(0);
        // 2026-10-31 (day 30) at 23:59.
        clock.set(
            WowTime {
                day: 30,
                weekday: 6,
                ..sunday(23, 59)
            },
            0,
        );
        let before = clock.day_count();
        assert_eq!(before, 26 * 372 + 9 * 31 + 30);
        clock.tick_frame(60.0, 60_000);
        assert_eq!(hm(&clock), (0, 0));
        assert_eq!(
            (clock.time().month, clock.time().day),
            (10, 0),
            "the date is 2026-11-01"
        );
        assert_eq!(clock.day_count(), before + 1);
        // A set at midnight does not move it.
        clock.set(
            WowTime {
                day: 30,
                weekday: 6,
                ..sunday(0, 0)
            },
            0,
        );
        assert_eq!(clock.day_count(), 26 * 372 + 9 * 31 + 30);
    }

    /// `0x642c80`.
    #[test]
    fn a_time_is_formatted_as_the_reference_formats_it() {
        assert_eq!(format_time(&sunday(14, 30)), "10/18/2026 (Sun) 14:30");
        assert_eq!(format_time(&sunday(0, 5)), "10/18/2026 (Sun) 0:05");
        assert_eq!(format_time(&WowTime::UNSET), "A/A/A (Any) A:A");
        let zero = WowTime {
            minute: 0,
            hour: 0,
            weekday: 0,
            day: 0,
            month: 0,
            year: 0,
            spare: 0,
        };
        assert_eq!(format_time(&zero), "Not Set");
        assert_eq!(
            format_time(&WowTime { spare: 1, ..zero }),
            "1/1/2000 (Sun) 0:00"
        );
    }

    fn lines(clock: &mut GameTime, ev: SessionEvent) -> Vec<String> {
        clock.on_message(&ev, 0).expect("a time message")
    }

    /// `0x6c5e80`: the set, the bridge's line, then the rate, and the line says the rate as sent.
    #[test]
    fn the_login_packet_sets_the_time_then_the_rate() {
        let mut clock = GameTime::new(0);
        let out = lines(
            &mut clock,
            SessionEvent::TimeSpeed {
                time: WowTime::decode(0xffff_ffff),
                speed: 100.0,
            },
        );
        assert_eq!(hm(&clock), (0, 1), "an unset hour and minute land at 00:01");
        assert_eq!(clock.scale(), 60.0);
        assert_eq!(
            out,
            [
                "Time set to A/A/A (Any) 0:01",
                "Gamespeed set from 0.017 to 100.000"
            ]
        );
        assert!(clock.live());
    }

    /// `0x6c5de0`, `0x6c6120`, `0x6c6010`: a rate alone is not a time; a set prints; a sync is silent.
    #[test]
    fn each_time_message_does_what_its_handler_does() {
        let mut clock = GameTime::new(0);
        let out = lines(&mut clock, SessionEvent::GameSpeedSet { speed: 0.5 });
        assert_eq!(out, ["Gamespeed set from 0.017 to 0.500"]);
        assert_eq!(
            (clock.scale(), clock.live(), hm(&clock)),
            (0.5, false, (-1, -1))
        );

        let out = lines(
            &mut clock,
            SessionEvent::GameTimeSet {
                time: sunday(14, 30),
            },
        );
        assert_eq!(out, ["Time set to 10/18/2026 (Sun) 14:30"]);
        assert_eq!((hm(&clock), clock.live()), ((14, 30), true));

        let out = lines(
            &mut clock,
            SessionEvent::GameTimeUpdate {
                time: sunday(14, 20),
            },
        );
        assert!(out.is_empty());
        assert_eq!((hm(&clock), clock.skip()), ((14, 30), 10));

        assert!(clock.on_message(&SessionEvent::LoggedOut, 0).is_none());
    }

    fn world_time(clock: GameTime, elapsed_ms: u64) -> WorldTime {
        let mut app = App::new();
        app.insert_resource(ServerTime(clock))
            .insert_resource(FrameStep {
                elapsed: std::time::Duration::from_millis(elapsed_ms),
                ..default()
            })
            .init_resource::<WorldTime>()
            .add_systems(Update, publish_world_time);
        app.update();
        *app.world().resource::<WorldTime>()
    }

    /// `StampLightState` (`0x482bcc`): the minute of day, the day phase and the day count; noon
    /// until a server has given the clock a time.
    #[test]
    fn the_sky_reads_the_clock_once_a_server_has_set_it() {
        let idle = world_time(GameTime::new(0), 0);
        assert!(!idle.live);
        assert_eq!((idle.minute, idle.minute_f), (720, 720.0));

        let mut clock = GameTime::new(0);
        clock.set(sunday(14, 30), 0);
        let t = world_time(clock, 0);
        assert!(t.live);
        assert_eq!(t.minute, 870);
        assert!((t.minute_f - 870.0).abs() < 1e-3);
        let serial = (26 * 372 + 9 * 31 + 17) as f64;
        assert!((t.day - (serial + 870.0 / 1440.0)).abs() < 1e-6);
    }

    /// The clock and its two systems under the real lifecycle gate, the screen set by `start`.
    fn world_app(start: crate::client_state::ClientState, clock: GameTime) -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .add_plugins(crate::client_state::ClientStatePlugin { start })
            .init_resource::<benilla_world::schedule::WorldLive>()
            .init_resource::<FrameStep>()
            .init_resource::<WorldTime>()
            .insert_resource(ServerTime(clock));
        register(&mut app);
        app
    }

    /// One frame of `secs` of frame time.
    fn frame(app: &mut App, secs: u64) {
        let delta = std::time::Duration::from_secs(secs);
        let mut step = app.world_mut().resource_mut::<FrameStep>();
        step.delta = delta;
        step.elapsed += delta;
        app.update();
    }

    fn held(app: &App) -> GameTime {
        app.world().resource::<ServerTime>().0
    }

    /// `0x401684`, `0x401f04`: `ClientInitializeGame` registers the frame advance (`0x401ec0`) at
    /// world entry and `ClientDestroyGame` removes it, so a clock at the glue does not advance and a
    /// banked skip is not paid there.
    #[test]
    fn the_clock_does_not_advance_at_the_glue() {
        use crate::client_state::ClientState;
        // Never given a time: `GetGameTime` keeps answering -1, -1 past a minute of frame time.
        let mut app = world_app(ClientState::Login, GameTime::new(0));
        frame(&mut app, 180);
        assert_eq!(hm(&held(&app)), (-1, -1));
        // Given one, with ten minutes banked to skip: nothing moves at either glue screen.
        let mut clock = at(14, 30);
        clock.sync(sunday(14, 20), 0);
        let mut app = world_app(ClientState::CharSelect, clock);
        frame(&mut app, 120);
        let c = held(&app);
        assert_eq!((hm(&c), c.skip()), ((14, 30), 10));
    }

    /// The reference's object outlives the world and only its tick is unregistered, so a logout
    /// leaves the clock frozen at the value it had.
    #[test]
    fn the_clock_runs_in_the_world_and_holds_after_a_logout() {
        use crate::client_state::ClientState;
        let mut app = world_app(ClientState::InWorld, at(14, 30));
        frame(&mut app, 60);
        assert_eq!(
            hm(&held(&app)),
            (14, 31),
            "a minute of frame time at a sixtieth"
        );
        app.world_mut()
            .resource_mut::<NextState<ClientState>>()
            .set(ClientState::CharSelect);
        frame(&mut app, 600);
        frame(&mut app, 600);
        assert_eq!(
            hm(&held(&app)),
            (14, 31),
            "frozen at the glue after the logout"
        );
        // The next world entry takes the frame time from there, with no catch-up.
        app.world_mut()
            .resource_mut::<NextState<ClientState>>()
            .set(ClientState::InWorld);
        frame(&mut app, 60);
        assert_eq!(hm(&held(&app)), (14, 32));
    }

    /// `0x401ec0`: each frame builds the clock's fraction from the frame time.
    #[test]
    fn the_frame_system_steps_the_clock_by_the_frame_time() {
        let mut clock = GameTime::new(0);
        clock.set(sunday(14, 30), 0);
        clock.set_scale(60.0);
        let mut app = App::new();
        app.insert_resource(ServerTime(clock))
            .insert_resource(FrameStep {
                delta: std::time::Duration::from_secs(2),
                elapsed: std::time::Duration::from_secs(2),
                ..default()
            })
            .add_systems(Update, tick_game_time);
        app.update();
        let clock = app.world().resource::<ServerTime>().0;
        assert_eq!(hm(&clock), (16, 30), "two seconds at 60 is two hours");
    }
}
