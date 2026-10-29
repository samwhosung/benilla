//! `DropItemOnUnit 0x48d960`: a held item dropped on a unit, which in 1.12 feeds the pet. On the
//! pet (the guid global `[0xb714a0]`) the validator `0x6ea1e0` casts the learned Feed Pet with no
//! cast item, so the wire is `CMSG_CAST_SPELL` with `TARGET_FLAG_ITEM` and the food's guid, never
//! the pet's (`0x6e5513`-`0x6e55a7`). On another player of our faction group the trade leg runs
//! first: our open trade partner gets the item in a slot (`0x4bfc50`), and with no trade open the
//! drop asks to trade with the cursor-drop mode (`0x5d3fb0` with 1), so the window opens with the
//! item in it. The feed's own gates are silent, the attempt's rungs report as any cast's, and each
//! refusal keeps the payload (`ClearCursor 0x495190` runs on success only); the server alone
//! judges range. A left click on another player's model or
//! plate runs the same trade leg from the world click (`0x4925d0`), with its own gates.

use bevy::prelude::*;

use benilla_ui::script::CursorPayload;

use crate::spell::{CastCommit, TargetedBind};

/// `0x6ea1e0`'s gates on the unit, in order: is this a pet I can feed? The food is not checked
/// here; the server judges the diet (`Spell.cpp:5912`, `SPELL_FAILED_WRONG_PET_FOOD`).
fn feedable_pet(pet: &benilla_protocol::ObjectFields, self_guid: u64) -> bool {
    // 1: `UNIT_FIELD_PETNUMBER` is set (`0x6ea1fd`, unit block `+0x214`): a pet or a charm.
    pet.unit_pet_number() != 0
        // 2: `UNIT_FIELD_CREATEDBY` is my guid (`0x6ea24f`-`0x6ea25c`).
        && pet.unit_created_by() == Some(self_guid)
}

/// `DropItemOnUnit(unit)`'s legs (`0x48d960`) inside its call
/// ([`benilla_ui::script::ScriptCall::DropItemOnUnit`]): the trade leg, then the pet leg on a token
/// that resolves to the pet's unit (`0x48da6c`-`0x48da7f`, guids compared, so `"target"` and
/// `"targettarget"` feed a targeted pet). The world click's entry into the pet leg, a left click
/// on the pet's model with an item held (`0x4927e8`), is [`ClickFeed`], inside the click's call.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct DropItemLeg<'w, 's> {
    trade: TradeLeg<'w, 's>,
    cast: crate::spell::ScriptCast<'w, 's>,
    self_q: Query<
        'w,
        's,
        (&'static crate::net::Guid, &'static crate::net::ObjectStore),
        With<crate::net::SelfPlayer>,
    >,
    stores: Query<'w, 's, &'static crate::net::ObjectStore>,
    index: Res<'w, crate::net::GuidIndex>,
    learned: Res<'w, super::LearnedAbilities>,
}

impl DropItemLeg<'_, '_> {
    /// The drop on the unit `token` names.
    pub(crate) fn drop_on(&mut self, script: &mut benilla_ui::script::UiScript, token: &str) {
        // Only an item on the cursor drops (`0x48d98f`-`0x48d9aa`).
        if !matches!(script.cursor_payload(), Some(CursorPayload::Item(_))) {
            return;
        }
        let pet_guid = self.cast.ladder.pet_guid();
        let Some((self_guid, self_store)) = self.self_q.iter().next() else {
            return;
        };
        self.trade.drop_on(script, token);
        let Some(guid) = self.trade.unit_guid(token) else {
            return;
        };
        let leg = PetLeg {
            pet_guid,
            pet: self
                .index
                .0
                .get(&pet_guid)
                .and_then(|&e| self.stores.get(e).ok()),
            me: self_guid.0,
            me_store: self_store,
            feed_pet: self.learned.feed_pet,
        };
        leg.run(script, &mut self.cast, guid);
    }
}

/// The world click's pet leg (`0x492793`-`0x4927f8`, in its object leg `0x4925d0`, ahead of the
/// greeting and the select): a left click on our pet's model with an item held feeds it through
/// `0x6ea1e0`, and the cursor clears on success. The press pick names the unit: the live hover can
/// differ after a drag.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct ClickFeed<'w, 's> {
    press: Res<'w, crate::target::PressPick>,
    cast: crate::spell::ScriptCast<'w, 's>,
    self_q: Query<
        'w,
        's,
        (&'static crate::net::Guid, &'static crate::net::ObjectStore),
        With<crate::net::SelfPlayer>,
    >,
    stores: Query<'w, 's, &'static crate::net::ObjectStore>,
    index: Res<'w, crate::net::GuidIndex>,
    learned: Res<'w, super::LearnedAbilities>,
}

