//! The **query** half of the overhead markers: when to ask the server what an NPC's `!`/`?` should
//! be — and, for a GameObject, when to ask a question whose answer is thrown away. The rendering
//! half is [`super`].
//!
//! **Both markers, not just the questgiver one**. The gold `!` and the flight
//! master's green `!` are the same `unit+0xb2c`: `0x607480` installs whichever arrives into that
//! one slot, `0x6073f0` zeroes it (along with `+0xcb8`), and `0x607380` — the per-unit path — tears
//! it down once and then re-issues *both* queries, `0x182` on `UNIT_NPC_FLAGS` bit 1 and `0x1aa` on
//! bit 3. One slot cannot have two lifetimes, so one system owns both asks; `ui_taxi` keeps the
//! resulting fact ([`crate::ui_taxi::FlightMasterStatus`]) and nothing else.
//!
//! **A GameObject is queried and never rendered**. The reference sends
//! `CMSG_QUESTGIVER_STATUS_QUERY` for a quest-flagged GameObject from both object sweeps, but its
//! answer handler `0x5dc9f0` resolves the GUID with typemask **8** (`0x468460` is a bitmask AND
//! against `OBJECT_FIELD_TYPE`), so a GameObject's `0x21` returns NULL and the packet dies at
//! `0x5dca2f` — and even if it did not, a GameObject has no `+0xcb8` status slot, no `+0xb2c`
//! marker slot, and (11 models out of ~1600, none of them a poster) no attachment 18 to hang a
//! marker from. So: **send the query, drop the answer, render nothing.**
//!
//! The server only ever *answers* `CMSG_QUESTGIVER_STATUS_QUERY` (vmangos `QuestHandler.cpp`) — it
//! never pushes — so every refresh point is the client's own to trigger, and a status that is never
//! re-asked for is a marker frozen at whatever it was when we first saw the NPC. That made this a
//! surprisingly deep fidelity question; the answer is decisions 0650 (the reference's trigger set,
//! byte-enumerated) and 0654 (what benilla implements of it), and it lives on [`query_statuses`].

use std::collections::HashMap;

use bevy::prelude::*;

use benilla_protocol::EntityKind;

use crate::net::{ClientCommand, GuidIndex, NetCommands, NetEntity, ObjectStore, SelfPlayer};
use crate::target::cursor_mode::go_reaction;
use crate::target::ring::Factions;
use crate::ui_quest::QuestGiver;

/// `UNIT_NPC_FLAGS` questgiver service bit (vanilla; `update_object.rs`'s accessor doc).
const NPC_FLAG_QUESTGIVER: u32 = 0x2;
/// `UNIT_NPC_FLAGS` flightmaster service bit — the other half of `0x607380`'s re-gate
/// (`0x6073de shr eax,3; test al,1`), and the gate on the only live `0x1aa` sender there is.
const NPC_FLAG_FLIGHTMASTER: u32 = 0x8;
/// `GAMEOBJECT_FLAGS` bit **2** (mask `0x4`) — the whole of the sweep's GameObject gate, and the
/// one place a GameObject GUID can reach this opcode at all.
///
/// Byte-pinned: `0x5eb0ef mov eax,[edx+0xc]; 0x5eb0f2 shr eax,0x2;
/// 0x5eb0f5 test al,1` — shift **2**, so mask `0x4`, not the `0x2` the unit
/// leg tests 25 bytes later. `[<GO block>+0xc]` is absolute field index 9 = `GAMEOBJECT_FLAGS`,
/// named from the binary's own UpdateField name table (`0x83b8a8` → the string at `0x83be84`).
/// The *name* `GO_FLAG_INTERACT_COND` is vmangos's, not the image's — but the bit is settled from
/// shipped content as well as from the branch: across the genuine 2005-06 sniff corpus, all 122
/// GameObjects the real client sent a status query for carry it, and none without it does, against
/// a 79.2 % base rate.
const GO_FLAG_INTERACT_COND: u32 = 0x4;

