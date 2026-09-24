//! The wire `InventoryResult` → GlobalStrings-key vocabulary for the red error line — the
//! inventory half of the refusal-message law (`ui_action::errors`: every string comes from the
//! VM's own loaded `GlobalStrings.lua`, never hardcoded; `mount_result_key` is the twin table).
//! The drain lives in [`super::feed::feed_containers`].
//!
//! **Not every `SMSG_INVENTORY_CHANGE_FAILURE` is an error to show, and the client has no
//! special case that says so.** The packet carries two jobs — clear the item's client-side
//! pending ("gray") lock, and *maybe* print a red line. Reason 59 (`EQUIP_ERR_NONE`) is the
//! server's pure sentinel for the first: vmangos sends it verbatim as *"free gray item after use
//! fail"* (`SpellHandler.cpp:130`, the shapeshifted item-use refusal) and *"just remove gray
//! item state"* (`ItemHandler.cpp:838`/`:936`). It rides *alongside* the real message — a
//! shapeshifted mount press answers 59 **and** `SPELL_FAILED_NO_ITEMS_WHILE_SHAPESHIFTED` — so
//! anything printed for it doubles the refusal (director's report B198).
//!
//! The reference silences it **through the data, not through control flow**. Handler
//! `0x5e3991` maps the reason through the 67-wide errorId jump table `0x622794` into the error
//! registry `0xb4b498` and calls `CGGameUI::DisplayError 0x496720` unconditionally. 59's slot is
//! populated and non-null — errorId 362, key `ERR_CANT_BE_DISENCHANTED` — but that key is simply
//! **absent from the shipped 1.12 `GlobalStrings.lua`**, so `GetText` hands back the pre-seeded
//! empty string and the sink's `cmp byte [ecx],0` guard at `0x4945b4` returns before anything
//! renders or sounds. So the table below is total, and *silence is what an unresolvable key
//! does* — the same law the cast path already runs (`ui_action::cast_fail`). Reason **0** is the
//! only coded suppression (`0x5e39a9`), and it is already gated a layer up, at
//! `benilla_protocol::events::decode` (`if reason != 0`), so it never reaches this table.

