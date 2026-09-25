//! The probe shield (decision 0677) — **a probe body cannot die, and does not need GM mode to stay
//! alive.**
//!
//! ## The failure it removes
//!
//! Probe characters kept dying, and the reason was structural: the only protection we had was **GM
//! mode**, and GM mode is exactly what a session has to switch off to measure anything real.
//! `Player::SetGameMaster(true)` re-templates the player to faction 35 ("Friendly", enemy mask 0)
//! and freezes the mirror timers, so with it on nothing is hostile, nothing aggros, environmental
//! damage is skipped and breath/fatigue never tick (0649, 0657). Every session doing hostility,
//! reaction-colour, nameplate, threat, damage or drowning work therefore sends `.gm off` — and the
//! moment it does, the body is a level-1 standing wherever the last session parked it. Measured on
//! this project's own transcripts: `.gm off` **257×**, `.revive` **121×** (0651). Measured live
//! while writing this: `.gm off` on a probe parked in Valley of Spears killed it in **~2 seconds**,
//! inside the gap between two probe-chat lines.
//!
//! Telling sessions to "put GM mode back when you are done" was the old advice and it cannot work:
//! it asks every session to remember a manual step at the exact moment it is thinking about
//! something else, and it trades one silent wrongness (a dead probe) for another (faction 35).
//!
//! ## What it does instead
//!
//! vmangos has a second, *invisible* protection: **`.cheat god`**
//! (`Player::SetCheatGod` → `SetInvincibilityHpThreshold(1)`). `Unit::DealDamage` then clamps every
//! hit at 1 hp instead of calling `Kill` — and `Player::EnvironmentalDamage` (fall, lava, slime,
//! drowning, fatigue) routes through the same `DealDamage`, so it is covered too. Unlike GM mode it
//! touches **nothing** a client can see: no `UNIT_FIELD_FLAGS` bit, no `PLAYER_FLAGS` bit, no
//! faction re-template, no frozen timers. Damage still lands, combat still happens, hostility is
//! still faithful — the body simply cannot reach 0.
//!
//! **The shield is therefore always on** — and, since it does not stop *aggro*, GM mode **stays on
//! by default too** (decision 0679). The two do different jobs: GM mode stops the town attacking a
//! parked body, the shield stops anything killing it. Without the shield, `.gm off` was a two-second
//! death sentence; with it, `.gm off` (`WOW_GM=off`) is a free, safe switch a session flips whenever
//! it needs faithful factions. That — making the drop safe, not making it the default — is the whole
//! win. A shielded body with GM off in a hostile camp survives, but survives *pinned*: 1 hp, stunned,
//! permanently in combat, never regenerating, which is its own kind of useless.
//!
//! ## The three things about it that are not obvious
//!
//! 1. **It is not persisted**, and that leaves a window nothing here can close.
//!    `m_invincibilityHpThreshold` is a runtime `Unit` member, zeroed in the constructor and never
//!    saved, so it must be re-sent on **every world entry** — which is why this is a client module
//!    and not a one-off DB fix. But a command cannot be sent before we are *in* the world, so a body
//!    parked somewhere hostile is unshielded for the few hundred ms it takes to get the first line
//!    out. MEASURED: a probe left at 1 hp in a centaur camp entered the world at `29.41`, had the
//!    shield confirmed at `29.75` (+337 ms) and was already **dead** when the banner printed at
//!    `29.97`. This is the "it was alive at 1 hp when I closed the window, it was dead next login"
//!    report, and it is why **GM mode stays on by default** (0679): the fix is to never let the body
//!    be whittled down and mobbed in the first place, not to arm faster.
//! 2. **It is target-sensitive.** `.cheat god on` re-targets to the current selection
//!    (`ChatHandler::GetSelectedPlayer` → `sObjectMgr.GetPlayer(guid)`), which is `nullptr` for a
//!    creature — the server answers *"Player not found!"* and the body is left mortal. VERIFIED
//!    live: with a centaur Tab-selected the bare form was refused, and the same command with the
//!    name appended succeeded before and after. So we **always name the character**.
//! 3. **`.die` clears it silently.** `HandleDieHelper` calls `SetCheatGod(false)` with
//!    `notify = false` before killing, so deliberate death-testing still works out of the box —
//!    but the shield is down afterwards and nothing says so. Hence the death watch below.
//!
//! ## What it never touches
//!
//! Only a body on a **probe account** (`probe<N>`). The director's own account is never given a
//! command it did not ask for, whether they typed the login or an env var did.
//!
//! ## Knobs
//!
//! - `WOW_GM=off` — drop GM mode, for anything about hostility, reaction colour, nameplates,
//!   threat, aggro, damage or breath/fatigue. Safe now, which is the point. `WOW_RIG="… gm:off"` is
//!   honoured the same way, so the rig's explicit token wins over the default.
//! - `WOW_GOD=off` — run unshielded (a death-arc test that wants a real corpse). Reported as a
//!   `WARN` finding on the preflight banner, because the body can now die.