/// Ask the dialog status of every questgiver-flagged creature, and re-ask when the answer could
/// have changed. The server only ever *answers* queries (vmangos `QuestHandler.cpp`), so every
/// refresh point is the client's own to trigger — a status never re-asked for is a marker frozen
/// at whatever it was when we first saw the NPC.
///
/// The reference's refresh law is a **descriptor field watch**, not a hand-picked event list: it
/// registers handlers on specific self-player fields through `0x468070`, and the handler sweeps
/// every visible object (`0x5eb070` → `0x468380`, one query per object passing the questgiver
/// gate). The benilla side, and the still-open gaps, are decision 0650 (the byte-level trigger
/// set, superseding 0647's wrong "level is a deviation") and decision 0654 (what benilla
/// implements of it).
///
/// **The sweep** — every visible questgiver is re-asked when any *self* input to the server's
/// answer moves. The reference gets this from six descriptor watches plus four packet handlers;
/// ours is [`self_generation`] (the descriptor half, recomputed only when our store actually
/// changes) XOR [`QuestGiver::reask_epoch`] (the packet half, bumped from `net/apply`). A changed
/// generation clears the whole asked set, which is the sweep.
///
/// **Per unit** — the reference queries one unit from its create/init path (`0x607380`) and when
/// its own fields move: the questgiver bit (`0x60b490`), the flightmaster bit (`0x60b4c5`), and
/// anything that could change its *reaction* to us (`0x606f0a`). Ours keys the asked set on
/// [`unit_ask_key`], so any of those re-asks that one NPC; and the entry is dropped when the guid
/// leaves [`GuidIndex`], which is the create-path query (the reference caches the answer *on* the
/// unit at `unit+0xcb8`, so its cache dies with the object — ours is a map and needs the prune).
///
/// **Per GameObject — on a sweep, and ONLY on a sweep.** A quest-giving GameObject (a wanted
/// poster, a suspicious barrel) is a questgiver on the wire exactly as a creature is: the same
/// sweep callback tests typemask bit 5 and sends the same opcode for it (`0x5eb0a0` @ `0x5eb159`,
/// and its sibling `0x5eb3f0` @ `0x5eb456`), gated on [`GO_FLAG_INTERACT_COND`] and on the
/// GameObject's own reaction toward us being **> 1** ([`go_reaction`], the reference's `0x5f7fd0`).
/// The wire corpus shows it happening in 20 of 62 real sessions.
///
/// But there is **no bring-up query for a GameObject** — the contrast with units is exact and it
/// is the point (a GameObject GUID can reach this opcode through exactly two instructions in the
/// whole image and both are inside a sweep callback; the
/// `CGGameObject_C` ctor and its vtable slot 3 call no sender, where `CGUnit_C`'s slot 3 does). So
/// a GameObject is asked about only when a sweep happens to run while it is in view, and a player
/// who completes a quest elsewhere and then walks up to the turn-in object never asks about it at
/// all. That is a real, slightly surprising reference behaviour, and **a per-guid asked-set that
/// queries on first sight is a deviation, not a fix** — hence the `swept` gate below rather than
/// an [`unit_ask_key`] analogue. Nothing visible turns on any of this: the answer is dropped
/// ([`crate::net::apply`]'s own typemask gate), which is decision 1872's whole point.
///
/// **The teardown legs — and there are two of them, neither the one this file used to claim**.
/// `0x5eb0a0` is a **2×2 on (questgiver bit × reaction)**, and its
/// `0x5eb134` is a *convergent* block — reached both when the bit is clear (`0x5eb125 je`) and when
/// the bit is set but the reaction failed (`0x5eb132 jg` not taken). That convergence is why three
/// incompatible readings of this callback were live at once. The table:
///
/// | `UNIT_NPC_FLAGS & 0x2` | reaction **> 1** | reaction **≤ 1** |
/// |---|---|---|
/// | **set** | sends `0x182` (`0x5eb159`) | **tears down** (`0x5eb143`) |
/// | **clear** | **nothing** (`0x5eb15e`) | **tears down** (`0x5eb143`) |
///
/// So the sweep tears down **iff the reaction is Hated/Hostile**, whether or not the unit is a
/// questgiver — and it *never* tears down on the flag alone, which is what decision 0647 recorded
/// and what this file did until now. A flag that goes off is torn down by the **`UNIT_NPC_FLAGS`
/// field watch** instead (`0x6043c5` registers `0x604a20` → `0x60b420`, which XORs old against new
/// and calls `0x607380` on *any* change to bit `0x2`) — and `0x607380`'s **first instruction after
/// its prologue** is `0x607384 call 0x6073f0`, an unconditional teardown no branch can skip, with
/// the reaction gate (`0x6073b2`) sitting *after* it and guarding only the re-issued queries. Ours
/// is [`unit_ask_key`] moving, which is the same event observed from outside.
///
/// **The two sweeps therefore disagree, deliberately.** The light sweep (`0x5eb070`, 11 callers)
/// runs the table above. The full re-query sweep (`0x5eb3c0`, 2 callers — **your revive**, and a
/// change to your own record's reaction inputs) runs `0x5eb3f0`, whose creature arm is an
/// unconditional `0x607380`: every creature's marker is torn down and then re-asked for. `full`
/// below is that distinction.
///
/// There is no GameObject counterpart to any of it: a GameObject has no status to invalidate
/// (`+0xcb8` is `0x9a8` bytes past the end of a `0x310`-byte `CGGameObject_C`), and all 8 of
/// `0x6073f0`'s call sites are unit-side.
///
/// **Still not covered**, and why: the reference also sweeps when an *item* moves between
/// containers (an `ITEM` typeid watch, `0x5d9375` — quest availability can depend on carried
/// items). benilla does not stream item objects (`EntityKind` has no `Item`), so there is nothing
/// to watch; the equipment and equipped-bag guids in our own descriptor *are* folded, which covers
/// equipping, unequipping and swapping a bag but not moving a stack inside one. Per unit, the
/// reference's reaction refresh also keys on charm/persuade/duel-team/`PLAYER_BYTES_3`; we key on
/// the faction template, and catch a standing change through the reputation sweep instead.
#[allow(clippy::type_complexity)] // a Bevy system: one param per resource
pub(super) fn query_statuses(
    self_q: Query<Ref<ObjectStore>, With<SelfPlayer>>,
    objects: Query<
        (
            Entity,
            &crate::net::Guid,
            &NetEntity,
            Ref<ObjectStore>,
            Option<&crate::ui_taxi::FlightMasterStatus>,
        ),
        Without<SelfPlayer>,
    >,
    index: Res<GuidIndex>,
    factions: Option<Res<Factions>>,
    reputations: Res<crate::net::Reputations>,
    mut quest: ResMut<QuestGiver>,
    commands: Res<NetCommands>,
    mut ecs: Commands,
    mut state: Local<QueryState>,
) {
    let Some(store) = self_q.iter().next() else {
        return;
    };
    // The descriptor half is only recomputed when our own store actually changed — it walks 20
    // quest-log slots, 128 skill slots and 23 inventory slots, which has no business running every
    // frame. The packet half (the epoch) is a plain counter, so it is always cheap to fold.
    if store.is_changed() {
        state.fields = self_generation(&store.0);
    }
    let generation = state.fields ^ (u64::from(quest.reask_epoch()) << 32);
    // **The FULL re-query sweep's own two triggers** (`0x5eb3c0`, both self-gated on
    // `IsLocalPlayer`): your **revive** — the `≤0 → >0` crossing that `0x6046f0` sends down the
    // sibling branch of its death edge — and a change to **your own record's** reaction inputs
    // (`0x606e20` → `0x606eef`). Everything else in the trigger set reaches the *light* sweep
    // `0x5eb070`, which does not tear anything down. Tracked as edges rather than folded into the
    // generation because the direction is the whole point: the death edge is a light sweep and the
    // revive edge is a full one, and the folded alive/dead bit cannot tell them apart.
    let alive = store.0.unit_health().unwrap_or(1) != 0;
    // Compared only between two frames that both *saw* the field: our template arrives with the
    // create block, and treating "absent → present" as a change would fire one spurious full sweep
    // at every login.
    let my_faction = store.0.unit_faction_template();
    let full = (alive && state.was_dead)
        || (my_faction.is_some() && state.my_faction.is_some() && state.my_faction != my_faction);
    state.was_dead = !alive;
    if my_faction.is_some() {
        state.my_faction = my_faction;
    }
    // **This frame IS a sweep.** A sweep walks every object in the manager once, synchronously,
    // when one of the reference's local-player state changes fires it; a changed generation is that
    // moment. The GameObject leg below fires only on it, because that is the only way a GameObject
    // GUID ever reaches the wire (`0x5eb159`, `0x5eb456`) — and a full sweep is a sweep too, even
    // when it arrives through an edge the generation does not carry.
    let swept = state.generation != generation || full;
    state.generation = generation;
    // Object lifetime is the other half of the cache key: a guid that left the world drops both
    // its "already asked" mark and its cached status, so re-entering view re-asks from scratch.
    // The map is NOT cleared by a sweep — it holds each unit's last ask key, and a sweep re-asking
    // everyone must stay distinguishable from that unit's own key actually moving, because only
    // the second is a `0x607380` and only a `0x607380` tears the marker down.
    // …and it can only shrink on a frame the object index moved (a guid left the world): the
    // two retains used to walk both maps every frame, the second through `ResMut` — which
    // marked `QuestGiver` changed on every frame whether or not a status left.
    if index.is_changed() {
        state.asked.retain(|guid, _| index.0.contains_key(guid));
        quest.retain_statuses(|npc| index.0.contains_key(&npc));
    }
    // On a frame that is not a sweep, a unit's verdict below is a function of its own
    // descriptor and of the two reaction catalogs, and nothing else: with all three still, the
    // arm is a no-op (same reaction → same teardown already done, same key → no `0x607380`), so
    // the unit is not visited. A city's whole idle population used to take the reaction
    // resolve and the ask-key insert every frame for it.
    let catalogs_moved =
        reputations.is_changed() || factions.as_ref().is_some_and(|f| f.is_changed());
    for (entity, guid, net, obj, fm) in &objects {
        if !swept && !full && !catalogs_moved && !obj.is_changed() {
            continue;
        }
        // `0x6073f0` zeroes `+0xb2c` (the marker instance) and `+0xcb8` (the questgiver status)
        // **together**, and the flight master's green `!` lives in that same `+0xb2c` — so every
        // teardown below takes both. Only removed when there is something to remove: a `Commands`
        // write per unit per frame would be a queue of no-ops.
        let tear_down = |quest: &mut QuestGiver, ecs: &mut Commands| {
            quest.clear_status(guid.0);
            if fm.is_some() {
                ecs.entity(entity)
                    .remove::<crate::ui_taxi::FlightMasterStatus>();
            }
        };
        match net.kind {
            // `0x5eb0a0` @ `0x5eb0d3`: `cmp eax,9; je` — typemask EXACTLY `OBJECT|UNIT`, a plain
            // creature. A player is `0x19` and falls out of the sweep here, before any flag test.
            EntityKind::Unit => {
                // `0x6061e0(ecx = the swept unit, arg = the player)` — the unit's reaction toward
                // **us**, which is the direction both `call` sites in the callback use
                // (`0x5eb12a`/`0x5eb137`, both `ecx = esi`) and the direction
                // [`ring_reaction`] resolves. `<= 1` is Hated/Hostile and nothing else: Unfriendly
                // and Neutral both pass. It reads Neutral when anything is missing, so a cold
                // catalog can never blank a marker.
                let reaction = crate::target::ring_reaction(
                    factions.as_deref(),
                    &reputations,
                    Some(&*obj),
                    Some(&store),
                );
                if reaction <= 1 {
                    // The sweep's ONE teardown (`0x5eb143`), and it is convergent: it fires for a
                    // hostile creature whether or not it is a questgiver. Dropping the ask key too
                    // means the marker comes back through a `0x607380` when the reaction recovers.
                    state.asked.remove(&guid.0);
                    tear_down(&mut quest, &mut ecs);
                    continue;
                }
                let key = unit_ask_key(&obj.0);
                let key_moved = state.asked.insert(guid.0, key) != Some(key);
                // **`0x607380` — teardown first, unconditionally, then re-gate.** Reached from this
                // unit's own field watches (`UNIT_NPC_FLAGS` via `0x60b420`'s XOR, so *either*
                // direction; the flightmaster bit; its reaction inputs), from its create/init
                // hooks, and from the full sweep's callback. `0x607384 call 0x6073f0` is the first
                // instruction after the prologue and no branch can skip it.
                //
                // This is where **B257** actually lives, and it used to hang off the wrong
                // mechanism: an escort's giver drops `UNIT_NPC_FLAGS` in the same server tick as
                // the quest-log write (vmangos `FollowerAI::StartFollow`, `ScriptedEscortAI`'s
                // "disable npcflags"), and its cached AVAILABLE status — with its `!` — stayed
                // frozen over the NPC for the whole escort. The flag is half of [`unit_ask_key`],
                // so the drop moves the key, and the key moving is a `0x607380`.
                let per_unit = key_moved || full;
                if per_unit {
                    tear_down(&mut quest, &mut ecs);
                }
                // …and re-gate. Reaction is already `> 1` — the arm above returned otherwise —
                // so what is left is the two service bits, and **they are not symmetric**:
                //
                // - `0x182` (questgiver) is issued by `0x607380` @`0x6073cd` on bit `0x2` **and**
                //   inline by the light sweep's own callback (`0x5eb159`), so every sweep re-asks;
                // - `0x1aa` (taxi node status) has exactly ONE live sender site image-wide —
                //   `0x607380` @`0x6073e8` — so it is re-asked on this unit's own key moving and
                //   on a full sweep, and **never** on a light one. `0x5eb0a0` does not test bit
                //   `0x8` at all.
                let flags = obj.0.unit_npc_flags();
                if (per_unit || swept) && flags & NPC_FLAG_QUESTGIVER != 0 {
                    let _ = commands
                        .0
                        .send(ClientCommand::QuestgiverStatusQuery { npc: guid.0 });
                }
                if per_unit && flags & NPC_FLAG_FLIGHTMASTER != 0 {
                    let _ = commands
                        .0
                        .send(ClientCommand::TaxiNodeStatusQuery { guid: guid.0 });
                }
            }
            // `0x5eb0da shr ecx,5; test cl,1` — typemask bit 5, `TYPEMASK_GAMEOBJECT`. Sweep-only,
            // no asked-set, no teardown: see this function's doc.
            EntityKind::GameObject if swept => {
                if obj.0.gameobject_flags() & GO_FLAG_INTERACT_COND == 0 {
                    continue;
                }
                // `0x5eb101 cmp eax,1; jle` — the GameObject's reaction toward us must be at least
                // **2**. Not a friendliness test: Unfriendly(2) and Neutral(3) both pass, and a
                // faction-less GameObject resolves Neutral. `None` = unresolvable (no catalog yet)
                // and passes, the same "no opinion" rule the cursor's own gate uses.
                let reaction = go_reaction(
                    factions.as_deref(),
                    obj.0.gameobject_faction(),
                    Some(&store),
                );
                if !reaction.is_none_or(|r| r > 1) {
                    continue;
                }
                let _ = commands
                    .0
                    .send(ClientCommand::QuestgiverStatusQuery { npc: guid.0 });
            }
            _ => {}
        }
    }
}

