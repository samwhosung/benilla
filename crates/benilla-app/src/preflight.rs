//! The session preflight: an always-on banner naming the body just logged into, and warnings for
//! the states that silently invalidate a run's readings.
//!
//! 1. Dead or a ghost: nothing can be interacted with, the world renders through the death filter
//!    and movement is rooted or ghost-speed.
//! 2. GM mode on: vmangos's `Player::SetGameMaster` re-templates the player to faction template 35
//!    (FactionTemplate.dbc: faction 31, group and enemy masks 0) and freezes the mirror timers, so
//!    nothing is hostile, nothing aggros, environmental damage is skipped and breath never ticks.
//!    It also keeps outdoor-only auras on indoors (`Player.cpp:6284` is `!IsGameMaster()`-gated),
//!    so a GM stays mounted inside. It is on by default for a probe body; [`crate::probe_shield`]
//!    makes dropping it safe.
//! 3. Movement server-blocked: rooted, stunned, confused, fleeing or on a taxi.
//! 4. No server at all ([`offline_notice`]): a capture run spawns no IO thread, so nothing on the
//!    packet path executes and a clean exit proves nothing about it. This fires at startup, since
//!    the banner itself waits on [`EnteredWorldMessage`].
//! 5. Two `Camera2d`s on one target with different MSAA ([`camera_2d_msaa_agrees`]), fatal the
//!    frame it happens with an error that names no camera.
//!
//! The banner is not env-gated and re-fires on every world entry, since the state can change.

use std::collections::HashMap;

use bevy::camera::{NormalizedRenderTarget, RenderTarget};
use bevy::prelude::*;
use bevy::render::view::Msaa;
use bevy::window::PrimaryWindow;

use crate::area::AreaTableRes;
use crate::names::NameCache;
use crate::net::{DisconnectedMessage, EnteredWorldMessage, ObjectStore, SelfGuid, SelfPlayer};
use crate::probe_shield::{ProbeShield, ShieldReport};
use benilla_world::world_map::CurrentMap;

/// `PLAYER_FLAGS_GM` (vmangos `Player.h`), set by `SetGameMaster(true)` with the faction-35
/// re-template; a public flag on our own descriptor.
const PLAYER_FLAGS_GM: u32 = 0x0000_0008;

/// The `UNIT_FIELD_FLAGS` bits (vmangos `UnitDefines.h`) that block or drive the mover, each
/// through its own gate: STUNNED alone reaches the input tick (`0x5145b0` → `0x514755`, no turn or
/// pitch), CONFUSED and FLEEING act through `0x5fa550` (mask `0xc00004`), POSSESSED redirects to
/// the charmer (`0x5fa582`), and the taxi bit shares no gate.
const MOVE_BLOCKERS: &[(u32, &str)] = &[
    (0x0004_0000, "STUNNED (no turning, no pitch)"),
    (
        0x0010_0000,
        "on a TAXI FLIGHT (input ignored for the whole ride)",
    ),
    (0x0040_0000, "CONFUSED (the server drives the movement)"),
    (0x0080_0000, "FLEEING (the server drives the movement)"),
    (0x0100_0000, "POSSESSED (another unit holds the reins)"),
];

/// The `UNIT_FIELD_FLAGS` bits that leave the mover alone and take abilities away.
const ABILITY_BLOCKERS: &[(u32, &str)] = &[
    (0x0002_0000, "PACIFIED (no melee, no pacify-blocked casts)"),
    (
        0x0020_0000,
        "DISARMED (one weapon — the main hand's, else the off hand's — reads as absent: unarmed \
         swing and Ready idle, that weapon off the hand, Spell-Reset on the Attack button, and \
         weapon-requiring abilities greyed)",
    ),
];

/// Named on entry: an unattended run must not be left fighting (`docs/METHOD.md`).
use crate::player::UNIT_FLAG_IN_COMBAT;

/// `UNIT_FLAG_SILENCED`: the 1.12 client refuses locally (`0x6094f0`, from `0x6e4b60`), but only
/// spells whose `Spell.dbc` `PreventionType` (column 165) is 1.
const UNIT_FLAG_SILENCED: u32 = 0x0000_2000;

