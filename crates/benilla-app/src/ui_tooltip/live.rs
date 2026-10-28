//! The spell and aura tooltip views built at the setter's call, the call host's half of their
//! questions ([`SpellViewQuery`], [`AuraViewQuery`]): the reference's builders read the player,
//! its pet and the caches as they stand at each call (`0x52e610`, `0x52f880`) and keep nothing,
//! so a listener's `SetSpell` reads what its own packet left.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::live::LiveAsk;
use benilla_ui::script::{AuraTooltipView, AuraViewQuery, SpellTooltipView, SpellViewQuery};

use super::spell_view::{build_aura_view, build_view, ViewCaster, ViewCtx};
use crate::items::Items;
use crate::net::{NetCommands, ObjectStore, Objects, SelfPlayer};
use crate::ui_action::Spells;

/// What the spell and aura builders read as they run: the catalogs, the active player and its pet,
/// their reaches and motion, the item cache, the bind point, the talent modifier tables and the
/// standings. Absent in a harness without them, where the setters read the store.
#[derive(SystemParam)]
pub(crate) struct TooltipLive<'w, 's> {
    spells: Res<'w, Spells>,
    commands: Res<'w, NetCommands>,
    items: Res<'w, Items>,
    spell_mods: Res<'w, crate::spell::SpellModifiers>,
    me: Query<'w, 's, &'static ObjectStore, With<SelfPlayer>>,
    /// The caster's and the pet's reach and motion and each one's auto-attack target, which the
    /// melee range cell reads.
    range_units: crate::spell::RangeUnits<'w, 's>,
    objects: Objects<'w, 's>,
    home_bind: Option<Res<'w, crate::net::HomeBind>>,
    area_names: Option<Res<'w, crate::ui_quest_log::QuestHeaderNamesRes>>,
    sub_classes: Option<Res<'w, crate::ui_items::ItemSubClasses>>,
    skill_lines: Option<Res<'w, crate::ui_spellbook::SkillLines>>,
    /// `Faction.dbc`, the reputation line's.
    factions: Option<Res<'w, crate::target::Factions>>,
    reputations: Option<Res<'w, crate::net::Reputations>>,
}

impl TooltipLive<'_, '_> {
    /// `f` over the builders' context as of here: against the player, or with `pet` against the
    /// player's charm, else its summon (the builder's unit selector, `0x6e3130`).
    fn with_ctx<R>(&self, pet: bool, f: impl FnOnce(&Spells, &ViewCtx) -> R) -> R {
        let store = self.me.single().ok();
        let home_area: Option<String> = self
            .home_bind
            .as_deref()
            .and_then(|b| b.0)
            .and_then(|id| self.area_names.as_deref()?.0.resolve(id as i32))
            .map(str::to_string);
        // The ranged-slot scale reads the active player's inventory slot 17 (`0x6e36c1`).
        let caster = crate::spell::with_ranged_slot(
            self.range_units.caster(),
            store.map(|s| &s.0),
            |g| self.objects.object(g),
            Some(&self.items),
        );
        let mut vctx = ViewCtx {
            factions: self.factions.as_deref().map(|f| f.catalog()),
            reputations: self.reputations.as_deref(),
            home_area: home_area.as_deref(),
            form: store.map_or(0, |s| s.0.unit_shapeshift_form()),
            store,
            caster: ViewCaster::Player,
            range_caster: caster,
            attack_target: self.range_units.caster_attack_target(),
            objects: &self.objects,
            items: &self.items,
            commands: &self.commands,
            sub_classes: self.sub_classes.as_deref().map(|c| &c.0),
            skill_lines: self.skill_lines.as_deref().map(|s| &s.catalog),
            spell_mods: &self.spell_mods,
        };
        if pet {
            // The pet views read the pet's level, form and reach where the player's read the
            // player's.
            let guid = store.and_then(|s| s.0.unit_pet_guid());
            let entity = guid.and_then(|g| self.objects.entity(g));
            let unit = guid.and_then(|g| self.objects.object(g));
            vctx.caster = ViewCaster::Pet(unit);
            vctx.form = unit.map_or(0, |p| p.unit_shapeshift_form());
            vctx.range_caster = entity.and_then(|e| self.range_units.unit(e)).unwrap_or(
                benilla_formats::RangeUnit::still(crate::spell::DEFAULT_REACH),
            );
            vctx.attack_target = entity.and_then(|e| self.range_units.attack_target(e));
        }
        f(&self.spells, &vctx)
    }

    fn spell_view(&self, q: &SpellViewQuery) -> Option<SpellTooltipView> {
        self.with_ctx(q.pet, |spells, vctx| build_view(q.spell_id, spells, vctx))
    }

    /// The aura builder passes no caster: its tokens expand against the player's context.
    fn aura_view(&self, q: &AuraViewQuery) -> Option<AuraTooltipView> {
        self.with_ctx(false, |spells, vctx| build_aura_view(q.0, spells, vctx))
    }
}

impl crate::script_calls::live::LiveFamily for TooltipLive<'_, '_> {
    fn answer(&mut self, ask: &mut LiveAsk<'_>) -> bool {
        ask.answer(|q: &SpellViewQuery| self.spell_view(q))
            || ask.answer(|q: &AuraViewQuery| self.aura_view(q))
    }
}
