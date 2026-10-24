//! The dressing-room drain: applies the stock window's `DressUpModel` verbs ([`DressUpIntent`]) in
//! call order, since `DressUpItem` can reset and try on in one call, and composes the booth's look
//! ([`DressUpPreview`]): the player's own gear with each substitution in its [`equip_slot`], the
//! held triple through [`held_lanes`]. A try-on whose template ([`Items::template`]) is cached
//! lands in its place in that order; one that is not lands when the record arrives.
//! There is no fit check: the reference previews whatever it is handed (`DressUpFrame.lua:2-16`).

use benilla_protocol::CharEnumItem;
use benilla_ui::script::DressUpIntent;
use bevy::prelude::*;

use crate::entities::equip_slot;
use crate::items::Items;
use crate::net::{NetCommands, NetEntity, ObjectStore, SelfPlayer};
use crate::portrait::{DressUpLook, DressUpPreview};
use crate::query_cache::Ask;
use crate::ui_script::UiDrain;

/// Every rendered equipment slot, the set [`equip_slot`] can map an item into.
const LOOK_SLOTS: [u8; 14] = [0, 2, 3, 4, 5, 6, 7, 8, 9, 14, 15, 16, 17, 18];

/// Main hand, off hand and ranged: three slots for the widget's two held lanes ([`held_lanes`]).
const HELD_SLOTS: [usize; 3] = [15, 16, 17];

/// One held lane, the widget's record at `+0x3fc` (main) or `+0x404` (off) and whether its model
/// is still attached. An eviction detaches the model and keeps the record (`0x504a0c`, `0x504a4f`
/// store nothing), and the next lane choice reads the record (`0x504935`).
#[derive(Clone, Copy)]
struct Lane {
    item: CharEnumItem,
    attached: bool,
}

/// The widget's two held lanes. `TryOn` (`0x504d90` → `0x5048e8`) installs into lane `0x0f`
/// (HandRight) or `0x10` (HandLeft, or the Shield point) by the item's InventoryType: a two-hander
/// (17), main-hand (21), thrown (25) or right-hand ranged (26) takes the main lane; a shield (14),
/// bow (15), off-hand (22) or holdable (23) the off lane (`0x504985`-`0x5049ab`). A one-hander (13)
/// takes the off lane when the main lane's record is a 13 or a 21, the model is the local
/// player's and dual wield is known (`0x504935`-`0x50495d`), else the main lane: the booth shows
/// only the player (`DressUpFrame.lua:5`, `Blizzard_AuctionDressUp.lua:8`). An item the other
/// lane cannot coexist with detaches it (`0x504bc0`), then the target lane's occupant goes.
///
/// Deviation: the base is the player's melee pair, held drawn, because the booth shares the
/// select screen's assembly; the reference clones the live model with its attachments, drawn or
/// sheathed (`0x5059a0`, `0x70ea00`). A worn ranged weapon, shown only while ranged-drawn, appears
/// once tried on.
fn held_lanes(equipment: &mut [CharEnumItem; 19], room: &DressUpRoom, dual_wield: bool) {
    // `SetUnit` seeds both lanes with the player's own pair (`0x5043e2`, `0x504414`); an undressed
    // look has none here, `Undress` having zeroed both (`0x5044f3`, `0x504530`).
    let base = |slot: usize| {
        (equipment[slot].display_id != 0).then_some(Lane {
            item: equipment[slot],
            attached: true,
        })
    };
    let (mut main, mut off) = (base(15), base(16));

    for &item in &room.held {
        let to_off = match item.inventory_type {
            13 => dual_wield && main.is_some_and(|l| matches!(l.item.inventory_type, 13 | 21)),
            14 | 15 | 22 | 23 => true,
            _ => false,
        };
        let (mine, other) = if to_off {
            (&mut off, &mut main)
        } else {
            (&mut main, &mut off)
        };
        if let Some(other) = other.as_mut() {
            let (m, o) = if to_off {
                (other.item.inventory_type, item.inventory_type)
            } else {
                (item.inventory_type, other.item.inventory_type)
            };
            if !lanes_coexist(m, o, dual_wield) {
                other.attached = false;
            }
        }
        *mine = Some(Lane {
            item,
            attached: true,
        });
    }

    // Write back by lane, never through `equip_slot`: the index picks the held law `held_wants`
    // asks (15 and 16 melee-drawn, 17 ranged-drawn with its own bow-left split), and an off-hand
    // one-hander is `INVTYPE_WEAPON` 13, which `equip_slot` maps to the main hand.
    for slot in HELD_SLOTS {
        equipment[slot] = CharEnumItem::default();
    }
    for (lane, held) in [(15usize, main), (16, off)] {
        let Some(Lane { item, .. }) = held.filter(|l| l.attached) else {
            continue;
        };
        let ranged = matches!(item.inventory_type, 15 | 25 | 26);
        equipment[if ranged { 17 } else { lane }] = item;
    }
}

/// `0x504bc0(main, off, dualWield)`: a one-hand main (13, 21) coexists with a shield or holdable
/// (14, 23), and with a one-hand off (13, 22) under dual wield; anything else evicts. The flag is
/// the local player's known dual-wield effect (`0x504974`-`0x504981`, `0x5049c6`-`0x5049d3`).
fn lanes_coexist(main: u8, off: u8, dual_wield: bool) -> bool {
    matches!(main, 13 | 21) && (matches!(off, 14 | 23) || (dual_wield && matches!(off, 13 | 22)))
}