/// A wire `InventoryResult` refusal → its GlobalStrings key, resolved to text through the VM's
/// own loaded `GlobalStrings.lua` at the drain. **Total** — every reason has a key, because the
/// reference's own lookup is (see the module doc); a key with no string displays nothing.
///
/// The FULL build-5875 set, read key by key off the binary's errorId table `0x622794` — which
/// agreed with the vmangos `ItemDefines.h` tag comments on 66 of 67
/// entries. Positions come from that enum (sequential with every `#if` band ≤ 5875 included:
/// stunned=37, dead=38, INVENTORY_FULL=50; an earlier table had the TBC-era 39/40 for
/// stunned/dead — on this wire those are CANT_DO_RIGHT_NOW / INT_BAG_ERROR). Reason 1's string
/// carries a `%d` the drain fills with the packet's required level.
pub(super) fn equip_error_key(reason: u8) -> &'static str {
    match reason {
        1 => "ERR_CANT_EQUIP_LEVEL_I",
        2 => "ERR_CANT_EQUIP_SKILL",
        3 => "ERR_WRONG_SLOT",
        // 4 BAG_FULL and the BAG_FULL3/4/6 aliases. **51 is NOT one of them** — see below.
        4 | 53 | 56 | 62 => "ERR_BAG_FULL",
        5 => "ERR_BAG_IN_BAG",
        6 => "ERR_TRADE_EQUIPPED_BAG",
        7 => "ERR_AMMO_ONLY",
        8 => "ERR_PROFICIENCY_NEEDED",
        9 | 12 | 18 => "ERR_NO_SLOT_AVAILABLE",
        10 | 11 => "ERR_CANT_EQUIP_EVER",
        13 => "ERR_2HANDED_EQUIPPED",
        14 => "ERR_2HSKILLNOTFOUND",
        15 | 16 => "ERR_WRONG_BAG_TYPE",
        17 => "ERR_ITEM_MAX_COUNT",
        19 | 55 => "ERR_CANT_STACK",
        20 => "ERR_NOT_EQUIPPABLE",
        21 => "ERR_CANT_SWAP",
        22 => "ERR_SLOT_EMPTY",
        23 | 54 => "ERR_ITEM_NOT_FOUND",
        24 => "ERR_DROP_BOUND_ITEM",
        25 => "ERR_OUT_OF_RANGE",
        26 => "ERR_TOO_FEW_TO_SPLIT",
        27 => "ERR_SPLIT_FAILED",
        28 => "ERR_SPELL_FAILED_REAGENTS_GENERIC",
        29 => "ERR_NOT_ENOUGH_MONEY",
        30 => "ERR_NOT_A_BAG",
        31 => "ERR_DESTROY_NONEMPTY_BAG",
        32 => "ERR_NOT_OWNER",
        33 => "ERR_ONLY_ONE_QUIVER",
        34 => "ERR_NO_BANK_SLOT",
        35 => "ERR_NO_BANK_HERE",
        36 => "ERR_ITEM_LOCKED",
        37 => "ERR_GENERIC_STUNNED",
        38 => "ERR_PLAYER_DEAD",
        39 => "ERR_CLIENT_LOCKED_OUT",
        40 => "ERR_INTERNAL_BAG_ERROR",
        // ERR_ONLY_ONE_BOLT's 1.12 string genuinely reads "quiver", same as 33's.
        41 => "ERR_ONLY_ONE_BOLT",
        42 => "ERR_ONLY_ONE_AMMO",
        43 => "ERR_CANT_WRAP_STACKABLE",
        44 => "ERR_CANT_WRAP_EQUIPPED",
        45 => "ERR_CANT_WRAP_WRAPPED",
        46 => "ERR_CANT_WRAP_BOUND",
        47 => "ERR_CANT_WRAP_UNIQUE",
        48 => "ERR_CANT_WRAP_BAGS",
        49 => "ERR_LOOT_GONE",
        50 => "ERR_INV_FULL",
        // 51 `EQUIP_ERR_BANK_FULL` — "Your bank is full", NOT the bag line. vmangos's own enum
        // annotates this entry `// ERR_BAG_FULL`, and that comment is simply **wrong**: the
        // binary's pad sets errorId 1 (`0x622661 mov eax,1`), whose registry slot is
        // `ERR_BANK_FULL` (init `0x484cda`, key ptr `0x842180`). We inherited the bad comment
        // when this table was built from the tag list; the binary is the authority.
        51 => "ERR_BANK_FULL",
        52 | 57 => "ERR_VENDOR_SOLD_OUT",
        58 => "ERR_OBJECT_IS_BUSY",
        // 59 `EQUIP_ERR_NONE` — the lock-clear sentinel. Its key is real and its registry slot
        // is populated; the key just has no 1.12 string, so it renders as nothing (module doc,
        // B198). Mapped rather than special-cased, because that is exactly what the client does.
        59 => "ERR_CANT_BE_DISENCHANTED",
        60 => "ERR_NOT_IN_COMBAT",
        61 => "ERR_NOT_WHILE_DISARMED",
        63 => "ERR_CANT_EQUIP_RANK",
        64 => "ERR_CANT_EQUIP_REPUTATION",
        65 => "ERR_TOO_MANY_SPECIAL_BAGS",
        66 => "ERR_LOOT_CANT_LOOT_THAT_NOW",
        // Past the enum. The binary's jump table is bounded `cmp ecx,0x42; ja 0x62278d` — 0..=66
        // indexed, everything above falling to the pad's default **errorId 9 = ERR_BAG_FULL**
        // ("That bag is full."). The enum's own trailing comment says the same: any greater
        // value shows as bag full. Not a hole, and not our old hex debug line.
        _ => "ERR_BAG_FULL",
    }
}

#[cfg(test)]
mod tests {
    use super::equip_error_key;

    /// Pins the build-5875 `InventoryResult` positions against the sender's enum (vmangos
    /// `ItemDefines.h`, every band ≤ 5875 included). The director's live repro: a full-inventory
    /// vendor buy arrives as 0x32 = 50 = INVENTORY_FULL. Stunned/dead sit at 37/38 on this wire —
    /// an earlier table had the TBC-era 39/40, which here mean "right now"/"Internal Bag Error".
    #[test]
    fn equip_error_table_matches_the_5875_enum() {
        assert_eq!(equip_error_key(50), "ERR_INV_FULL");
        assert_eq!(equip_error_key(37), "ERR_GENERIC_STUNNED");
        assert_eq!(equip_error_key(38), "ERR_PLAYER_DEAD");
        assert_eq!(equip_error_key(39), "ERR_CLIENT_LOCKED_OUT");
        assert_eq!(equip_error_key(40), "ERR_INTERNAL_BAG_ERROR");
        assert_eq!(equip_error_key(1), "ERR_CANT_EQUIP_LEVEL_I");
    }

    /// **51 is the bank, not a bag.** The one place the binary's errorId table (`0x622794` →
    /// errorId 1 → `ERR_BANK_FULL`) disagreed with the vmangos tag comments this table was first
    /// built from: `ItemDefines.h` annotates `EQUIP_ERR_BANK_FULL` `// ERR_BAG_FULL`, and that
    /// comment is wrong. A full bank used to say "That bag is full."
    #[test]
    fn a_full_bank_says_bank_not_bag() {
        assert_eq!(equip_error_key(51), "ERR_BANK_FULL");
        // Its neighbours really are the bag aliases — the correction is surgical.
        for alias in [4u8, 53, 56, 62] {
            assert_eq!(equip_error_key(alias), "ERR_BAG_FULL", "reason {alias}");
        }
    }

