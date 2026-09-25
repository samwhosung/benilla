//! The dressing-room feed: applies the stock window's `DressUpModel` verbs ([`DressUpIntent`]) in
//! order, since `DressUpItem` can reset and try on in one call, and composes the booth's look
//! ([`DressUpPreview`]): the player's own gear with each substitution in its [`equip_slot`], the
//! held triple through [`held_lanes`]. A tried-on item waits on its template ([`Items::template`]).
//! There is no fit check: the reference previews whatever it is handed (`DressUpFrame.lua:2-16`).

use benilla_protocol::CharEnumItem;
use benilla_ui::script::{DressUpIntent, UiScript};
use bevy::prelude::*;

use crate::entities::equip_slot;
use crate::items::Items;
use crate::net::{NetCommands, NetEntity, ObjectStore, SelfPlayer};
use crate::portrait::{DressUpLook, DressUpPreview};
use crate::ui_script::UiFeed;

/// Every rendered equipment slot, the set [`equip_slot`] can map an item into.
const LOOK_SLOTS: [u8; 14] = [0, 2, 3, 4, 5, 6, 7, 8, 9, 14, 15, 16, 17, 18];

/// Main hand, off hand and ranged: three slots for the widget's two held lanes ([`held_lanes`]).
const HELD_SLOTS: [usize; 3] = [15, 16, 17];

/// The widget's two held lanes. `TryOn` (`0x504d90`) installs into lane `0x0f` (HandRight) or
/// `0x10` (HandLeft, or the Shield point): a bow (15) takes the off lane, a gun, crossbow, wand
/// (26) or thrown (25) the main one. An item the other lane cannot coexist with evicts it
/// (`0x504bc0`), and a ranged weapon coexists with nothing.
///
/// Deviation: the base is the player's melee pair, held drawn, because the booth shares the
/// select screen's assembly; the reference clones the live model with its attachments, drawn or
/// sheathed (`0x5059a0`, `0x70ea00`). A worn ranged weapon, shown only while ranged-drawn, appears
/// once tried on.
fn held_lanes(equipment: &mut [CharEnumItem; 19], room: &DressUpRoom) {
    // The player's own melee pair; substitutions replay below.
    let base = |slot: usize| {
        (room.worn[slot].is_none() && equipment[slot].display_id != 0).then_some(equipment[slot])
    };
    let (mut main, mut off) = (base(15), base(16));

    for &slot in &room.held_order {
        let Some(item) = room.worn[slot] else {
            continue;
        };
        // A bow is the one ranged type that takes the off lane.
        let to_off = slot == 16 || (slot == 17 && item.inventory_type == 15);
        let (mine, other) = if to_off {
            (&mut off, &mut main)
        } else {
            (&mut main, &mut off)
        };
        if let Some(evicted) = *other {
            let (m, o) = if to_off {
                (evicted.inventory_type, item.inventory_type)
            } else {
                (item.inventory_type, evicted.inventory_type)
            };
            if !lanes_coexist(m, o) {
                *other = None;
            }
        }
        *mine = Some(item);
    }

    // Write back by lane, never through `equip_slot`: the index picks the held law `held_wants`
    // asks (15 and 16 melee-drawn, 17 ranged-drawn with its own bow-left split), and an off-hand
    // one-hander is `INVTYPE_WEAPON` 13, which `equip_slot` maps to the main hand.
    for slot in HELD_SLOTS {
        equipment[slot] = CharEnumItem::default();
    }
    for (lane, item) in [(15usize, main), (16, off)] {
        let Some(item) = item else { continue };
        let ranged = matches!(item.inventory_type, 15 | 25 | 26);
        equipment[if ranged { 17 } else { lane }] = item;
    }
}

