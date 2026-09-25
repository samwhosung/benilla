//! The session preflight — one always-on banner naming **what body we just logged
//! into**, plus loud warnings for the avatar states that silently invalidate a session's work.
//!
//! This is instrumentation for the *reader of the log*, not a game system. It exists because three
//! things cost a session ten minutes each and none of them announces itself:
//!
//! 1. **The character is dead or a ghost**, left that way by an earlier session. Nothing can be
//!    interacted with, attacked, looted or talked to; the world renders through the death filter;
//!    movement is rooted (unreleased) or ghost-speed (released). Every symptom reads like a client
//!    bug, and the client is fine.
//! 2. **GM mode is on** — which every probe character used to carry by default, as the only guard
//!    against unattended aggro killing it. vmangos's `Player::SetGameMaster` re-templates the player
//!    to **faction template 35** and freezes the mirror timers, so nothing is hostile, nothing
//!    aggros, fall/environmental damage is skipped and breath/fatigue never tick.
//!    FactionTemplate.dbc row 35 (VERIFIED from the extracted DBC): faction 31 "Friendly", own group
//!    mask 0, enemy mask 0 — it is hostile to nothing and nothing is hostile to it. Any hostility,
//!    reaction-colour, nameplate, aggro, threat, damage or drowning measurement taken with GM on is
//!    simply wrong, quietly. It also silently suspends the **indoor dismount**: the outdoor-only
//!    aura sweep is `!IsGameMaster()`-gated, so a GM rides into the Goldshire inn and stays mounted.
//! It is now **off** by default: [`crate::probe_shield`] keeps the body
//!    alive without poisoning any of that, and this banner reports which it is.
//! 3. **Movement is server-blocked** — rooted, stunned, confused, fleeing, or mid-taxi-flight. The
//!    controller ignores input and the honest report is "the mover is broken".
//! 4. **The run has no server at all** ([`offline_notice`]). Capture mode passes
//!    `NetPlugin { connect: false }`, so no IO thread is spawned and the drain receives nothing —
//!    the whole packet path (`net::apply`, the movement wire, every `MSG_MOVE_*`) simply does not
//!    execute. This one is the mirror image of the other three: they make a healthy client *look*
//!    broken, while this makes an untested change *look proven*. A capture run over a change to the
//!    packet path exits clean no matter what that change does, and "clean run" then lands in a
//!    commit message as evidence it never was (twice in one day — decisions 0725 and 0726, both
//!    pure wire work, both reported with a capture run behind them). The banner above cannot catch
//!    it: that one waits on [`EnteredWorldMessage`], which needs a server. So this notice fires at
//!    **startup**, before anything can be concluded from the run.
//!
//! 5. **Two `Camera2d`s on one target disagree about MSAA** ([`camera_2d_msaa_agrees`], decision
//!    1659). Not an avatar state, but the same shape as the four above: it is invisible until it
//!    kills the frame, and when it does, wgpu's `Attachments have differing sample counts` names
//!    two numbers and no camera. One scan on the frame the cameras spawn turns that into a line
//!    that names them.
//!
//! The banner is **not env-gated**: a warning nobody knows to switch on is not a warning. It costs
//! one line per world entry when everything is fine, and it re-fires on every re-entry (a relog, a
//! `.character race` forced logout, a reconnect) because the state can have changed.
//!
//! The same audience — the reader of the log — is why [`benilla_world::build_id::banner`] exists.
//! It is **not** registered here any more: this module is dev-only since 1174, and
//! "which build produced this log" is the first thing a report from *someone else's machine* has to
//! establish — which is precisely the player build. The banner now registers beside the stamp
//! itself, in `lib::run`, where it is always compiled.
//!
//! The other half of decision 0649 — the pre-connect **account guard**, which keeps a scripted
//! run from logging in as an account its checkout did not declare — lives in
//! [`crate::run_mode`] now, not here: it is consulted by the login policy, and 1174's seam does not
//! let gameplay call an instrument. Its reasoning went with it.

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

