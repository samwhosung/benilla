//! The probe shield: a probe body cannot die, and does not need GM mode to stay alive.
//!
//! GM mode (`Player::SetGameMaster`) re-templates the player to faction 35 ("Friendly", enemy
//! mask 0) and freezes the mirror timers, so nothing hostility-, damage- or breath-related can be
//! measured with it on. `.cheat god` (`Player::SetCheatGod` → `SetInvincibilityHpThreshold(1)`)
//! instead makes `Unit::DealDamage` stop at 1 hp, environmental damage included, and changes
//! nothing a client can see. The shield is always armed; GM mode also stays on by default, since
//! the shield does not stop aggro, and `WOW_GM=off` drops it safely when a run needs faithful
//! factions. A shielded body with GM off in a hostile camp survives pinned at 1 hp.
//!
//! - Not persisted: the threshold is a runtime `Unit` member, so it is re-sent on every world
//!   entry, and a body parked somewhere hostile is unshielded for the few hundred ms before the
//!   first line goes out. GM mode on by default keeps it from being whittled down there.
//! - Target-sensitive: `.cheat god on` acts on the current selection and answers "Player not
//!   found!" for a creature, so the command always names the character.
//! - `.die` clears it silently (`HandleDieHelper` calls `SetCheatGod(false)`), so a death re-arms
//!   it.
//!
//! Only a probe account (`probe<N>`) is ever sent anything. `WOW_GM=off` (or `gm:off` in
//! `WOW_RIG`) drops GM mode; `WOW_GOD=off` runs unshielded, reported on the preflight banner.

use bevy::prelude::*;

use crate::login::LoginIntent;
use crate::names::NameCache;
use crate::net::{EnteredWorldMessage, ObjectStore, SelfGuid, SelfPlayer, ServerSaidMessage};

/// `PLAYER_FLAGS_GM`: the body carries the faction-35 re-template.
const PLAYER_FLAGS_GM: u32 = 0x0000_0008;

/// Spacing between the shield's lines: two server-side flips inside one net drain merge to a
/// no-op, and `.gm off` must land after the shield is up.
const STEP_SECS: f32 = 0.8;

/// How long to wait for our own name, which makes the command selection-proof, before arming
/// without it.
const NAME_WAIT_SECS: f32 = 3.0;

/// How long the server gets to confirm before the body is reported unshielded; the round trip is
/// ~100-150 ms.
const CONFIRM_SECS: f32 = 5.0;

pub(crate) struct ProbeShieldPlugin;

impl Plugin for ProbeShieldPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ProbeShield>().add_systems(
            Update,
            drive_shield.after(benilla_world::schedule::WorldStage::Net),
        );
    }
}

/// What the preflight banner says about the shield.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum ShieldReport {
    /// Not a probe account: the shield never sends anything.
    #[default]
    NotOurs,
    /// `WOW_GOD=off`: deliberately unshielded. This body CAN die.
    Disabled,
    /// The command is out, the server has not answered yet.
    Arming,
    /// The server confirmed it; this body cannot die.
    Armed,
    /// The server refused or did not confirm inside [`CONFIRM_SECS`]; the body is mortal.
    Unconfirmed,
}

/// The shield's state for this session.
#[derive(Resource)]
pub(crate) struct ProbeShield {
    report: ShieldReport,
    /// Queued lines and when the next one may go out.
    steps: Vec<String>,
    next_at: f32,
    /// `Time::elapsed_secs` after which an unanswered `.cheat god on` is called unconfirmed.
    confirm_by: f32,
    /// Set at world entry, cleared once the batch is built.
    arm_wanted: Option<f32>,
    /// Our own death state last frame, so a death edge differs from arriving dead; `None` until
    /// the first descriptor read of this world entry.
    was_dead: Option<bool>,
}

impl Default for ProbeShield {
    fn default() -> Self {
        Self {
            report: ShieldReport::NotOurs, // decided once we know whose account this is
            steps: Vec::new(),
            next_at: 0.0,
            confirm_by: 0.0,
            arm_wanted: None,
            was_dead: None,
        }
    }
}

impl ProbeShield {
    /// What the preflight banner should say.
    pub(crate) fn report(&self) -> ShieldReport {
        self.report
    }
}

/// Whether `user` is a probe account, `probe` followed by digits: the identity an unattended run
/// logs in with (`docs/CONTRIBUTING.md`, "Running it unattended"). No other account is touched.
fn is_probe_account(user: &str) -> bool {
    // Byte-wise: a `&str` slice at 5 would panic mid-char on non-ASCII input.
    let Some((prefix, digits)) = user.as_bytes().split_at_checked(5) else {
        return false;
    };
    prefix.eq_ignore_ascii_case(b"probe")
        && !digits.is_empty()
        && digits.iter().all(u8::is_ascii_digit)
}

