//! The views beside the four tokens that the lane builds as a packet left them: the party and
//! raid tokens with their pets, off the members' descriptors and group records (`0x5174d0` reads
//! the object, else the record `0x496400`, else the pet record `0x496420`); the `"pet"` token; the
//! units a `target` chain ends on, by guid; and every unit's aura list, which `UnitBuff` and
//! `UnitDebuff` walk off the descriptor or the record (`0x519500`, `0x5198f0`). Each is rebuilt
//! only when what it is built from moved, and what the VM held before is kept to put back
//! ([`Saved`]).

use std::collections::{HashMap, HashSet};

use benilla_ui::script::{UiScript, UnitAuraList, UnitState};

use crate::names::NameCache;
use crate::net::{NetCommands, ObjectStore};
use crate::target::RelationWorld;
use benilla_protocol::messages::GroupMemberEntry;

use crate::ui_party::{GroupSources, GroupState, Roster};

/// What the VM held of each view the lane pushed over, by view: put back as the drain ends, when
/// every store is the live one again and the feeds' own pushes are the views as of there.
#[derive(Default)]
pub(super) struct Saved {
    tokens: HashMap<String, Option<UnitState>>,
    by_guid: HashMap<u64, Option<UnitState>>,
    auras: HashMap<u64, Option<UnitAuraList>>,
    /// The party's faction group the group tokens were last built with, while the lane built
    /// any.
    own_group: Option<Option<String>>,
}

impl Saved {
    /// Push `view` under `token` when the VM holds another; whether it did.
    pub(super) fn token(
        &mut self,
        script: &mut UiScript,
        token: &str,
        view: Option<UnitState>,
    ) -> bool {
        let held = script.unit_view(token);
        if held == view {
            return false;
        }
        self.tokens.entry(token.to_string()).or_insert(held);
        script.set_unit(token, view);
        true
    }

    /// Push `view` for the chain end `guid` when the VM holds another; whether it did.
    fn by_guid(&mut self, script: &mut UiScript, guid: u64, view: Option<UnitState>) -> bool {
        let held = script.unit_view_by_guid(guid);
        if held == view {
            return false;
        }
        self.by_guid.entry(guid).or_insert(held);
        script.set_unit_by_guid(guid, view);
        true
    }

    /// Push `list` as `guid`'s auras when the VM holds another; whether it did.
    fn auras(&mut self, script: &mut UiScript, guid: u64, list: Option<UnitAuraList>) -> bool {
        let held = script.unit_auras_view(guid);
        if held == list {
            return false;
        }
        self.auras.entry(guid).or_insert(held);
        script.set_unit_auras(guid, list);
        true
    }

    /// Put back what the VM held of every view pushed over, but the tokens of `frozen`, which a
    /// call during the drain seated itself; the views put back.
    pub(super) fn restore(&mut self, script: &mut UiScript, frozen: &[&str]) -> usize {
        let mut pushes = 0;
        for (token, view) in self.tokens.drain() {
            if !frozen.contains(&token.as_str()) && script.unit_view(&token) != view {
                script.set_unit(&token, view);
                pushes += 1;
            }
        }
        for (guid, view) in self.by_guid.drain() {
            if script.unit_view_by_guid(guid) != view {
                script.set_unit_by_guid(guid, view);
                pushes += 1;
            }
        }
        for (guid, list) in self.auras.drain() {
            if script.unit_auras_view(guid) != list {
                script.set_unit_auras(guid, list);
                pushes += 1;
            }
        }
        self.own_group = None;
        pushes
    }
}

/// What the families read, each store and record as of the place they are built for.
pub(super) struct Families<'a, 'w, 's> {
    /// The guids whose store or record moved since the VM's views were built.
    pub(super) dirty: &'a HashSet<u64>,
    /// The guids a group token names, whose roster record stands in for its object.
    pub(super) grouped: &'a HashSet<u64>,
    /// The object manager's lookup (`0x468460`), as of here.
    pub(super) object: &'a dyn Fn(u64) -> Option<&'a ObjectStore>,
    /// Our guid and descriptor as of here, and as the frame left it.
    pub(super) me: Option<(u64, &'a ObjectStore)>,
    pub(super) live_me: Option<(u64, &'a ObjectStore)>,
    /// The roster, its cells and records as of here.
    pub(super) group: &'a GroupState,
    /// The roster's cells moved since the views were built: every group token is built over them.
    pub(super) roster_moved: bool,
    pub(super) tables: &'a crate::ui_unit::SnapshotTables<'w>,
    pub(super) names: &'a NameCache,
    pub(super) commands: &'a NetCommands,
    pub(super) guild: &'a crate::ui_guild::GuildState,
    pub(super) factions: Option<&'a crate::target::Factions>,
    pub(super) areas: Option<&'a benilla_formats::AreaTableCatalog>,
    pub(super) auras: &'a crate::ui_aura::AuraViews<'w>,
    pub(super) world: &'a RelationWorld<'w, 's>,
    /// `UnitIsPVPFreeForAll` (`0x516540`) of a held unit as of here, by guid.
    pub(super) ffa: &'a dyn Fn(u64) -> bool,
    /// What a held unit's snapshot reads, for the chain ends.
    pub(super) held: &'a crate::ui_unit::held::HeldUnits<'a>,
}

impl Families<'_, '_, '_> {
    fn moved(&self, guid: u64) -> bool {
        guid != 0 && self.dirty.contains(&guid)
    }
}

