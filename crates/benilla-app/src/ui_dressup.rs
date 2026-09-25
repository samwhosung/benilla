//! The app-side **dressing-room feed**: the bridge between the stock
//! window's `DressUpModel` widget verbs ([`DressUpIntent`], queued engine-side) and the booth's
//! look ([`DressUpPreview`]).
//!
//! Three jobs, each frame, in the feed phase before the VM ticks ([`UiFeed`]):
//!
//! - **Apply the intents, in order.** `Dress` drops every substitution (the ref's
//!   `SetUnit("player")` on open and `Dress()` on Reset); `Undress` strips every worn piece but
//!   the hands; `TryOn(id)` records one; `Close` — derived here from the window going invisible,
//!   the stock file having no hook of ours — empties the room. Order matters — `DressUpItem`
//!   resets *then* tries on in the same breath.
//! - **Resolve each tried-on item to a display.** An item id is all a `|Hitem:` link carries, so the
//!   display id / inventory type come from the ask-once template cache
//!   ([`Items::template`] — `CMSG_ITEM_QUERY_SINGLE` on a miss, exactly as the reference's
//!   ItemCache does). A substitution whose answer is still in flight stays **pending** and lands on
//!   a later frame: that is the normal case for an item the player has never seen, e.g. one linked
//!   in chat by someone else.
//! - **Compose the look.** The player's own body + appearance + their `PLAYER_VISIBLE_ITEM_*`
//!   displays, with each resolved substitution written into the slot its `InventoryType` maps to
//!   (the shared [`equip_slot`] table — the same one the glue/select preview dresses by, so a robe
//!   lands on the chest here exactly as it does there). The **held** triple then goes through
//!   [`held_lanes`], which is where the two previews stop agreeing: the character-select mannequin
//!   drops the ranged slot and this window puts it in a hand. 1060's own record
//!   claimed "a wand in the ranged hand here exactly as it does there" — that was wrong in the
//!   second half, and the ranged slot was silently discarded until 1076.
//!
//! **What is NOT here:** any notion of *fit*. The reference's dressing room previews whatever it is
//! handed — a plate helm on a mage, a mail chest on a rogue — because it is a look, not an equip:
//! `DressUpItemLink` reaches `TryOn` with no class/level/proficiency check anywhere in the path
//! (`DressUpFrame.lua:2-16`). Ours does the same.

use benilla_protocol::CharEnumItem;
use benilla_ui::script::{DressUpIntent, UiScript};
use bevy::prelude::*;

use crate::entities::equip_slot;
use crate::items::Items;
use crate::net::{NetCommands, NetEntity, ObjectStore, SelfPlayer};
use crate::portrait::{DressUpLook, DressUpPreview};
use crate::ui_script::UiFeed;

/// The equipment slots a dressing-room look reads off the player — every rendered slot
/// (`EQUIPMENT_SLOT_*`), which is exactly the set [`equip_slot`] can map an item into.
const LOOK_SLOTS: [u8; 14] = [0, 2, 3, 4, 5, 6, 7, 8, 9, 14, 15, 16, 17, 18];

/// The three **held** equipment slots — main hand · off hand · ranged. The widget keeps only *two*
/// lanes for them, which is the whole reason [`held_lanes`] exists.
const HELD_SLOTS: [usize; 3] = [15, 16, 17];