/// The faction template vmangos swaps in with GM mode (`SetFactionTemplateId(35)`).
const GM_FACTION_TEMPLATE: u32 = 35;

/// How long after world entry to wait for the zone name, which resolves only once the tile under
/// us has streamed (~2 s).
const ZONE_WAIT_SECS: f32 = 4.0;

/// How long an in-flight `.gm off` gets to land before the GM flag is reported: its answer takes
/// ~0.9 s, the self descriptor ~0.3 s.
const GM_OFF_WAIT_SECS: f32 = 3.0;

/// The backstop: report whatever there is this long after world entry.
const DESCRIPTOR_WAIT_SECS: f32 = 8.0;

pub(crate) struct PreflightPlugin;

impl Plugin for PreflightPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Preflight>()
            .add_systems(Startup, offline_notice)
            .add_systems(Update, camera_2d_msaa_agrees)
            .add_systems(
                Update,
                report_session.after(benilla_world::schedule::WorldStage::Net),
            );
    }
}

/// Warn once at startup that this run has no server; a `warn!` so a skim of the log sees it.
fn offline_notice(net: Option<Res<crate::net::NetOffline>>) {
    if net.is_none() {
        return;
    }
    warn!(
        "preflight: NET OFF — no IO thread this run, so the drain gets no packets and NOTHING on \
         the wire path executes (net::apply, the movement stream, every MSG_MOVE_*). Fine for a \
         visual capture; NOT evidence for a change to any of it. docs/METHOD.md's gate is a clean run \
         of the AFFECTED path — for wire work that means a live server run."
    );
}

/// Every `Camera2d` on one render target must carry the same `Msaa`: Bevy keys the 2D depth
/// texture on the target alone (`core_2d::prepare_core_2d_depth_textures`) but the colour target
/// on the sample count too, so a mismatch panics in `render_system` naming no camera. A camera
/// that never names an `Msaa` gets `Sample4`. Checked whenever a `Camera2d` is added.
fn camera_2d_msaa_agrees(
    added: Query<(), (With<Camera2d>, Added<Camera2d>)>,
    cams: Query<(Entity, &RenderTarget, &Msaa, Option<&Name>), With<Camera2d>>,
    primary: Query<Entity, With<PrimaryWindow>>,
) {
    if added.is_empty() {
        return;
    }
    let primary = primary.single().ok();
    let mut first: HashMap<NormalizedRenderTarget, (Entity, Option<String>, u32)> = HashMap::new();
    for (entity, target, msaa, name) in &cams {
        // A target that will not normalize has no depth texture yet.
        let Some(key) = target.normalize(primary) else {
            continue;
        };
        let label = name.map(|n| n.as_str().to_owned());
        let samples = msaa.samples();
        let Some((other, other_label, other_samples)) = first.get(&key) else {
            first.insert(key, (entity, label, samples));
            continue;
        };
        if *other_samples == samples {
            continue;
        }
        error!(
            "preflight: {} ({other}) renders {}x MSAA and {} ({entity}) renders {}x, both to the \
             SAME target ({key:?}) — one of them will die with `Attachments have differing sample \
             counts` the frame it goes active. Bevy keys the Core2d DEPTH texture on the target \
             alone (core_2d::prepare_core_2d_depth_textures) but the COLOUR target on the sample \
             count too, so they share one depth attachment and cannot share a colour one. Usually \
             the cause is a camera that never named an Msaa: silence is Sample4, not off, so \
             name one on every camera.",
            other_label.as_deref().unwrap_or("an unnamed Camera2d"),
            other_samples,
            label.as_deref().unwrap_or("an unnamed Camera2d"),
            samples,
        );
    }
}

/// The banner's once-per-entry latch: armed by [`EnteredWorldMessage`], disarmed when the report
/// goes out, the wait expires or the session ends.
#[derive(Resource, Default)]
struct Preflight {
    /// `Time::elapsed_secs` at the world entry still owed a report.
    armed_at: Option<f32>,
}