/// Whether this run wants GM mode off: `WOW_GM=off`, or `gm:off` in `WOW_RIG`, so the shield
/// never sends `.gm` against the rig's own step. The preflight banner reads it too.
pub(crate) fn wants_gm_off() -> bool {
    std::env::var("WOW_GM").is_ok_and(|v| v.eq_ignore_ascii_case("off"))
        || std::env::var("WOW_RIG").is_ok_and(|spec| rig_asks_for_gm_off(&spec))
}

/// Whether a `WOW_RIG` spec carries `gm:off`, matched as [`crate::capture::probe_rig`] matches
/// it: case-insensitive key, anything but `on`/`1` false.
fn rig_asks_for_gm_off(spec: &str) -> bool {
    spec.split_whitespace().any(|tok| {
        matches!(
            tok.to_ascii_lowercase().split_once(':'),
            Some(("gm", v)) if !matches!(v, "on" | "1")
        )
    })
}

/// Whether `WOW_GOD=off` asked for an unshielded run, read once.
fn disabled_by_env() -> bool {
    static OFF: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| {
        std::env::var("WOW_GOD").is_ok_and(|v| v.eq_ignore_ascii_case("off"))
    });
    *OFF
}

/// Classify one `CHAT_MSG_SYSTEM` line as an answer about god mode. The chat strings are
/// `mangos_string` 368/369 (`LANG_YOU_SET_GOD`, `LANG_YOUR_GOD_SET`); `LANG_CHEAT_GOD_ON/OFF`
/// (349/350) arrive as `SMSG_NOTIFICATION` and never reach here.
fn god_verdict(text: &str) -> Option<bool> {
    let lower = text.to_ascii_lowercase();
    if !lower.contains("god mode") {
        return None;
    }
    // "You set god mode to on for X." / "Your god mode has been turned off by X."
    if lower.contains(" to on") || lower.contains("turned on") {
        Some(true)
    } else if lower.contains(" to off") || lower.contains("turned off") {
        Some(false)
    } else {
        None
    }
}

/// Arm the shield on every world entry, confirm it against the server's own answer, and put it
/// back up whenever something drops it.
fn drive_shield(
    mut shield: ResMut<ProbeShield>,
    mut entered: MessageReader<EnteredWorldMessage>,
    mut said: MessageReader<ServerSaidMessage>,
    time: Res<Time>,
    intent: Res<LoginIntent>,
    names: Res<NameCache>,
    self_guid: Res<SelfGuid>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
) {
    let entered_world = entered.read().next().is_some();
    let ours = intent.account().is_some_and(is_probe_account);
    if !ours || disabled_by_env() {
        // Still drain, so a later frame never sees a stale backlog.
        said.read().for_each(drop);
        shield.report = if ours {
            ShieldReport::Disabled
        } else {
            ShieldReport::NotOurs
        };
        if entered_world && ours {
            warn!(
                "probe-shield: DISABLED by WOW_GOD=off — this probe character CAN die. That is the \
                 right setting for a death-arc test; unset it for everything else."
            );
        }
        return;
    }

    if entered_world {
        shield.arm_wanted = Some(time.elapsed_secs());
        shield.was_dead = None;
        shield.report = ShieldReport::Arming;
    }

    // The server's answer is the only evidence: the flag rides no descriptor field.
    for msg in said.read() {
        match god_verdict(&msg.text) {
            Some(true) => {
                if shield.report != ShieldReport::Armed {
                    info!(
                        "probe-shield: ARMED — this character cannot die (vmangos `.cheat god`: \
                         damage clamps at 1 hp instead of killing). Hostility, factions, aggro and \
                         environmental damage all stay faithful, unlike GM mode. It is NOT \
                         persisted, so it is re-armed on every world entry."
                    );
                }
                shield.report = ShieldReport::Armed;
            }
            Some(false) => {
                warn!(
                    "probe-shield: the shield was turned OFF — putting it back up. If you meant to \
                     run unshielded, use WOW_GOD=off (which also stops this re-arm) rather than \
                     `.cheat god off`."
                );
                rearm(&mut shield, &time, &names, &self_guid);
            }
            None => {}
        }
    }

    // A death proves the shield was down; `.die` clears it silently before killing.
    if let Ok(store) = self_q.single() {
        let dead = store.0.unit_is_dead() || store.0.player_is_ghost();
        match shield.was_dead {
            None => shield.was_dead = Some(dead), // first read of this entry: a state, not an edge
            Some(false) if dead => {
                shield.was_dead = Some(true);
                warn!(
                    "probe-shield: THE CHARACTER DIED — so the shield was down. `.die` clears it \
                     silently by design; anything else means it never landed. Re-arming now (it \
                     does not revive: send `.revive`)."
                );
                rearm(&mut shield, &time, &names, &self_guid);
            }
            Some(_) => shield.was_dead = Some(dead),
        }
    }

    // Build the batch once the body, and ideally our own name, is there to address.
    if let Some(wanted_at) = shield.arm_wanted {
        let Ok(store) = self_q.single() else { return };
        let name = self_guid.0.and_then(|g| names.peek(g));
        let waited = time.elapsed_secs() - wanted_at;
        if name.is_none() && waited < NAME_WAIT_SECS {
            return;
        }
        if name.is_none() {
            warn!(
                "probe-shield: our own name has not resolved after {NAME_WAIT_SECS:.0}s — arming \
                 without it. `.cheat god` re-targets to the current selection, so this can be \
                 refused with \"Player not found!\" if anything is targeted."
            );
        }
        shield.arm_wanted = None;
        shield.steps.push(god_line(name));
        shield.confirm_by = time.elapsed_secs() + CONFIRM_SECS;
        let gm_is_on = store.0.player_flags() & PLAYER_FLAGS_GM != 0;
        match (gm_is_on, wants_gm_off()) {
            // The default: GM mode stays on, so nothing aggros a parked body.
            (false, false) => {
                info!(
                    "probe-shield: GM mode is off — turning it back ON, so a parked body is not \
                     permanently mobbed. Readings taken now are faction-35 readings and are wrong \
                     for anything about hostility, colour, threat, aggro, damage or timers \
                     — set WOW_GM=off for those, which is safe: the shield holds."
                );
                shield.steps.push(".gm on".into());
            }
            (true, true) => {
                info!(
                    "probe-shield: WOW_GM=off — turning GM mode OFF so factions, hostility and \
                     damage are faithful. Safe: the shield is what keeps this body alive now."
                );
                shield.steps.push(".gm off".into());
            }
            _ => {} // already as asked for
        }
        shield.next_at = time.elapsed_secs();
    }

    // Send the queued lines, one per `STEP_SECS`, god first.
    if !shield.steps.is_empty() {
        let Some(mut script) = script else { return };
        while !shield.steps.is_empty() && time.elapsed_secs() >= shield.next_at {
            let line = shield.steps.remove(0);
            debug!("probe-shield: sending {line:?}");
            script.push_chat_input(line);
            shield.next_at = time.elapsed_secs() + STEP_SECS;
        }
        return;
    }

    // Nothing outstanding: a `.cheat god on` that was never answered did not land.
    if shield.report == ShieldReport::Arming && time.elapsed_secs() > shield.confirm_by {
        shield.report = ShieldReport::Unconfirmed;
        warn!(
            "probe-shield: NOT CONFIRMED — `.cheat god on` went out and the server never answered \
             it. Treat this character as mortal. Check the account's GM level (`.cheat` needs \
             SEC_GAMEMASTER 3; probe accounts are 6): \
             SELECT gmlevel FROM realmd.account_access WHERE id = <account>."
        );
    }
}