/// Push every family's views whose inputs moved; the views pushed.
pub(super) fn push(script: &mut UiScript, saved: &mut Saved, f: &Families) -> usize {
    group_tokens(script, saved, f)
        + pet_token(script, saved, f)
        + chain_ends(script, saved, f)
        + aura_lists(script, saved, f)
}

/// `party1..4`, `partypet1..4`, `raid1..40` and `raidpet1..40`: a member's token when its
/// descriptor or record moved, a pet's when its owner's or its own did, and every member's when
/// the party's faction group, ours, did or the roster's cells moved, which name who each token is
/// and empty the rows past its end.
fn group_tokens(script: &mut UiScript, saved: &mut Saved, f: &Families) -> usize {
    use crate::ui_party::{PARTY_PET_TOKENS, PARTY_TOKENS, RAID_PET_TOKENS, RAID_TOKENS};
    let src = GroupSources {
        tables: f.tables,
        group: f.group,
        object: f.object,
        me: f.me,
        factions: f.factions,
        names: f.names,
        commands: f.commands,
        areas: f.areas,
        guild: f.guild,
        ffa: f.ffa,
    };
    let own_group = src.own_group();
    let built_with = saved.own_group.get_or_insert_with(|| {
        f.live_me
            .and_then(|(_, store)| crate::ui_unit::faction_group(store, f.factions))
    });
    let faction_moved = *built_with != own_group;
    if faction_moved {
        *built_with = own_group.clone();
    }
    let all = faction_moved || f.roster_moved;
    let mut pushes = 0;
    let pet_moved = |owner: u64, roster: Roster| {
        f.moved(owner)
            || src
                .group_pet_guid(owner, roster)
                .is_some_and(|g| f.moved(g))
    };
    let slots: Vec<&GroupMemberEntry> = f.group.party_slots().take(PARTY_TOKENS.len()).collect();
    for i in 0..PARTY_TOKENS.len() {
        let m = slots.get(i).copied();
        let guid = m.map_or(0, |m| m.guid);
        if all || f.moved(guid) {
            let view = m.map(|m| src.party_unit(m, &own_group));
            pushes += usize::from(saved.token(script, PARTY_TOKENS[i], view));
        }
        if f.roster_moved || pet_moved(guid, Roster::Party) {
            let view = m.and_then(|m| src.group_pet(m.guid, Roster::Party));
            pushes += usize::from(saved.token(script, PARTY_PET_TOKENS[i], view));
        }
    }
    let rows: Vec<u64> = f.group.raid_rows().take(RAID_TOKENS.len()).collect();
    for i in 0..RAID_TOKENS.len() {
        let guid = rows.get(i).copied();
        if all || f.moved(guid.unwrap_or(0)) {
            let view = guid.and_then(|g| src.raid_unit(g, &own_group));
            pushes += usize::from(saved.token(script, RAID_TOKENS[i], view));
        }
        if f.roster_moved || pet_moved(guid.unwrap_or(0), Roster::Raid) {
            let view = guid.and_then(|g| src.group_pet(g, Roster::Raid));
            pushes += usize::from(saved.token(script, RAID_PET_TOKENS[i], view));
        }
    }
    pushes
}

/// `"pet"`, the unit our charm, else our summon, names as of here (`0x515a5e`-`0x515a6e`), when
/// our descriptor or its moved.
fn pet_token(script: &mut UiScript, saved: &mut Saved, f: &Families) -> usize {
    let guid = crate::ui_unit::pet_token_guid(f.me.map(|(_, s)| s));
    if !f.me.is_some_and(|(me, _)| f.moved(me)) && !f.moved(guid) {
        return 0;
    }
    let src = crate::ui_pet::PetSources {
        names: f.names,
        commands: f.commands,
        types: f.tables.types(f.names),
        catalog: f.tables.catalog(),
        study: f.tables.study(),
        group: f.group,
        object: f.object,
    };
    let view = crate::ui_pet::pet_state(guid, (f.object)(guid), &src);
    usize::from(saved.token(script, "pet", view))
}

/// Each unit a `target` chain ends on, by guid, when its descriptor moved.
fn chain_ends(script: &mut UiScript, saved: &mut Saved, f: &Families) -> usize {
    let mut pushes = 0;
    for guid in script.chain_end_guids() {
        if f.moved(guid) {
            let view = f.held.chain_end(guid, (f.object)(guid));
            pushes += usize::from(saved.by_guid(script, guid, view));
        }
    }
    pushes
}

/// Each moved unit's aura list a token can reach, the player's among them: `UnitBuff("player")`
/// walks the fields, not the buff cache (`0x51960f`).
fn aura_lists(script: &mut UiScript, saved: &mut Saved, f: &Families) -> usize {
    let mut pushes = 0;
    let mut dirty: Vec<u64> = f.dirty.iter().copied().filter(|&g| g != 0).collect();
    dirty.sort_unstable();
    for guid in dirty {
        let list = f.auras.list(
            guid,
            (f.object)(guid),
            f.grouped.contains(&guid),
            Some(f.group),
            f.world,
            f.me.map(|(_, s)| s),
        );
        pushes += usize::from(saved.auras(script, guid, list));
    }
    pushes
}