impl ClickFeed<'_, '_> {
    /// The leg on the left click's release.
    pub(crate) fn click(&mut self, script: &mut benilla_ui::script::UiScript) {
        let pet_guid = self.cast.ladder.pet_guid();
        if pet_guid == 0 || self.press.hovered.guid != Some(pet_guid) {
            return;
        }
        // The held item `[0xb4e248]` (`0x4927b9`-`0x4927c8`).
        if !matches!(script.cursor_payload(), Some(CursorPayload::Item(_))) {
            return;
        }
        let Some((self_guid, self_store)) = self.self_q.iter().next() else {
            return;
        };
        let leg = PetLeg {
            pet_guid,
            pet: self
                .index
                .0
                .get(&pet_guid)
                .and_then(|&e| self.stores.get(e).ok()),
            me: self_guid.0,
            me_store: self_store,
            feed_pet: self.learned.feed_pet,
        };
        leg.run(script, &mut self.cast, pet_guid);
    }
}

/// What the pet leg of `DropItemOnUnit` reads: the pet's guid and object, ours, and the latched
/// Feed Pet.
struct PetLeg<'a> {
    pet_guid: u64,
    pet: Option<&'a crate::net::ObjectStore>,
    me: u64,
    me_store: &'a crate::net::ObjectStore,
    feed_pet: Option<u32>,
}

impl PetLeg<'_> {
    /// The pet leg on the unit `guid`: the pet test, `0x6ea1e0`'s gates, the feed, and on its
    /// success the cursor cleared.
    fn run(
        &self,
        script: &mut benilla_ui::script::UiScript,
        cast: &mut crate::spell::ScriptCast,
        guid: u64,
    ) {
        // The pet test: the unit's guid against the pet's (`0x48da6c`-`0x48da7f`).
        if self.pet_guid == 0 || guid != self.pet_guid {
            return;
        }
        let Some(pet) = self.pet else {
            return; // not streamed in: silent, payload kept
        };
        // The Feed Pet latch `[0xcecad8]` with a `Spell.dbc` record (`0x6ea207`-`0x6ea225`): an
        // imp passes the unit's gates, but only hunters learn it.
        let Some(spell_id) = self.feed_pet.filter(|&id| {
            cast.ladder
                .spells
                .as_ref()
                .is_some_and(|s| s.catalog.get(id).is_some())
        }) else {
            return;
        };
        if !feedable_pet(&pet.0, self.me) {
            debug!("DropItemOnUnit — not a pet I made; refused, payload kept");
            return;
        }
        // The food: the held item's live guid. Only an item payload can be dropped on a unit.
        let Some(CursorPayload::Item(held)) = script.cursor_payload() else {
            return;
        };
        let slot0 = u8::try_from(held.slot.saturating_sub(1)).unwrap_or(0);
        let Some(item_guid) =
            crate::ui_items::slot_guid(&self.me_store.0, held.bag, slot0, &cast.ladder.objects)
        else {
            return; // the slot emptied: silent
        };
        if feed(cast, spell_id, item_guid) {
            // `ClearCursor(1,1)`, on success only (`0x48da8d`-`0x48da96`).
            script.clear_cursor_payload();
        }
    }
}

/// `0x6ea266`-`0x6ea274`: Feed Pet through the attempt as the active player, no item, the
/// selection as its candidate (`0x6e5a90(spell, 0, 0)`), whose every rung may refuse it; the
/// attempt raises the cursor for the item its word wants, and `BindTarget 0x6e5b40` binds the
/// food to it and sends. Whether the bind sent, the answer `DropItemOnUnit` clears the cursor on.
fn feed(cast: &mut crate::spell::ScriptCast, spell_id: u32, item_guid: u64) -> bool {
    let crate::spell::ScriptCast { targeting, ladder } = cast;
    ladder.send(spell_id, &targeting.context(), CastCommit::Spell);
    let Some((pending, commit)) = ladder.ground.wanting_item() else {
        debug!("DropItemOnUnit — Feed Pet {spell_id} refused by the attempt; payload kept");
        return false;
    };
    if pending != spell_id {
        return false;
    }
    debug!("DropItemOnUnit — Feed Pet {spell_id} at item {item_guid:#x}");
    ladder.commit_targeted(spell_id, commit, TargetedBind::Item(item_guid));
    true
}

