//! NPC greetings: the select greeting and the interaction-window hello and goodbye.
//!
//! Both read `CreatureDisplayInfo.field[11]` NPCSoundID → `NPCSounds {hello +0x4, goodbye +0x8,
//! pissed +0xc}` → a `SoundEntries` kit, played 3D at the NPC, and both respect the unit's channel
//! latch `[unit+0xb1c]`: a unit refuses a new line while its last one sounds
//! ([`kit::source_playing`]).
//!
//! ## The select greeting (`0x60c270`, left-click)
//!
//! Left-clicking a unit ([`crate::target::select_on_click`], the reference's `0x4925d0`, before
//! `SetTarget`) plays the cycling welcome ([`NpcGreetingRequest`]). A counter per NPC
//! ([`GreetSeq`], `[0xc4d970]`, reset when the greeted NPC changes) steps `0x623910`
//! ([`greet_line`]): five hello takes, then the pissed kit's takes in order, then a wrap.
//!
//! Gates stand ahead of the select play, in the reference's order. The unit's channel latch comes
//! first (`0x60c28c`), then a dead unit (`0x60c2a5`). The reaction pair (`0x6061e0` both ways,
//! each at least neutral, `0x60c2d9`-`0x60c2f2`) is between the unit and the active mover, the
//! object at `[0xc4da98]` that only `SetActiveMover` writes (`0x6006e0`), so a possessed creature
//! or an Eye of Kilrogg stands in for the player there; none streamed bails. The dead or ghost
//! gate (`0x605f30` at `0x60c31d`) is on the local player, unresolved or not. Only a click that
//! passes all of them re-tracks the NPC (`0x60c326`) and resets the sequence.
//!
//! ## The window hello and goodbye (`0x60c3b0`, right-click)
//!
//! The interaction latch speaks them ([`LatchVocal`]), not the windows: registering on it
//! (`0x4930d0`, 14 call sites, none a mouseover) for a different unit plays the NPC's hello once
//! with a free-picked take (`0x60c464`), the same `+0x4` slot, and releasing it to nothing
//! through `0x4931c0` plays the goodbye (`+0x8`), so every window on the latch greets alike. A
//! swap to another NPC's window plays the new hello and no goodbye for the old (`0x4930fd`:
//! `dl = (new_guid != 0)`), and a walk-away close (`0x493305`) plays none. The gate is a
//! non-player unit (`0x493159`, `0x493182`, `0x4931f8`-`0x493203`): a game object (a mailbox)
//! or a player speaks nothing. Every such call also zeroes the select greeting's tracked NPC
//! (`0x60c3d1`), so the next left-click starts its sequence over.

use bevy::prelude::*;

use benilla_formats::NpcGreetingCatalog;
use benilla_protocol::EntityKind;

use crate::net::{ActiveMover, GuidIndex, NetEntity, ObjectStore, Reputations, SelfPlayer};
use crate::target::{both_reactions_at_least_neutral, Factions};
use crate::ui_session::LatchVocal;
use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_world::schedule::WorldStage;
use benilla_world::view::WorldCamera;

use super::kit::{self, play_kit_ext, KitRef, Latch, PlayExtras, SoundCategory, SoundKits};
use super::{SoundConfig, SoundOutput};

/// Hello takes before the select greeter steps into the pissed kit (`0x62394a: cmp ebx,5`).
const HELLO_TAKES: usize = 5;

/// Plays an NPC's select greeting, sent by [`crate::target::select_on_click`] (`0x60c270`).
#[derive(Message, Clone, Copy)]
pub(crate) struct NpcGreetingRequest {
    pub(crate) npc: Entity,
}

/// The display→greeting catalog (`CreatureDisplayInfo.NPCSoundID` → `NPCSounds`).
#[derive(Resource)]
struct NpcGreetings(NpcGreetingCatalog);

/// The select greeting's counter `[0xc4d970]`, keyed by the tracked NPC `[0xc4d908]` and reset
/// when it changes; it advances only on a play that started (`0x60c28c`).
#[derive(Resource, Default)]
struct GreetSeq {
    tracked: Option<Entity>,
    seq: usize,
}

/// The line and take a select play produces (`0x623910`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GreetLine {
    /// HELLO (`+0x4`), free-pick take.
    Hello,
    /// PISSED (`+0xc`), take `variation`.
    Pissed(usize),
}

