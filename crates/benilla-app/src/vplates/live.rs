//! The plates' reads inside a packet, the call host's half of their questions
//! ([`PlateUnitQuery`], [`PlateGateQuery`]): a unit's callback reads the unit, its reaction, its
//! records and the raid board as they stand where it runs, and the gate run from
//! `InvalidateNameDesc` (`0x60706a`) judges the unit there, its spatial tests as the frame's gate
//! last left them ([`super::PlateSpots`]).

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_protocol::EntityKind;
use benilla_ui::script::live::LiveAsk;
use benilla_ui::script::{PlateGateQuery, PlateUnit, PlateUnitQuery};

use crate::net::{NetEntity, ObjectStore, SelfPlayer};

/// What a plate's callback and the gate inside a packet read: the unit by its guid as of the
/// place they run, the relation inputs, the name cache and the form table, the raid board, the
/// script bits and the frame gate's spatial verdicts.
#[derive(SystemParam)]
pub(crate) struct NamePlateLive<'w, 's> {
    index: Option<Res<'w, crate::net::GuidIndex>>,
    stores: crate::ui_unit::views::ViewStores<'w, 's>,
    kinds: Query<'w, 's, &'static NetEntity>,
    me: Query<'w, 's, Entity, With<SelfPlayer>>,
    relations: crate::target::RelationWorld<'w, 's>,
    names: Option<Res<'w, crate::names::NameCache>>,
    spells: Option<Res<'w, crate::ui_action::Spells>>,
    group: Option<Res<'w, crate::ui_party::GroupState>>,
    mode: Option<Res<'w, super::VPlateMode>>,
    spots: Option<Res<'w, super::PlateSpots>>,
}

impl NamePlateLive<'_, '_> {
    /// The unit `guid` names as of here (`0x468460`, mask 8), whether it is a player, and its
    /// store.
    fn held(&self, guid: u64) -> Option<(bool, &ObjectStore)> {
        let entity = *self.index.as_deref()?.0.get(&guid)?;
        let kind = self.kinds.get(entity).ok()?.kind;
        if !matches!(kind, EntityKind::Unit | EntityKind::Player) {
            return None;
        }
        let store = self.stores.get(entity).ok().filter(|s| s.is_unit())?;
        Some((kind == EntityKind::Player, store))
    }

    /// `f` with the judge over the reads as of here, the party slots `party` in place of the
    /// roster's when given.
    fn judge<R>(&self, party: Option<[u64; 4]>, f: impl FnOnce(&super::gate::Judge) -> R) -> R {
        let standing = party.map(|party| crate::spell::group_relation::GroupRoster {
            party: party.into_iter().filter(|&g| g != 0).collect(),
            raid: self
                .relations
                .roster()
                .map(|r| r.raid.clone())
                .unwrap_or_default(),
        });
        let my_store = self.me.single().ok().and_then(|e| self.stores.get(e).ok());
        let my_level = my_store.and_then(|s| s.0.unit_level()).unwrap_or(1);
        let my_drunk = my_store.and_then(|s| s.0.player_drunk_byte()).unwrap_or(0);
        let unit_of = |guid: u64, store: &ObjectStore| self.relations.unit_of(guid, store);
        let mut rel = self.relations.get();
        if let Some(standing) = standing.as_ref() {
            rel.roster = Some(standing);
        }
        let judge = super::gate::Judge {
            rel,
            me: self.relations.me(),
            unit_of: &unit_of,
            types: crate::creature_type::CreatureTypeSources {
                names: self.names.as_deref(),
                forms: self.spells.as_deref().map(|s| &s.forms),
                words: None,
            },
            names: self.names.as_deref(),
            my_level,
            my_drunk,
            group: self.group.as_deref(),
            mode: self.mode.as_deref().copied().unwrap_or_default(),
        };
        f(&judge)
    }

    /// The unit as a callback reads it now, with the party slots `party` when given.
    fn unit(&self, guid: u64, party: Option<[u64; 4]>) -> Option<PlateUnit> {
        let (is_player, store) = self.held(guid)?;
        Some(self.judge(party, |j| j.unit_now(guid, is_player, Some(store))))
    }

    /// The gate on `guid` as of here: its spatial tests as the frame's gate last passed them (the
    /// scene node drawn, `0x60f649`; not the camera's unit, `0x60f68c`; projected into view,
    /// `0x60f773`; within 20 yards, `0x60f7b8`), and its own as the unit stands now.
    fn gate(&self, guid: u64) -> Option<PlateUnit> {
        if !self.spots.as_deref().is_some_and(|s| s.0.contains(&guid)) {
            return None;
        }
        let (is_player, store) = self.held(guid)?;
        self.judge(None, |j| {
            let rank = j.admits(guid, is_player, Some(store))?;
            Some(j.unit(guid, is_player, Some(store), rank))
        })
    }
}

impl crate::script_calls::live::LiveFamily for NamePlateLive<'_, '_> {
    fn answer(&mut self, ask: &mut LiveAsk<'_>) -> bool {
        ask.answer(|q: &PlateUnitQuery| self.unit(q.guid, q.party))
            || ask.answer(|q: &PlateGateQuery| Some(self.gate(q.0)))
    }
}