/// The command, naming the character when known: the bare form acts on the current selection.
fn god_line(name: Option<&str>) -> String {
    match name {
        Some(n) => format!(".cheat god on {n}"),
        None => ".cheat god on".into(),
    }
}

/// Queue another `.cheat god on` and re-open the confirmation window.
fn rearm(shield: &mut ProbeShield, time: &Time, names: &NameCache, self_guid: &SelfGuid) {
    let name = self_guid.0.and_then(|g| names.peek(g));
    shield.steps.push(god_line(name));
    shield.next_at = time.elapsed_secs();
    shield.confirm_by = time.elapsed_secs() + CONFIRM_SECS;
    shield.report = ShieldReport::Arming;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_probe_accounts_are_ours() {
        for user in ["probe0", "probe7", "PROBE12"] {
            assert!(is_probe_account(user), "{user} is a probe account");
        }
        for user in ["one", "two", "probe", "probeone", "aprobe1", "probe1x"] {
            assert!(!is_probe_account(user), "{user} is NOT a probe account");
        }
    }

    #[test]
    fn the_servers_own_words_are_what_confirm_it() {
        // vmangos 368, the answer to `.cheat god on <name>`.
        assert_eq!(
            god_verdict("You set god mode to on for |cffffffff|Hplayer:Probetwo|h[Probetwo]|h|r."),
            Some(true)
        );
        assert_eq!(
            god_verdict("You set god mode to off for [Probetwo]."),
            Some(false)
        );
        // vmangos 369, when another GM does it to us.
        assert_eq!(
            god_verdict("Your god mode has been turned off by [Someone]."),
            Some(false)
        );
        for other in [
            "Player not found!",
            "GM mode is OFF",
            "There is no such command",
            "Welcome to World of Warcraft!",
        ] {
            assert_eq!(god_verdict(other), None, "{other}");
        }
    }

    #[test]
    fn the_rigs_own_gm_token_wins_over_the_default() {
        assert!(rig_asks_for_gm_off("gnome mage 39 gm:off"));
        assert!(rig_asks_for_gm_off("GM:0"));
        for spec in [
            "tauren druid 60 gm:on at:ThunderBluff",
            "GM:1",
            "60 gear:dps-preraid-bis",
            "",
        ] {
            assert!(!rig_asks_for_gm_off(spec), "{spec:?}");
        }
    }

    #[test]
    fn the_command_always_names_its_target() {
        assert_eq!(god_line(Some("Probetwo")), ".cheat god on Probetwo");
        assert_eq!(god_line(None), ".cheat god on");
    }
}