/// `PLAYER_FLAGS_GM` (vmangos `Player.h`) — set by `SetGameMaster(true)` alongside the faction-35
/// re-template. PUBLIC, so it rides our own descriptor like any other player flag.
const PLAYER_FLAGS_GM: u32 = 0x0000_0008;

/// The `UNIT_FIELD_FLAGS` bits that mean "the server is driving, or refusing to let you drive"
/// (vmangos `UnitDefines.h`) — each is a distinct reason the **mover** looks broken.
///
/// **PACIFIED and DISARMED used to sit in here and do not belong**: neither
/// touches movement, and a banner telling a session "movement is server-blocked" because the
/// character is disarmed sends someone hunting a mover bug that is not there. They moved to
/// [`ABILITY_BLOCKERS`]. What is left is movement — and note they are **not one gate**: STUNNED
/// is the only bit that reaches the local input tick (`0x5145b0` → `0x514755`, killing the turn
/// and pitch emitters), CONFUSED and FLEEING act through the `IsSelfControlled` predicate
/// `0x5fa550` (mask `0xc00004`, which STUNNED is *not* in), POSSESSED does not refuse at all but
/// redirects to the charmer (`0x5fa582`), and the taxi bit shares no gate with any of them.
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

/// The `UNIT_FIELD_FLAGS` bits that leave the mover alone and take **abilities** away instead —
/// the other half of the split above. A session that reads "movement is server-blocked" and finds
/// the character walking fine has been misdirected; these say what is actually gone.
const ABILITY_BLOCKERS: &[(u32, &str)] = &[
    (0x0002_0000, "PACIFIED (no melee, no pacify-blocked casts)"),
    (
        0x0020_0000,
        "DISARMED (one weapon — the main hand's, else the off hand's — reads as absent: unarmed \
         swing and Ready idle, that weapon off the hand, Spell-Reset on the Attack button, and \
         weapon-requiring abilities greyed)",
    ),
];

/// Worth naming on entry: an unattended probe that logs in already fighting is the exact shape of
/// the accident the unattended-combat ban exists for (docs/METHOD.md). The bit itself is declared once
/// ([`crate::player`]).
use crate::player::UNIT_FLAG_IN_COMBAT;

/// `UNIT_FLAG_SILENCED` — the client's OWN local refusal, not just the server's: the CC validator
/// `0x6094f0` (reached from `TryCast 0x6e4b60`, bailing at `0x6e4f42`) refuses before any packet
/// goes out. But it is **per spell** — the arm runs only where `Spell.dbc PreventionType` (column
/// 165) is `1`, so a silence stops the ~4800 rows that declare it and nothing else (decision
/// 1903). The banner said "spell casts will refuse" flat, which is wrong for every other row.
const UNIT_FLAG_SILENCED: u32 = 0x0000_2000;

/// The GM faction template vmangos swaps in with GM mode (`SetFactionTemplateId(35)`).
const GM_FACTION_TEMPLATE: u32 = 35;

/// How long after world entry we keep waiting for the zone name before reporting without it. The
/// descriptor lands in a few hundred ms but the area id only resolves once the tile under us has
/// streamed (~2 s), and "Eastern Plaguelands" is the line's whole orienting value — a raw map id
/// and a coordinate triple do not tell a reader at a glance that this character is nowhere useful.
const ZONE_WAIT_SECS: f32 = 4.0;

/// How long we let an in-flight `.gm off` settle before banner-ing the GM flag anyway.
///
/// `WOW_GM=off` is a **round trip** and it loses the race with the descriptor: the shield sends
/// `.gm off` at world entry and the server's answer lands ~0.9 s later, while the self descriptor
/// is here in ~0.3 s. Without this grace the banner shouts "GM MODE IS ON — any damage or drowning
/// reading taken now is wrong" at precisely the run that asked for GM off, which reads as *discard
/// this measurement* and is exactly backwards (a 2026-08-13 lava probe hit it). Wait, then report
/// what actually held — a `.gm off` that never lands is still warned about, one grace later.
const GM_OFF_WAIT_SECS: f32 = 3.0;

/// The hard backstop: report whatever we have this long after world entry. Only reached when
/// something is badly wrong with the stream, and reporting late beats never reporting.
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