/// `0x623910`: [`HELLO_TAKES`] hello free-picks, then the pissed kit's takes in order, then a
/// hello that resets the counter. Returns the line and the next counter.
fn greet_line(seq: usize, pissed_variations: usize) -> (GreetLine, usize) {
    if seq < HELLO_TAKES {
        (GreetLine::Hello, seq + 1)
    } else {
        let p = seq - HELLO_TAKES;
        if p < pissed_variations {
            (GreetLine::Pissed(p), seq + 1)
        } else {
            // edi >= count: hello, variation -1, counter reset.
            (GreetLine::Hello, 0)
        }
    }
}

fn load_npc_greetings(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_npc_greeting_catalog(&mut chain)
    };
    match loaded {
        Ok(cat) => {
            info!("sound: {} NPC greeting rows", cat.len());
            commands.insert_resource(NpcGreetings(cat));
        }
        Err(e) => warn!("sound: NPC greetings failed to load: {e:#}"),
    }
}

/// Whether an object greets: a non-player unit (`0x493159`, `0x493182`, `0x4931f8`-`0x493203`).
/// A player, a game object such as a mailbox and a book have no NPC to speak.
fn speaks(kind: EntityKind) -> bool {
    matches!(kind, EntityKind::Unit)
}

/// A live non-player unit (type-mask bit 3 set, bit 4 clear) whose display has an NPCSoundID row.
fn resolve(
    npc: Entity,
    units: &Query<(&NetEntity, &Transform, Option<&ObjectStore>)>,
    greetings: &NpcGreetings,
) -> Option<(benilla_formats::NpcGreeting, Vec3)> {
    let (net, transform, store) = units.get(npc).ok()?;
    if !speaks(net.kind) {
        return None;
    }
    if store.is_some_and(|s| s.0.unit_is_dead()) {
        return None;
    }
    let row = *greetings.0.for_display(net.display_id?)?;
    Some((row, transform.translation))
}

/// The local player may hear an NPC: streamed, and neither dead nor a ghost (`0x605f30`). The gate
/// of the select greeting (`0x60c31d`) and of the window hello and goodbye (`0x60c401`).
fn player_may_hear(me: Option<&ObjectStore>) -> bool {
    me.is_some_and(|s| !s.0.is_dead_or_ghost())
}

/// The select greeting's two gates (`0x60c270`): both reaction directions between the unit and the
/// active mover at least neutral (`0x60c2d9`, `0x60c2ef`), none streamed bailing (`0x60c2d0`), then
/// [`player_may_hear`] on the local player (`0x60c31d`).
fn select_greeting_allowed(
    factions: Option<&Factions>,
    reputations: &Reputations,
    npc: Option<&ObjectStore>,
    mover: Option<&ObjectStore>,
    me: Option<&ObjectStore>,
) -> bool {
    mover.is_some()
        && both_reactions_at_least_neutral(factions, reputations, npc, mover)
        && player_may_hear(me)
}

/// The select greeting's gates and re-track, in the reference's order (`0x60c28c`-`0x60c33e`): a
/// sounding line (the latch) or a dead unit ends the click, then [`select_greeting_allowed`]'s
/// verdict, and only a click that passes all three tracks the NPC, resetting the sequence when it
/// is a different one. `true` means go on to pick the line.
fn begin_select(seq: &mut GreetSeq, npc: Entity, latched: bool, dead: bool, allowed: bool) -> bool {
    if latched || dead || !allowed {
        return false;
    }
    if seq.tracked != Some(npc) {
        *seq = GreetSeq {
            tracked: Some(npc),
            seq: 0,
        };
    }
    true
}

/// A window hello or goodbye forgets the tracked NPC (`0x60c3d1`, ahead of every gate after the
/// unit-type check), so the next click greeting starts over wherever it lands.
fn forget_tracked(seq: &mut GreetSeq) {
    seq.tracked = None;
}

/// Plays one line, kit 0 being none and `None` a free-picked take, gated on the unit's channel
/// latch (`0x60c28c`).
fn play_line(
    npc: Entity,
    kit_id: u32,
    variant: Option<usize>,
    pos: Vec3,
    listener: Vec3,
    kits: &mut SoundKits,
    assets: &WorldAssets,
    out: &mut SoundOutput,
    config: &SoundConfig,
) {
    if kit_id == 0 || kit::source_playing(out, npc) {
        return;
    }
    if let Err(e) = play_kit_ext(
        kits,
        assets,
        out,
        config,
        listener,
        KitRef::Id(kit_id),
        Some(pos),
        SoundCategory::Sfx,
        PlayExtras {
            variant,
            source: Some(npc),
            // The one play that takes `[unit+0xb1c]`; the unit's other sounds do not.
            latch: Latch::Greeting,
            ..default()
        },
    ) {
        warn!("npc vocal (kit {kit_id}): {e:#}");
    }
}