/// What the trade leg of `DropItemOnUnit` reads and writes: the unit resolver, the relation the
/// faction gate asks and the trade session.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct TradeLeg<'w, 's> {
    tokens: crate::ui_unit::UnitTokens<'w, 's>,
    selection: Res<'w, crate::target::Selection>,
    relations: crate::target::RelationWorld<'w, 's>,
    trade: Option<ResMut<'w, crate::ui_trade::TradeSession>>,
    net: Res<'w, crate::net::NetCommands>,
    errors: Res<'w, crate::ui_action::UiErrorKeys>,
    /// The initiate's name lookup (`0x5d4031`).
    names: Res<'w, crate::names::NameCache>,
    /// The bags the world click's held-item tests read, lent with the host out; with no cooldown
    /// list, which a system holding this may hold to write, as those tests read none.
    bags: Option<crate::ui_items::BagReads<'w, 's>>,
}

impl TradeLeg<'_, '_> {
    /// The guid of the unit `token` names, held as a unit (`0x48d9be`, `0x48d9db`), or `None`.
    fn unit_guid(&self, token: &str) -> Option<u64> {
        let (entity, guid) = self.tokens.resolve(token, &self.selection)?;
        let store = self.tokens.stores.get(entity).ok()?;
        store.is_unit().then_some(guid)
    }

    /// `DropItemOnUnit`'s player leg (`0x48d9e4`-`0x48da62`): `token` names a unit (`0x48d9db`)
    /// that is a player (`0x48d9e4`) other than us (`0x48d9f5`), the active player's object is held
    /// (`0x48da13`) and `CanCooperate` passes (`0x48da1f`). Our open trade partner (`[0xb71728]`,
    /// `0x48da36`) takes the item through `0x4bfc50` (`0x48da50`); with no partner the drop sends
    /// `CMSG_INITIATE_TRADE` with mode 1 (`0x48da62`); a trade with someone else takes nothing.
    fn drop_on(&mut self, script: &mut benilla_ui::script::UiScript, token: &str) {
        use crate::ui_session::LatchedWindow;
        let Some(trade) = self.trade.as_deref_mut() else {
            return;
        };
        let Some((entity, guid)) = self.tokens.resolve(token, &self.selection) else {
            return;
        };
        let Ok(store) = self.tokens.stores.get(entity) else {
            return;
        };
        let is_player = store.0.object_type_mask().unwrap_or(0) & 0x10 != 0;
        if !store.is_unit() || !is_player || self.relations.me_guid() == Some(guid) {
            return;
        }
        let Some(me) = self.relations.me() else {
            return;
        };
        let unit = self.relations.unit_of(guid, store);
        if !self.relations.get().can_cooperate(&me, &unit) {
            return;
        }
        match trade.cell() {
            Some(partner) if partner == guid => offer(script),
            Some(_) => {}
            None => ask(trade, &self.net, &self.names, &self.errors, guid),
        }
    }

    /// The world click's player leg (`0x492635`-`0x49278e`), on the clicked object `entity`: a
    /// player (`0x492635`, ourselves not excepted), an item or coins on the cursor (`0x49263f`-
    /// `0x492662`) and no spell awaiting a target (`0x492668`, the caller's test). A held bag
    /// with items, or an empty one worn, is refused (`0x4926f4`, `0x49271f`), then a dead active
    /// player (`0x492744`). Our open trade partner takes the item through `0x4bfc50`
    /// (`0x49276b`), which takes no coins; anyone else with a trade open is `ERR_ALREADY_TRADING`
    /// (`0x492777`), and with none the click asks to trade in the cursor-drop mode (`0x49278e`).
    fn click_on(
        &mut self,
        script: &mut benilla_ui::script::UiScript,
        entity: Entity,
        guid: u64,
        dead: bool,
    ) {
        let reads = self.bags.take();
        match reads.as_ref() {
            Some(reads) => script.reading(&mut reads.with(None), |script| {
                self.click_held(script, entity, guid, dead)
            }),
            None => self.click_held(script, entity, guid, dead),
        }
        self.bags = reads;
    }

    /// [`Self::click_on`] with the bags lent.
    fn click_held(
        &mut self,
        script: &mut benilla_ui::script::UiScript,
        entity: Entity,
        guid: u64,
        dead: bool,
    ) {
        use crate::ui_session::LatchedWindow;
        let Some(trade) = self.trade.as_deref_mut() else {
            return;
        };
        let Ok(store) = self.tokens.stores.get(entity) else {
            return;
        };
        if store.0.object_type_mask().unwrap_or(0) & 0x10 == 0 {
            return;
        }
        let held = match script.cursor_payload() {
            Some(CursorPayload::Item(_)) => script.cursor_item_from_container(),
            Some(CursorPayload::Money(money)) => money.copper > 0,
            _ => false,
        };
        if !held {
            return;
        }
        if let Some(key) = script.trade_click_refusal() {
            self.errors.push(crate::ui_action::UiError::key(key));
            return;
        }
        if dead {
            self.errors
                .push(crate::ui_action::UiError::key("ERR_PLAYER_DEAD"));
            return;
        }
        match trade.cell() {
            Some(partner) if partner == guid => offer(script),
            Some(_) => self
                .errors
                .push(crate::ui_action::UiError::key("ERR_ALREADY_TRADING")),
            None => ask(trade, &self.net, &self.names, &self.errors, guid),
        }
    }
}