/// The per-unit ask key: re-ask that NPC whenever this moves. `UNIT_NPC_FLAGS` carries both service
/// bits the reference watches — questgiver `0x2` (`0x60b490`) and flightmaster `0x8` (`0x60b4c5`) —
/// and `UNIT_FIELD_FACTIONTEMPLATE` is the reaction input we can see (`0x606f0a`); a charm or a
/// faction swap changes what the server will answer for a hostile giver.
fn unit_ask_key(fields: &benilla_protocol::ObjectFields) -> u64 {
    u64::from(fields.unit_npc_flags())
        | (u64::from(fields.unit_faction_template().unwrap_or(0)) << 32)
}

/// Fold every *self* descriptor field the reference watches into one value — a change to any of
/// them sweeps every visible questgiver, because each is an input the server's `GetDialogStatus`
/// reads. The reference registers one handler per field (`0x468070`); we hash the lot, which is
/// the same thing observed from the outside. Recomputed only when our own store changes.
///
/// Level (`SatisfyQuestLevel`, the grey `!`), the quest log, money, every skill
/// (`SatisfyQuestSkill`), `PLAYER_FLAGS`, health, and our equipment/bag guids — the last standing
/// in for the reference's `ITEM` watch, as far as our object model can see it.
fn self_generation(fields: &benilla_protocol::ObjectFields) -> u64 {
    const FNV_PRIME: u64 = 0x0000_0100_0000_01B3;
    let mut h = 0xcbf2_9ce4_8422_2325u64; // FNV offset
    let mut fold = |v: u64| {
        h ^= v;
        h = h.wrapping_mul(FNV_PRIME);
    };
    fold(u64::from(fields.unit_level().unwrap_or(0)));
    // Health enters as the ALIVE/DEAD bit, not the raw value — and that is the PINNED reading now,
    // not a conservative guess. The `UNIT_FIELD_HEALTH` watch
    // gates on the zero CROSSING, in `0x6046f0`, one level above where the handler chain suggested:
    // `0x604774 jg` / `0x604778 jle` require `old > 0 && new <= 0` = **death** before the sweep is
    // reached, and `0x6047f0`/`0x6047f4` take `old <= 0 && new > 0` = **resurrect** down the sibling
    // branch. Ordinary damage, heal and regen ticks fall through both and reach nothing — which is
    // why the captures show no query bursts tracking combat (one carries 28 consecutive non-lethal
    // packets, 70 → 2 HP, with zero bursts; the single packet crossing to 0 has the only burst in
    // the stretch).
    //
    // Our bit flips on BOTH crossings, and that is now known to be exactly right — the note this
    // comment used to carry ("one named over-refresh... the reference sweeps GameObject questgivers
    // there") was wrong on both halves. `0x5eb3c0` is not
    // a GameObject sweep: `0x468380` applies no type filter at all, so it walks **every** object in
    // the manager — it is the FULL re-query sweep, and its creature arm is the heavier one
    // (`0x607380`: an unconditional marker teardown plus two queries, questgiver `0x182` and taxi
    // `0x1aa`), the GameObject arm the lighter. So the death edge sweeps (`0x5eb070`) and the
    // **revive** edge sweeps too (`0x5eb3c0`), and one sweep per crossing is the reference's own
    // behaviour rather than a deviation we were paying for. 0654, 1872.
    fold(u64::from(fields.unit_health().unwrap_or(1) == 0));
    fold(u64::from(fields.player_flags()));
    fold(u64::from(fields.player_money().unwrap_or(0)));
    for slot in 0..benilla_protocol::messages::PLAYER_QUEST_LOG_SLOTS {
        if let Some(s) = fields.player_quest_log(slot) {
            if s.quest_id != 0 {
                fold(u64::from(s.quest_id) ^ (u64::from(s.state) << 32));
            }
        }
    }
    // The reference watches the skill *value* half specifically (`edx=0x84c`), so the id + current
    // value are what matter — a rank-up is the event, not a temporary bonus ticking.
    for slot in 0..benilla_protocol::messages::PLAYER_SKILL_SLOTS {
        if let Some(s) = fields.player_skill(slot) {
            if s.skill_id != 0 {
                fold(u64::from(s.skill_id) ^ (u64::from(s.value) << 32));
            }
        }
    }
    for i in 0..23 {
        if let Some(guid) = fields.player_inv_slot(i) {
            fold(guid);
        }
    }
    h
}