/// `0x504bc0(main, off, dualWield)`: a one-hand main (13, 21) coexists with a shield or holdable
/// (14, 23), and with a one-hand off (13, 22) under dual-wield; anything else evicts. Dual-wield is
/// taken as allowed here; the reference reads it from `[0xc4d770]`, whose source is untraced.
fn lanes_coexist(main: u8, off: u8) -> bool {
    matches!(main, 13 | 21) && matches!(off, 13 | 14 | 22 | 23)
}

/// The room's substitutions over the player's own gear, resolved and pending.
#[derive(Resource, Default)]
pub(crate) struct DressUpRoom {
    /// False after `Close`: the booth is empty.
    open: bool,
    /// `Undress()` has taken off the player's own pieces until the next `Dress`. The hands keep
    /// theirs here; the reference's `0x504490` clears bodyslots `0..0xb` and both hand lanes.
    bare: bool,
    /// Resolved substitutions by equipment slot.
    worn: [Option<CharEnumItem>; 19],
    /// Held substitutions (slots 15-17) in landing order, latest last: a held try-on can evict the
    /// other lane, so the order decides what survives.
    held_order: Vec<usize>,
    /// Item ids still waiting on their template, retried every frame.
    pending: Vec<u32>,
}

impl DressUpRoom {
    fn apply(&mut self, intent: DressUpIntent) {
        match intent {
            DressUpIntent::Dress => {
                self.open = true;
                self.bare = false;
                self.worn = Default::default();
                self.held_order.clear();
                self.pending.clear();
            }
            DressUpIntent::Undress => {
                self.open = true;
                self.bare = true;
                // Every worn piece off, base (`bare`) and tried-on alike; held items stay.
                for slot in 0..self.worn.len() {
                    if !HELD_SLOTS.contains(&slot) {
                        self.worn[slot] = None;
                    }
                }
            }
            DressUpIntent::TryOn(item) => {
                self.open = true;
                self.pending.push(item);
            }
            DressUpIntent::Close => {
                self.open = false;
                self.bare = false;
                self.worn = Default::default();
                self.held_order.clear();
                self.pending.clear();
            }
        }
    }

    /// An id the server never answers stays pending, and its slot shows the player's own gear.
    fn resolve_pending(&mut self, items: &Items, commands: &NetCommands) {
        let mut pending = std::mem::take(&mut self.pending);
        pending.retain(|item| {
            let Some(t) = items.template(*item, 0, commands) else {
                return true; // in flight
            };
            let (display_id, inventory_type) = (t.display_info_id, t.inventory_type as u8);
            if let Some(slot) = equip_slot(inventory_type) {
                self.worn[slot] = Some(CharEnumItem {
                    display_id,
                    inventory_type,
                });
                // Held slots record their order, which eviction reads.
                if HELD_SLOTS.contains(&slot) {
                    self.held_order.retain(|&s| s != slot);
                    self.held_order.push(slot);
                }
            }
            // A non-worn item maps to no slot and previews nothing.
            false
        });
        self.pending = pending;
    }
}

pub(crate) struct DressUpUiPlugin;

impl Plugin for DressUpUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DressUpRoom>()
            .add_systems(Update, feed_dressup.in_set(UiFeed));
    }
}