/// Wait for the self descriptor after each world entry, then print the banner once.
fn report_session(
    mut state: ResMut<Preflight>,
    mut entered: MessageReader<EnteredWorldMessage>,
    mut ended: MessageReader<DisconnectedMessage>,
    time: Res<Time>,
    self_q: Query<(&ObjectStore, &Transform), With<SelfPlayer>>,
    self_guid: Res<SelfGuid>,
    names: Res<NameCache>,
    map: Option<Res<CurrentMap>>,
    world: benilla_world::world_point::WorldPoint,
    area_table: Option<Res<AreaTableRes>>,
    shield: Res<ProbeShield>,
) {
    if entered.read().next().is_some() {
        state.armed_at = Some(time.elapsed_secs());
    }
    // An ended session disarms the wait, after the arm, so an entry and a disconnect in one drain
    // leave it disarmed (`SMSG_CHARACTER_LOGIN_FAILED` revokes an announced entry).
    if ended.read().next().is_some() {
        state.armed_at = None;
    }
    let Some(armed_at) = state.armed_at else {
        return;
    };
    let expired = time.elapsed_secs() - armed_at >= DESCRIPTOR_WAIT_SECS;
    let Ok((store, transform)) = self_q.single() else {
        if expired {
            state.armed_at = None;
            warn!("preflight: no self descriptor {DESCRIPTOR_WAIT_SECS:.0}s after entering the world — the avatar never streamed in");
        }
        return;
    };
    // MAXHEALTH is always in the login snapshot; without it the create block is not applied yet.
    if store.0.unit_max_health().unwrap_or(0) == 0 && !expired {
        return;
    }
    // The area is the finest one under us, so print zone / subzone.
    let zone = world
        .area()
        .zip(area_table.as_deref())
        .map(|(id, cat)| {
            let sub = cat.0.name(id);
            let top = cat.0.top_zone(id).and_then(|z| cat.0.name(z));
            match (top, sub) {
                (Some(top), Some(sub)) if top != sub => format!(" \"{top} / {sub}\""),
                (Some(n), _) | (_, Some(n)) => format!(" \"{n}\""),
                _ => String::new(),
            }
        })
        .filter(|z| !z.is_empty());
    if zone.is_none() && time.elapsed_secs() - armed_at < ZONE_WAIT_SECS {
        return; // the tile under us has not streamed yet
    }
    // A requested `.gm off` the server has not answered yet.
    if hold_for_gm_off(
        store.0.player_flags() & PLAYER_FLAGS_GM != 0,
        crate::probe_shield::wants_gm_off(),
        shield.report(),
        time.elapsed_secs() - armed_at,
    ) {
        return;
    }
    state.armed_at = None;

    let name = self_guid
        .0
        .and_then(|g| names.peek(g))
        .unwrap_or("<unnamed>");
    let race = store
        .0
        .unit_race()
        .and_then(|r| crate::ui_unit::race_names(r).map(|(d, _)| d))
        .unwrap_or("?");
    let class = store
        .0
        .unit_class()
        .and_then(|c| crate::ui_unit::class_names(c).map(|(d, _)| d))
        .unwrap_or("?");
    let zone = zone.unwrap_or_default();
    // The raw WoW triple, as `.go xyz <x> <y> <z> <map>` takes it.
    let [wx, wy, wz] = benilla_assets::coords::bevy_to_wow(transform.translation);
    info!(
        "preflight: {name} — level {lvl} {race} {class}, {hp}/{maxhp} hp, map {map}{zone} @ [{wx:.1}, {wy:.1}, {wz:.1}], faction template {faction}{shielded}",
        lvl = store.0.unit_level().unwrap_or(0),
        hp = store.0.unit_health().unwrap_or(0),
        maxhp = store.0.unit_max_health().unwrap_or(0),
        map = map.map_or(-1, |m| m.0 as i64),
        faction = store.0.unit_faction_template().unwrap_or(0),
        // A shielded body rides the banner, not the warnings.
        shielded = match shield.report() {
            ShieldReport::Armed => ", SHIELDED (cannot die)",
            ShieldReport::Arming => ", shield arming",
            _ => "",
        },
    );

    for line in findings(&store.0, shield.report()) {
        warn!("preflight: {line}");
    }
}

