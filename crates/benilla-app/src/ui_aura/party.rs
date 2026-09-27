//! The `partyN` and `partypetN` aura lists. `UnitBuff 0x519500` and `UnitDebuff 0x5198f0` read a
//! token's live object, gated as any unit's; a guid with no live object diverts to the roster
//! record's aura block instead (`0x519592`-`0x5195a4` to `0x519735`, `0x519b25`).

use benilla_formats::SpellCatalog;
use benilla_ui::script::AuraState;

use crate::net::ObjectStore;
use crate::ui_party::GroupState;

/// The tokens this feeds, `partyN` then `partypetN`.
pub(super) const PARTY_AURA_TOKENS: [&str; 8] = [
    "party1",
    "party2",
    "party3",
    "party4",
    "partypet1",
    "partypet2",
    "partypet3",
    "partypet4",
];

/// One unit's guid and its list.
pub(super) type UnitAuras = (u64, Vec<AuraState>);

/// Each of [`PARTY_AURA_TOKENS`] with the guid it names and that unit's list, `None` for a token
/// naming nobody. `held` is the object manager's lookup (`0x468460`); `live` lists a held unit.
pub(super) fn party_aura_lists<'a>(
    group: Option<&GroupState>,
    held: impl Fn(u64) -> Option<&'a ObjectStore>,
    live: impl Fn(&ObjectStore) -> Vec<AuraState>,
    catalog: Option<&SpellCatalog>,
) -> Vec<(&'static str, Option<UnitAuras>)> {
    let Some(group) = group else {
        return PARTY_AURA_TOKENS.iter().map(|t| (*t, None)).collect();
    };
    let members: Vec<u64> = group.party_slots().map(|m| m.guid).collect();
    let list_of = |guid: u64| match held(guid) {
        Some(store) => live(store),
        None => super::record_auras(&group.roster_aura_slots(guid), catalog),
    };
    let (member_tokens, pet_tokens) = PARTY_AURA_TOKENS.split_at(4);
    let mut out = Vec::with_capacity(PARTY_AURA_TOKENS.len());
    for (i, token) in member_tokens.iter().enumerate() {
        let guid = members.get(i).copied();
        out.push((*token, guid.map(|g| (g, list_of(g)))));
    }
    for (i, token) in pet_tokens.iter().enumerate() {
        let pet = members
            .get(i)
            .and_then(|&m| group.party_pet_guid(m, held(m).map(|s| &s.0)));
        out.push((*token, pet.map(|g| (g, list_of(g)))));
    }
    out
}

#[cfg(test)]
mod tests {
    use benilla_protocol::messages::{
        member_status, GroupMemberEntry, ObjectFields, PartyMemberStatsInfo,
    };
    use benilla_ui::script::UiScript;
    use bevy::prelude::*;

    use crate::char_select::ClientState;
    use crate::net::{Guid, GuidIndex, NetCommands, ObjectStore, Reputations, SelfPlayer};
    use crate::ui_party::GroupState;

    const ME: u64 = 0x10;
    const MEMBER: u64 = 0x1234;
    const PET: u64 = 0xF140_0000_0000_0077;
    const AURA: u16 = 47;
    const AURAFLAGS: u16 = 95;
    const SUMMON: u16 = 8;
    /// `UNIT_FIELD_FLAGS`; bit 27 is `UNIT_FLAG_AURAS_VISIBLE`, which opens `UnitBuff`'s gate.
    const FLAGS: u16 = 46;

    /// The feed on a bare app with a VM, us, and a party of one, `MEMBER`, with no record.
    fn party_app() -> App {
        let (tx, _) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .insert_state(ClientState::InWorld)
            .init_resource::<crate::target::Selection>()
            .init_resource::<crate::ui_pet::PetBar>()
            .init_resource::<GuidIndex>()
            .init_resource::<Reputations>()
            .init_resource::<GroupState>()
            .insert_resource(NetCommands(tx))
            .add_plugins(super::super::UiAuraPlugin);
        app.insert_non_send_resource(UiScript::new().unwrap());
        app.world_mut()
            .non_send_resource::<UiScript>()
            .run(
                r#"
                SEEN = {}
                local f = CreateFrame("Frame")
                f:RegisterEvent("UNIT_AURA")
                -- The party's tokens only: the player's list fires on the first frame too.
                f:SetScript("OnEvent", function()
                    if string.find(arg1, "^party") then table.insert(SEEN, arg1) end
                end)
            "#,
            )
            .unwrap();
        let me = app
            .world_mut()
            .spawn((ObjectStore(ObjectFields::default()), Guid(ME), SelfPlayer))
            .id();
        app.world_mut().resource_mut::<GuidIndex>().0.insert(ME, me);
        let member = GroupMemberEntry {
            name: "Brisca".into(),
            guid: MEMBER,
            status: member_status::ONLINE,
            flags: 0,
        };
        app.world_mut().resource_mut::<GroupState>().apply_list(
            0,
            0,
            vec![member],
            ME,
            None,
            Some(ME),
        );
        app
    }