fn feed_dressup(
    script: Option<NonSendMut<UiScript>>,
    mut room: ResMut<DressUpRoom>,
    mut preview: ResMut<DressUpPreview>,
    items: Res<Items>,
    commands: Res<NetCommands>,
    self_q: Query<(&ObjectStore, &NetEntity), With<SelfPlayer>>,
    // `ResMut` because the guild cache is lazy: a miss sends the `CMSG_GUILD_QUERY` whose answer
    // paints the tabard.
    mut guilds: ResMut<crate::ui_guild::GuildState>,
) {
    let Some(mut script) = script else {
        return;
    };
    for intent in script.take_dressup_intents() {
        room.apply(intent);
    }
    // The room empties when neither the dressing room nor the auction house's
    // `AuctionDressUpFrame` is shown, read off the frames (the stock files carry no hook of ours).
    // The reference's widget keeps its state while hidden, but `DressUpItem` re-issues
    // `SetUnit("player")` on a hidden frame, so the kept state is never seen.
    if room.open
        && !script.frame_visible("DressUpFrame")
        && !script.frame_visible("AuctionDressUpFrame")
    {
        room.apply(DressUpIntent::Close);
    }
    // The stock rotate buttons set the yaw with `DressUpModel:SetRotation`; the booth mirrors it.
    preview.yaw = script.model_pane_facing("DressUpModel");

    room.resolve_pending(&items, &commands);

    // The crest comes off the guild cache this system holds, not the outfit `player_look` builds.
    let look = match (room.open, self_q.single().ok()) {
        (true, Some((store, net))) => {
            player_look(store, net, &room, &items, &commands).map(|l| DressUpLook {
                emblem: crate::ui_guild::unit_guild_emblem(&store.0, &mut guilds, &commands),
                ..l
            })
        }
        _ => None,
    };
    if preview.look != look {
        preview.look = look;
    }
}