/// The room's substitutions over the player's own gear, resolved and pending.
#[derive(Resource, Default)]
pub(crate) struct DressUpRoom {
    /// False after `Close`: the booth is empty.
    open: bool,
    /// `Undress()` has taken off the player's own pieces until the next `Dress`: `0x504490` clears
    /// component slots `0..0xb` (`0x5044b0`) and detaches and zeroes both held lanes
    /// (`0x5044c2`-`0x504532`).
    bare: bool,
    /// Resolved body substitutions by equipment slot; the held ones are [`Self::held`].
    worn: [Option<CharEnumItem>; 19],
    /// Held try-ons in landing order, latest last: each picks its lane from the lanes the earlier
    /// ones left ([`held_lanes`]).
    held: Vec<CharEnumItem>,
    /// Try-ons whose record the cache did not hold at the call, each with the callback `0x504ba0`
    /// registered (`0x504579`), read again every frame until the record lands.
    pending: Vec<u32>,
}

impl DressUpRoom {
    /// One verb, as its call does it. Returns 1 for a try-on that raised `ERR_NOT_EQUIPPABLE`.
    fn apply(&mut self, intent: DressUpIntent, items: &Items, commands: &NetCommands) -> u32 {
        match intent {
            // The rebuild (`0x504cd0` → `0x505b50` → `0x5059a0`) cancels no cache callback: a
            // try-on still waiting lands on the rebuilt model.
            DressUpIntent::Dress => {
                self.open = true;
                self.bare = false;
                self.worn = Default::default();
                self.held.clear();
            }
            DressUpIntent::Undress => {
                self.open = true;
                self.bare = true;
                // Every piece off, base (`bare`) and tried-on alike, the hands included.
                self.worn = Default::default();
                self.held.clear();
            }
            // The lookup with the callback (`0x504579`): a cached record goes on inside the call
            // (`0x5045dd`-`0x5045ee`), a missing one waits for `0x504ba0`.
            DressUpIntent::TryOn(item) => {
                self.open = true;
                match items.template(item, 0, Ask::Event(commands)) {
                    Some(t) => return self.land(t),
                    None => self.pending.push(item),
                }
            }
            DressUpIntent::Close => {
                self.open = false;
                self.bare = false;
                self.worn = Default::default();
                self.held.clear();
                self.pending.clear();
            }
        }
        0
    }

    /// One frame: the records that landed for the try-ons still waiting, whose callbacks
    /// (`0x504ba0`) ran as each arrived, ahead of the verbs called since; then those verbs in call
    /// order. Returns the `ERR_NOT_EQUIPPABLE` count to show.
    fn step(
        &mut self,
        intents: impl IntoIterator<Item = DressUpIntent>,
        items: &Items,
        commands: &NetCommands,
    ) -> u32 {
        let mut refused = self.resolve_pending(items);
        for intent in intents {
            refused += self.apply(intent, items, commands);
        }
        refused
    }

    /// The try-ons whose record has landed, in try-on order; an id the server never answers stays
    /// pending, and its slot shows the player's own gear. Returns the refusal count, as
    /// [`Self::land`].
    fn resolve_pending(&mut self, items: &Items) -> u32 {
        let mut refused = 0;
        for item in std::mem::take(&mut self.pending) {
            match items.template(item, 0, Ask::Read) {
                Some(t) => refused += self.land(t),
                None => self.pending.push(item),
            }
        }
        refused
    }

    /// `0x504540` with the record in hand, by its inventory type. Returns 1 when it is not
    /// equippable, an `ERR_NOT_EQUIPPABLE` to show ([`not_equippable`]).
    fn land(&mut self, t: &benilla_protocol::ItemInfo) -> u32 {
        // A record with no display does nothing (`0x50458b`-`0x504590`).
        if t.display_info_id == 0 {
            return 0;
        }
        if not_equippable(t.inventory_type) {
            return 1;
        }
        // Neck, finger, trinket and relic preview nothing (the map's 1 at `0x504b37`).
        if matches!(t.inventory_type, 2 | 11 | 12 | 28) {
            return 0;
        }
        let item = CharEnumItem {
            display_id: t.display_info_id,
            inventory_type: t.inventory_type as u8,
        };
        match equip_slot(item.inventory_type) {
            // The held arms pick their lane as the look is composed.
            Some(slot) if HELD_SLOTS.contains(&slot) => self.held.push(item),
            Some(slot) => self.worn[slot] = Some(item),
            None => {}
        }
        0
    }
}

/// `TryOn`'s dispatch, `dec eax; cmp eax,0x1b; ja` then the byte map at `0x504b7c`
/// (`0x5045a4`-`0x5045b8`): non-equip (0, an unsigned -1 there), bag (18), ammo (24), quiver (27)
/// and anything above 28 show `ERR_NOT_EQUIPPABLE` (`0x504b2d`, `push 0x13; call 0x496720`).
fn not_equippable(inventory_type: u32) -> bool {
    matches!(inventory_type, 0 | 18 | 24 | 27) || inventory_type > 28
}

pub(crate) struct DressUpUiPlugin;

impl Plugin for DressUpUiPlugin {
    fn build(&self, app: &mut App) {
        // A drain, the frame its verbs are called: a try-on's cache lookup reads the cache as it
        // stood at the call, before the next frame's packets land.
        app.init_resource::<DressUpRoom>()
            .add_systems(Update, drain_dressup.in_set(UiDrain));
    }
}