/// Whether to hold the banner for a `.gm off` sent but not answered: only a run that asked for it,
/// only on a body the shield commands, and only until [`GM_OFF_WAIT_SECS`].
fn hold_for_gm_off(gm_flag_set: bool, wants_off: bool, shield: ShieldReport, waited: f32) -> bool {
    gm_flag_set
        && wants_off
        && matches!(shield, ShieldReport::Arming | ShieldReport::Armed)
        && waited < GM_OFF_WAIT_SECS
}

/// Everything about this avatar that will quietly invalidate a run's readings, worst first.
fn findings(
    fields: &benilla_protocol::messages::ObjectFields,
    shield: ShieldReport,
) -> Vec<String> {
    let mut out = Vec::new();
    let unit_flags = fields.unit_flags();

    // Dead and ghost are exclusive on the wire: a released ghost's health is 1.
    if fields.unit_is_dead() {
        out.push(
            "THE CHARACTER IS DEAD (health 0, corpse not released) — the mover is server-rooted \
             and nothing can be cast, attacked, looted or interacted with. Fix it before anything \
             else: WOW_PROBE_CHAT=\".revive\" (probe accounts are gmlevel 6)."
                .into(),
        );
    } else if fields.player_is_ghost() {
        out.push(
            "THE CHARACTER IS A GHOST (spirit released, corpse elsewhere) — the world renders \
             through the death filter, NPCs and objects refuse every interaction, and movement \
             runs at ghost speed on water. Fix it before anything else: WOW_PROBE_CHAT=\".revive\"."
                .into(),
        );
    }

    // Being armed rides the banner line; only the bad states are findings.
    match shield {
        ShieldReport::Disabled => out.push(
            "THE PROBE SHIELD IS OFF (WOW_GOD=off) — this character CAN die, so an unattended run \
             can leave a corpse for the next session. Deliberate for a death-arc test; unset \
             WOW_GOD for anything else."
                .into(),
        ),
        ShieldReport::Unconfirmed => out.push(
            "THE PROBE SHIELD DID NOT CONFIRM — `.cheat god on` went out and the server never \
             answered. Treat this character as mortal and do not leave it parked anywhere hostile."
                .into(),
        ),
        ShieldReport::NotOurs | ShieldReport::Arming | ShieldReport::Armed => {}
    }

    if fields.player_flags() & PLAYER_FLAGS_GM != 0 {
        out.push(format!(
            "GM MODE IS ON — vmangos re-templates the player to faction {GM_FACTION_TEMPLATE} \
             (\"Friendly\", enemy mask 0) and freezes the mirror timers, so NOTHING is hostile, \
             nothing aggros, fall/environmental damage is skipped and breath/fatigue never tick. \
             Any hostility, reaction-colour, nameplate, threat, aggro, damage or drowning reading \
             taken now is wrong. It ALSO suspends the INDOOR DISMOUNT: \
             `CheckAreaExploreAndOutdoor` drops outdoor-only auras only `if (… && \
             !IsGameMaster())`, so a GM rides into a building and stays mounted. {}",
            match shield {
                // The default on a probe body, so this warning shows on most runs.
                ShieldReport::Arming | ShieldReport::Armed =>
                    "This is the default. Re-run with WOW_GM=off for those readings — safe, \
                     because the probe shield keeps the body alive without it.",
                // `WOW_GM` does not reach a non-probe body, and vmangos persists GM mode in
                // `characters.extra_flags` bit 0, restored at login by `GM.LoginState = 2`.
                ShieldReport::NotOurs =>
                    "WOW_GM does not reach this body — the shield only ever commands probe accounts \
                     — so the way out is typing `.gm off` yourself. It persists across \
                     logins (vmangos GM.LoginState = 2), which is why it is on now.",
                _ =>
                    "Re-run with WOW_GM=off for those readings. Note the probe shield is NOT up on \
                     this run, so an unshielded body with GM mode off can be killed.",
            }
        ));
    }

    let hits = |table: &[(u32, &'static str)]| -> Vec<&'static str> {
        table
            .iter()
            .filter(|(bit, _)| unit_flags & bit != 0)
            .map(|(_, label)| *label)
            .collect()
    };
    let blocked = hits(MOVE_BLOCKERS);
    if !blocked.is_empty() {
        out.push(format!(
            "MOVEMENT IS SERVER-BLOCKED — {} — the controller will look broken because the server \
             is refusing (or driving) the movement, not because the mover is.",
            blocked.join(", ")
        ));
    }
    let disabled = hits(ABILITY_BLOCKERS);
    if !disabled.is_empty() {
        out.push(format!(
            "ABILITIES ARE SERVER-BLOCKED — {} — the mover is fine; what will not work is the \
             action bar and the swing.",
            disabled.join(", ")
        ));
    }

    if unit_flags & UNIT_FLAG_IN_COMBAT != 0 {
        out.push(
            "IN COMBAT on arrival — something is already fighting this character. Do not leave it \
             unattended (docs/METHOD.md's unattended-combat ban): break the fight or move it out."
                .into(),
        );
    }
    if unit_flags & UNIT_FLAG_SILENCED != 0 {
        out.push(
            "SILENCED — the spells that declare `PreventionType = 1` (most casts) refuse locally \
             for as long as the aura holds; everything else is unaffected."
                .into(),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::ObjectFields;

    /// Build a player descriptor from `(field index, value)` pairs.
    fn player(fields: &[(u16, u32)]) -> ObjectFields {
        ObjectFields::from_pairs(fields)
    }

    const HEALTH: u16 = 22;
    const MAXHEALTH: u16 = 28;
    const UNIT_FLAGS: u16 = 46;
    const PLAYER_FLAGS: u16 = 190;

    /// A `WOW_GM=off` run is not warned about GM mode while its `.gm off` is in flight.
    #[test]
    fn the_banner_waits_out_a_gm_off_round_trip_but_not_forever() {
        // Flag still set, off asked for, shield ours, answer not back yet.
        assert!(hold_for_gm_off(true, true, ShieldReport::Arming, 0.4));
        assert!(hold_for_gm_off(true, true, ShieldReport::Armed, 0.4));

        // A `.gm off` that never lands is still reported.
        assert!(!hold_for_gm_off(
            true,
            true,
            ShieldReport::Armed,
            GM_OFF_WAIT_SECS + 0.1
        ));

        // Nothing else waits: not a run that did not ask for off,
        assert!(!hold_for_gm_off(true, false, ShieldReport::Armed, 0.4));
        // not a body the shield does not command,
        assert!(!hold_for_gm_off(true, true, ShieldReport::NotOurs, 0.4));
        assert!(!hold_for_gm_off(true, true, ShieldReport::Disabled, 0.4));
        // and not a clear flag.
        assert!(!hold_for_gm_off(false, true, ShieldReport::Armed, 0.4));
    }

    #[test]
    fn a_healthy_avatar_reports_nothing() {
        let f = player(&[(HEALTH, 60), (MAXHEALTH, 60)]);
        assert!(findings(&f, ShieldReport::Armed).is_empty());
    }

    #[test]
    fn dead_and_ghost_never_report_together() {
        // Dead: health 0 with a real MAXHEALTH.
        let dead = findings(&player(&[(MAXHEALTH, 60)]), ShieldReport::Armed);
        assert_eq!(dead.len(), 1);
        assert!(dead[0].contains("IS DEAD"));
        // Ghost: health 1 and PLAYER_FLAGS_GHOST.
        let ghost = findings(
            &player(&[(HEALTH, 1), (MAXHEALTH, 60), (PLAYER_FLAGS, 0x10)]),
            ShieldReport::Armed,
        );
        assert_eq!(ghost.len(), 1);
        assert!(ghost[0].contains("IS A GHOST"));
    }

    #[test]
    fn gm_mode_is_reported_on_a_perfectly_healthy_avatar() {
        let f = player(&[
            (HEALTH, 60),
            (MAXHEALTH, 60),
            (PLAYER_FLAGS, PLAYER_FLAGS_GM),
        ]);
        let out = findings(&f, ShieldReport::Armed);
        assert_eq!(out.len(), 1);
        assert!(out[0].contains("GM MODE IS ON"));
    }

    #[test]
    fn a_shielded_body_says_nothing_but_an_unshielded_one_does() {
        let healthy = player(&[(HEALTH, 60), (MAXHEALTH, 60)]);
        for quiet in [
            ShieldReport::Armed,
            ShieldReport::Arming,
            ShieldReport::NotOurs,
        ] {
            assert!(findings(&healthy, quiet).is_empty(), "{quiet:?}");
        }
        // Both failure shapes mean the body can die.
        let off = findings(&healthy, ShieldReport::Disabled);
        assert_eq!(off.len(), 1);
        assert!(off[0].contains("SHIELD IS OFF") && off[0].contains("WOW_GOD"));
        let unconfirmed = findings(&healthy, ShieldReport::Unconfirmed);
        assert_eq!(unconfirmed.len(), 1);
        assert!(unconfirmed[0].contains("DID NOT CONFIRM"));
    }

    #[test]
    fn the_gm_warning_always_names_the_way_out() {
        // The way out is always named, and never "put it back on", which re-poisons every
        // faction reading.
        let gm = player(&[
            (HEALTH, 60),
            (MAXHEALTH, 60),
            (PLAYER_FLAGS, PLAYER_FLAGS_GM),
        ]);
        for report in [
            ShieldReport::Armed,
            ShieldReport::Arming,
            ShieldReport::Disabled,
            ShieldReport::Unconfirmed,
        ] {
            let out = findings(&gm, report);
            let line = out.iter().find(|l| l.contains("GM MODE IS ON")).unwrap();
            assert!(line.contains("WOW_GM=off"), "{report:?}: {line}");
            assert!(!line.contains("put it back on"), "{report:?}: {line}");
        }
        // On a non-probe body `WOW_GM` does nothing; the way out is typing `.gm off`.
        let not_ours = findings(&gm, ShieldReport::NotOurs);
        let line = not_ours
            .iter()
            .find(|l| l.contains("GM MODE IS ON"))
            .unwrap();
        assert!(
            line.contains(".gm off") && !line.contains("WOW_GM=off"),
            "{line}"
        );
    }

    #[test]
    fn the_gm_warning_names_the_indoor_dismount() {
        // The server's outdoor-only aura sweep skips a GM, so a GM stays mounted indoors.
        let gm = player(&[
            (HEALTH, 60),
            (MAXHEALTH, 60),
            (PLAYER_FLAGS, PLAYER_FLAGS_GM),
        ]);
        let line = findings(&gm, ShieldReport::Armed).remove(0);
        assert!(line.contains("INDOOR DISMOUNT"), "{line}");
    }

    #[test]
    fn every_move_blocker_is_named_in_one_line() {
        let f = player(&[
            (HEALTH, 60),
            (MAXHEALTH, 60),
            (UNIT_FLAGS, 0x0004_0000 | 0x0010_0000), // stunned + taxi
        ]);
        let out = findings(&f, ShieldReport::Armed);
        assert_eq!(out.len(), 1);
        assert!(out[0].contains("STUNNED") && out[0].contains("TAXI FLIGHT"));
    }

    /// PACIFIED and DISARMED touch no movement gate, so they never report as a movement block.
    #[test]
    fn ability_blockers_are_their_own_line_and_never_say_movement() {
        let disarmed = player(&[(HEALTH, 60), (MAXHEALTH, 60), (UNIT_FLAGS, 0x0020_0000)]);
        let out = findings(&disarmed, ShieldReport::Armed);
        assert_eq!(out.len(), 1);
        assert!(out[0].starts_with("ABILITIES ARE SERVER-BLOCKED"));
        assert!(out[0].contains("DISARMED"));
        assert!(
            !out[0].contains("MOVEMENT"),
            "a disarm is not a movement block: {}",
            out[0]
        );

        // Both at once: two lines, each naming only its own flags.
        let both = player(&[
            (HEALTH, 60),
            (MAXHEALTH, 60),
            (UNIT_FLAGS, 0x0004_0000 | 0x0002_0000), // stunned + pacified
        ]);
        let out = findings(&both, ShieldReport::Armed);
        assert_eq!(out.len(), 2);
        let movement = out.iter().find(|l| l.starts_with("MOVEMENT")).unwrap();
        let abilities = out.iter().find(|l| l.starts_with("ABILITIES")).unwrap();
        assert!(movement.contains("STUNNED") && !movement.contains("PACIFIED"));
        assert!(abilities.contains("PACIFIED") && !abilities.contains("STUNNED"));
    }
}
