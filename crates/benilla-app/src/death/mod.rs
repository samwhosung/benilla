//! Death and resurrection: the wire-fed stores, the events into the Lua UI and the intents back
//! to the wire. Dead is health 0 and ghost is `PLAYER_FLAGS` bit 0x10, read off the self
//! descriptor each frame; `PLAYER_ALIVE` fires on release and on a pre-release res,
//! `PLAYER_UNGHOST` on ghost to alive.

use bevy::prelude::*;

use benilla_assets::coords::wow_to_bevy;
use benilla_ui::script::{DeathAction, DeathUiState, ScriptValue, UiScript};

use crate::net::{ClientCommand, NetCommands, ObjectStore, Objects, SelfGuid, SelfPlayer};
use crate::ui_action::Spells;
use crate::ui_script::{UiFeed, UiInput};

pub(crate) mod net;

/// Where our corpse is, the `MSG_CORPSE_QUERY` answer, in raw WoW coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CorpsePoint {
    /// The map to route toward; for a corpse inside an instance, the entrance's.
    pub(crate) display_map: i32,
    /// The corpse, or the dungeon entrance standing in for it.
    pub(crate) position: [f32; 3],
    /// The corpse's real map id, never adjusted.
    #[allow(dead_code)] // for the map markers
    pub(crate) corpse_map: u32,
}

/// A pending resurrection offer (`SMSG_RESURRECT_REQUEST`), the RESURRECT popup's data.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ResurrectOffer {
    /// Answered in `CMSG_RESURRECT_RESPONSE`.
    pub(crate) caster: u64,
    /// Empty on the wire for a player caster, resolved through the name cache.
    pub(crate) name: String,
    /// Accepting applies resurrection sickness; picks the popup variant.
    pub(crate) sickness: bool,
    /// The reclaim-delay gate still holds: RESURRECT, not RESURRECT_NO_TIMER.
    pub(crate) has_timer: bool,
}

/// The death state the descriptor does not carry, filled from the wire and cleared at session
/// teardown.
#[derive(Resource, Default)]
pub(crate) struct DeathNet {
    /// When the corpse becomes reclaimable: `SMSG_CORPSE_RECLAIM_DELAY` anchored at arrival.
    pub(crate) reclaim_at: Option<f64>,
    /// Bumped per `SMSG_CORPSE_RECLAIM_DELAY`: the 1.12 client's handler re-fires the range
    /// events on each.
    pub(crate) reclaim_generation: u32,
    /// The last `MSG_CORPSE_QUERY` answer; a not-found drops it.
    pub(crate) corpse: Option<CorpsePoint>,
    /// A pending resurrection offer; cleared when answered or when the popup times out.
    pub(crate) resurrect: Option<ResurrectOffer>,
    /// Bumped per `SMSG_RESURRECT_REQUEST`: the offer announces per message (`0x5e7bc0` through
    /// `0x5ded50`), because the stock UI can hide the popup without answering it.
    pub(crate) resurrect_generation: u32,
    /// Our streamed corpse object's guid, which `CMSG_RECLAIM_CORPSE` carries like the 1.12 client.
    pub(crate) corpse_guid: Option<u64>,
    /// The spirit healer awaiting the XP-loss answer; Accept sends `CMSG_SPIRIT_HEALER_ACTIVATE`.
    pub(crate) spirit_healer: Option<u64>,
    /// Bumped per ask: a Cancel clears nothing, so only a fresh ask re-shows the dialog.
    pub(crate) confirm_generation: u32,
    /// Water-walking is granted on our mover; mirrored, not applied: walking on the water surface
    /// is not built.
    #[allow(dead_code)]
    pub(crate) water_walk: bool,
}

impl DeathNet {
    /// Arm the spirit healer's XP-loss question. The reference raises it on the click with no
    /// packet (`0x5df730`), and vmangos also pushes `SMSG_SPIRIT_HEALER_CONFIRM`; both come here.
    pub(crate) fn ask_spirit_healer(&mut self, npc: u64) {
        self.spirit_healer = Some(npc);
        self.confirm_generation = self.confirm_generation.wrapping_add(1);
    }
}