/// The select greeting.
fn play_select_greetings(
    mut reqs: MessageReader<NpcGreetingRequest>,
    mut seq: ResMut<GreetSeq>,
    units: Query<(&NetEntity, &Transform, Option<&ObjectStore>)>,
    self_store: Query<&ObjectStore, With<SelfPlayer>>,
    mover: Query<&ObjectStore, With<ActiveMover>>,
    factions: Option<Res<Factions>>,
    reputations: Res<Reputations>,
    greetings: Option<Res<NpcGreetings>>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    cam: Query<&Transform, (With<WorldCamera>, Without<NetEntity>)>,
) {
    if reqs.is_empty() {
        return;
    }
    let (Some(greetings), Some(mut kits), Some(assets)) = (greetings, kits, assets) else {
        reqs.clear();
        return;
    };
    let listener = cam.single().map(|t| t.translation).unwrap_or(Vec3::ZERO);

    for req in reqs.read() {
        let Ok((net, transform, npc_store)) = units.get(req.npc) else {
            continue;
        };
        if !speaks(net.kind) {
            continue;
        }
        let allowed = select_greeting_allowed(
            factions.as_deref(),
            &reputations,
            npc_store,
            mover.single().ok(),
            self_store.single().ok(),
        );
        let dead = npc_store.is_some_and(|s| s.0.unit_is_dead());
        let latched = kit::source_playing(&out, req.npc);
        if !begin_select(&mut seq, req.npc, latched, dead, allowed) {
            continue;
        }
        // A unit with no greeting row still took the track above; `0x623910` answers -1 for it.
        let Some(row) = net
            .display_id
            .and_then(|d| greetings.0.for_display(d))
            .copied()
        else {
            continue;
        };
        let pos = transform.translation;
        let (line, next) = greet_line(seq.seq, kits.variations(row.pissed));
        let (kit_id, variant) = match line {
            GreetLine::Hello => (row.hello, None),
            GreetLine::Pissed(p) => (row.pissed, Some(p)),
        };
        if kit_id == 0 {
            continue;
        }
        // Commit the counter only if the play started (`0x60c398`); the channel was idle, so a
        // live one now means it did.
        play_line(
            req.npc, kit_id, variant, pos, listener, &mut kits, &assets, &mut out, &config,
        );
        if kit::source_playing(&out, req.npc) {
            seq.seq = next;
        }
    }
}

/// The window hello and goodbye, from the latch's lines.
fn play_window_greetings(
    mut lines: MessageReader<LatchVocal>,
    index: Res<GuidIndex>,
    mut select_seq: ResMut<GreetSeq>,
    units: Query<(&NetEntity, &Transform, Option<&ObjectStore>)>,
    self_store: Query<&ObjectStore, With<SelfPlayer>>,
    greetings: Option<Res<NpcGreetings>>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    cam: Query<&Transform, (With<WorldCamera>, Without<NetEntity>)>,
) {
    if lines.is_empty() {
        return;
    }
    let (Some(greetings), Some(mut kits), Some(assets)) = (greetings, kits, assets) else {
        lines.clear();
        return;
    };
    let listener = cam.single().map(|t| t.translation).unwrap_or(Vec3::ZERO);

    for &line in lines.read() {
        let (guid, hello) = match line {
            LatchVocal::Hello(g) => (g, true),
            LatchVocal::Goodbye(g) => (g, false),
        };
        // A despawned NPC resolves nothing: the reference's teardown clear is silent (`0x605950`).
        let Some(&npc) = index.0.get(&guid) else {
            continue;
        };
        // The call zeroes the select greeting's tracked NPC for any unit, before the gates below.
        if units.get(npc).is_ok_and(|(net, ..)| speaks(net.kind)) {
            forget_tracked(&mut select_seq);
        }
        // No reaction gate here (`0x60c3b0`), only the player's: dead or a ghost hears nothing.
        if !player_may_hear(self_store.single().ok()) {
            continue;
        }
        let Some((row, pos)) = resolve(npc, &units, &greetings) else {
            continue;
        };
        let kit_id = if hello { row.hello } else { row.goodbye };
        // Both window lines free-pick their take (`variation = -1`).
        play_line(
            npc, kit_id, None, pos, listener, &mut kits, &assets, &mut out, &config,
        );
    }
}

