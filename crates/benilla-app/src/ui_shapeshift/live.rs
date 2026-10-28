//! The stance bar's getters at the call, the call host's half of
//! [`benilla_ui::script::ShapeshiftList`] and [`benilla_ui::script::ShapeshiftForm`]: the stored
//! form list as the registrar's learns leave it ([`crate::ui_action::PlayerActions::forms`]), and
//! each entry's four values off the active player's store as of the drain's place, as `0x4b45c0`
//! reads the form byte, the aura slots and the usability walk `0x6e3d60` inside the call.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::live::LiveAsk;
use benilla_ui::script::{ShapeshiftForm, ShapeshiftFormView, ShapeshiftList};

use crate::items::Items;
use crate::spell::{usable, Cooldowns};
use crate::target::Selection;
use crate::ui_action::{PlayerActions, Spells};

/// What the stance bar's getters read as they run.
#[derive(SystemParam)]
pub(crate) struct StanceLive<'w, 's> {
    actions: Option<Res<'w, PlayerActions>>,
    spells: Option<Res<'w, Spells>>,
    objects: crate::net::Objects<'w, 's>,
    stores: crate::ui_unit::views::ViewStores<'w, 's>,
    // The usability walk's inputs (`0x6e3d60`), as the action bar's walk reads them.
    cooldowns: Option<Res<'w, Cooldowns>>,
    selection: Option<Res<'w, Selection>>,
    relations: crate::target::RelationWorld<'w, 's>,
    items: Option<Res<'w, Items>>,
    spell_mods: Option<Res<'w, crate::spell::SpellModifiers>>,
    own_control: Option<Res<'w, crate::player::OwnControlLost>>,
    skill_lines: Option<Res<'w, crate::ui_spellbook::SkillLines>>,
    sub_classes: Option<Res<'w, crate::ui_items::ItemSubClasses>>,
}

impl StanceLive<'_, '_> {
    /// The list's spells as they stand, `None` without the book.
    fn list(&self) -> Option<Vec<u32>> {
        Some(self.actions.as_deref()?.forms.clone())
    }

    /// Entry `index`, 0-based, as `0x4b45c0` reads it: `Some(None)` past the list (`0x4b4633`);
    /// `None` with no active player held (`0x4b4639`), where the VM's copy answers, and in a
    /// harness without the walk's inputs.
    fn form(&self, index: usize) -> Option<Option<ShapeshiftFormView>> {
        let (actions, spells) = (self.actions.as_deref()?, self.spells.as_deref()?);
        let (cooldowns, items) = (self.cooldowns.as_deref()?, self.items.as_deref()?);
        let spell_mods = self.spell_mods.as_deref()?;
        let Some((id, d)) = actions
            .forms
            .get(index)
            .and_then(|&id| Some((id, spells.catalog.get(id)?)))
        else {
            return Some(None);
        };
        let me = self.objects.player();
        let store = me.and_then(|e| self.stores.get(e).ok())?;
        let active = super::form_active(id, d, store.0.unit_shapeshift_form(), Some(store));
        // `isCastable`: 1 for the active form, else the walk `IsUsableAction` runs (`0x4b479c`).
        let castable = active || {
            let target = self.selection.as_deref().and_then(|s| s.guid);
            // Ourselves selected is no target, as the action bar's walk reads it.
            let target_store = target
                .and_then(|g| self.objects.entity(g))
                .filter(|&e| Some(e) != me)
                .and_then(|e| self.stores.get(e).ok());
            let stacks =
                crate::ui_items::stack_counts(&store.0, &self.stores.item_objects(&self.objects));
            let ctx = usable::UsableCtx {
                store,
                target: target.zip(target_store),
                rel: self.relations.get(),
                cooldowns,
                stacks: &stacks,
                spell_mods,
                control_lost: self.own_control.as_deref().is_some_and(|c| c.0),
                skill_lines: self.skill_lines.as_deref().map(|s| &s.catalog),
                sub_classes: self.sub_classes.as_deref().map(|s| &s.0),
            };
            usable::spell_usable(id, d, spells, &ctx, &self.objects, items).0
        };
        Some(Some(ShapeshiftFormView {
            spell_id: id,
            texture: super::form_texture(d, active),
            name: d.name.clone(),
            active,
            castable,
            cooldown: None,
        }))
    }
}

impl crate::script_calls::live::LiveFamily for StanceLive<'_, '_> {
    fn answer(&mut self, ask: &mut LiveAsk<'_>) -> bool {
        if ask.answer(|_: &ShapeshiftList| self.list()) {
            return true;
        }
        ask.answer(|q: &ShapeshiftForm| self.form(q.0))
    }
}