/// **The dress-up widget has two held lanes, and a ranged weapon takes one of them** (decision
/// 1076).
///
/// `DressUpModel::TryOn 0x504d90` installs into lane `0x0f` (→ HandRight) or lane `0x10` (→
/// HandLeft, or the Shield point for a shield) — never a third. `INVTYPE_RANGED` (15, bows) lands
/// in the **off** lane; gun/crossbow/wand (26) and thrown (25) land in the **main** one, matching
/// the world's own drawn-ranged split exactly.
///
/// Before installing, `0x504bc0(mainInvType, offInvType, dualWield)` asks whether the two lanes may
/// coexist; if not, the incoming item **evicts** the other. `0x0f` appears on neither side of that
/// test, so a ranged weapon coexists with nothing: a bow always clears the main hand and overwrites
/// the off one, a crossbow always clears the off hand and overwrites the main one. Never both.
///
/// The base the try-ons land on is the player's **melee** pair only. `Dress()` / `SetUnit` do not
/// install gear slot-by-slot at all — `0x5059a0` duplicates the unit's live `CM2Model` and
/// deep-copies its attachment tree (`0x70ea00`) — so the base is literally what the player is
/// showing in the world, where a ranged weapon renders only while ranged-drawn and our booth is
/// frozen melee-drawn (0465 / 1060). A *worn* bow therefore does not appear until it is tried on,
/// which is also the behaviour that shipped, and is why the ranged slot of a dressing-room look is
/// only ever filled by a substitution.
fn held_lanes(equipment: &mut [CharEnumItem; 19], room: &DressUpRoom) {
    // The player's own melee pair — anything the room substituted is replayed below instead.
    let base = |slot: usize| {
        (room.worn[slot].is_none() && equipment[slot].display_id != 0).then_some(equipment[slot])
    };
    let (mut main, mut off) = (base(15), base(16));

    for &slot in &room.held_order {
        let Some(item) = room.worn[slot] else {
            continue;
        };
        // Which lane the item installs into. A bow is the one ranged type that goes LEFT.
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

    // Back into the enum array's held triple — where the index is **which held law `held_wants`
    // asks**, not "the slot this item's inventory type names": 15/16 are the melee-drawn main/off
    // attach points, and 17 is the ranged-drawn arm, which carries the bow-left / gun-right split
    // itself. So a lane writes back at its own lane, and a ranged item writes back at 17 whichever
    // lane it won.
    //
    // Routing this through [`equip_slot`] instead is what collapsed a dual-wielded pair into one
    // weapon: an off-hand one-hander is `INVTYPE_WEAPON` 13 just like a main-hand
    // one, and that table — which answers "where is this item WORN", the right question everywhere
    // else — maps 13 to the main hand, so the off hand overwrote the main.
    for slot in HELD_SLOTS {
        equipment[slot] = CharEnumItem::default();
    }
    for (lane, item) in [(15usize, main), (16, off)] {
        let Some(item) = item else { continue };
        let ranged = matches!(item.inventory_type, 15 | 25 | 26);
        equipment[if ranged { 17 } else { lane }] = item;
    }
}

/// `0x504bc0(mainInvType, offInvType, dualWield)` — may these two held lanes coexist?
///
/// TRUE only for a one-hand main (`WEAPON` 13 / `WEAPONMAINHAND` 21) beside a `SHIELD` (14) or
/// `HOLDABLE` (23), plus the dual-wield arm for a `WEAPON` (13) / `WEAPONOFFHAND` (22) off-hand.
/// Everything else — a two-hander beside anything, and **any** ranged type on either side —
/// evicts.
///
/// **The dual-wield arm is taken as permitted, and that is a stated assumption.** The binary gates
/// it on a global (`0xc4d770`) whose source is unidentified, and we have no dual-wield
/// capability streamed to read. Permitted is the conservative direction here: it evicts strictly
/// less than the alternative, so it can only ever leave the pre-1076 behaviour in place for a
/// melee pair — while the ranged evictions this record is actually about are unconditional in the
/// binary and unaffected by the global either way.
fn lanes_coexist(main: u8, off: u8) -> bool {
    matches!(main, 13 | 21) && matches!(off, 13 | 14 | 22 | 23)
}

/// What the room is currently showing: the substitutions applied on top of the player's own gear,
/// and the ones still waiting on a template answer.
#[derive(Resource, Default)]
pub(crate) struct DressUpRoom {
    /// Open? `false` = the window is closed and the booth stays empty (the `Close` intent).
    open: bool,
    /// `Undress()` has stripped the body: the player's own worn pieces are not part of the look
    /// until the next `Dress`. The hands are untouched — the widget clears components bodyslots
    /// `0..0xb` (`0x504490`) and no hand lane.
    bare: bool,
    /// Resolved substitutions by equipment slot — `(display id, inventory type)`, the same pair the
    /// enum-shaped array carries.
    worn: [Option<CharEnumItem>; 19],
    /// The **held** substitutions in the order they landed, most recent last (slots 15/16/17 only).
    ///
    /// The widget installs each try-on on top of the model it is already showing, and a held item
    /// can *evict* the opposite lane, so which one arrived last decides what survives — a fact the
    /// per-slot array above cannot express on its own. See [`held_lanes`].
    held_order: Vec<usize>,
    /// Item ids whose template has not answered yet, oldest first. Retried every frame; a
    /// substitution only becomes visible once its display id is known.
    pending: Vec<u32>,
}

impl DressUpRoom {
    /// Apply one intent (see [`DressUpIntent`]).
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
                // Every worn piece off — base (`bare`, read by `player_look`) and tried-on alike —
                // while a held substitution, like a held base item, stays in its hand.
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

    /// Resolve whatever is still waiting on a template answer. `Items::template` asks once and
    /// answers on a later frame; an id the server never answers for simply stays pending, showing
    /// the player's own gear in that slot rather than a hole.
    fn resolve_pending(&mut self, items: &Items, commands: &NetCommands) {
        let mut pending = std::mem::take(&mut self.pending);
        pending.retain(|item| {
            let Some(t) = items.template(*item, 0, commands) else {
                return true; // still in flight — keep asking
            };
            let (display_id, inventory_type) = (t.display_info_id, t.inventory_type as u8);
            if let Some(slot) = equip_slot(inventory_type) {
                self.worn[slot] = Some(CharEnumItem {
                    display_id,
                    inventory_type,
                });
                // Held slots also record *when* they landed — the eviction law reads the order.
                if HELD_SLOTS.contains(&slot) {
                    self.held_order.retain(|&s| s != slot);
                    self.held_order.push(slot);
                }
            }
            // A non-worn item (a potion, a bag) resolves to no slot and simply previews nothing —
            // the reference's `TryOn` is equally happy to be handed one.
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
    // The guild identity cache (1257) — `ResMut` because it is lazy: a miss is what sends the
    // `CMSG_GUILD_QUERY` whose answer paints the tabard the room may be previewing (1704).
    mut guilds: ResMut<crate::ui_guild::GuildState>,
) {
    let Some(mut script) = script else {
        return;
    };
    for intent in script.take_dressup_intents() {
        room.apply(intent);
    }
    // The window went away: the room empties and the booth stops baking. Read off the frame
    // itself — the file is the reference's and has no hook of ours in its OnHide (1969); the
    // reference's widget keeps its state while hidden, but its next `DressUpItem` re-issues
    // `SetUnit("player")` precisely because the frame was not visible, so nothing kept is ever
    // observable.
    // Two windows drive the one room since 1971: the dressing room and the auction house's
    // embedded dress-up pane (`AuctionDressUpFrame`); the booth empties when neither is up.
    if room.open
        && !script.frame_visible("DressUpFrame")
        && !script.frame_visible("AuctionDressUpFrame")
    {
        room.apply(DressUpIntent::Close);
    }
    // The pane's rotate buttons own the yaw through the stock `Model_RotateLeft/Right/OnUpdate`,
    // which write `DressUpModel:SetRotation`; the booth mirrors the pane (the paper doll's own
    // law, 0208 §5 / 1751).
    preview.yaw = script.model_pane_facing("DressUpModel");

    room.resolve_pending(&items, &commands);

    // The guild join is the caller's, not [`player_look`]'s: that function resolves an *outfit* out
    // of the descriptor and the room's substitutions, and the crest is neither — it comes off a
    // separate lazy cache this system holds the handle to.
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

/// The player's own dressed look with the room's substitutions written in — `None` while the
/// descriptor cannot answer race/sex, or before the body display is known (the frame or two right
/// after entering the world).
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
        // A substitution wins over what the player is actually wearing — that IS the preview.
        if let Some(worn) = room.worn[idx] {
            equipment[idx] = worn;
            continue;
        }
        // Undressed: the player's own worn pieces are off; the hands (attachments, not bodyslots)
        // keep what they hold.
        if room.bare && !HELD_SLOTS.contains(&idx) {
            continue;
        }
        // …and past that point we are dressing the player's OWN gear, which is where the two
        // equipment-display preferences apply — and where the reference applies them too, by
        // construction rather than by a test. `DressUpModel::SetUnit 0x476cb0`
        // `rep movsd`s all twelve per-bodyslot `ItemDisplayInfo` pointers verbatim off the live
        // player (head `+0x4a8`, cloak `+0x4d0`) and deep-copies its attach tree, so a piece the
        // world already suppressed is simply not in what gets cloned: **hidden in the world ⇒
        // hidden in the dressing room.** A TRY-ON gates on nothing and previews the helm or cloak
        // regardless — which is the branch above, and the reason this test sits *below* it — and
        // `Dress()` (the Reset button) drops the substitutions and so re-clones back to hidden.
        // (This replaced an INFERRED guess that the mannequin was unconditional.)
        if (idx == 0 && s.player_hides_helm()) || (idx == 14 && s.player_hides_cloak()) {
            continue;
        }
        let Some(entry) = s.player_visible_item_entry(slot).filter(|e| *e != 0) else {
            continue;
        };
        // Template-only ask, like the inspect feed's: the visible-item field carries an item
        // ENTRY, and the display id lives on its template.
        if let Some(t) = items.template(entry, 0, commands) {
            equipment[idx] = CharEnumItem {
                display_id: t.display_info_id,
                inventory_type: t.inventory_type as u8,
            };
        }
    }
    // …and then the widget's own two-lane law over the held triple.
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
        // The outfit's, not the crest's: [`feed_dressup`] stamps that in (see its note there).
        emblem: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use benilla_protocol::{EntityKind, ItemInfo, ObjectFields};

    use crate::items::test_template;
    use crate::net::ClientCommand;

    /// A self-player descriptor: a human male wearing `entries` (equipment slot → item entry).
    /// The raw field indices are the wire's own (`UNIT_FIELD_BYTES_0` 36, `PLAYER_BYTES` 193,
    /// `PLAYER_VISIBLE_ITEM_1_CREATOR` 258 + 12 per slot, entry at +2) — the same literal-index
    /// idiom the descriptor fixtures elsewhere use, since the constants are crate-private to
    /// benilla-protocol.
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

    /// An item template with a known display + inventory type.
    fn worn(name: &str, display_info_id: u32, inventory_type: u32) -> ItemInfo {
        ItemInfo {
            display_info_id,
            inventory_type,
            ..test_template(name)
        }
    }

    /// A try-on replaces exactly its own slot and leaves the rest of the player's gear standing —
    /// the whole point of the room: you see YOUR character in the item, not a mannequin.
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

    /// Reset (`DressUpModel:Dress()`) drops every substitution — the player's own gear comes back —
    /// and closing the window empties the room entirely.
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

    /// `DressUpModel:Undress()` strips every worn piece — the player's own and a tried-on one —
    /// and no hand lane (bodyslots `0..0xb` only, `0x504490`); a later try-on lands on the bare
    /// body, and `Dress()` puts the player's own gear back.
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

    /// An item whose template has not answered yet stays PENDING rather than previewing nothing:
    /// linked-in-chat items are the normal case here, and the first click on one always misses the
    /// cache. It lands on the frame the answer does — and the ask goes out exactly once.
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
        // Exactly one query for the unknown entry (`Items` asks once).
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

    /// A non-worn item (a potion) is handed to `TryOn` by any ctrl-click on one, and previews
    /// nothing — it maps to no equipment slot. It must not stay pending forever either.
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

    /// The director's report (2026-08-06): *"when I ctrl click a bow or cross bow I still see only
    /// the swords no bow"*. Decision 1076 — the widget installs a tried-on ranged weapon at a hand,
    /// and it coexists with neither melee lane, so the sword and shield go.
    ///
    /// Both ranged directions in one test because they evict *opposite* lanes in the binary and a
    /// single-sided implementation would still pass one half: a bow takes the off lane (clearing
    /// the main), a crossbow takes the main (clearing the off).
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

    /// The other half of the same law, and the reason the fix is not "always render slot 17": a
    /// **worn** bow stays invisible. `Dress()`/`SetUnit` clone the live world model rather than
    /// installing gear per slot, and there a ranged weapon shows only while ranged-drawn — our
    /// booth is frozen melee-drawn. So the hunter standing in the room holds their sword.
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

    /// Eviction runs **both ways, in try-on order** — the half a "hide the melee pair whenever the
    /// ranged slot is filled" shortcut would get wrong. Try on a bow, then a two-hander: the
    /// two-hander takes the main lane and the bow, which can coexist with nothing, goes.
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

    /// The lanes that DO coexist are left alone — a one-hander beside a shield is the ordinary case
    /// and 1076 must not start evicting there. (A two-hander beside a shield is a different matter,
    /// and the reference evicts that one; it is asserted in the same breath so the coexistence set
    /// is pinned rather than merely "nothing changed".)
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

    /// The director's report (2026-08-07): *"there is an issue now where the preview is only
    /// showing 1 wep, while I was just previewing some boots"* — decision 1079, a regression from
    /// 1076's write-back.
    ///
    /// A dual-wielded pair is two items of the **same** inventory type: an off-hand one-hander is
    /// `INVTYPE_WEAPON` 13 exactly like the main-hand one, and [`equip_slot`] answers "where is
    /// this WORN", which for 13 is the main hand. Both lanes therefore landed on slot 15 and the
    /// off hand overwrote the main. Nothing about it needed a held try-on — the boots are in the
    /// test because that is what the director was previewing, and the pair has to survive an
    /// unrelated substitution untouched.
    ///
    /// Every off-lane inventory type is looped rather than only 13, so what is pinned is "a lane
    /// keeps its own item" and not "13 got a special case".
    #[test]
    fn a_dual_wielded_pair_keeps_both_hands() {
        let (cmds, _rx) = commands();
        let mut items = Items::default();
        items.insert_template(1500, Some(worn("Main Sword", 5500, 13))); // INVTYPE_WEAPON
        items.insert_template(2000, Some(worn("Shiny Boots", 7000, 8))); // FEET — the try-on

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

    /// **The dressing room inherits the equipment-display preferences, and a try-on overrides
    /// them** (`DressUpModel::SetUnit 0x476cb0`'s verbatim clone of the live
    /// player's display pointers). Hidden helm + hidden cloak, so the player's own two pieces are
    /// absent from the look — and then a tried-on helm shows anyway, because previewing it is the
    /// whole feature.
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

        // A try-on is not their own gear, so nothing suppresses it — this is the branch the
        // reference reaches by never testing the flag on the TryOn path at all.
        room.apply(DressUpIntent::TryOn(2000));
        room.resolve_pending(&items, &cmds);
        let look = player_look(&store, &net(), &room, &items, &cmds).expect("a look");
        assert_eq!(
            look.equipment[0].display_id, 7000,
            "the tried-on helm previews regardless of the preference"
        );

        // …and Reset drops the substitution, so the mannequin goes back to bare-headed.
        room.apply(DressUpIntent::Dress);
        let look = player_look(&store, &net(), &room, &items, &cmds).expect("a look");
        assert_eq!(
            look.equipment[0].display_id, 0,
            "Reset re-clones the hidden helm away"
        );
    }
}