use bevy::prelude::*;

use crate::login::LoginIntent;
use crate::names::NameCache;
use crate::net::{EnteredWorldMessage, ObjectStore, SelfGuid, SelfPlayer, ServerSaidMessage};

/// `PLAYER_FLAGS_GM` — the bit that says the body is carrying the faction-35 re-template.
const PLAYER_FLAGS_GM: u32 = 0x0000_0008;

/// Spacing between the shield's lines. Two server-side flips inside one net drain merge to a no-op
/// (0441's lesson, which the rig also spaces for), and `.gm off` must land *after* the shield is up
/// — never before, or the body is naked for exactly as long as the gap.
const STEP_SECS: f32 = 0.8;

/// How long we wait for our own name to resolve before arming anyway. The name makes the command
/// selection-proof (§2 above), so it is worth a short wait; going without it is still better than
/// leaving the body mortal.
const NAME_WAIT_SECS: f32 = 3.0;

/// How long the server gets to confirm before we stop claiming the body is shielded. The live
/// round trip measured ~100–150 ms; this is a wide margin whose only job is to stop a silent
/// refusal from reading as success (0651's lesson).
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

/// What the preflight banner says about the shield. Copy and tiny on purpose: the banner is the
/// reader's one orienting line and it must not have to reason about the state machine.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum ShieldReport {
    /// Not a probe account — the shield has no opinion about this body and never sends anything.
    #[default]
    NotOurs,
    /// `WOW_GOD=off`: deliberately unshielded. This body CAN die.
    Disabled,
    /// The command is out, the server has not answered yet.
    Arming,
    /// The server confirmed it. This body cannot die.
    Armed,
    /// The server did not confirm inside [`CONFIRM_SECS`], or refused. Treat the body as mortal.
    Unconfirmed,
}

/// The shield's state for this session.
#[derive(Resource)]
pub(crate) struct ProbeShield {
    /// What the banner reports.
    report: ShieldReport,
    /// Queued lines and when the next one may go out — the rig's cadence, for the rig's reason.
    steps: Vec<String>,
    next_at: f32,
    /// `Time::elapsed_secs` after which an unanswered `.cheat god on` is called unconfirmed.
    confirm_by: f32,
    /// Set at world entry, cleared once the batch is built — the "we owe this body a shield" latch.
    arm_wanted: Option<f32>,
    /// Our own death state last frame, so a death *edge* is distinguishable from arriving dead.
    /// `None` until the first descriptor read of this world entry.
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
    /// What the preflight banner should say ([`crate::preflight`]).
    pub(crate) fn report(&self) -> ShieldReport {
        self.report
    }
}

/// Whether `user` is a probe account — `probe` followed by digits, the identity every unattended
/// run logs in with (`docs/CONTRIBUTING.md`, "Running it unattended"). Nothing else is ours to
/// modify: a player's account is theirs, and a plain test account is not a probe.
fn is_probe_account(user: &str) -> bool {
    // Byte-wise (a &str slice at 5 would panic mid-char on non-ASCII input; bytes can't).
    let Some((prefix, digits)) = user.as_bytes().split_at_checked(5) else {
        return false;
    };
    prefix.eq_ignore_ascii_case(b"probe")
        && !digits.is_empty()
        && digits.iter().all(u8::is_ascii_digit)
}