/// The feed's memory in two scopes: the body's state survives a `/reload`, as the reference's
/// engine-side death mirror (`[0xb4e340]`) does, while what the VM was told dies with the VM.
#[derive(Resource, Default)]
struct DeathFeedState {
    /// World-scoped: the last `(guid, dead, ghost)` the self body showed.
    mirror: Option<(u64, bool, bool)>,
    /// World-scoped: the death edge, anchoring the release window. The wire carries only the
    /// timer bit; the 1.12 client arms `now + 360000 ms` at its own death edge (`0x5e9a30`).
    died_at: Option<f64>,
    /// VM-scoped: the announce latches.
    vm: crate::ui_script::VmMemo<DeathAnnounced>,
}

/// What the live VM has been told; a first snapshot counts as an edge, so logging in or
/// reloading dead brings the popup up.
#[derive(Default)]
struct DeathAnnounced {
    /// The last `(guid, dead, ghost)` this VM was given an event for.
    last: Option<(u64, bool, bool)>,
    /// The last corpse-range verdict announced.
    corpse_range: Option<CorpseRange>,
    /// A fresh reclaim delay re-fires the range events, as the 1.12 client does (`0x4962d0`),
    /// so RECOVER_CORPSE re-shows with the new delay.
    reclaim_generation: u32,
    /// Held back while a player caster's name is still resolving.
    offer_generation: u32,
    /// A fresh ask re-fires `CONFIRM_XP_LOSS`.
    confirm_generation: u32,
}

/// The corpse-range verdicts the reference's events distinguish.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CorpseRange {
    Out,
    In,
    InInstance,
}

/// The forced-release window, vmangos `CORPSE_REPOP_TIME` (`Player.h:71`); never on the wire.
const RELEASE_WINDOW_SECS: f64 = 360.0;

/// The corpse-range dialog radius, 40 yd inclusive: the 1.12 client compares d² ≤ `[0xb4e2ac]`
/// (1600.0) against the corpse-query position. The server's reclaim gate,
/// `CORPSE_RECLAIM_RADIUS` 39 (`Corpse.h:40`), sits 1 yd inside it.
const CORPSE_RANGE_SQ: f32 = 40.0 * 40.0;

/// The spirit-healer dialog range: `CheckSpiritHealerDist` compares d² ≤ `[0xc4c28c]` (30.864).
const SPIRIT_HEALER_RANGE_SQ: f32 = 5.5556 * 5.5556;

