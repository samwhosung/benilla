//! Every unit a unit token can name now, as the app holds the client's state: the guid globals,
//! the group tables, and each held unit a `target` chain off them reaches (`0x515970`'s bases and
//! hops). The VM's getters resolve a token at the call through the lent host; this walk is for the
//! app's own readers of the whole set: the unit views' rewind, which versions only the units some
//! token can name, and the targeting word's no-host list of the units `SpellCanTargetUnit` clears.

use benilla_ui::script::UnitGuids;

use crate::net::ObjectStore;
use crate::ui_party::{raid_row_guids, GroupState};

/// The group tables of [`UnitGuids`]: `party1..4` are the own subgroup's slots and `partypetN`
/// their pets (`0x4e81d0`); `raidN` is `GetRaidRosterInfo` row N, as every other raid-token reader
/// resolves it, and `raidpetN` that row's pet (`0x491960`).
fn fill_group<'a>(
    g: &mut UnitGuids,
    group: Option<&GroupState>,
    held: impl Fn(u64) -> Option<&'a ObjectStore>,
) {
    let Some(group) = group else {
        return;
    };
    let live = |m: u64| held(m).map(|s| &s.0);
    for (i, m) in group.party_slots().take(4).enumerate() {
        g.party[i] = m.guid;
        g.party_pets[i] = group.party_pet_guid(m.guid, live(m.guid)).unwrap_or(0);
    }
    g.raid = raid_row_guids(group);
    g.raid_pets.extend(
        g.raid
            .iter()
            .map(|&m| group.raid_pet_guid(m, live(m)).unwrap_or(0)),
    );
}

/// What [`reach`] reads.
pub(crate) struct Inputs<'g, H> {
    pub(crate) group: Option<&'g GroupState>,
    /// Our guid.
    pub(crate) me: u64,
    /// `pet`, `target`, `mouseover` and `npc`'s guids, 0 for none.
    pub(crate) pet: u64,
    pub(crate) target: u64,
    pub(crate) mouseover: u64,
    pub(crate) npc: u64,
    /// The object manager's lookup (`0x468460`).
    pub(crate) held: H,
}

/// The units the tokens reach, as [`UnitGuids`]: the bases, the group tables, then each held unit
/// their `target` chains reach, to its `UNIT_FIELD_TARGET`.
pub(crate) fn reach<'a, H>(inputs: Inputs<'_, H>) -> UnitGuids
where
    H: Fn(u64) -> Option<&'a ObjectStore>,
{
    let Inputs {
        group,
        me,
        pet,
        target,
        mouseover,
        npc,
        held,
    } = inputs;
    let mut g = UnitGuids {
        player: me,
        pet,
        target,
        mouseover,
        npc,
        ..Default::default()
    };
    fill_group(&mut g, group, &held);
    let mut walk: Vec<u64> = [me, pet, target, mouseover, npc].into();
    walk.extend(g.party.iter().chain(&g.party_pets).copied());
    walk.extend(g.raid.iter().chain(&g.raid_pets).copied());
    while let Some(guid) = walk.pop() {
        if guid == 0 || g.held.contains_key(&guid) {
            continue;
        }
        if let Some(unit) = held(guid).filter(|s| s.is_unit()) {
            let next = unit.0.unit_target().unwrap_or(0);
            g.held.insert(guid, next);
            walk.push(next);
        }
    }
    g
}

/// The published mouseover `[0xb4e2c8]` while it names a held unit, else 0: the resolver takes it
/// only as a unit (`0x515bca mov ecx,8`, `0x515bd9 je`), so a GameObject, a corpse or nothing names
/// nobody.
pub(crate) fn mouseover_unit<'a>(
    published: Option<u64>,
    held: impl Fn(u64) -> Option<&'a ObjectStore>,
) -> u64 {
    published
        .filter(|&guid| held(guid).is_some_and(|s| s.is_unit()))
        .unwrap_or(0)
}