/// The player's own look with the room's substitutions written in.
fn player_look(
    store: &ObjectStore,
    net: &NetEntity,
    room: &DressUpRoom,
    items: &Items,
    commands: &NetCommands,
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
        // Undressed: the player's own pieces are off, the held ones stay.
        if room.bare && !HELD_SLOTS.contains(&idx) {
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
        // The visible-item field carries an item entry; the display id is on its template.
        if let Some(t) = items.template(entry, 0, commands) {
            equipment[idx] = CharEnumItem {
                display_id: t.display_info_id,
                inventory_type: t.inventory_type as u8,
            };
        }
    }
    held_lanes(&mut equipment, room);
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
        room.apply(DressUpIntent::Dress);
        room.apply(DressUpIntent::TryOn(2000));
        room.resolve_pending(&items, &cmds);

        let look = player_look(&store, &net(), &room, &items, &cmds).expect("a look");
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
        room.apply(DressUpIntent::TryOn(2000));
        room.resolve_pending(&items, &cmds);
        assert_eq!(
            player_look(&store, &net(), &room, &items, &cmds)
                .unwrap()
                .equipment[4]
                .display_id,
            7000
        );

        room.apply(DressUpIntent::Dress);
        assert_eq!(
            player_look(&store, &net(), &room, &items, &cmds)
                .unwrap()
                .equipment[4]
                .display_id,
            5000,
            "Reset puts the player's own chest back on"
        );

        room.apply(DressUpIntent::Close);
        assert!(!room.open, "closing empties the room (the booth goes dark)");
    }

    #[test]
    fn undress_strips_the_body_but_not_the_hands_and_a_try_on_lands_on_the_bare_body() {
        let (cmds, _rx) = commands();
        let mut items = Items::default();
        items.insert_template(1000, Some(worn("Worn Chest", 5000, 5)));
        items.insert_template(2000, Some(worn("Shiny Chest", 7000, 5)));
        items.insert_template(3000, Some(worn("Sword", 6000, 13)));
        items.insert_template(4000, Some(worn("Worn Helm", 8000, 1)));
        let store = player(&[(4, 1000), (15, 3000), (0, 4000)]);

        let mut room = DressUpRoom::default();
        room.apply(DressUpIntent::Dress);
        room.apply(DressUpIntent::TryOn(2000));
        room.resolve_pending(&items, &cmds);
        room.apply(DressUpIntent::Undress);
        let look = player_look(&store, &net(), &room, &items, &cmds).unwrap();
        assert_eq!(look.equipment[4].display_id, 0, "the tried-on chest is off");
        assert_eq!(look.equipment[0].display_id, 0, "the worn helm is off");
        assert_eq!(
            look.equipment[15].display_id, 6000,
            "the sword stays in the hand"
        );

        room.apply(DressUpIntent::TryOn(2000));
        room.resolve_pending(&items, &cmds);
        let look = player_look(&store, &net(), &room, &items, &cmds).unwrap();
        assert_eq!(
            look.equipment[4].display_id, 7000,
            "a try-on lands on the bare body"
        );
        assert_eq!(
            look.equipment[0].display_id, 0,
            "…which stays bare elsewhere"
        );

        room.apply(DressUpIntent::Dress);
        let look = player_look(&store, &net(), &room, &items, &cmds).unwrap();
        assert_eq!(
            look.equipment[4].display_id, 5000,
            "Dress puts the player's own chest back"
        );
        assert_eq!(look.equipment[0].display_id, 8000);
    }

    /// A chat-linked item usually misses the cache on the first click; the ask goes out once.
    #[test]
    fn an_unknown_item_waits_for_its_template_then_lands() {
        let (cmds, rx) = commands();
        let mut items = Items::default();
        items.insert_template(1000, Some(worn("Worn Chest", 5000, 5)));
        let store = player(&[(4, 1000)]);

        let mut room = DressUpRoom::default();
        room.apply(DressUpIntent::TryOn(2000));
        room.resolve_pending(&items, &cmds);
        assert_eq!(room.pending, vec![2000], "still waiting on the answer");
        assert_eq!(
            player_look(&store, &net(), &room, &items, &cmds)
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
        room.resolve_pending(&items, &cmds);
        assert!(room.pending.is_empty());
        assert_eq!(
            player_look(&store, &net(), &room, &items, &cmds)
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
        room.apply(DressUpIntent::TryOn(3000));
        room.resolve_pending(&items, &cmds);
        assert!(room.pending.is_empty(), "resolved, just not worn anywhere");
        assert!(room.worn.iter().all(Option::is_none));
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
            room.apply(DressUpIntent::Dress);
            room.apply(DressUpIntent::TryOn(item));
            room.resolve_pending(&items, &cmds);

            let look = player_look(&store, &net(), &room, &items, &cmds).expect("a look");
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
        room.apply(DressUpIntent::Dress);
        let look = player_look(&store, &net(), &room, &items, &cmds).expect("a look");
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
        room.apply(DressUpIntent::Dress);
        room.apply(DressUpIntent::TryOn(2500));
        room.resolve_pending(&items, &cmds);
        room.apply(DressUpIntent::TryOn(2700));
        room.resolve_pending(&items, &cmds);

        let look = player_look(&store, &net(), &room, &items, &cmds).expect("a look");
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
        room.apply(DressUpIntent::Dress);
        room.apply(DressUpIntent::TryOn(1600));
        room.resolve_pending(&items, &cmds);

        let look = player_look(&player(&[(15, 1500)]), &net(), &room, &items, &cmds).unwrap();
        assert_eq!(look.equipment[15].display_id, 5500, "the one-hander stays");
        assert_eq!(look.equipment[16].display_id, 5600, "…beside the shield");

        let look = player_look(&player(&[(15, 1550)]), &net(), &room, &items, &cmds).unwrap();
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
            room.apply(DressUpIntent::Dress);
            room.apply(DressUpIntent::TryOn(2000));
            room.resolve_pending(&items, &cmds);

            let look = player_look(&store, &net(), &room, &items, &cmds).expect("a look");
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
        room.apply(DressUpIntent::Dress);
        let look = player_look(&store, &net(), &room, &items, &cmds).expect("a look");
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
        room.apply(DressUpIntent::TryOn(2000));
        room.resolve_pending(&items, &cmds);
        let look = player_look(&store, &net(), &room, &items, &cmds).expect("a look");
        assert_eq!(
            look.equipment[0].display_id, 7000,
            "the tried-on helm previews regardless of the preference"
        );

        // Reset drops the substitution: bare-headed again.
        room.apply(DressUpIntent::Dress);
        let look = player_look(&store, &net(), &room, &items, &cmds).expect("a look");
        assert_eq!(
            look.equipment[0].display_id, 0,
            "Reset re-clones the hidden helm away"
        );
    }
}