/// Derive the death state from the self descriptor, push the snapshot, and fire the death events
/// on its edges.
fn feed_death(
    script: Option<NonSendMut<UiScript>>,
    self_q: Query<(&ObjectStore, &Transform), With<SelfPlayer>>,
    self_guid: Res<SelfGuid>,
    death_net: Res<DeathNet>,
    // Real time: `reclaim_at` is stamped on `Time<Real>` by the net path.
    time: Res<Time<Real>>,
    mut feed: ResMut<DeathFeedState>,
    names: Res<crate::names::NameCache>,
    net: Res<NetCommands>,
    objects: Objects,
    transforms: Query<&Transform>,
    map: Option<Res<benilla_world::world_map::CurrentMap>>,
    status: Res<crate::net::NetStatus>,
    spells: Option<Res<Spells>>,
    items: Res<crate::items::Items>,
) {
    // A disconnect keeps the self avatar with its frozen descriptor; only a live session's
    // descriptor may drive the state machine.
    if !status.connected {
        return;
    }
    let Some(mut script) = script else {
        return;
    };
    let Some(guid) = self_guid.0 else {
        return;
    };
    let Ok((store, self_t)) = self_q.single() else {
        return;
    };
    let now = time.elapsed_secs_f64();
    let dead = store.0.unit_is_dead();
    let ghost = store.0.player_is_ghost();

    // ── The snapshot, set before the events fire so their handlers read current values ──
    let release_remaining = if store.0.player_release_timer_running() {
        feed.died_at
            .map(|t| ((t + RELEASE_WINDOW_SECS - now).max(0.0)) as f32)
    } else {
        None // GetReleaseTimeRemaining() == -1, the no-timer DEATH text
    };
    let recovery_delay = death_net
        .reclaim_at
        .map_or(0.0, |at| (at - now).max(0.0) as f32);
    let spirit_healer_in_range = death_net.spirit_healer.is_some_and(|npc| {
        objects
            .entity(npc)
            .and_then(|e| transforms.get(e).ok())
            .is_some_and(|t| {
                t.translation.distance_squared(self_t.translation) <= SPIRIT_HEALER_RANGE_SQ
            })
    });
    // Resolved first: the lookup borrows the VM that `set_death` needs mutably.
    let sickness = sickness_duration(store.0.unit_level().unwrap_or(0), &|key: &str| {
        script
            .lua()
            .globals()
            .get::<String>(key)
            .ok()
            .filter(|t| !t.is_empty())
    });
    script.set_death(DeathUiState {
        release_remaining,
        recovery_delay,
        resurrect_sickness: death_net.resurrect.as_ref().is_some_and(|o| o.sickness),
        resurrect_has_timer: death_net.resurrect.as_ref().is_some_and(|o| o.has_timer),
        spirit_healer_in_range,
        sickness_duration: sickness,
        // `HasSoulstone()`; the dead gate is first, so the inventory walk runs only while dead.
        self_res_label: resolve_self_res(&store.0, &objects, &items, spells.as_deref(), &net)
            .map(|r| r.label().to_owned()),
    });

    // ── The world edges: the release window and the corpse asks, which a `/reload` must not
    // repeat.
    let prev = feed.mirror.filter(|&(g, ..)| g == guid);
    feed.mirror = Some((guid, dead, ghost));
    match (prev, dead, ghost) {
        // Alive to dead, or logging in dead: arm the release window.
        (Some((_, false, false)) | None, true, false) => {
            feed.died_at = Some(now);
        }
        // Dead to ghost (released), or to alive (a pre-release res).
        (Some((_, true, false)), d, g) if g || !d => {
            feed.died_at = None;
            if g {
                // Now a ghost: ask where the corpse is.
                let _ = net.0.send(ClientCommand::CorpseQuery);
            }
        }
        // Ghost to alive: reclaim, spirit healer or an accepted res.
        (Some((_, _, true)), false, false) => {
            feed.died_at = None;
            // The 1.12 client re-queries on the ghost-bit edge (`0x5ee990`), and the not-found
            // answer drops the map markers; the server's own push is sent only with a looter
            // (`Map.cpp:3617-3629`).
            let _ = net.0.send(ClientCommand::CorpseQuery);
        }
        // Logging in a ghost: no edge, just the corpse ask.
        (None, _, true) => {
            let _ = net.0.send(ClientCommand::CorpseQuery);
        }
        _ => {}
    }

    // ── The VM edges: a fresh VM's memo is empty, so reloading dead re-fires PLAYER_DEAD.
    let memo = feed.vm.get(&script);
    let prev = memo.last.filter(|&(g, ..)| g == guid);
    memo.last = Some((guid, dead, ghost));
    match (prev, dead, ghost) {
        (Some((_, false, false)) | None, true, false) => {
            script.fire_event("PLAYER_DEAD", vec![]);
        }
        (Some((_, true, false)), d, g) if g || !d => {
            script.fire_event("PLAYER_ALIVE", vec![]);
        }
        (Some((_, _, true)), false, false) => {
            script.fire_event("PLAYER_UNGHOST", vec![]);
        }
        _ => {}
    }

    // ── The offer and confirm announcements ──
    if let Some(name) = offer_announcement(memo, &death_net, store.0.is_dead_or_ghost(), |offer| {
        // A player caster's name is empty on the wire; hold the popup until the cache resolves it.
        if offer.name.is_empty() {
            names.resolve(offer.caster, &net).map(str::to_owned)
        } else {
            Some(offer.name.clone())
        }
    }) {
        script.fire_event("RESURRECT_REQUEST", vec![ScriptValue::Str(name)]);
    }
    if death_net.spirit_healer.is_some() && memo.confirm_generation != death_net.confirm_generation
    {
        memo.confirm_generation = death_net.confirm_generation;
        // `arg1` is the XP the res costs: `PLAYER_NEXT_LEVEL_XP` × 0.05, truncated (`0x5df837`),
        // and 0 without the field, as `0x5df806` pushes 0 for a non-local object. No stock
        // FrameXML reads it; `UIParent.lua:399` asks `GetResSicknessDuration()`.
        let xp_cost = store
            .0
            .player_next_level_xp()
            .map_or(0, |next| (f64::from(next) * 0.05) as i64);
        script.fire_event("CONFIRM_XP_LOSS", vec![ScriptValue::Int(xp_cost)]);
    }

    // ── The corpse-run range gate: only a ghost, against the query's display position; the
    // coordinate transform is isometric, so the Bevy-space compare is in yards.
    let range = if ghost {
        death_net.corpse.map(|cp| {
            // Within range on the display map: IN_RANGE when it is the corpse's map, else
            // IN_INSTANCE, at a dungeon's entrance (`0x492130`, `0x4920f0`).
            let on_display_map = map
                .as_ref()
                .is_some_and(|m| m.0 == u32::try_from(cp.display_map).unwrap_or(u32::MAX));
            let near = on_display_map
                && wow_to_bevy(cp.position).distance_squared(self_t.translation) <= CORPSE_RANGE_SQ;
            let same_map = u32::try_from(cp.display_map) == Ok(cp.corpse_map);
            match (near, same_map) {
                (false, _) => CorpseRange::Out,
                (true, true) => CorpseRange::In,
                (true, false) => CorpseRange::InInstance,
            }
        })
    } else {
        None
    };
    if memo.reclaim_generation != death_net.reclaim_generation {
        memo.reclaim_generation = death_net.reclaim_generation;
        memo.corpse_range = None; // re-announce whatever holds now
    }
    if range != memo.corpse_range {
        match range {
            Some(CorpseRange::In) => script.fire_event("CORPSE_IN_RANGE", vec![]),
            Some(CorpseRange::InInstance) => script.fire_event("CORPSE_IN_INSTANCE", vec![]),
            // Leaving range and the ghost state ending both fire the one hide event.
            Some(CorpseRange::Out) | None if memo.corpse_range.is_some() => {
                script.fire_event("CORPSE_OUT_OF_RANGE", vec![]);
            }
            _ => {}
        }
        memo.corpse_range = range;
    }
}