/// Say out loud, once at startup, that this run has **no server** — so nobody reads its clean exit
/// as evidence about code that never ran (module header §4).
///
/// Deliberately a `warn!` rather than an `info!`. A netless run is completely normal and completely
/// fine — it is what every visual capture wants — so this is not a complaint about the run. It is
/// aimed at the *reader*, who is usually a session skimming for the word "error" before pasting
/// "clean run" into a commit message, and `warn` is the lowest level that survives that skim.
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

/// **Every `Camera2d` sharing a render target must carry the same `Msaa`** — checked out loud on
/// the frame they spawn, because the failure is fatal, immediate, and names nobody.
///
/// Bevy's two prepare passes disagree about whether the sample count is part of a texture's
/// identity. `view::prepare_view_targets` keys the **colour** target on `(target, usage, hdr,
/// msaa)`, so every camera gets an attachment at its own count. `core_2d::
/// prepare_core_2d_depth_textures` keys the **depth** texture on `camera.target` *alone* and stamps
/// it with whichever camera the hash map reaches first — `core_3d`'s equivalent keys on `(target,
/// msaa)`, so the asymmetry is 2D-only and there is nothing here for `Camera3d`. Two `Camera2d`s on
/// one window at different counts therefore share one depth texture, and the one that did not
/// create it opens its pass with mismatched attachments: `Attachments have differing sample
/// counts: the depth attachment's texture view has count N but ... count M`. Two numbers, no
/// camera, no system, no frame — a panic in `render_system` on a thread called `Compute Task Pool`.
///
/// It costs a session an afternoon of source archaeology, and it is one query to catch. The trap
/// that produces it is silence: `Camera` requires `Msaa` and `Msaa::default()` is `Sample4`, so a
/// camera that never mentions multisampling does not get none, it gets four (1628 found that
/// costing a full-window 4× texture on the player-UI camera; 1659 found the same silence on the
/// egui overlay camera crashing the debug panel outright, one commit after 1628 broke the tie that
/// had been hiding it). So the error below says *which* camera and *what* count, and names the
/// likely cause.
///
/// Gated on `Added<Camera2d>` rather than run once at `PostStartup`: our three 2D cameras all spawn
/// in `Startup` today, but a fourth added later must be checked too, and the guard makes the
/// steady-state cost one empty query.
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
        // A target that will not normalize has no swapchain yet and no depth texture either.
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
             the cause is a camera that never named an Msaa: silence is Sample4, not off \
             (decisions 1628, 1659).",
            other_label.as_deref().unwrap_or("an unnamed Camera2d"),
            other_samples,
            label.as_deref().unwrap_or("an unnamed Camera2d"),
            samples,
        );
    }
}