fn drain_dressup(
    mut hosted_vm: crate::script_calls::HostedVm,
    mut room: ResMut<DressUpRoom>,
    mut preview: ResMut<DressUpPreview>,
    items: Res<Items>,
    commands: Res<NetCommands>,
    self_q: Query<(&ObjectStore, &NetEntity), With<SelfPlayer>>,
    // A miss sends the `CMSG_GUILD_QUERY` whose answer paints the tabard.
    guilds: Res<crate::ui_guild::GuildState>,
    // The known spells the dual-wield flag `0x5eab70` reads.
    known: (
        Res<crate::ui_action::PlayerActions>,
        Option<Res<crate::ui_action::Spells>>,
    ),
    mut sink: crate::ui_action::MessageSink,
) {
    hosted_vm.run(|script| {
        let Some(script) = script else {
            return;
        };
        let mut intents = script.take_dressup_intents();
        // The room empties when neither the dressing room nor the auction house's
        // `AuctionDressUpFrame` is shown, read off the frames (the stock files carry no hook of
        // ours), and a verb on a hidden pane does nothing (`0x504596`). The reference's widget
        // keeps its state while hidden, but `DressUpItem` re-issues `SetUnit("player")` on a
        // hidden frame, so the kept state is never seen.
        if !script.frame_visible("DressUpFrame") && !script.frame_visible("AuctionDressUpFrame") {
            intents = vec![DressUpIntent::Close];
        }
        // A refusal shows as its record lands: in the call when cached, else from the cache's
        // callback (`0x504ba0`).
        let refused = room.step(intents, &items, &commands);
        let lines = (0..refused)
            .filter_map(|_| crate::ui_action::keyed_line(script, "ERR_NOT_EQUIPPABLE"))
            .collect::<Vec<_>>();
        crate::ui_action::show_messages(script, &mut sink, "ui_dressup", lines);

        // The crest comes off the guild cache this system holds, not the outfit `player_look`
        // builds.
        let dual_wield = known.0.knows_dual_wield(known.1.as_deref());
        let look = match (room.open, self_q.single().ok()) {
            (true, Some((store, net))) => {
                player_look(store, net, &room, &items, dual_wield).map(|l| DressUpLook {
                    emblem: crate::ui_guild::unit_guild_emblem(
                        &store.0,
                        &guilds,
                        Ask::New(&commands),
                    ),
                    ..l
                })
            }
            _ => None,
        };
        if preview.look != look {
            preview.look = look;
        }
    })
}