/// `SPELL_EFFECT_SELF_RESURRECT`, the `Spell.dbc` effect the item leg scans for (`0x5ed650`).
const SPELL_EFFECT_SELF_RESURRECT: u32 = 94;

/// What `UseSoulstone()` (`0x48ad70`) would spend now; resolved app-side, where the spell catalog
/// and item cache live.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum SelfRes {
    /// `PLAYER_SELF_RES_SPELL` is non-zero: send `CMSG_SELF_RES` and the server casts it.
    Spell {
        #[allow(dead_code)] // the server reads its own field
        spell: u32,
        label: String,
    },
    /// The field is zero but a carried item's on-use spell self-resurrects: use the item, named
    /// for the item.
    Item {
        bag_index: u8,
        slot: u8,
        guid: u64,
        entry: u32,
        label: String,
    },
}

impl SelfRes {
    /// What `HasSoulstone()` returns.
    fn label(&self) -> &str {
        match self {
            Self::Spell { label, .. } | Self::Item { label, .. } => label,
        }
    }
}

/// `HasSoulstone()`'s answer and `UseSoulstone()`'s routing, one predicate in the 1.12 client
/// (`0x48ac80`, `0x48ad70`), with its gates in its order:
///
/// 1. Dead: nil while `UNIT_FIELD_HEALTH` is positive; a released ghost has health 1.
/// 2. The field: non-zero is the spell leg, named from `Spell.dbc`, `"UNKNOWN"` when unresolved.
/// 3. On a zero field, the carried inventory at the walker's default mask (`0x47`) for an item
///    whose on-use spell carries [`SPELL_EFFECT_SELF_RESURRECT`].
///
/// An item template still in flight reads as no self-res until it lands.
fn resolve_self_res(
    store: &benilla_protocol::ObjectFields,
    objects: &Objects,
    items: &crate::items::Items,
    spells: Option<&Spells>,
    commands: &NetCommands,
) -> Option<SelfRes> {
    if !store.unit_is_dead() {
        return None;
    }
    if let Some(spell) = store.player_self_res_spell() {
        let label = spells
            .and_then(|s| s.catalog.get(spell))
            .map_or_else(|| "UNKNOWN".to_owned(), |d| d.name.clone());
        return Some(SelfRes::Spell { spell, label });
    }
    let slots = crate::ui_items::collect_inventory(
        store,
        objects,
        crate::ui_items::InventoryScope::DEFAULT,
    );
    slots.into_iter().find_map(|(bag_index, slot, guid)| {
        let entry = objects.object(guid)?.object_entry()?;
        let t = items.template(entry, guid, commands)?;
        let self_res = t.spells.iter().any(|sp| {
            sp.trigger == 0
                && spells.is_some_and(|c| {
                    c.catalog
                        .get(sp.spell_id)
                        .is_some_and(|d| d.effects.contains(&SPELL_EFFECT_SELF_RESURRECT))
                })
        });
        self_res.then(|| SelfRes::Item {
            bag_index,
            slot,
            guid,
            entry,
            label: t.name.clone(),
        })
    })
}