/// The banner's once-per-entry latch: armed by [`EnteredWorldMessage`], disarmed when the report
/// goes out, when the wait expires, or when the session that armed it ends.
#[derive(Resource, Default)]
struct Preflight {
    /// `Time::elapsed_secs` at the world entry we still owe a report for; `None` = nothing pending.
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
    // **A session that ended disarms the wait** — after the arm, so the entry-and-death-in-one-drain
    // race resolves the way it does everywhere else: the dead session is the last
    // word. Without this the latch outlives the session that armed it and the banner fires from the
    // glue screen, telling a future reader that "the avatar never streamed in" when the truth is
    // that nobody ever entered a world for it to stream into. A refused character login makes that
    // reachable on purpose (`SMSG_CHARACTER_LOGIN_FAILED` revokes an entry already announced); a
    // socket that dies mid-entry always could.
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
    // MAXHEALTH is always in the login snapshot; an empty store means the create block hasn't been
    // applied yet, and reading it would report a level-0 dead corpse of no race.
    if store.0.unit_max_health().unwrap_or(0) == 0 && !expired {
        return;
    }
    // `CurrentArea` is the FINEST area under us ("Darrowshire"), which on its own can be unplaceable;
    // the parent zone ("Eastern Plaguelands") is what orients a reader, so print zone / subzone.
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
        return; // the tile under us hasn't streamed yet — give the zone name its grace
    }
    // The other in-flight state the banner would otherwise misreport: a requested `.gm off` that
    // the server has not answered yet (see [`GM_OFF_WAIT_SECS`]).
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
    // Printed as the raw WoW triple, which is what `.go xyz <x> <y> <z> <map>` takes — the whole
    // point of putting the position in the banner is that it can be pasted back.
    let [wx, wy, wz] = benilla_assets::coords::bevy_to_wow(transform.translation);
    info!(
        "preflight: {name} — level {lvl} {race} {class}, {hp}/{maxhp} hp, map {map}{zone} @ [{wx:.1}, {wy:.1}, {wz:.1}], faction template {faction}{shielded}",
        lvl = store.0.unit_level().unwrap_or(0),
        hp = store.0.unit_health().unwrap_or(0),
        maxhp = store.0.unit_max_health().unwrap_or(0),
        map = map.map_or(-1, |m| m.0 as i64),
        faction = store.0.unit_faction_template().unwrap_or(0),
        // A shielded body is good news, so it rides the banner rather than the warning ladder —
        // but it is still *stated*, because "can this thing die?" is the first question an
        // unattended run needs answered.
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

/// Whether to hold the banner for a `.gm off` that has gone out but not been answered
/// ([`GM_OFF_WAIT_SECS`] is the why). Pure, so the race it guards is testable without an app.
///
/// All four clauses matter. Only a run that *asked* for GM off waits; only a body the shield
/// actually commands (`probe<N>`) can have a `.gm off` in flight at all — on the director's own
/// account `WOW_GM` is inert, so waiting there would just delay a true warning; and the
/// grace expires, so a `.gm off` that never lands is still reported, one grace later.
fn hold_for_gm_off(gm_flag_set: bool, wants_off: bool, shield: ShieldReport, waited: f32) -> bool {
    gm_flag_set
        && wants_off
        && matches!(shield, ShieldReport::Arming | ShieldReport::Armed)
        && waited < GM_OFF_WAIT_SECS
}

/// Everything about this avatar that will quietly invalidate a session's work, worst first. Pure
/// over the descriptor (plus the one piece of state that rides no descriptor field — the probe
/// shield) so the whole ladder is unit-testable.
fn findings(
    fields: &benilla_protocol::messages::ObjectFields,
    shield: ShieldReport,
) -> Vec<String> {
    let mut out = Vec::new();
    let unit_flags = fields.unit_flags();

    // Dead and ghost are mutually exclusive on the wire (a released ghost's health is 1, decision
    // 0308 §1) — report whichever holds, never both.
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

    // The shield's bad states are findings; being armed is good news and rides the banner line.
    match shield {
        ShieldReport::Disabled => out.push(
            "THE PROBE SHIELD IS OFF (WOW_GOD=off) — this character CAN die, so an unattended run \
             can leave a corpse for the next session. Deliberate for a death-arc test; unset \
             WOW_GOD for anything else (decision 0677)."
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
             !IsGameMaster())`, so a GM rides into a building and stays mounted (decision 0934). {}",
            match shield {
                // GM mode is the DEFAULT — it is what stops a parked body being mobbed, and
                // the shield is what makes dropping it safe. So this warning is expected on most
                // runs, and its job is to make sure nobody measures hostility through faction 35.
                ShieldReport::Arming | ShieldReport::Armed =>
                    "This is the default. Re-run with WOW_GM=off for those readings — safe, \
                     because the probe shield (decision 0677) keeps the body alive without it.",
                // Not a probe body — a player's account, or a plain test account. `WOW_GM` would be
                // inert here (the shield only ever commands `probe<N>`), and saying otherwise
                // sends the reader after a switch that does nothing. The state is also PERSISTED:
                // vmangos saves it in `characters.extra_flags` bit 0 and `GM.LoginState = 2`
                // restores it, so it stays on across logins until somebody turns it off.
                ShieldReport::NotOurs =>
                    "WOW_GM does not reach this body — the shield only ever commands probe accounts \
                     (0677) — so the way out is typing `.gm off` yourself. It persists across \
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

    /// Build a player descriptor from `(field index, value)` pairs — the wire indices the
    /// accessors read.
    fn player(fields: &[(u16, u32)]) -> ObjectFields {
        ObjectFields::from_pairs(fields)
    }

    const HEALTH: u16 = 22;
    const MAXHEALTH: u16 = 28;
    const UNIT_FLAGS: u16 = 46;
    const PLAYER_FLAGS: u16 = 190;

    /// The banner must not accuse a `WOW_GM=off` run of measuring through GM mode while the
    /// `.gm off` it just sent is still in flight — the race [`GM_OFF_WAIT_SECS`] exists for, and
    /// the one a lava probe walked into on 2026-08-13.
    #[test]
    fn the_banner_waits_out_a_gm_off_round_trip_but_not_forever() {
        // The race itself: flag still set, we asked for off, shield is ours, answer not back yet.
        assert!(hold_for_gm_off(true, true, ShieldReport::Arming, 0.4));
        assert!(hold_for_gm_off(true, true, ShieldReport::Armed, 0.4));

        // …but the grace expires, so a `.gm off` that never lands is still reported.
        assert!(!hold_for_gm_off(
            true,
            true,
            ShieldReport::Armed,
            GM_OFF_WAIT_SECS + 0.1
        ));

        // Nothing else ever waits. GM mode is the DEFAULT: a run that did not ask for it off
        // must be warned immediately, not three seconds late…
        assert!(!hold_for_gm_off(true, false, ShieldReport::Armed, 0.4));
        // …a body the shield does not command (the director's own account) can have no `.gm off`
        // in flight at all, so waiting there would only delay a true warning…
        assert!(!hold_for_gm_off(true, true, ShieldReport::NotOurs, 0.4));
        assert!(!hold_for_gm_off(true, true, ShieldReport::Disabled, 0.4));
        // …and with the flag already clear there is nothing to wait for.
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
        // Ghost: health 1 and PLAYER_FLAGS_GHOST — the wire never shows both.
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
        // Armed is good news: it rides the banner line, never the warning ladder. Neither does a
        // body the shield has no opinion about (the director's own account).
        for quiet in [
            ShieldReport::Armed,
            ShieldReport::Arming,
            ShieldReport::NotOurs,
        ] {
            assert!(findings(&healthy, quiet).is_empty(), "{quiet:?}");
        }
        // Both failure shapes are findings, because both mean "this body can die".
        let off = findings(&healthy, ShieldReport::Disabled);
        assert_eq!(off.len(), 1);
        assert!(off[0].contains("SHIELD IS OFF") && off[0].contains("WOW_GOD"));
        let unconfirmed = findings(&healthy, ShieldReport::Unconfirmed);
        assert_eq!(unconfirmed.len(), 1);
        assert!(unconfirmed[0].contains("DID NOT CONFIRM"));
    }

    #[test]
    fn the_gm_warning_always_names_the_way_out() {
        // GM mode is the default, so this warning fires on nearly every probe run. Its whole
        // job is that nobody measures hostility through faction 35 without being told how to stop —
        // and the old advice ("put it back on when you are done") must not come back, because a
        // session that complies re-poisons every faction reading.
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
        // …but the way out has to be one that WORKS on this body. `WOW_GM` only ever commands a
        // probe account, so on anyone else's — the director's own, which is exactly who
        // rides into an inn and asks why they are still mounted — the switch is `.gm off`, typed.
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
        // The consequence that cost a session: the outdoor-only aura sweep is
        // `!IsGameMaster()`-gated server-side, so a GM never dismounts riding into a building — and
        // the client, whose mount lane is nothing but the MOUNTDISPLAYID watcher, has no say in it.
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

    /// **The two tables are separate lines, and neither claims the other's symptom** (decision
    /// 1903). PACIFIED and DISARMED touch no movement gate — sending a session to hunt a mover
    /// bug is the whole cost of filing them under one banner, and this is what makes that
    /// regression loud.
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

        // Both halves at once: two lines, each naming only its own flags.
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