    /// Stream `guid` in with these fields, as the object manager holds it.
    fn stream(app: &mut App, guid: u64, fields: &[(u16, u32)]) {
        let e = app
            .world_mut()
            .spawn(ObjectStore(ObjectFields::from_pairs(fields)))
            .id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(guid, e);
    }

    fn stats(app: &mut App, full: bool, info: PartyMemberStatsInfo) {
        app.world_mut()
            .resource_mut::<GroupState>()
            .apply_stats(MEMBER, full, info);
    }

    /// The spell ids `UnitBuff` (or `UnitDebuff`) enumerates for `token`, index 1 up to the nil,
    /// through `UnitAura`'s tenth return, as the icon needs a spell catalog.
    fn listed(app: &mut App, token: &str, harmful: bool) -> Vec<i64> {
        let (verb, filter) = if harmful {
            ("UnitDebuff", "HARMFUL")
        } else {
            ("UnitBuff", "HELPFUL")
        };
        app.world_mut()
            .non_send_resource::<UiScript>()
            .eval::<Vec<i64>>(&format!(
                r#"local out = {{}}
                   for i = 1, 48 do
                       local _, count = {verb}("{token}", i)
                       if count == nil then break end
                       local _, _, _, _, _, _, _, _, _, id = UnitAura("{token}", i, "{filter}")
                       table.insert(out, id)
                   end
                   return out"#
            ))
            .unwrap()
    }

    fn seen(app: &mut App) -> Vec<String> {
        app.world_mut()
            .non_send_resource::<UiScript>()
            .eval::<Vec<String>>("local s = SEEN SEEN = {} return s")
            .unwrap()
    }

    /// A live member reads its own `UNIT_FIELD_AURA` slots; `UnitBuff` keeps the unit gate, open
    /// here through `UNIT_FLAG_AURAS_VISIBLE` (`0x5195bc`), and a stale slot stays out.
    #[test]
    fn a_live_members_auras_come_off_its_descriptor() {
        let mut app = party_app();
        stream(
            &mut app,
            MEMBER,
            &[
                (FLAGS, 0x0800_0000),
                (AURA, 1126),
                (AURA + 1, 6673), // stale: nibble 0x1 has no effect bit
                (AURA + 33, 589),
                (AURA + 34, 11976),
                (AURAFLAGS, 0x0000_0018),
                (AURAFLAGS + 4, 0x0000_0880),
            ],
        );
        // A record that disagrees, to show the live object wins.
        stats(
            &mut app,
            true,
            PartyMemberStatsInfo {
                status: Some(member_status::ONLINE),
                auras_negative: Some(vec![(40, 770)]),
                ..Default::default()
            },
        );
        app.update();
        assert_eq!(listed(&mut app, "party1", true), [589, 11976]);
        assert_eq!(listed(&mut app, "party1", false), [1126]);
        assert!(
            listed(&mut app, "party2", true).is_empty(),
            "no second member"
        );
        assert_eq!(seen(&mut app), ["party1"]);
    }