/// `0x4bfc50` on our open trade partner: the held item to a slot of ours.
fn offer(script: &mut benilla_ui::script::UiScript) {
    if let Err(e) = script.offer_cursor_item() {
        debug!("ui_trade: the offer of the held item raised: {e}");
    }
}

/// `0x5d3fb0` with mode 1: `CMSG_INITIATE_TRADE`, the window to open with what the cursor holds.
fn ask(
    trade: &mut crate::ui_trade::TradeSession,
    net: &crate::net::NetCommands,
    names: &crate::names::NameCache,
    errors: &crate::ui_action::UiErrorKeys,
    guid: u64,
) {
    info!(target: "trade", "a drop on {guid:#x}; sending CMSG_INITIATE_TRADE");
    trade.initiate_send(guid, true, net, names, errors);
}

/// The world click's trade leg (`0x4925d0`) for a nameplate's left click (`0x7cb910` →
/// `0x4949f0` action 1), ahead of the select that follows it in the same function (`0x49280c`)
/// and so ahead of the targeting cursor's bind, whose state it reads (`0x492668`). The body click's
/// is [`ClickTrade`], inside the click's call.
pub(crate) fn trade_on_plate_click(
    mut vm: crate::script_calls::VmWith<(
        TradeLeg<'static, 'static>,
        Res<'static, crate::spell::SpellTargeting>,
    )>,
    plate: Res<crate::vplates::PlateClicks>,
    me: Query<&crate::net::ObjectStore, With<crate::net::SelfPlayer>>,
    guids: Query<&crate::net::Guid>,
) {
    if plate.left.is_empty() {
        return;
    }
    vm.turn(|script, (mut trade, targeting)| {
        let Some(script) = script else {
            return;
        };
        // The active player (`0x4925f4`).
        let Some(me) = me.iter().next() else {
            return;
        };
        if targeting.active() {
            return;
        }
        let dead = me.0.is_dead_or_ghost();
        for (entity, guid) in plate
            .left
            .iter()
            .filter_map(|&e| guids.get(e).ok().map(|g| (e, g.0)))
        {
            trade.click_on(script, entity, guid, dead);
        }
    })
}

/// The body click's trade leg (`0x4925d0`), on the press pick's unit, past the router's charmed
/// return (`0x481f98`, in the click's call): the leg runs only with no spell awaiting a target
/// (`0x492668`).
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct ClickTrade<'w, 's> {
    trade: TradeLeg<'w, 's>,
    targeting: Res<'w, crate::spell::SpellTargeting>,
    press: Res<'w, crate::target::PressPick>,
    inspect: Res<'w, crate::ui_script::InspectMode>,
    me: Query<'w, 's, &'static crate::net::ObjectStore, With<crate::net::SelfPlayer>>,
}

impl ClickTrade<'_, '_> {
    /// The leg on the left click's release.
    pub(crate) fn click(&mut self, script: &mut benilla_ui::script::UiScript) {
        // The active player (`0x4925f4`).
        let Some(me) = self.me.iter().next() else {
            return;
        };
        if self.targeting.active() || self.inspect.enabled {
            return;
        }
        let dead = me.0.is_dead_or_ghost();
        if let (Some(entity), Some(guid)) = (self.press.hovered.target, self.press.hovered.guid) {
            self.trade.click_on(script, entity, guid, dead);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `0x6ea1e0`'s gates on the unit, each failed alone.
    #[test]
    fn the_feed_gates_are_the_pet_number_and_the_creator() {
        use benilla_protocol::messages::ObjectFields;
        const ME: u64 = 0xDEAD_BEEF;
        // Field 139 is `UNIT_FIELD_PETNUMBER`; 14/15 are `UNIT_FIELD_CREATEDBY`'s guid pair.
        let pet = |pet_number: u32, created_by: Option<u64>| {
            let mut pairs: Vec<(u16, u32)> = vec![(139, pet_number)];
            if let Some(g) = created_by {
                pairs.push((14, g as u32));
                pairs.push((15, (g >> 32) as u32));
            }
            ObjectFields::from_pairs(&pairs)
        };
        assert!(feedable_pet(&pet(55, Some(ME)), ME), "my own pet feeds");
        // 1: no pet number, though it names me.
        assert!(!feedable_pet(&pet(0, Some(ME)), ME));
        // 2: someone else's.
        assert!(!feedable_pet(&pet(55, Some(1)), ME));
    }
}