    /// **B198's regression pin, and the shape of the silence.** Mounting in cat form answers
    /// `SPELL_FAILED_NO_ITEMS_WHILE_SHAPESHIFTED` *and* an inventory failure 59 —
    /// `HandleUseItemOpcode`'s "free gray item after use fail" (vmangos `SpellHandler.cpp:130`).
    /// 59 must print NOTHING or the player gets the refusal twice. The client has no special
    /// case for it: the key is real and mapped, and it is the *absent GlobalStrings entry* that
    /// silences it — pinned end-to-end in the resolution test below.
    #[test]
    fn the_lock_clear_sentinel_maps_to_the_stringless_key() {
        assert_eq!(equip_error_key(59), "ERR_CANT_BE_DISENCHANTED");
    }

    /// Reason 16 keeps the GENERIC key in this table — the substitution is the drain's fork, not
    /// a second table entry, because choosing it needs the named bag (`feed::bag_family_name`).
    /// Both 15 and 16 map here; only 16 can be overridden, and only when its bag resolves.
    #[test]
    fn the_wrong_bag_reasons_share_the_generic_key() {
        assert_eq!(equip_error_key(15), "ERR_WRONG_BAG_TYPE");
        assert_eq!(equip_error_key(16), "ERR_WRONG_BAG_TYPE");
    }

    /// Past the enum the binary clamps rather than falling through: its jump table is bounded
    /// `cmp ecx,0x42; ja`, and the default pad is errorId 9 = `ERR_BAG_FULL`. Ours used to print
    /// a hex debug line here.
    #[test]
    fn codes_past_the_enum_clamp_to_the_bag_full_default() {
        assert_eq!(equip_error_key(67), "ERR_BAG_FULL");
        assert_eq!(equip_error_key(0xFF), "ERR_BAG_FULL");
    }

    /// The RUNTIME leg on the real data (the `mount_result_key` test's pattern), and the load-
    /// bearing one now that the drain has no runtime fallback: every key the table can emit
    /// resolves to a non-empty string in the shipped 1.12 `GlobalStrings.lua` — **except reason
    /// 59's**, which must resolve to nothing, because that absence is the entire mechanism by
    /// which B198's duplicate line stays off the screen. A typo'd key would now silently swallow
    /// a real refusal; this test is what catches it. Also pins the director's earlier repro
    /// end-to-end (0x32 → "Inventory is full."), reason 1's `%d` fill, and 51's bank line.
    /// Skips without client data.
    #[test]
    fn every_equip_error_key_resolves_in_the_real_global_strings() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let src = chain
            .read_file("Interface\\FrameXML\\GlobalStrings.lua")
            .expect("GlobalStrings.lua in the chain");
        let s = benilla_ui::script::UiScript::new().expect("VM");
        s.run(&String::from_utf8_lossy(&src)).expect("runs clean");
        let g = |key: &str| s.lua().globals().get::<String>(key).ok();

        // Reason 0 never reaches this table (gated at `events::decode`), so the sweep starts at 1.
        for reason in 1..=66u8 {
            let key = equip_error_key(reason);
            let text = g(key).unwrap_or_default();
            if reason == 59 {
                // THE mechanism, asserted on the real shipped data rather than assumed: the key
                // is mapped and the lookup happens, and it comes back empty. `0x4945b4`'s guard
                // is what the drain's `is_empty()` continue mirrors.
                assert!(
                    text.is_empty(),
                    "reason 59's {key} resolved to {text:?} — B198's duplicate line is back"
                );
                continue;
            }
            assert!(!text.is_empty(), "{key} (reason {reason}) missing");
        }
        assert_eq!(g(equip_error_key(50)).unwrap(), "Inventory is full.");
        assert_eq!(
            g(equip_error_key(1)).unwrap().replace("%d", "30"),
            "You must reach level 30 to use that item."
        );
        // The 51 correction, in the words the player actually sees.
        assert_eq!(g(equip_error_key(51)).unwrap(), "Your bank is full");
        // The past-the-enum clamp resolves too — it is a real line, not a placeholder.
        assert_eq!(g(equip_error_key(67)).unwrap(), "That bag is full.");
        // Reason 16's two outcomes, both on real data: the generic line the table gives, and the
        // substituted one the drain swaps in once `feed::bag_family_name` resolves a bag. The
        // `%s` fill is a BagFamily name ("Arrows"), pinned in `benilla_formats::itembagfamily`.
        assert_eq!(
            g(equip_error_key(16)).unwrap(),
            "That item doesn't go in that container."
        );
        assert_eq!(
            g("ERR_WRONG_BAG_TYPE_SUBCLASS")
                .unwrap()
                .replace("%s", "Arrows"),
            "Only Arrows can be placed in that."
        );
    }
}