    /// With no live object, both bindings walk the record's block (`0x519735`, `0x519b25`), with
    /// no unit gate, a stack count of 1 (`0x51980f`), and nil for an offline record (`0x519741`).
    #[test]
    fn an_out_of_range_member_reads_the_stats_aura_block() {
        let mut app = party_app();
        stats(
            &mut app,
            true,
            PartyMemberStatsInfo {
                status: Some(member_status::ONLINE),
                auras: Some(vec![(0, 1126), (3, 21562)]),
                auras_negative: Some(vec![(33, 589)]),
                ..Default::default()
            },
        );
        app.update();
        assert_eq!(
            listed(&mut app, "party1", false),
            [1126, 21562],
            "buffs, ungated: no factions, which would close the live gate"
        );
        assert_eq!(listed(&mut app, "party1", true), [589]);
        let count = app
            .world_mut()
            .non_send_resource::<UiScript>()
            .eval::<i64>(r#"local _, count = UnitDebuff("party1", 1) return count"#)
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(seen(&mut app), ["party1"]);

        stats(
            &mut app,
            false,
            PartyMemberStatsInfo {
                status: Some(member_status::OFFLINE),
                ..Default::default()
            },
        );
        app.update();
        assert!(listed(&mut app, "party1", false).is_empty());
        assert!(listed(&mut app, "party1", true).is_empty());
    }

    /// The record is slot-indexed: deltas patch named slots, the halves split at slot 32, and each
    /// walk is ascending slot whatever order the slots filled in.
    #[test]
    fn record_auras_list_in_slot_order_and_split_buffs_from_debuffs() {
        let mut app = party_app();
        stats(
            &mut app,
            true,
            PartyMemberStatsInfo {
                status: Some(member_status::ONLINE),
                auras: Some(vec![(5, 1126)]),
                auras_negative: Some(vec![(40, 770)]),
                ..Default::default()
            },
        );
        app.update();
        let _ = seen(&mut app);
        // A later aura in a lower slot, then a debuff, as two deltas.
        stats(
            &mut app,
            false,
            PartyMemberStatsInfo {
                auras: Some(vec![(2, 21562)]),
                ..Default::default()
            },
        );
        stats(
            &mut app,
            false,
            PartyMemberStatsInfo {
                auras_negative: Some(vec![(34, 589)]),
                ..Default::default()
            },
        );
        app.update();
        assert_eq!(listed(&mut app, "party1", false), [21562, 1126]);
        assert_eq!(listed(&mut app, "party1", true), [589, 770]);
        assert_eq!(seen(&mut app), ["party1"]);

        // Slot 5 falls off: the delta names it with a 0, and only it goes.
        stats(
            &mut app,
            false,
            PartyMemberStatsInfo {
                auras: Some(vec![(5, 0)]),
                ..Default::default()
            },
        );
        app.update();
        assert_eq!(listed(&mut app, "party1", false), [21562]);
        assert_eq!(listed(&mut app, "party1", true), [589, 770]);
        assert_eq!(seen(&mut app), ["party1"]);

        // A delta that moves no aura fires nothing.
        stats(
            &mut app,
            false,
            PartyMemberStatsInfo {
                cur_hp: Some(7),
                ..Default::default()
            },
        );
        app.update();
        assert!(seen(&mut app).is_empty());
    }

    /// `partypetN` names the member's `CHARM`, else `SUMMON`, while the member is held, else the
    /// record's pet guid (`0x4e81d0`); the pet's list is its own descriptor while held, else the
    /// pet record's block (`0x496420`, `+0xe4`).
    #[test]
    fn a_party_pet_reads_its_descriptor_or_the_pet_block() {
        let mut app = party_app();
        stats(
            &mut app,
            true,
            PartyMemberStatsInfo {
                status: Some(member_status::ONLINE),
                pet_guid: Some(PET),
                pet_auras: Some(vec![(1, 1126)]),
                pet_auras_negative: Some(vec![(32, 770)]),
                ..Default::default()
            },
        );
        app.update();
        assert_eq!(listed(&mut app, "partypet1", false), [1126]);
        assert_eq!(listed(&mut app, "partypet1", true), [770]);
        assert!(listed(&mut app, "partypet2", true).is_empty());
        assert_eq!(seen(&mut app), ["party1", "partypet1"]);

        // A pet-aura delta moves the pet's list and fires for its token alone.
        stats(
            &mut app,
            false,
            PartyMemberStatsInfo {
                pet_auras_negative: Some(vec![(32, 0), (33, 589)]),
                ..Default::default()
            },
        );
        app.update();
        assert_eq!(listed(&mut app, "partypet1", true), [589]);
        assert_eq!(seen(&mut app), ["partypet1"]);

        // Member and pet in view: the pet guid comes off the member's SUMMON, the list off the
        // pet's own slots.
        stream(
            &mut app,
            MEMBER,
            &[(SUMMON, PET as u32), (SUMMON + 1, (PET >> 32) as u32)],
        );
        stream(
            &mut app,
            PET,
            &[(AURA + 35, 11976), (AURAFLAGS + 4, 0x0000_2000)],
        );
        app.update();
        assert_eq!(listed(&mut app, "partypet1", true), [11976]);
        assert!(listed(&mut app, "partypet1", false).is_empty());
        assert!(seen(&mut app).contains(&"partypet1".to_string()));
    }
}