/// The sickness a spirit-healer res applies at `level`, per vmangos `Player::ResurrectPlayer`:
/// none below 11, `level - 10` minutes through 19, 10 minutes from 20. Worded with `GENERIC_MIN`,
/// the minute arm of the reference's formatter (`0x51a3a0` into `0x52fa50`).
///
/// Deviation: the minutes come from the server's table, not from spell 15007's `Spell.dbc`
/// duration, because the aura the server applies is what the dialog must warn about.
fn sickness_duration(level: u32, get: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    let minutes = match level {
        0..=10 => return None,
        11..=19 => level - 10,
        _ => 10,
    };
    // `GetText(token, nil, ordinal)`'s plural pick, through the shared primitive.
    let template = benilla_ui::strings::plural("GENERIC_MIN", Some(minutes), get)?;
    Some(benilla_ui::strings::fill(
        &template,
        &[benilla_ui::strings::Arg::D(i64::from(minutes))],
    ))
}

/// Drive the FFXDeath screen pass off `PLAYER_FLAGS_GHOST`, instant both ways; in the reference
/// the flag's watcher `0x5ee990` also drives the death light and ambience.
fn drive_death_look(
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    mut ffx: ResMut<benilla_world::ffx_glow::FfxDeathFade>,
) {
    let ghost = ghost_probe()
        .unwrap_or_else(|| self_q.single().is_ok_and(|store| store.0.player_is_ghost()));
    let target = if ghost { 1.0 } else { 0.0 };
    if ffx.0 != target {
        ffx.0 = target;
    }
}

/// `WOW_GHOST_PROBE=1|0`: pin the ghost-world look on or off without dying: the screen pass,
/// the death light (`LightParams` slot 4) and the DeathClouds sky. The death events still follow
/// the wire.
pub(crate) fn ghost_probe() -> Option<bool> {
    static PROBE: std::sync::OnceLock<Option<bool>> = std::sync::OnceLock::new();
    *PROBE.get_or_init(|| match std::env::var("WOW_GHOST_PROBE").ok()?.trim() {
        "1" | "true" => Some(true),
        "0" | "false" => Some(false),
        _ => None,
    })
}

/// Map the queued Lua death intents onto the wire, after `UiInput` so a click drains that frame.
fn drain_death(
    script: Option<NonSendMut<UiScript>>,
    mut death_net: ResMut<DeathNet>,
    net: Res<NetCommands>,
    targeting: crate::spell::cast_target::CastTargeting,
    mut ladder: crate::spell::CastLadder,
    mut ui_errors: ResMut<crate::ui_action::UiErrorKeys>,
    // The soulstone's item leg is an item use (`0x5d8d00`), so it carries the bind gate.
    mut gate: crate::ui_bind_confirm::BindGate,
) {
    let Some(mut script) = script else {
        return;
    };
    for action in script.take_death_actions() {
        let _ = match action {
            DeathAction::Repop => net.0.send(ClientCommand::RepopRequest),
            // Else 0: vmangos resolves the corpse through the player and never reads the guid
            // (`MiscHandler.cpp:573-603`).
            DeathAction::RetrieveCorpse => net.0.send(ClientCommand::ReclaimCorpse {
                corpse: death_net.corpse_guid.unwrap_or(0),
            }),
            DeathAction::AcceptResurrect | DeathAction::DeclineResurrect => {
                let accept = action == DeathAction::AcceptResurrect;
                match death_net.resurrect.take() {
                    Some(offer) => net.0.send(ClientCommand::ResurrectResponse {
                        caster: offer.caster,
                        accept,
                    }),
                    None => Ok(()), // the offer timed out; nothing to answer
                }
            }
            DeathAction::AcceptXpLoss => match death_net.spirit_healer.take() {
                Some(npc) => net.0.send(ClientCommand::SpiritHealerActivate { npc }),
                None => Ok(()),
            },
            // Re-resolved at click time, as the reference's `OnCancel` calls `HasSoulstone()`
            // again; nothing is cleared here, the server zeroes `PLAYER_SELF_RES_SPELL`.
            DeathAction::UseSoulstone => {
                let store = targeting.self_store.single().ok().map(|s| s.0.clone());
                match store.as_ref().and_then(|store| {
                    resolve_self_res(
                        store,
                        &ladder.objects,
                        &ladder.items,
                        ladder.spells.as_deref(),
                        &ladder.commands,
                    )
                }) {
                    Some(SelfRes::Spell { .. }) => net.0.send(ClientCommand::SelfRes),
                    // An ordinary item use, `0x5d8d00`, like every bag click.
                    Some(SelfRes::Item {
                        bag_index,
                        slot,
                        guid,
                        entry,
                        ..
                    }) => {
                        let t = ladder
                            .items
                            .template(entry, guid, &ladder.commands)
                            .cloned();
                        let it = crate::ui_items::ItemUse {
                            guid: Some(guid),
                            start_quest: t.as_ref().map_or(0, |t| t.start_quest),
                            bag_index,
                            slot,
                            entry,
                            spell_index: t.as_ref().and_then(|t| t.use_spell_index()).unwrap_or(0),
                            use_spell: t.as_ref().and_then(|t| t.use_spell).map(|u| u.spell_id),
                            on_object: None,
                            is_charter: false,
                        };
                        crate::ui_items::send_item_use(
                            it,
                            &targeting.context(),
                            &mut ladder,
                            &mut script,
                            &mut gate,
                            false,
                            &mut ui_errors,
                        );
                        Ok(())
                    }
                    // Nothing to spend: the reference returns silently.
                    None => Ok(()),
                }
            }
        };
    }
}