/// A despawning unit stops its greeting (`0x5fbb6c`).
fn stop_on_despawn(mut removed: RemovedComponents<NetEntity>, mut out: NonSendMut<SoundOutput>) {
    for entity in removed.read() {
        kit::stop_source(&mut out, entity);
    }
}

/// Registration hook for [`super::SoundPlugin`].
pub(super) fn plugin(app: &mut App) {
    app.add_message::<NpcGreetingRequest>()
        .init_resource::<GreetSeq>()
        .add_systems(Startup, load_npc_greetings.after(AssetSet::Open))
        .add_systems(
            Update,
            (
                play_select_greetings,
                // Both write the tracked NPC; a click and a window in one frame take it in turn.
                play_window_greetings.after(play_select_greetings),
                stop_on_despawn,
            )
                .in_set(WorldStage::Present),
        );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_sequence_cycles_5_hello_then_pissed_then_wraps() {
        for s in 0..5 {
            assert_eq!(greet_line(s, 3), (GreetLine::Hello, s + 1));
        }
        assert_eq!(greet_line(5, 3), (GreetLine::Pissed(0), 6));
        assert_eq!(greet_line(6, 3), (GreetLine::Pissed(1), 7));
        assert_eq!(greet_line(7, 3), (GreetLine::Pissed(2), 8));
        assert_eq!(greet_line(8, 3), (GreetLine::Hello, 0));
        // An NPC with no pissed takes wraps straight after the five hellos.
        assert_eq!(greet_line(5, 0), (GreetLine::Hello, 0));
    }

    use benilla_protocol::field::{
        FIELD_PLAYER_FLAGS, FIELD_UNIT_FACTIONTEMPLATE, FIELD_UNIT_FLAGS, FIELD_UNIT_HEALTH,
        FIELD_UNIT_MAXHEALTH,
    };
    use benilla_protocol::ObjectFields;

    /// A player-controlled player on faction template 1 (Human), with the given health and
    /// `PLAYER_FLAGS`.
    fn player(health: u32, player_flags: u32) -> ObjectStore {
        ObjectStore(ObjectFields::from_pairs(&[
            (FIELD_UNIT_FACTIONTEMPLATE, 1),
            (FIELD_UNIT_FLAGS, 1 << 3),
            (FIELD_UNIT_HEALTH, health),
            (FIELD_UNIT_MAXHEALTH, 100),
            (FIELD_PLAYER_FLAGS, player_flags),
        ]))
    }

    /// `0x605f30` over the local player gates both the select greeting (`0x60c31d`) and the window
    /// hello and goodbye (`0x60c401`): a corpse (health 0) and a ghost (health 1, the flag) are
    /// silent, and so is a player that has not streamed in (`0x60c315`).
    #[test]
    fn a_dead_or_ghost_or_absent_player_hears_nothing() {
        assert!(player_may_hear(Some(&player(100, 0))), "alive hears");
        assert!(!player_may_hear(Some(&player(0, 0))), "a corpse does not");
        assert!(!player_may_hear(Some(&player(1, 0x10))), "nor a ghost");
        assert!(!player_may_hear(None), "nor an unstreamed player");
    }

    /// The select greeting's reaction gate (`0x60c2d9`-`0x60c2f2`) on the real templates: Monster
    /// (14) is hostile to a Human, a Chicken (31) is neutral, and neutral passes (`cmp eax,3` /
    /// `jl`). A dead player is refused whatever the reactions.
    #[test]
    fn the_select_greeting_refuses_a_hostile_unit_and_a_dead_player() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let factions =
            Factions::from_catalog(benilla_formats::load_faction_catalog(&mut chain).expect("dbc"));
        let reps = Reputations::default();
        let unit = |template: u32| {
            ObjectStore(ObjectFields::from_pairs(&[(
                FIELD_UNIT_FACTIONTEMPLATE,
                template,
            )]))
        };
        let allowed = |npc: &ObjectStore, mover: Option<&ObjectStore>, me: &ObjectStore| {
            select_greeting_allowed(Some(&factions), &reps, Some(npc), mover, Some(me))
        };
        let me = player(100, 0);
        assert!(
            !allowed(&unit(14), Some(&me), &me),
            "a hostile monster is silent"
        );
        assert!(
            allowed(&unit(31), Some(&me), &me),
            "a neutral critter greets"
        );
        assert!(
            !allowed(&unit(31), Some(&me), &player(0, 0)),
            "a dead player hears no greeting"
        );
        assert!(
            !allowed(&unit(31), Some(&me), &player(1, 0x10)),
            "nor does a ghost"
        );

        // The reaction pair is with the active mover, not the player: a monster hostile to the
        // player greets the creature we possess, which is its own kind, and a friend of the player
        // is refused when the mover is hostile to it. No mover streamed, no greeting.
        let possessed = unit(14);
        assert!(allowed(&unit(14), Some(&possessed), &me));
        assert!(!allowed(&unit(14), None, &me));
    }

    /// The pair is both directions: with no catalog every faction leg reads neutral, so only a rung
    /// ahead of them (two free-for-all players, hostile both ways) is refused.
    #[test]
    fn the_reaction_gate_is_the_pair_not_one_direction() {
        let reps = Reputations::default();
        let ffa = ObjectStore(ObjectFields::from_pairs(&[
            (FIELD_UNIT_FLAGS, 1 << 3),
            (FIELD_PLAYER_FLAGS, 0x80),
        ]));
        let other = ObjectStore(ObjectFields::from_pairs(&[
            (FIELD_UNIT_FLAGS, 1 << 3),
            (FIELD_PLAYER_FLAGS, 0x80),
        ]));
        assert!(!select_greeting_allowed(
            None,
            &reps,
            Some(&other),
            Some(&ffa),
            Some(&ffa)
        ));
        let calm = player(100, 0);
        assert!(select_greeting_allowed(
            None,
            &reps,
            Some(&calm),
            Some(&calm),
            Some(&calm)
        ));
    }

    /// Only a non-player unit has a hello and a goodbye: a mailbox, a player and the rest do not.
    #[test]
    fn only_a_non_player_unit_speaks() {
        assert!(speaks(EntityKind::Unit));
        for kind in [
            EntityKind::Player,
            EntityKind::GameObject,
            EntityKind::DynamicObject,
            EntityKind::Corpse,
            EntityKind::Other,
        ] {
            assert!(!speaks(kind), "{kind:?}");
        }
    }

    fn two_npcs() -> (Entity, Entity) {
        let mut world = World::new();
        (world.spawn_empty().id(), world.spawn_empty().id())
    }

    /// The latch is the first gate (`0x60c28c`), ahead of the re-track and reset (`0x60c33e`): a
    /// click on A while A's line sounds changes nothing, so B's sequence carries on. Every gate
    /// that refuses leaves the track alone, and only a passing click on a new NPC resets.
    #[test]
    fn a_refused_click_leaves_the_tracked_npc_and_its_sequence_alone() {
        let (a, b) = two_npcs();
        let mut seq = GreetSeq {
            tracked: Some(b),
            seq: 3,
        };
        assert!(!begin_select(&mut seq, a, true, false, true), "latched");
        assert!(!begin_select(&mut seq, a, false, true, true), "dead");
        assert!(!begin_select(&mut seq, a, false, false, false), "refused");
        assert_eq!((seq.tracked, seq.seq), (Some(b), 3), "B continues");

        // The same NPC again keeps its count; a different one starts over.
        assert!(begin_select(&mut seq, b, false, false, true));
        assert_eq!(seq.seq, 3);
        assert!(begin_select(&mut seq, a, false, false, true));
        assert_eq!((seq.tracked, seq.seq), (Some(a), 0));
    }

    /// A window hello or goodbye zeroes the tracked NPC (`0x60c3d1`): left-click three times, open
    /// the window and close it, and the next left-click on the same NPC is the first hello again.
    #[test]
    fn a_window_greeting_restarts_the_click_sequence() {
        let (a, _) = two_npcs();
        let mut seq = GreetSeq::default();
        assert!(begin_select(&mut seq, a, false, false, true));
        seq.seq = 3;
        forget_tracked(&mut seq);
        assert!(begin_select(&mut seq, a, false, false, true));
        assert_eq!(seq.seq, 0, "the sequence starts over");
    }
}