/// Whether this run wants GM mode **off** — `WOW_GM=off`, or an explicit `gm:off` in `WOW_RIG`.
/// The default is on (decision 0679), so this is the flag a session sets when it needs faithful
/// factions, hostility, colours, threat, damage or timers. An explicit ask always beats the
/// default, and the rig's token is an explicit ask: two modules must not send `.gm` at each other,
/// so the one with the spec in front of it wins (0651's rig owns its own `gm:` step).
///
/// [`crate::preflight`] reads it too — the banner has to know a `.gm off` is on its way out before
/// it accuses the run of measuring through GM mode.
pub(crate) fn wants_gm_off() -> bool {
    std::env::var("WOW_GM").is_ok_and(|v| v.eq_ignore_ascii_case("off"))
        || std::env::var("WOW_RIG").is_ok_and(|spec| rig_asks_for_gm_off(&spec))
}

/// Whether a `WOW_RIG` spec carries an explicit `gm:off` — the rig's own token
/// ([`crate::capture::probe_rig`]), matched the way the rig matches it (case-insensitive key,
/// anything but `on`/`1` falsy).
fn rig_asks_for_gm_off(spec: &str) -> bool {
    spec.split_whitespace().any(|tok| {
        matches!(
            tok.to_ascii_lowercase().split_once(':'),
            Some(("gm", v)) if !matches!(v, "on" | "1")
        )
    })
}

/// Whether `WOW_GOD=off` asked for an unshielded run — a launch-time knob, read once (the shield
/// drive asks every frame).
fn disabled_by_env() -> bool {
    static OFF: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| {
        std::env::var("WOW_GOD").is_ok_and(|v| v.eq_ignore_ascii_case("off"))
    });
    *OFF
}

/// Classify one `CHAT_MSG_SYSTEM` line as an answer about god mode. `None` = not about it.
///
/// The strings are vmangos `mangos_string` 368/369 (`LANG_YOU_SET_GOD`, `LANG_YOUR_GOD_SET`) — the
/// only ones that reach us as *chat*. `LANG_CHEAT_GOD_ON/OFF` (349/350) travel as
/// `SMSG_NOTIFICATION` instead and never appear here, which is why matching on "GOD mode is ON"
/// would silently never fire.
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

    // The server's verdict — the only ground truth about a flag that rides no descriptor field.
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

    // A death is proof the shield was down, whatever we believed. `.die` is the usual cause: it
    // clears god mode silently (`notify = false`) before killing, by design, so this is the normal
    // path for a session that wanted a corpse — not necessarily a fault.
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

    // Build the batch once the body — and ideally our own name — is there to address.
    if let Some(wanted_at) = shield.arm_wanted {
        let Ok(store) = self_q.single() else { return };
        let name = self_guid.0.and_then(|g| names.peek(g));
        let waited = time.elapsed_secs() - wanted_at;
        if name.is_none() && waited < NAME_WAIT_SECS {
            return; // the name makes the command selection-proof — it is worth a short wait
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
            // The default (decision 0679): GM mode stays on, so nothing aggros a parked body. The
            // shield is what makes it SAFE to drop, not a reason to drop it by default.
            (false, false) => {
                info!(
                    "probe-shield: GM mode is off — turning it back ON, so a parked body is not \
                     permanently mobbed. Readings taken now are faction-35 readings and are wrong \
                     for anything about hostility, colour, threat, aggro, damage or timers \
                     (0649/0657) — set WOW_GM=off for those, which is safe: the shield holds."
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

    // Send the queued lines, one per STEP_SECS — god first, always.
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

/// The command, always naming the character when we know it — `.cheat god` re-targets to the
/// current selection, and a creature selection makes the bare form fail.
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
        // A player's account, and plain test accounts: never touched.
        for user in ["one", "two", "probe", "probeone", "aprobe1", "probe1x"] {
            assert!(!is_probe_account(user), "{user} is NOT a probe account");
        }
    }

    #[test]
    fn the_servers_own_words_are_what_confirm_it() {
        // vmangos 368, what `.cheat god on <name>` actually answers (captured live).
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
        // Everything else on the same channel is not an answer about the shield.
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
        // GM mode is ON by default (0679), so the rig's token only matters when it asks for OFF —
        // and then the shield must not fight it back on.
        assert!(rig_asks_for_gm_off("gnome mage 39 gm:off"));
        assert!(rig_asks_for_gm_off("GM:0"));
        // An explicit gm:on, and every spec that says nothing, leave the default alone.
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
        // The bare form re-targets to the selection and is refused with a creature targeted
        // (verified live), so the name is the whole point.
        assert_eq!(god_line(Some("Probetwo")), ".cheat god on Probetwo");
        assert_eq!(god_line(None), ".cheat god on");
    }
}