/// Clear [`DeathFeedState`]'s world scope on `DisconnectedMessage`, the edge [`DeathNet`] resets
/// on: a relog as a ghost must find no mirror, or the corpse query is never re-sent.
fn end_session_death_feed(
    mut msgs: MessageReader<crate::net::DisconnectedMessage>,
    mut feed: ResMut<DeathFeedState>,
) {
    if msgs.read().next().is_some() {
        *feed = DeathFeedState::default();
    }
}

/// The death stores, the feed into the UI, the intent drain and the root and water-walk messages.
pub(crate) struct DeathPlugin;

impl Plugin for DeathPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<DeathNet>()
            .init_resource::<DeathFeedState>()
            .add_systems(
                Update,
                (
                    feed_death.in_set(UiFeed),
                    drain_death.after(UiInput),
                    drive_death_look,
                    // Before the feed, so the frame a session ends carries no memory of it.
                    end_session_death_feed.before(feed_death),
                ),
            );
    }
}

/// The name `RESURRECT_REQUEST` fires with this frame, if any. Per message, and only while dead
/// or a ghost (`0x5ded50` gated on `0x605f30`); an unresolved name holds without consuming it.
fn offer_announcement(
    memo: &mut DeathAnnounced,
    death_net: &DeathNet,
    dead_or_ghost: bool,
    name_of: impl FnOnce(&ResurrectOffer) -> Option<String>,
) -> Option<String> {
    if !dead_or_ghost || memo.offer_generation == death_net.resurrect_generation {
        return None;
    }
    let name = name_of(death_net.resurrect.as_ref()?)?;
    memo.offer_generation = death_net.resurrect_generation;
    Some(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    fn offer(caster: u64, name: &str) -> ResurrectOffer {
        ResurrectOffer {
            caster,
            name: name.into(),
            sickness: false,
            has_timer: true,
        }
    }

    /// PLAYER_UNGHOST hides the popup with `StaticPopup_Hide`, which runs no `OnCancel`, so no
    /// Decline clears `resurrect`.
    #[test]
    fn every_resurrect_message_announces_while_dead_and_never_while_alive() {
        let mut net = DeathNet::default();
        let mut memo = DeathAnnounced::default();
        let name = |o: &ResurrectOffer| Some(o.name.clone());

        net.resurrect = Some(offer(1, "Healer"));
        net.resurrect_generation += 1;
        assert_eq!(
            offer_announcement(&mut memo, &net, true, name).as_deref(),
            Some("Healer")
        );
        assert_eq!(
            offer_announcement(&mut memo, &net, true, name),
            None,
            "once per message"
        );

        // Alive with the offer unanswered: a fresh VM must not announce it.
        let mut reloaded = DeathAnnounced::default();
        assert_eq!(offer_announcement(&mut reloaded, &net, false, name), None);

        // Dead again, a new priest offers.
        net.resurrect = Some(offer(2, "Ptwo"));
        net.resurrect_generation += 1;
        assert_eq!(
            offer_announcement(&mut memo, &net, true, name).as_deref(),
            Some("Ptwo"),
            "the second offer announces"
        );

        // A name still resolving holds the announcement without consuming it.
        net.resurrect_generation += 1;
        assert_eq!(offer_announcement(&mut memo, &net, true, |_| None), None);
        assert_eq!(
            offer_announcement(&mut memo, &net, true, name).as_deref(),
            Some("Ptwo")
        );
    }

    /// A `/reload` must not restart the release window; a relog must clear it.
    #[test]
    fn the_session_edge_clears_the_world_scoped_death_memory_and_nothing_else_does() {
        let mut app = App::new();
        app.add_message::<crate::net::DisconnectedMessage>()
            .insert_resource(DeathFeedState {
                mirror: Some((0x1234, true, true)),
                died_at: Some(12.0),
                vm: crate::ui_script::VmMemo::default(),
            });

        // No session edge: the memory stands.
        app.world_mut()
            .run_system_once(end_session_death_feed)
            .expect("the teardown runs");
        assert_eq!(
            app.world().resource::<DeathFeedState>().mirror,
            Some((0x1234, true, true)),
            "nothing but the session edge may clear the body's own state machine"
        );

        // A `/logout` is a session edge although `session_over` is false for it.
        app.world_mut()
            .write_message(crate::net::DisconnectedMessage::new(
                "logged out".into(),
                benilla_protocol::SessionEnd::LoggedOut,
            ));
        app.world_mut()
            .run_system_once(end_session_death_feed)
            .expect("the teardown runs");
        let feed = app.world().resource::<DeathFeedState>();
        assert!(
            feed.mirror.is_none(),
            "a relog must find no mirror, or the login-while-ghost arm cannot fire"
        );
        assert!(
            feed.died_at.is_none(),
            "the release window is the dead session's, and re-arms at the next login"
        );
    }
}

#[cfg(test)]
mod self_res_tests {
    use std::collections::HashMap;

    use benilla_formats::{SpellCatalog, SpellDisplay};
    use benilla_protocol::messages::{ItemInfo, ItemSpellEntry};
    use benilla_protocol::ObjectFields;

    use super::{resolve_self_res, SelfRes, SPELL_EFFECT_SELF_RESURRECT};
    use crate::items::Items;
    use crate::net::{ClientCommand, NetCommands};
    use crate::ui_action::Spells;

    /// Reincarnation's effect spell, what the server writes into the field.
    const REINCARNATION: u32 = 21169;
    /// `PLAYER_SELF_RES_SPELL`.
    const F_SELF_RES: u16 = 1224;
    /// `UNIT_FIELD_HEALTH` and `UNIT_FIELD_MAXHEALTH`: `unit_is_dead()` also needs a non-zero max.
    const F_HEALTH: u16 = 22;
    const F_MAXHEALTH: u16 = 28;
    /// `PLAYER_FIELD_PACK_SLOT_1`: the backpack's 16 slots, two dwords per guid.
    const F_PACK_1: u16 = 532;

    fn commands() -> (NetCommands, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        (NetCommands(tx), rx)
    }

    fn catalog(pairs: impl IntoIterator<Item = (u32, SpellDisplay)>) -> Spells {
        Spells {
            catalog: SpellCatalog::from_displays(pairs.into_iter().collect::<HashMap<_, _>>()),
            ..Spells::empty_for_tests()
        }
    }

    fn named(name: &str, effects: [u32; 3]) -> SpellDisplay {
        SpellDisplay {
            name: name.to_string(),
            effects,
            ..Default::default()
        }
    }

    /// `0x48acba`: nil while health is positive, including a released ghost's 1
    /// (`BuildPlayerRepop`).
    #[test]
    fn the_dead_gate_precedes_the_field() {
        let (net, _rx) = commands();
        let items = Items::default();
        // Nothing streamed: the spell leg answers before the inventory walk.
        let mut objs = crate::ui_items::TestObjects::new();
        let objects = objs.get();
        let spells = catalog([(REINCARNATION, named("Reincarnation", [94, 0, 0]))]);

        let alive = ObjectFields::from_pairs(&[
            (F_MAXHEALTH, 4000),
            (F_HEALTH, 4000),
            (F_SELF_RES, REINCARNATION),
        ]);
        assert_eq!(
            resolve_self_res(&alive, &objects, &items, Some(&spells), &net),
            None,
            "alive with a self-res owed: nil"
        );

        let ghost = ObjectFields::from_pairs(&[
            (F_MAXHEALTH, 4000),
            (F_HEALTH, 1),
            (F_SELF_RES, REINCARNATION),
        ]);
        assert_eq!(
            resolve_self_res(&ghost, &objects, &items, Some(&spells), &net),
            None,
            "a released ghost still holds the field, and still answers nil"
        );

        let dead = ObjectFields::from_pairs(&[
            (F_MAXHEALTH, 4000),
            (F_HEALTH, 0),
            (F_SELF_RES, REINCARNATION),
        ]);
        assert_eq!(
            resolve_self_res(&dead, &objects, &items, Some(&spells), &net),
            Some(SelfRes::Spell {
                spell: REINCARNATION,
                label: "Reincarnation".into(),
            })
        );
    }

    /// The reference pushes the literal (`0x48ad1d`).
    #[test]
    fn an_unresolvable_spell_id_reads_unknown_not_nil() {
        let (net, _rx) = commands();
        let items = Items::default();
        let mut objs = crate::ui_items::TestObjects::new();
        let objects = objs.get();
        let dead =
            ObjectFields::from_pairs(&[(F_MAXHEALTH, 4000), (F_HEALTH, 0), (F_SELF_RES, 999_999)]);
        for spells in [None, Some(catalog([]))] {
            assert_eq!(
                resolve_self_res(&dead, &objects, &items, spells.as_ref(), &net),
                Some(SelfRes::Spell {
                    spell: 999_999,
                    label: "UNKNOWN".into(),
                }),
                "no catalog and an empty catalog are the same miss"
            );
        }
    }

    /// The item leg (`0x5ed630`); the backpack holds a self-res on an equip trigger, an unrelated
    /// on-use spell, and the real one.
    #[test]
    fn a_zero_field_falls_through_to_a_carried_item() {
        let (net, _rx) = commands();
        let mut items = Items::default();
        let mut objs = crate::ui_items::TestObjects::new();

        let item = |name: &str, spells: Vec<ItemSpellEntry>| ItemInfo {
            spells,
            ..crate::items::test_template(name)
        };
        let block = |spell_id: u32, trigger: u32| ItemSpellEntry {
            index: 0,
            spell_id,
            trigger,
            charges: 0,
            cooldown_ms: -1,
            category: 0,
            category_cooldown_ms: -1,
        };
        // 3026 "Use Soulstone" is the real effect-94 spell; 439 "Healing Potion" is not.
        items.insert_template(10, Some(item("Healthstone", vec![block(439, 0)])));
        items.insert_template(11, Some(item("Worn Trinket", vec![block(3026, 1)])));
        items.insert_template(
            12,
            Some(item("Ankh of Reincarnation", vec![block(3026, 0)])),
        );
        for (guid, entry) in [(0xA0_u64, 10_u32), (0xA1, 11), (0xA2, 12)] {
            objs.spawn(guid, ObjectFields::from_pairs(&[(3, entry)]));
        }
        let objects = objs.get();
        let spells = catalog([
            (
                3026,
                named("Use Soulstone", [SPELL_EFFECT_SELF_RESURRECT, 0, 0]),
            ),
            (439, named("Healing Potion", [6, 0, 0])),
        ]);

        // Backpack slots 23, 24 and 25 hold the three, in that order.
        let mut pairs = vec![(F_MAXHEALTH, 4000), (F_HEALTH, 0)];
        for (i, guid) in [0xA0_u64, 0xA1, 0xA2].iter().enumerate() {
            let f = F_PACK_1 + 2 * i as u16;
            pairs.push((f, (*guid & 0xffff_ffff) as u32));
            pairs.push((f + 1, (*guid >> 32) as u32));
        }
        let dead = ObjectFields::from_pairs(&pairs);

        match resolve_self_res(&dead, &objects, &items, Some(&spells), &net) {
            Some(SelfRes::Item { entry, label, .. }) => {
                assert_eq!(
                    entry, 12,
                    "the EQUIP-trigger copy and the potion are skipped"
                );
                assert_eq!(label, "Ankh of Reincarnation", "the ITEM names the button");
            }
            other => panic!("expected the item leg, got {other:?}"),
        }

        // Without it the answer is nil, not the spell leg's "UNKNOWN".
        let bare = ObjectFields::from_pairs(&[(F_MAXHEALTH, 4000), (F_HEALTH, 0)]);
        assert_eq!(
            resolve_self_res(&bare, &objects, &items, Some(&spells), &net),
            None
        );
    }
}