/// The player's own look with the room's substitutions written in.
fn player_look(
    store: &ObjectStore,
    net: &NetEntity,
    room: &DressUpRoom,
    items: &Items,
    dual_wield: bool,
) -> Option<DressUpLook> {
    let s = &store.0;
    let mut equipment = [CharEnumItem::default(); 19];
    for slot in LOOK_SLOTS {
        let idx = slot as usize;
        // A substitution wins over the player's own piece.
        if let Some(worn) = room.worn[idx] {
            equipment[idx] = worn;
            continue;
        }
        // Undressed: the player's own pieces are off, the held ones too.
        if room.bare {
            continue;
        }
        // Hidden in the world, hidden here: `SetUnit` (`0x505d70`) clones the live player's
        // bodyslot display pointers verbatim (`0x476cb0`). A try-on, handled above, gates on
        // nothing.
        if (idx == 0 && s.player_hides_helm()) || (idx == 14 && s.player_hides_cloak()) {
            continue;
        }
        let Some(entry) = s.player_visible_item_entry(slot).filter(|e| *e != 0) else {
            continue;
        };
        // The visible-item field carries an item entry; the display id is on its template, which
        // the field's change looked up (`0x5ed764`).
        if let Some(t) = items.template(entry, 0, Ask::Read) {
            equipment[idx] = CharEnumItem {
                display_id: t.display_info_id,
                inventory_type: t.inventory_type as u8,
            };
        }
    }
    held_lanes(&mut equipment, room, dual_wield);
    Some(DressUpLook {
        display_id: net.display_id?,
        race: s.unit_race()?,
        sex: s.unit_gender()?,
        skin: s.player_skin().unwrap_or(0),
        face: s.player_face().unwrap_or(0),
        hair_style: s.player_hair_style().unwrap_or(0),
        hair_color: s.player_hair_color().unwrap_or(0),
        facial_hair: s.player_facial_hair().unwrap_or(0),
        equipment,
        // `feed_dressup` stamps the crest in.
        emblem: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use benilla_protocol::{EntityKind, ItemInfo, ObjectFields};

    use crate::items::test_template;
    use crate::net::ClientCommand;

    /// A human male self-player wearing `entries` (slot, item entry). Field indices:
    /// `UNIT_FIELD_BYTES_0` 36, `PLAYER_BYTES` 193, `PLAYER_VISIBLE_ITEM_1_CREATOR` 258 + 12 per
    /// slot, entry at +2.
    fn player(entries: &[(u8, u32)]) -> ObjectStore {
        let mut pairs = vec![
            (36u16, 1 | 1 << 8),  // race 1 (human), class 1, gender 0 (male)
            (193u16, 3 | 4 << 8), // skin 3, face 4, hair 0, hair colour 0
        ];
        for (slot, entry) in entries {
            pairs.push((258 + 2 + 12 * u16::from(*slot), *entry));
        }
        ObjectStore(ObjectFields::from_pairs(&pairs))
    }

    fn net() -> NetEntity {
        NetEntity {
            kind: EntityKind::Player,
            display_id: Some(49),
            scale: 1.0,
        }
    }

    fn commands() -> (NetCommands, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        (NetCommands(tx), rx)
    }

    fn worn(name: &str, display_info_id: u32, inventory_type: u32) -> ItemInfo {
        ItemInfo {
            display_info_id,
            inventory_type,
            ..test_template(name)
        }
    }

    /// A try-on looks its item up with the callback `0x504ba0` (`0x504579`); the resolves while it
    /// waits read. A refused item is asked by no resolve after it, only by the next try-on.
    #[test]
    fn a_refused_try_on_is_asked_by_the_next_try_on_never_by_a_frame() {
        let (cmds, rx) = commands();
        let mut items = Items::default();
        let mut room = DressUpRoom::default();
        room.apply(DressUpIntent::Dress, &items, &cmds);
        room.apply(DressUpIntent::TryOn(2000), &items, &cmds);
        room.resolve_pending(&items);
        assert_eq!(crate::query_cache::queries(&rx).len(), 1);
        items.insert_template(2000, None);
        for _ in 0..600 {
            room.resolve_pending(&items);
        }
        assert!(
            crate::query_cache::queries(&rx).is_empty(),
            "no frame asks a refusal"
        );
        room.apply(DressUpIntent::TryOn(2000), &items, &cmds);
        room.resolve_pending(&items);
        assert_eq!(
            crate::query_cache::queries(&rx).len(),
            1,
            "the next try-on asks again"
        );
    }

    #[test]
    fn a_try_on_substitutes_only_its_own_slot() {
        let (cmds, _rx) = commands();
        let mut items = Items::default();
        // Worn: a chest (slot 4) and a sword in the main hand (slot 15).
        items.insert_template(1000, Some(worn("Worn Chest", 5000, 5)));
        items.insert_template(1500, Some(worn("Worn Sword", 5500, 21)));
        // Tried on: a different chest.
        items.insert_template(2000, Some(worn("Shiny Chest", 7000, 5)));
        let store = player(&[(4, 1000), (15, 1500)]);

        let mut room = DressUpRoom::default();
        room.apply(DressUpIntent::Dress, &items, &cmds);
        room.apply(DressUpIntent::TryOn(2000), &items, &cmds);
        room.resolve_pending(&items);

        let look = player_look(&store, &net(), &room, &items, true).expect("a look");
        assert_eq!(
            look.equipment[4].display_id, 7000,
            "the tried-on chest shows"
        );
        assert_eq!(
            look.equipment[15].display_id, 5500,
            "the sword the player is actually holding is untouched"
        );
        assert_eq!(look.race, 1);
        assert_eq!(look.sex, 0);
        assert_eq!((look.skin, look.face), (3, 4));
        assert_eq!(look.display_id, 49, "the player's own body");
    }

    /// Reset is `DressUpModel:Dress()`.
    #[test]
    fn reset_drops_substitutions_and_close_empties_the_room() {
        let (cmds, _rx) = commands();
        let mut items = Items::default();
        items.insert_template(1000, Some(worn("Worn Chest", 5000, 5)));
        items.insert_template(2000, Some(worn("Shiny Chest", 7000, 5)));
        let store = player(&[(4, 1000)]);

        let mut room = DressUpRoom::default();
        room.apply(DressUpIntent::TryOn(2000), &items, &cmds);
        room.resolve_pending(&items);
        assert_eq!(
            player_look(&store, &net(), &room, &items, true)
                .unwrap()
                .equipment[4]
                .display_id,
            7000
        );

        room.apply(DressUpIntent::Dress, &items, &cmds);
        assert_eq!(
            player_look(&store, &net(), &room, &items, true)
                .unwrap()
                .equipment[4]
                .display_id,
            5000,
            "Reset puts the player's own chest back on"
        );

        room.apply(DressUpIntent::Close, &items, &cmds);
        assert!(!room.open, "closing empties the room (the booth goes dark)");
    }

    /// The look with the dual-wield flag `0x504953` reads (`0x5eab70`, a known spell whose first
    /// effect is 40).
    fn look_dw(
        store: &ObjectStore,
        room: &DressUpRoom,
        items: &Items,
        dual_wield: bool,
    ) -> DressUpLook {
        player_look(store, &net(), room, items, dual_wield).expect("a look")
    }

    /// The held pair, main hand then off hand.
    fn hands(look: &DressUpLook) -> (u32, u32) {
        (look.equipment[15].display_id, look.equipment[16].display_id)
    }

    #[test]
    fn undress_strips_the_body_and_the_hands_and_a_try_on_lands_on_the_bare_body() {
        let (cmds, _rx) = commands();
        let mut items = Items::default();
        items.insert_template(1000, Some(worn("Worn Chest", 5000, 5)));
        items.insert_template(2000, Some(worn("Shiny Chest", 7000, 5)));
        items.insert_template(3000, Some(worn("Sword", 6000, 13)));
        items.insert_template(4000, Some(worn("Worn Helm", 8000, 1)));
        let store = player(&[(4, 1000), (15, 3000), (0, 4000)]);

        let mut room = DressUpRoom::default();
        room.apply(DressUpIntent::Dress, &items, &cmds);
        room.apply(DressUpIntent::TryOn(2000), &items, &cmds);
        room.resolve_pending(&items);
        room.apply(DressUpIntent::Undress, &items, &cmds);
        let look = player_look(&store, &net(), &room, &items, true).unwrap();
        assert_eq!(look.equipment[4].display_id, 0, "the tried-on chest is off");
        assert_eq!(look.equipment[0].display_id, 0, "the worn helm is off");
        assert_eq!(
            look.equipment[15].display_id, 0,
            "the sword is detached from the hand (0x5044ec)"
        );

        room.apply(DressUpIntent::TryOn(2000), &items, &cmds);
        room.resolve_pending(&items);
        let look = player_look(&store, &net(), &room, &items, true).unwrap();
        assert_eq!(
            look.equipment[4].display_id, 7000,
            "a try-on lands on the bare body"
        );
        assert_eq!(
            look.equipment[0].display_id, 0,
            "…which stays bare elsewhere"
        );

        room.apply(DressUpIntent::Dress, &items, &cmds);
        let look = player_look(&store, &net(), &room, &items, true).unwrap();
        assert_eq!(
            look.equipment[4].display_id, 5000,
            "Dress puts the player's own chest back"
        );
        assert_eq!(look.equipment[0].display_id, 8000);
        assert_eq!(look.equipment[15].display_id, 6000, "…and the sword");
    }

    /// `Undress` (`0x504c00` → `0x504490`) detaches both lanes and zeroes their records
    /// (`0x5044c2`-`0x504532`): the player's own pair and a tried-on weapon alike, ranged
    /// included, until the next `Dress`. A held try-on after it meets empty lanes.
    #[test]
    fn undress_empties_both_hands_until_the_next_dress() {
        let (cmds, _rx) = commands();
        let mut items = Items::default();
        items.insert_template(1500, Some(worn("Worn Sword", 5500, 21)));
        items.insert_template(1600, Some(worn("Worn Shield", 5600, 14)));
        items.insert_template(2500, Some(worn("Short Bow", 8500, 15)));
        items.insert_template(2600, Some(worn("Dagger", 8600, 13)));
        let store = player(&[(15, 1500), (16, 1600)]);

        let mut room = DressUpRoom::default();
        room.apply(DressUpIntent::Dress, &items, &cmds);
        room.apply(DressUpIntent::TryOn(2500), &items, &cmds);
        room.resolve_pending(&items);
        assert_eq!(
            look_dw(&store, &room, &items, true).equipment[17].display_id,
            8500
        );
        room.apply(DressUpIntent::Undress, &items, &cmds);
        let look = look_dw(&store, &room, &items, true);
        assert_eq!(hands(&look), (0, 0), "the player's own pair is off");
        assert_eq!(
            look.equipment[17].display_id, 0,
            "…and so is the tried-on bow"
        );

        // Both lane records are zero, so a one-hander takes the main lane (`0x504935`).
        room.apply(DressUpIntent::TryOn(2600), &items, &cmds);
        room.resolve_pending(&items);
        assert_eq!(hands(&look_dw(&store, &room, &items, true)), (8600, 0));

        room.apply(DressUpIntent::Dress, &items, &cmds);
        assert_eq!(
            hands(&look_dw(&store, &room, &items, true)),
            (5500, 5600),
            "Dress re-seeds both lanes from the player (0x504350)"
        );
    }

    /// `0x5048e8`: an `INVTYPE_WEAPON` takes the off lane when the main lane's record is a 13 or
    /// a 21 (`0x504935`, `0x50493f`-`0x50494d`), the model is the local player's and dual wield
    /// is known (`0x504953`); otherwise the main lane (`0x50495f`).
    #[test]
    fn a_tried_on_one_hander_goes_to_the_off_hand_beside_a_one_hander_under_dual_wield() {
        let (cmds, _rx) = commands();
        let mut items = Items::default();
        items.insert_template(1500, Some(worn("Worn Sword", 5500, 13)));
        items.insert_template(1510, Some(worn("Worn Mace", 5510, 21)));
        items.insert_template(1520, Some(worn("Worn Staff", 5520, 17)));
        items.insert_template(2000, Some(worn("New Sword", 7000, 13)));

        let tried = |held: &[(u8, u32)], dw: bool| {
            let mut room = DressUpRoom::default();
            room.apply(DressUpIntent::Dress, &items, &cmds);
            room.apply(DressUpIntent::TryOn(2000), &items, &cmds);
            room.resolve_pending(&items);
            hands(&look_dw(&player(held), &room, &items, dw))
        };
        assert_eq!(tried(&[(15, 1500)], true), (5500, 7000), "beside a 13");
        assert_eq!(tried(&[(15, 1510)], true), (5510, 7000), "beside a 21");
        assert_eq!(
            tried(&[(15, 1500)], false),
            (7000, 0),
            "no dual wield: the main hand takes it"
        );
        assert_eq!(
            tried(&[(15, 1520)], true),
            (7000, 0),
            "a two-hander is no one-hander: the main hand takes it"
        );
        assert_eq!(tried(&[], true), (7000, 0), "an empty main lane takes it");
    }

    /// Two one-handers tried on bare-handed under dual wield: the first takes the empty main lane,
    /// the second goes beside it.
    #[test]
    fn a_second_tried_on_one_hander_goes_beside_the_first() {
        let (cmds, _rx) = commands();
        let mut items = Items::default();
        items.insert_template(2000, Some(worn("Sword A", 7000, 13)));
        items.insert_template(2001, Some(worn("Sword B", 7001, 13)));
        let mut room = DressUpRoom::default();
        room.apply(DressUpIntent::Dress, &items, &cmds);
        room.apply(DressUpIntent::TryOn(2000), &items, &cmds);
        room.resolve_pending(&items);
        room.apply(DressUpIntent::TryOn(2001), &items, &cmds);
        room.resolve_pending(&items);
        assert_eq!(
            hands(&look_dw(&player(&[]), &room, &items, true)),
            (7000, 7001)
        );
        assert_eq!(
            hands(&look_dw(&player(&[]), &room, &items, false)),
            (7001, 0),
            "without dual wield the second replaces the first"
        );
    }

    /// `0x504bc0`'s dual-wield arm (fed at `0x504974`-`0x504981`, `0x5049c6`-`0x5049d3`): a
    /// one-hand off (13, 22) coexists with a one-hand main only under dual wield.
    #[test]
    fn an_off_hand_weapon_evicts_the_main_hand_without_dual_wield() {
        let (cmds, _rx) = commands();
        let mut items = Items::default();
        items.insert_template(1500, Some(worn("Worn Sword", 5500, 13)));
        items.insert_template(2000, Some(worn("Left Dagger", 7000, 22)));
        let mut room = DressUpRoom::default();
        room.apply(DressUpIntent::Dress, &items, &cmds);
        room.apply(DressUpIntent::TryOn(2000), &items, &cmds);
        room.resolve_pending(&items);
        let store = player(&[(15, 1500)]);
        assert_eq!(hands(&look_dw(&store, &room, &items, true)), (5500, 7000));
        assert_eq!(hands(&look_dw(&store, &room, &items, false)), (0, 7000));
    }

    /// An eviction detaches the other lane's model and keeps its record (`0x504a0c`, `0x504a4f`
    /// store nothing), and the lane choice reads the record: a sword evicted by a bow still sends
    /// a later one-hander to the off lane under dual wield.
    #[test]
    fn an_evicted_lane_keeps_its_record_for_the_next_lane_choice() {
        let (cmds, _rx) = commands();
        let mut items = Items::default();
        items.insert_template(1500, Some(worn("Worn Sword", 5500, 21)));
        items.insert_template(2500, Some(worn("Short Bow", 8500, 15)));
        items.insert_template(2600, Some(worn("Dagger", 8600, 13)));
        let store = player(&[(15, 1500)]);
        let mut room = DressUpRoom::default();
        room.apply(DressUpIntent::Dress, &items, &cmds);
        room.apply(DressUpIntent::TryOn(2500), &items, &cmds);
        room.resolve_pending(&items);
        room.apply(DressUpIntent::TryOn(2600), &items, &cmds);
        room.resolve_pending(&items);
        let look = look_dw(&store, &room, &items, true);
        assert_eq!(
            hands(&look),
            (0, 8600),
            "the dagger in the off hand, the sword still off"
        );
        assert_eq!(
            look.equipment[17].display_id, 0,
            "the bow went with its lane"
        );
    }

    /// `TryOn` of a cached record sets its slot inside the call (`0x504579` returns the record,
    /// `0x5045dd`-`0x5045ee` set the component), so an `Undress` after it in the same chunk
    /// (`0x504c00` → `0x504490`) takes it off again.
    #[test]
    fn a_cached_try_on_then_undress_in_one_chunk_leaves_the_body_bare() {
        let (cmds, _rx) = commands();
        let mut items = Items::default();
        items.insert_template(2000, Some(worn("Shiny Chest", 7000, 5)));
        items.insert_template(2500, Some(worn("Short Bow", 8500, 15)));
        let store = player(&[]);
        let mut room = DressUpRoom::default();
        room.step(
            [
                DressUpIntent::Dress,
                DressUpIntent::TryOn(2000),
                DressUpIntent::TryOn(2500),
                DressUpIntent::Undress,
            ],
            &items,
            &cmds,
        );
        let look = look_dw(&store, &room, &items, true);
        assert_eq!(look.equipment[4].display_id, 0, "the chest came off again");
        assert_eq!(look.equipment[17].display_id, 0, "…and the bow (0x504530)");
    }

    /// The cache's callback `0x504ba0` lands a record when it arrives, ahead of the script calls
    /// made after it: an `Undress` made once the record is in clears it.
    #[test]
    fn a_record_that_arrived_before_an_undress_is_cleared_by_it() {
        let (cmds, _rx) = commands();
        let mut items = Items::default();
        let store = player(&[]);
        let mut room = DressUpRoom::default();
        room.step(
            [DressUpIntent::Dress, DressUpIntent::TryOn(2000)],
            &items,
            &cmds,
        );
        items.insert_template(2000, Some(worn("Shiny Chest", 7000, 5)));
        room.step([DressUpIntent::Undress], &items, &cmds);
        assert_eq!(
            look_dw(&store, &room, &items, true).equipment[4].display_id,
            0,
            "the chest landed, then came off"
        );
    }

    /// `Dress` (`0x504cd0` → `0x505b50`) rebuilds the model and cancels no cache callback: a
    /// try-on still waiting for its record lands on the rebuilt model when it arrives
    /// (`0x504ba0` → `0x504540`).
    #[test]
    fn a_try_on_waiting_for_its_record_survives_dress_and_lands_after_it() {
        let (cmds, _rx) = commands();
        let mut items = Items::default();
        items.insert_template(1000, Some(worn("Worn Chest", 5000, 5)));
        let store = player(&[(4, 1000)]);
        let mut room = DressUpRoom::default();
        room.step(
            [DressUpIntent::Dress, DressUpIntent::TryOn(2000)],
            &items,
            &cmds,
        );
        room.step([DressUpIntent::Dress], &items, &cmds);
        assert_eq!(
            look_dw(&store, &room, &items, true).equipment[4].display_id,
            5000,
            "the player's own chest until the record comes"
        );
        items.insert_template(2000, Some(worn("Shiny Chest", 7000, 5)));
        room.step([], &items, &cmds);
        assert_eq!(
            look_dw(&store, &room, &items, true).equipment[4].display_id,
            7000,
            "the record lands on the rebuilt model"
        );
    }

    /// A chat-linked item usually misses the cache on the first click; the ask goes out once.
    #[test]
    fn an_unknown_item_waits_for_its_template_then_lands() {
        let (cmds, rx) = commands();
        let mut items = Items::default();
        items.insert_template(1000, Some(worn("Worn Chest", 5000, 5)));
        let store = player(&[(4, 1000)]);

        let mut room = DressUpRoom::default();
        room.apply(DressUpIntent::TryOn(2000), &items, &cmds);
        room.resolve_pending(&items);
        assert_eq!(room.pending, vec![2000], "still waiting on the answer");
        assert_eq!(
            player_look(&store, &net(), &room, &items, true)
                .unwrap()
                .equipment[4]
                .display_id,
            5000,
            "until it lands, the player's own gear is what shows"
        );
        let asks = rx
            .try_iter()
            .filter(|c| matches!(c, ClientCommand::ItemQuery { entry: 2000, .. }))
            .count();
        assert_eq!(asks, 1);

        items.insert_template(2000, Some(worn("Shiny Chest", 7000, 5)));
        room.resolve_pending(&items);
        assert!(room.pending.is_empty());
        assert_eq!(
            player_look(&store, &net(), &room, &items, true)
                .unwrap()
                .equipment[4]
                .display_id,
            7000,
            "the answer landing is what makes it show"
        );
    }

    #[test]
    fn a_non_worn_item_previews_nothing_and_does_not_linger() {
        let (cmds, _rx) = commands();
        let mut items = Items::default();
        items.insert_template(3000, Some(worn("Healing Potion", 9000, 0)));
        let mut room = DressUpRoom::default();
        room.apply(DressUpIntent::TryOn(3000), &items, &cmds);
        room.resolve_pending(&items);
        assert!(room.pending.is_empty(), "resolved, just not worn anywhere");
        assert!(room.worn.iter().all(Option::is_none));
    }

    /// A record with display id 0 does nothing (`0x50458b`-`0x504590`): no slot, no lane, no
    /// eviction.
    #[test]
    fn a_try_on_with_no_display_does_nothing() {
        let (cmds, _rx) = commands();
        let mut items = Items::default();
        items.insert_template(1500, Some(worn("Worn Sword", 5500, 21)));
        items.insert_template(1600, Some(worn("Worn Shield", 5600, 14)));
        items.insert_template(2000, Some(worn("Invisible Axe", 0, 17)));
        items.insert_template(2100, Some(worn("Invisible Chest", 0, 5)));
        let store = player(&[(15, 1500), (16, 1600)]);
        let mut room = DressUpRoom::default();
        let refused = room.step(
            [
                DressUpIntent::Dress,
                DressUpIntent::TryOn(2000),
                DressUpIntent::TryOn(2100),
            ],
            &items,
            &cmds,
        );
        assert_eq!(refused, 0, "and no error");
        assert!(room.pending.is_empty(), "resolved");
        let look = look_dw(&store, &room, &items, true);
        assert_eq!(hands(&look), (5500, 5600), "the two-hander evicted nothing");
        assert!(room.worn.iter().all(Option::is_none));
    }

    /// `0x5045a4`-`0x5045b8`: `dec eax; cmp eax,0x1b; ja` and the byte map at `0x504b7c` send
    /// types 0 (an unsigned -1), 18, 24, 27 and above 28 to `push 0x13; call 0x496720`
    /// (`0x504b2d`), `ERR_NOT_EQUIPPABLE`; 2, 11, 12 and 28 to the silent return (`0x504b37`).
    #[test]
    fn an_unequippable_try_on_raises_not_equippable() {
        let (cmds, _rx) = commands();
        let mut items = Items::default();
        for t in [0u32, 2, 11, 12, 18, 24, 27, 28, 29] {
            items.insert_template(3000 + t, Some(worn("Thing", 9000 + t, t)));
        }
        let refused = |t: u32| {
            let mut room = DressUpRoom::default();
            let n = room.step(
                [DressUpIntent::Dress, DressUpIntent::TryOn(3000 + t)],
                &items,
                &cmds,
            );
            assert!(room.worn.iter().all(Option::is_none) && room.held.is_empty());
            n
        };
        for t in [0, 18, 24, 27, 29] {
            assert_eq!(refused(t), 1, "type {t} is not equippable");
        }
        for t in [2, 11, 12, 28] {
            assert_eq!(refused(t), 0, "type {t} previews nothing, silently");
        }
    }

    /// A bow takes the off lane and clears the main; a crossbow takes the main and clears the off.
    #[test]
    fn a_tried_on_ranged_weapon_takes_a_hand_and_clears_the_melee_pair() {
        let (cmds, _rx) = commands();
        let mut items = Items::default();
        items.insert_template(1500, Some(worn("Worn Sword", 5500, 21))); // WEAPONMAINHAND
        items.insert_template(1600, Some(worn("Worn Shield", 5600, 14))); // SHIELD
        items.insert_template(2500, Some(worn("Short Bow", 8500, 15))); // RANGED
        items.insert_template(2600, Some(worn("Crossbow", 8600, 26))); // RANGEDRIGHT
        let store = player(&[(15, 1500), (16, 1600)]);

        for (item, display, what) in [(2500u32, 8500u32, "bow"), (2600, 8600, "crossbow")] {
            let mut room = DressUpRoom::default();
            room.apply(DressUpIntent::Dress, &items, &cmds);
            room.apply(DressUpIntent::TryOn(item), &items, &cmds);
            room.resolve_pending(&items);

            let look = player_look(&store, &net(), &room, &items, true).expect("a look");
            assert_eq!(
                look.equipment[17].display_id, display,
                "the {what} is in the ranged slot, which is what puts it in a hand"
            );
            assert_eq!(
                (look.equipment[15].display_id, look.equipment[16].display_id),
                (0, 0),
                "…and a ranged weapon coexists with neither melee lane (0x504bc0)"
            );
        }
    }

    /// The booth is melee-drawn ([`held_lanes`]); a ranged weapon shows only while ranged-drawn.
    #[test]
    fn a_worn_ranged_weapon_is_not_shown_until_it_is_tried_on() {
        let (cmds, _rx) = commands();
        let mut items = Items::default();
        items.insert_template(1500, Some(worn("Worn Sword", 5500, 21)));
        items.insert_template(1700, Some(worn("Worn Bow", 5700, 15)));
        let store = player(&[(15, 1500), (17, 1700)]);

        let mut room = DressUpRoom::default();
        room.apply(DressUpIntent::Dress, &items, &cmds);
        let look = player_look(&store, &net(), &room, &items, true).expect("a look");
        assert_eq!(
            look.equipment[17].display_id, 0,
            "the worn bow stays stowed"
        );
        assert_eq!(look.equipment[15].display_id, 5500, "the sword is in hand");
    }

    /// Eviction follows try-on order: a two-hander after a bow evicts the bow.
    #[test]
    fn a_later_melee_try_on_evicts_the_ranged_one() {
        let (cmds, _rx) = commands();
        let mut items = Items::default();
        items.insert_template(2500, Some(worn("Short Bow", 8500, 15)));
        items.insert_template(2700, Some(worn("Great Axe", 8700, 17))); // TWOHAND
        let store = player(&[]);

        let mut room = DressUpRoom::default();
        room.apply(DressUpIntent::Dress, &items, &cmds);
        room.apply(DressUpIntent::TryOn(2500), &items, &cmds);
        room.resolve_pending(&items);
        room.apply(DressUpIntent::TryOn(2700), &items, &cmds);
        room.resolve_pending(&items);

        let look = player_look(&store, &net(), &room, &items, true).expect("a look");
        assert_eq!(look.equipment[15].display_id, 8700, "the axe is in hand");
        assert_eq!(look.equipment[17].display_id, 0, "…and the bow is gone");
    }

    #[test]
    fn a_shield_coexists_with_a_one_hander_but_not_with_a_two_hander() {
        let (cmds, _rx) = commands();
        let mut items = Items::default();
        items.insert_template(1500, Some(worn("Worn Sword", 5500, 21)));
        items.insert_template(1550, Some(worn("Worn Axe", 5550, 17))); // TWOHAND
        items.insert_template(1600, Some(worn("Bright Shield", 5600, 14)));

        let mut room = DressUpRoom::default();
        room.apply(DressUpIntent::Dress, &items, &cmds);
        room.apply(DressUpIntent::TryOn(1600), &items, &cmds);
        room.resolve_pending(&items);

        let look = player_look(&player(&[(15, 1500)]), &net(), &room, &items, true).unwrap();
        assert_eq!(look.equipment[15].display_id, 5500, "the one-hander stays");
        assert_eq!(look.equipment[16].display_id, 5600, "…beside the shield");

        let look = player_look(&player(&[(15, 1550)]), &net(), &room, &items, true).unwrap();
        assert_eq!(
            look.equipment[15].display_id, 0,
            "the two-hander is evicted"
        );
        assert_eq!(look.equipment[16].display_id, 5600);
    }

    /// An off-hand one-hander is `INVTYPE_WEAPON` 13, which [`equip_slot`] maps to the main hand;
    /// each lane keeps its own item beside an unrelated try-on, for every off-lane type.
    #[test]
    fn a_dual_wielded_pair_keeps_both_hands() {
        let (cmds, _rx) = commands();
        let mut items = Items::default();
        items.insert_template(1500, Some(worn("Main Sword", 5500, 13))); // INVTYPE_WEAPON
        items.insert_template(2000, Some(worn("Shiny Boots", 7000, 8))); // FEET, the try-on

        for (entry, display, inv, what) in [
            (1600u32, 5600u32, 13u32, "a second one-hander"),
            (1601, 5601, 22, "an off-hand-only weapon"),
            (1602, 5602, 23, "a held-in-off-hand"),
            (1603, 5603, 14, "a shield"),
        ] {
            items.insert_template(entry, Some(worn("Off Hand", display, inv)));
            let store = player(&[(15, 1500), (16, entry)]);

            let mut room = DressUpRoom::default();
            room.apply(DressUpIntent::Dress, &items, &cmds);
            room.apply(DressUpIntent::TryOn(2000), &items, &cmds);
            room.resolve_pending(&items);

            let look = player_look(&store, &net(), &room, &items, true).expect("a look");
            assert_eq!(look.equipment[7].display_id, 7000, "the boots went on");
            assert_eq!(
                (look.equipment[15].display_id, look.equipment[16].display_id),
                (5500, display),
                "the main hand keeps its sword beside {what}"
            );
        }
    }

    /// The room inherits hide-helm and hide-cloak through the clone (`SetUnit` `0x505d70`, copying
    /// display pointers at `0x476cb0`); a try-on overrides them.
    #[test]
    fn a_hidden_helm_stays_hidden_on_the_mannequin_until_one_is_tried_on() {
        let (cmds, _rx) = commands();
        let mut items = Items::default();
        items.insert_template(1000, Some(worn("Worn Helm", 5000, 1)));
        items.insert_template(1100, Some(worn("Worn Cloak", 5100, 16)));
        items.insert_template(1200, Some(worn("Worn Chest", 5200, 5)));
        items.insert_template(2000, Some(worn("Shiny Helm", 7000, 1)));
        // `PLAYER_FLAGS` (field 190) carrying HIDE_HELM 0x400 | HIDE_CLOAK 0x800.
        let mut store = player(&[(0, 1000), (14, 1100), (4, 1200)]);
        store
            .0
            .merge(ObjectFields::from_pairs(&[(190u16, 0x400 | 0x800)]));

        let mut room = DressUpRoom::default();
        room.apply(DressUpIntent::Dress, &items, &cmds);
        let look = player_look(&store, &net(), &room, &items, true).expect("a look");
        assert_eq!(
            look.equipment[0].display_id, 0,
            "the player's own helm is hidden"
        );
        assert_eq!(look.equipment[14].display_id, 0, "and so is their cloak");
        assert_eq!(
            look.equipment[4].display_id, 5200,
            "everything else they are wearing is untouched"
        );

        // The reference's `TryOn` path never tests the flag.
        room.apply(DressUpIntent::TryOn(2000), &items, &cmds);
        room.resolve_pending(&items);
        let look = player_look(&store, &net(), &room, &items, true).expect("a look");
        assert_eq!(
            look.equipment[0].display_id, 7000,
            "the tried-on helm previews regardless of the preference"
        );

        // Reset drops the substitution: bare-headed again.
        room.apply(DressUpIntent::Dress, &items, &cmds);
        let look = player_look(&store, &net(), &room, &items, true).expect("a look");
        assert_eq!(
            look.equipment[0].display_id, 0,
            "Reset re-clones the hidden helm away"
        );
    }
}