/// [`query_statuses`]'s memory across frames.
#[derive(Default)]
pub(super) struct QueryState {
    /// The last folded [`self_generation`] — kept separately from `generation` so the expensive
    /// descriptor walk can be skipped on frames where our own store didn't change.
    fields: u64,
    /// `fields` combined with the packet epoch; a change here is the sweep.
    generation: u64,
    /// Which guids we've asked about, and the [`unit_ask_key`] we asked at. Pruned by object
    /// lifetime only — never by a sweep, so "this unit's own key moved" stays a distinct event.
    asked: HashMap<u64, u64>,
    /// Our own alive/dead and faction-template from last frame — the two edges that make a sweep a
    /// **full** one. `None`/`false` initially, so the first frame in a world is never a full sweep.
    was_dead: bool,
    my_faction: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The re-ask law. The server only *answers* queries, so a status we never
    /// re-ask for is a marker frozen at whatever it was when the NPC came into view. Three
    /// invalidations, all exercised here and all faithful: our own level (the ding that turns a
    /// grey `!` gold — the reference's `UNIT_FIELD_LEVEL` field watch), the object leaving and
    /// re-entering the world (its per-object query at create), and the questgiver bit going off
    /// (its teardown branch).
    #[test]
    fn the_status_is_re_asked_on_a_ding_on_re_entry_and_dropped_with_the_flag() {
        use crate::net::{Guid, NetEntity};
        use benilla_protocol::{EntityKind, ObjectFields};

        const NPC: u64 = 0xdead_beef;
        const FIELD_LEVEL: u16 = 34; // UNIT_FIELD_LEVEL
        const FIELD_NPC_FLAGS: u16 = 147; // UNIT_NPC_FLAGS

        let net_entity = || NetEntity {
            kind: EntityKind::Unit,
            display_id: None,
            scale: 1.0,
        };
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx))
            .init_resource::<GuidIndex>()
            .init_resource::<QuestGiver>()
            .init_resource::<crate::net::Reputations>()
            .add_systems(Update, query_statuses);

        let me = app
            .world_mut()
            .spawn((
                SelfPlayer,
                net_entity(),
                Guid(1),
                ObjectStore(ObjectFields::from_pairs(&[(FIELD_LEVEL, 5)])),
            ))
            .id();
        let spawn_npc = |app: &mut App| {
            let e = app
                .world_mut()
                .spawn((
                    net_entity(),
                    Guid(NPC),
                    ObjectStore(ObjectFields::from_pairs(&[(FIELD_NPC_FLAGS, 0x2)])),
                ))
                .id();
            app.world_mut().resource_mut::<GuidIndex>().0.insert(NPC, e);
            e
        };
        let asked = |app: &mut App| -> usize {
            app.update();
            rx.try_iter()
                .filter(
                    |c| matches!(c, ClientCommand::QuestgiverStatusQuery { npc } if *npc == NPC),
                )
                .count()
        };

        let npc = spawn_npc(&mut app);
        assert_eq!(asked(&mut app), 1, "asked once when it comes into view");
        assert_eq!(asked(&mut app), 0, "not again while nothing has changed");

        // The ding: `SatisfyQuestLevel` is what made the answer UNAVAILABLE, so the whole set is
        // re-asked — the marker can't learn it went gold any other way.
        *app.world_mut()
            .entity_mut(me)
            .get_mut::<ObjectStore>()
            .unwrap() = ObjectStore(ObjectFields::from_pairs(&[(FIELD_LEVEL, 6)]));
        assert_eq!(asked(&mut app), 1, "our level changed — re-ask everyone");
        assert_eq!(asked(&mut app), 0, "…once");

        // Out of view: the cached status dies with the object (the reference caches it *on* the
        // unit), so the marker can't flash stale on the way back in.
        app.world_mut()
            .resource_mut::<QuestGiver>()
            .set_status(NPC, 5);
        app.world_mut().resource_mut::<GuidIndex>().0.remove(&NPC);
        app.world_mut().entity_mut(npc).despawn();
        assert_eq!(asked(&mut app), 0, "gone: nothing to ask");
        assert!(
            app.world().resource::<QuestGiver>().status(NPC).is_none(),
            "the cached status goes with the object"
        );

        let npc = spawn_npc(&mut app);
        assert_eq!(asked(&mut app), 1, "back in view — asked afresh");

        // The flag goes off: drop the answer, which drops the marker with it.
        app.world_mut()
            .resource_mut::<QuestGiver>()
            .set_status(NPC, 5);
        *app.world_mut()
            .entity_mut(npc)
            .get_mut::<ObjectStore>()
            .unwrap() = ObjectStore(ObjectFields::from_pairs(&[(FIELD_NPC_FLAGS, 0x1)]));
        assert_eq!(
            asked(&mut app),
            0,
            "no longer a questgiver — nothing to ask"
        );
        assert!(
            app.world().resource::<QuestGiver>().status(NPC).is_none(),
            "and its stale answer is dropped"
        );
    }

    /// **B257 — an escort giver's `!` outlived the accept.** The teardown branch used to be gated on
    /// the guid still being in `asked` ("did we ever query it?"), which is a memo about *questions*
    /// standing in for a fact about *answers* — and the sweep clears that memo. On accepting an
    /// escort both halves land in the same update: vmangos writes the quest-log slot and, from the
    /// script hook in the same handler, zeroes `UNIT_NPC_FLAGS`
    /// (`FollowerAI::StartFollow` / `ScriptedEscortAI`'s "disable npcflags"). So the quest-log write
    /// bumps the generation, `asked.clear()` runs first, `remove` finds nothing, and the cached
    /// AVAILABLE status stays — forever, because the flag stays off and this arm never queries.
    /// The reference tears down on the flag test alone (`0x5eb0a0`).
    ///
    /// The control below is the half that must NOT change: an ordinary giver, whose flag stays on,
    /// is re-asked by the same sweep rather than torn down.
    #[test]
    fn an_escort_giver_loses_its_marker_when_the_flag_drops_with_the_quest_log_write() {
        use crate::net::{Guid, NetEntity};
        use benilla_protocol::{EntityKind, ObjectFields};

        const ESCORT: u64 = 0x5115; // Mist: gives the quest, then follows with npcflags off
        const PLAIN: u64 = 0x5116; // an ordinary giver standing next to her
        const FIELD_NPC_FLAGS: u16 = 147;
        const FIELD_QUEST_LOG_1_1: u16 = 198;

        let net_entity = || NetEntity {
            kind: EntityKind::Unit,
            display_id: None,
            scale: 1.0,
        };
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx))
            .init_resource::<GuidIndex>()
            .init_resource::<QuestGiver>()
            .init_resource::<crate::net::Reputations>()
            .add_systems(Update, query_statuses);

        let me = app
            .world_mut()
            .spawn((SelfPlayer, net_entity(), Guid(1), ObjectStore::default()))
            .id();
        let spawn = |app: &mut App, guid: u64| {
            let e = app
                .world_mut()
                .spawn((
                    net_entity(),
                    Guid(guid),
                    ObjectStore(ObjectFields::from_pairs(&[(FIELD_NPC_FLAGS, 0x2)])),
                ))
                .id();
            app.world_mut()
                .resource_mut::<GuidIndex>()
                .0
                .insert(guid, e);
            e
        };
        let escort = spawn(&mut app, ESCORT);
        spawn(&mut app, PLAIN);
        app.update(); // both asked, both marked in `asked`

        // The server's answer: a gold `!` over each.
        for npc in [ESCORT, PLAIN] {
            app.world_mut()
                .resource_mut::<QuestGiver>()
                .set_status(npc, 5);
        }

        // The accept, as one update: the quest-log slot appears in OUR store (the sweep) and the
        // escort's questgiver bit goes off (the teardown) — in the same drained batch.
        *app.world_mut()
            .entity_mut(me)
            .get_mut::<ObjectStore>()
            .unwrap() = ObjectStore(ObjectFields::from_pairs(&[(FIELD_QUEST_LOG_1_1, 938)]));
        *app.world_mut()
            .entity_mut(escort)
            .get_mut::<ObjectStore>()
            .unwrap() = ObjectStore(ObjectFields::from_pairs(&[(FIELD_NPC_FLAGS, 0x0)]));
        app.update();

        assert!(
            app.world()
                .resource::<QuestGiver>()
                .status(ESCORT)
                .is_none(),
            "the escortee stopped being a questgiver: its stale AVAILABLE status — and the `!` \
             the marker layer draws from it — must go, sweep or no sweep"
        );
        assert_eq!(
            app.world().resource::<QuestGiver>().status(PLAIN),
            Some(5),
            "the control: a giver whose flag is still on keeps its answer until the server \
             replies to the sweep's fresh query"
        );

        // And it self-heals: when the escort ends and the bit returns, the NPC is asked again.
        *app.world_mut()
            .entity_mut(escort)
            .get_mut::<ObjectStore>()
            .unwrap() = ObjectStore(ObjectFields::from_pairs(&[(FIELD_NPC_FLAGS, 0x2)]));
        app.update();
        assert!(
            app.world()
                .resource::<QuestGiver>()
                .status(ESCORT)
                .is_none(),
            "still nothing cached — the answer arrives from the server, not from us"
        );
    }

    /// **The light sweep re-asks; only the FULL sweep tears down first**.
    /// The two sweeps are not interchangeable and this is the difference:
    /// `0x5eb0a0` (11 callers — level, money, the quest log, a skill, `PLAYER_FLAGS`, an item, your
    /// **death**, reputation, the group roster, the quest packets) sends and never tears down,
    /// while `0x5eb3f0` (2 callers — your **revive**, and your own record's reaction inputs) goes
    /// through `0x607380`, whose first post-prologue instruction is an unconditional teardown.
    ///
    /// The death and revive edges are the sharp end: they are the same folded alive/dead bit, one
    /// each way, and they land on *different sweeps*. A model that cannot tell them apart either
    /// blinks every marker on a damage-to-zero or fails to blink them on the way back.
    #[test]
    fn a_light_sweep_re_asks_but_only_a_revive_tears_the_marker_down_first() {
        use crate::net::{Guid, NetEntity};
        use benilla_protocol::{EntityKind, ObjectFields};

        const NPC: u64 = 0x7777;
        const FIELD_LEVEL: u16 = 34;
        const FIELD_HEALTH: u16 = 22;
        const FIELD_FACTION: u16 = 35; // UNIT_FIELD_FACTIONTEMPLATE
        const FIELD_NPC_FLAGS: u16 = 147;

        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx))
            .init_resource::<GuidIndex>()
            .init_resource::<QuestGiver>()
            .init_resource::<crate::net::Reputations>()
            .add_systems(Update, query_statuses);

        let me = app
            .world_mut()
            .spawn((
                SelfPlayer,
                Guid(1),
                ObjectStore(ObjectFields::from_pairs(&[
                    (FIELD_LEVEL, 5),
                    (FIELD_HEALTH, 100),
                    (FIELD_FACTION, 1),
                ])),
            ))
            .id();
        let npc = app
            .world_mut()
            .spawn((
                NetEntity {
                    kind: EntityKind::Unit,
                    display_id: None,
                    scale: 1.0,
                },
                Guid(NPC),
                ObjectStore(ObjectFields::from_pairs(&[(FIELD_NPC_FLAGS, 0x2)])),
            ))
            .id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(NPC, npc);
        let asked = |app: &mut App| -> usize {
            app.update();
            rx.try_iter()
                .filter(
                    |c| matches!(c, ClientCommand::QuestgiverStatusQuery { npc } if *npc == NPC),
                )
                .count()
        };
        let set_self = |app: &mut App, pairs: &[(u16, u32)]| {
            *app.world_mut()
                .entity_mut(me)
                .get_mut::<ObjectStore>()
                .unwrap() = ObjectStore(ObjectFields::from_pairs(pairs));
        };
        let restore = |app: &mut App| {
            app.world_mut()
                .resource_mut::<QuestGiver>()
                .set_status(NPC, 5);
        };
        let held = |app: &App| app.world().resource::<QuestGiver>().status(NPC);

        assert_eq!(asked(&mut app), 1, "the create-path query");
        restore(&mut app);

        // The packet half of the LIGHT sweep — a quest turn-in, a reputation change, a roster
        // change. Re-asks; the `!` stays up until the fresh answer lands.
        app.world_mut().resource_mut::<QuestGiver>().bump_reask();
        assert_eq!(asked(&mut app), 1, "a light sweep re-asks");
        assert_eq!(held(&app), Some(5), "…and does NOT tear the marker down");

        // Your DEATH — the `>0 → ≤0` edge, which `0x6046f0` routes to the light sweep. Same rule.
        set_self(
            &mut app,
            &[(FIELD_LEVEL, 5), (FIELD_HEALTH, 0), (FIELD_FACTION, 1)],
        );
        assert_eq!(asked(&mut app), 1, "death re-asks");
        assert_eq!(held(&app), Some(5), "…and is still a light sweep");

        // Your REVIVE — the `≤0 → >0` edge, the sibling branch, and the FULL sweep.
        set_self(
            &mut app,
            &[(FIELD_LEVEL, 5), (FIELD_HEALTH, 100), (FIELD_FACTION, 1)],
        );
        assert_eq!(asked(&mut app), 1, "revive re-asks");
        assert_eq!(
            held(&app),
            None,
            "…and tears every creature's marker down first (0x5eb3f0 -> 0x607380)"
        );

        // Your own record's reaction inputs moving — the full sweep's other caller. It is NOT in
        // the folded generation at all, so this also pins that `full` can raise a sweep by itself.
        restore(&mut app);
        set_self(
            &mut app,
            &[(FIELD_LEVEL, 5), (FIELD_HEALTH, 100), (FIELD_FACTION, 2)],
        );
        assert_eq!(asked(&mut app), 1, "our own faction change sweeps");
        assert_eq!(held(&app), None, "…fully");
    }

    /// **The sweep tears down on HOSTILITY, never on the questgiver flag**.
    /// `0x5eb0a0`'s `0x5eb134` is a convergent block, so the reaction test runs for
    /// every creature and `0x5eb143` is the callback's only teardown; the flag decides the *send*
    /// alone. On the real `FactionTemplate.dbc`, because a reaction gate that never resolves a real
    /// faction is not a gate.
    #[test]
    fn a_hostile_questgiver_is_torn_down_and_never_asked_about() {
        use crate::net::{Guid, NetEntity};
        use benilla_protocol::{EntityKind, ObjectFields};

        const FRIENDLY: u64 = 0x1001;
        const HOSTILE: u64 = 0x1002;
        const FIELD_LEVEL: u16 = 34;
        const FIELD_FACTION: u16 = 35;
        const FIELD_NPC_FLAGS: u16 = 147;
        /// `FactionTemplate.dbc` 35 = "friendly to players", 14 = the monster template that is
        /// hostile to everything — the pair the ring's own tests use.
        const TPL_FRIENDLY: u32 = 35;
        const TPL_MONSTER: u32 = 14;

        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let catalog = benilla_formats::load_faction_catalog(&mut chain).expect("dbc");

        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx))
            .init_resource::<GuidIndex>()
            .init_resource::<QuestGiver>()
            .init_resource::<crate::net::Reputations>()
            .insert_resource(crate::target::Factions::from_catalog(catalog))
            .add_systems(Update, query_statuses);

        app.world_mut().spawn((
            SelfPlayer,
            Guid(1),
            // Us: faction template 1 (PLAYER, Human).
            ObjectStore(ObjectFields::from_pairs(&[
                (FIELD_LEVEL, 5),
                (FIELD_FACTION, 1),
            ])),
        ));
        for (guid, tpl) in [(FRIENDLY, TPL_FRIENDLY), (HOSTILE, TPL_MONSTER)] {
            let e = app
                .world_mut()
                .spawn((
                    NetEntity {
                        kind: EntityKind::Unit,
                        display_id: None,
                        scale: 1.0,
                    },
                    Guid(guid),
                    ObjectStore(ObjectFields::from_pairs(&[
                        (FIELD_NPC_FLAGS, 0x2),
                        (FIELD_FACTION, tpl),
                    ])),
                ))
                .id();
            app.world_mut()
                .resource_mut::<GuidIndex>()
                .0
                .insert(guid, e);
        }
        let drain = |rx: &crossbeam_channel::Receiver<ClientCommand>| -> Vec<u64> {
            rx.try_iter()
                .filter_map(|c| match c {
                    ClientCommand::QuestgiverStatusQuery { npc } => Some(npc),
                    _ => None,
                })
                .collect()
        };

        // Bring both into view first. That frame is a `0x607380` for each of them (the create/init
        // hook: an unconditional teardown, then the re-gate), so a status seeded *before* it would
        // be torn down for a reason that has nothing to do with hostility — and it is not a state a
        // client can be in anyway, since a status cannot exist for a unit never seen.
        app.update();
        assert_eq!(
            drain(&rx),
            vec![FRIENDLY],
            "on sight, only the non-hostile questgiver is asked about"
        );

        // Now both wear a marker, and a sweep runs.
        for g in [FRIENDLY, HOSTILE] {
            app.world_mut()
                .resource_mut::<QuestGiver>()
                .set_status(g, 5);
        }
        app.world_mut().resource_mut::<QuestGiver>().bump_reask();
        app.update();
        let asked = drain(&rx);
        let quest = app.world().resource::<QuestGiver>();
        assert_eq!(
            asked,
            vec![FRIENDLY],
            "and the sweep asks about the same one — {asked:x?}"
        );
        assert_eq!(
            quest.status(FRIENDLY),
            Some(5),
            "the control: a friendly giver keeps its marker"
        );
        assert_eq!(
            quest.status(HOSTILE),
            None,
            "and a Hated/Hostile one is torn down (0x5eb143), questgiver flag or not"
        );
    }

    /// **The flight master's green `!` shares one slot with the gold one, so it shares its
    /// lifetime**. Three things, and the middle one is the asymmetry
    /// a re-implementer gets wrong:
    ///
    /// - `CMSG_TAXINODE_STATUS_QUERY` has **one** live sender site image-wide, `0x607380`
    ///   @`0x6073e8` — so it goes out on this unit's own key moving and on a full sweep, and
    ///   **never** on a light one. There is no mouseover trigger; the function that claim rested on
    ///   (`0x5eb220`) is the callback of a seeder nothing calls.
    /// - the questgiver query is the opposite: the light sweep's callback sends it inline
    ///   (`0x5eb159`), so every sweep re-asks.
    /// - and `0x6073f0` zeroes `+0xb2c` and `+0xcb8` together, so whatever tears one marker down
    ///   takes the other with it.
    #[test]
    fn a_flight_master_is_asked_by_the_per_unit_path_only_and_loses_its_green_with_the_gold() {
        use crate::net::{Guid, NetEntity};
        use crate::ui_taxi::FlightMasterStatus;
        use benilla_protocol::{EntityKind, ObjectFields};

        const FM: u64 = 0x9001; // flightmaster only
        const GIVER: u64 = 0x9002; // questgiver only — the asymmetry's control
        const VENDOR: u64 = 0x9003; // neither — never asked at all
        const FIELD_LEVEL: u16 = 34;
        const FIELD_HEALTH: u16 = 22;
        const FIELD_NPC_FLAGS: u16 = 147;

        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx))
            .init_resource::<GuidIndex>()
            .init_resource::<QuestGiver>()
            .init_resource::<crate::net::Reputations>()
            .add_systems(Update, query_statuses);

        let me = app
            .world_mut()
            .spawn((
                SelfPlayer,
                Guid(1),
                ObjectStore(ObjectFields::from_pairs(&[
                    (FIELD_LEVEL, 5),
                    (FIELD_HEALTH, 100),
                ])),
            ))
            .id();
        let spawn = |app: &mut App, guid: u64, flags: u32| {
            let e = app
                .world_mut()
                .spawn((
                    NetEntity {
                        kind: EntityKind::Unit,
                        display_id: None,
                        scale: 1.0,
                    },
                    Guid(guid),
                    ObjectStore(ObjectFields::from_pairs(&[(FIELD_NPC_FLAGS, flags)])),
                ))
                .id();
            app.world_mut()
                .resource_mut::<GuidIndex>()
                .0
                .insert(guid, e);
            e
        };
        let fm = spawn(&mut app, FM, NPC_FLAG_FLIGHTMASTER);
        spawn(&mut app, GIVER, NPC_FLAG_QUESTGIVER);
        spawn(&mut app, VENDOR, 0x4);

        let drain = |rx: &crossbeam_channel::Receiver<ClientCommand>| -> (Vec<u64>, Vec<u64>) {
            let (mut taxi, mut giver) = (Vec::new(), Vec::new());
            for c in rx.try_iter() {
                match c {
                    ClientCommand::TaxiNodeStatusQuery { guid } => taxi.push(guid),
                    ClientCommand::QuestgiverStatusQuery { npc } => giver.push(npc),
                    _ => {}
                }
            }
            (taxi, giver)
        };

        // On sight: the create/init hook IS a `0x607380`, so both services are asked, once each.
        app.update();
        let (taxi, giver) = drain(&rx);
        assert_eq!(taxi, vec![FM], "the flight master is asked on sight");
        assert_eq!(
            giver,
            vec![GIVER],
            "and so is the questgiver — the vendor never"
        );

        app.update();
        assert_eq!(drain(&rx), (vec![], vec![]), "then quiet");

        // A LIGHT sweep: the questgiver is re-asked inline by the callback; the flight master is
        // not, because `0x5eb0a0` does not test bit 0x8 at all.
        app.world_mut().resource_mut::<QuestGiver>().bump_reask();
        app.update();
        let (taxi, giver) = drain(&rx);
        assert_eq!(giver, vec![GIVER], "a light sweep re-asks the questgiver");
        assert_eq!(
            taxi,
            vec![] as Vec<u64>,
            "…and never the flight master: 0x1aa has one sender site and it is not in the sweep"
        );

        // The green marker is up, and a FULL sweep (a revive) takes it down and re-asks.
        app.world_mut()
            .entity_mut(fm)
            .insert(FlightMasterStatus { known: false });
        let set_self = |app: &mut App, hp: u32| {
            *app.world_mut()
                .entity_mut(me)
                .get_mut::<ObjectStore>()
                .unwrap() = ObjectStore(ObjectFields::from_pairs(&[
                (FIELD_LEVEL, 5),
                (FIELD_HEALTH, hp),
            ]));
        };
        set_self(&mut app, 0); // death — a LIGHT sweep
        app.update();
        assert_eq!(
            drain(&rx).0,
            vec![] as Vec<u64>,
            "death does not re-ask taxi"
        );
        assert!(
            app.world().entity(fm).get::<FlightMasterStatus>().is_some(),
            "…and a light sweep leaves the green marker up"
        );

        set_self(&mut app, 100); // revive — a FULL sweep
        app.update();
        assert_eq!(drain(&rx).0, vec![FM], "a revive re-asks the flight master");
        assert!(
            app.world().entity(fm).get::<FlightMasterStatus>().is_none(),
            "…after tearing its green marker down first (0x607384, the shared 0x6073f0)"
        );

        // And its own key moving is a `0x607380` too: the flightmaster bit going off takes the
        // marker with it, and does not re-ask.
        app.world_mut()
            .entity_mut(fm)
            .insert(FlightMasterStatus { known: false });
        *app.world_mut()
            .entity_mut(fm)
            .get_mut::<ObjectStore>()
            .unwrap() = ObjectStore(ObjectFields::from_pairs(&[(FIELD_NPC_FLAGS, 0)]));
        app.update();
        assert_eq!(
            drain(&rx).0,
            vec![] as Vec<u64>,
            "no longer a flight master"
        );
        assert!(
            app.world().entity(fm).get::<FlightMasterStatus>().is_none(),
            "and its green marker goes with the bit"
        );
    }

    /// **A GameObject is asked about on a SWEEP, and only on a sweep** (its only
    /// send sites, `0x5eb159` and `0x5eb456`, sit in sweep callbacks). Three things are being
    /// pinned here, and the third is the one a client "improves" without noticing:
    ///
    /// - the gate is `GAMEOBJECT_FLAGS` bit 2 (mask `0x4`) — `0x5eb0ef shr eax,0x2; test al,1` —
    ///   and a quest-less GameObject beside it is never asked about at all;
    /// - a sweep asks exactly once, and the frames between sweeps are silent;
    /// - **there is no bring-up query.** A GameObject entering view asks nothing; it waits for the
    ///   next sweep. The unit control in the same world does the opposite on the same frame, which
    ///   is what makes this a contrast and not an accident of ordering.
    #[test]
    fn a_gameobject_is_asked_on_a_sweep_and_never_at_first_sight() {
        use crate::net::{Guid, NetEntity};
        use benilla_protocol::ObjectFields;

        /// The Goldshire `Wanted Poster`'s real guid (`HIGHGUID_GAMEOBJECT`, template 68, spawn
        /// 26843) and its real `GAMEOBJECT_FLAGS` — INTERACT_COND | NODESPAWN, read off the live
        /// wire by `WOW_PROBE_GOQUEST`.
        const POSTER: u64 = 0xf110_0000_0044_68db;
        const POSTER_FLAGS: u32 = 0x24;
        /// A GameObject next to it with no quest condition: a plain door, `NODESPAWN` only.
        const DOOR: u64 = 0xf110_0000_0045_0001;
        const DOOR_FLAGS: u32 = 0x20;
        /// The control: an ordinary creature questgiver, which the same sweep treats differently.
        const NPC: u64 = 0x2222;
        const FIELD_LEVEL: u16 = 34;
        const FIELD_NPC_FLAGS: u16 = 147;
        const FIELD_GAMEOBJECT_FLAGS: u16 = 9;

        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx))
            .init_resource::<GuidIndex>()
            .init_resource::<QuestGiver>()
            .init_resource::<crate::net::Reputations>()
            .add_systems(Update, query_statuses);

        let spawn = |app: &mut App, guid: u64, kind: EntityKind, fields: &[(u16, u32)]| {
            let e = app
                .world_mut()
                .spawn((
                    NetEntity {
                        kind,
                        display_id: None,
                        scale: 1.0,
                    },
                    Guid(guid),
                    ObjectStore(ObjectFields::from_pairs(fields)),
                ))
                .id();
            app.world_mut()
                .resource_mut::<GuidIndex>()
                .0
                .insert(guid, e);
        };
        app.world_mut().spawn((
            SelfPlayer,
            Guid(1),
            ObjectStore(ObjectFields::from_pairs(&[(FIELD_LEVEL, 5)])),
        ));
        // Drain per update, then count per guid — several objects are in flight at once here.
        let asked = |app: &mut App| -> Vec<u64> {
            app.update();
            rx.try_iter()
                .filter_map(|c| match c {
                    ClientCommand::QuestgiverStatusQuery { npc } => Some(npc),
                    _ => None,
                })
                .collect()
        };

        // Settle the generation first: the very first frame in a fresh world IS a sweep (nothing
        // has been folded yet), so "first sight" can only be asked of a later frame.
        assert!(asked(&mut app).is_empty(), "nothing in view yet");

        spawn(
            &mut app,
            POSTER,
            EntityKind::GameObject,
            &[(FIELD_GAMEOBJECT_FLAGS, POSTER_FLAGS)],
        );
        spawn(
            &mut app,
            DOOR,
            EntityKind::GameObject,
            &[(FIELD_GAMEOBJECT_FLAGS, DOOR_FLAGS)],
        );
        spawn(
            &mut app,
            NPC,
            EntityKind::Unit,
            &[(FIELD_NPC_FLAGS, NPC_FLAG_QUESTGIVER)],
        );

        assert_eq!(
            asked(&mut app),
            vec![NPC],
            "first sight: the creature is asked from its own create path, the poster is NOT — \
             the GameObject class has no bring-up query"
        );
        assert!(
            asked(&mut app).is_empty(),
            "and the frames after it stay silent"
        );

        // A sweep: any of the reference's 13 local-player triggers. The packet epoch stands in for
        // its four packet handlers.
        app.world_mut().resource_mut::<QuestGiver>().bump_reask();
        let swept = asked(&mut app);
        assert!(
            swept.contains(&POSTER),
            "the sweep asks about the poster — {swept:x?}"
        );
        assert!(
            swept.contains(&NPC),
            "and re-asks the creature, as it always did — {swept:x?}"
        );
        assert!(
            !swept.contains(&DOOR),
            "but never the GameObject without GAMEOBJECT_FLAGS bit 2 — {swept:x?}"
        );
        assert_eq!(
            swept.iter().filter(|g| **g == POSTER).count(),
            1,
            "exactly once per sweep"
        );

        assert!(
            asked(&mut app).is_empty(),
            "between sweeps, nothing — a GameObject has no per-object key to re-ask on"
        );

        // ...and it is not one-shot: the next sweep asks again, because the reference keeps no
        // per-GameObject memo of having asked (there is nowhere on a CGGameObject_C to keep one).
        app.world_mut().resource_mut::<QuestGiver>().bump_reask();
        assert!(
            asked(&mut app).contains(&POSTER),
            "and again on the next sweep"
        );
    }

    /// The rest of the reference's trigger set: the remaining self-descriptor watches, the
    /// packet epoch that stands in for its four packet handlers, and the per-unit key that catches
    /// a unit's own service bits or faction moving. Each leg must re-ask exactly once.
    #[test]
    fn every_recorded_trigger_re_asks_exactly_once() {
        use crate::net::{Guid, NetEntity};
        use benilla_protocol::{EntityKind, ObjectFields};

        const NPC: u64 = 0x1234;
        const FIELD_HEALTH: u16 = 22; // UNIT_FIELD_HEALTH
        const FIELD_NPC_FLAGS: u16 = 147;
        const FIELD_FACTION: u16 = 35; // UNIT_FIELD_FACTIONTEMPLATE
        const FIELD_PLAYER_FLAGS: u16 = 190;
        const FIELD_COINAGE: u16 = 1176;
        const FIELD_SKILL_1_1: u16 = 718;
        const FIELD_INV_SLOT_0: u16 = 486; // PLAYER_FIELD_INV_SLOT_HEAD

        let net_entity = || NetEntity {
            kind: EntityKind::Unit,
            display_id: None,
            scale: 1.0,
        };
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx))
            .init_resource::<GuidIndex>()
            .init_resource::<QuestGiver>()
            .init_resource::<crate::net::Reputations>()
            .add_systems(Update, query_statuses);

        // Our own store starts with every watched field present, so each leg below is a CHANGE to
        // one field rather than a field appearing for the first time.
        let base_self = vec![
            (34u16, 5u32),
            (FIELD_HEALTH, 100),
            (FIELD_PLAYER_FLAGS, 0),
            (FIELD_COINAGE, 500),
            (FIELD_SKILL_1_1, 186 | (1 << 16)), // skill id 186, step 1
            (FIELD_SKILL_1_1 + 1, 75 | (300 << 16)), // value 75 / max 300
            (FIELD_INV_SLOT_0, 0xaaaa),
            (FIELD_INV_SLOT_0 + 1, 0),
        ];
        let me = app
            .world_mut()
            .spawn((
                SelfPlayer,
                net_entity(),
                Guid(1),
                ObjectStore(ObjectFields::from_pairs(&base_self)),
            ))
            .id();
        let npc_e = app
            .world_mut()
            .spawn((
                net_entity(),
                Guid(NPC),
                ObjectStore(ObjectFields::from_pairs(&[
                    (FIELD_NPC_FLAGS, 0x2),
                    (FIELD_FACTION, 35),
                ])),
            ))
            .id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(NPC, npc_e);

        let asked = |app: &mut App| -> usize {
            app.update();
            rx.try_iter()
                .filter(
                    |c| matches!(c, ClientCommand::QuestgiverStatusQuery { npc } if *npc == NPC),
                )
                .count()
        };
        // Replace one field of our own descriptor, keeping the rest — the shape a values-update
        // delta actually has.
        let set_self = |app: &mut App, field: u16, value: u32| {
            let mut pairs = base_self.clone();
            match pairs.iter_mut().find(|(f, _)| *f == field) {
                Some(p) => p.1 = value,
                None => pairs.push((field, value)),
            }
            *app.world_mut()
                .entity_mut(me)
                .get_mut::<ObjectStore>()
                .unwrap() = ObjectStore(ObjectFields::from_pairs(&pairs));
        };

        assert_eq!(asked(&mut app), 1, "the first sight");
        assert_eq!(asked(&mut app), 0, "then quiet");

        for (label, field, value) in [
            ("death", FIELD_HEALTH, 0u32), // the alive/dead bit, not the raw value — see 0654
            ("PLAYER_FLAGS", FIELD_PLAYER_FLAGS, 0x10),
            ("money", FIELD_COINAGE, 900),
            ("a skill rank", FIELD_SKILL_1_1 + 1, 80 | (300 << 16)),
            ("an equipped item", FIELD_INV_SLOT_0, 0xbbbb),
        ] {
            set_self(&mut app, field, value);
            assert_eq!(asked(&mut app), 1, "{label} changed — re-ask");
            assert_eq!(asked(&mut app), 0, "{label}: exactly once");
        }

        // The packet half: reputation, the group roster and the quest packets all land here.
        app.world_mut().resource_mut::<QuestGiver>().bump_reask();
        assert_eq!(asked(&mut app), 1, "a swept packet — re-ask");
        assert_eq!(asked(&mut app), 0, "exactly once");

        // Per unit: its flightmaster bit, then its faction template.
        let set_npc = |app: &mut App, pairs: &[(u16, u32)]| {
            *app.world_mut()
                .entity_mut(npc_e)
                .get_mut::<ObjectStore>()
                .unwrap() = ObjectStore(ObjectFields::from_pairs(pairs));
        };
        set_npc(
            &mut app,
            &[(FIELD_NPC_FLAGS, 0x2 | 0x8), (FIELD_FACTION, 35)],
        );
        assert_eq!(asked(&mut app), 1, "its flightmaster bit — re-ask that one");
        assert_eq!(asked(&mut app), 0, "exactly once");

        set_npc(
            &mut app,
            &[(FIELD_NPC_FLAGS, 0x2 | 0x8), (FIELD_FACTION, 12)],
        );
        assert_eq!(asked(&mut app), 1, "its faction template — re-ask that one");
        assert_eq!(asked(&mut app), 0, "exactly once");
    }
}
