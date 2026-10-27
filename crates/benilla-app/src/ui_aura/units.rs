//! The unit-token resolver's inputs (`0x515970`, [`UnitGuids`]) as the VM holds them: the guid
//! globals, the group tables, and each held unit a `target` chain can reach. The aura getters
//! resolve a token through them and read the unit's slots at the call ([`super::AuraLive`]); the
//! host's own lookups read them too (the mouseover, the group rows, the pet records, the field
//! events' fan-out), and the app's readers of the held units (the targeting feed's hover verdict)
//! walk them.

use benilla_ui::script::{UiScript, UnitGuids};

use crate::net::ObjectStore;
use crate::ui_party::{raid_row_guids, GroupState};

/// The group tables of [`UnitGuids`]: `party1..4` are the own subgroup's slots and `partypetN`
/// their pets (`0x4e81d0`); `raidN` is `GetRaidRosterInfo` row N, as every other raid-token reader
/// resolves it, and `raidpetN` that row's pet (`0x491960`).
fn fill_group<'a>(
    g: &mut UnitGuids,
    group: Option<&GroupState>,
    held: impl Fn(u64) -> Option<&'a ObjectStore>,
) {
    g.party = [0; 4];
    g.party_pets = [0; 4];
    g.raid.clear();
    g.raid_pets.clear();
    let Some(group) = group else {
        return;
    };
    let live = |m: u64| held(m).map(|s| &s.0);
    for (i, m) in group.party_slots().take(4).enumerate() {
        g.party[i] = m.guid;
        g.party_pets[i] = group.party_pet_guid(m.guid, live(m.guid)).unwrap_or(0);
    }
    g.raid = raid_row_guids(group);
    g.raid_pets.extend(
        g.raid
            .iter()
            .map(|&m| group.raid_pet_guid(m, live(m)).unwrap_or(0)),
    );
}

/// What [`UnitGuidFeed::feed`] reads this frame.
pub(super) struct Inputs<'g, H> {
    pub(super) group: Option<&'g GroupState>,
    /// Our guid.
    pub(super) me: u64,
    /// `pet`, `target`, `mouseover` and `npc`'s guids, 0 for none.
    pub(super) pet: u64,
    pub(super) target: u64,
    pub(super) mouseover: u64,
    pub(super) npc: u64,
    /// The object manager's lookup (`0x468460`).
    pub(super) held: H,
}

/// The resolver's inputs as last built, for one VM.
#[derive(Default)]
pub(super) struct UnitGuidFeed {
    guids: UnitGuids,
    /// Kept to save the allocations: the walk over the guids a token can name.
    walk: Vec<u64>,
}

impl UnitGuidFeed {
    /// Build the resolver's inputs and push them: the bases, the group tables, then each held
    /// unit their `target` chains reach. Pushing fires nothing.
    pub(super) fn feed<'a, H>(&mut self, script: &mut UiScript, inputs: Inputs<'_, H>)
    where
        H: Fn(u64) -> Option<&'a ObjectStore>,
    {
        let Inputs {
            group,
            me,
            pet,
            target,
            mouseover,
            npc,
            held,
        } = inputs;
        let mut g = std::mem::take(&mut self.guids);
        let mut walk = std::mem::take(&mut self.walk);
        (g.player, g.pet, g.target, g.mouseover, g.npc) = (me, pet, target, mouseover, npc);
        fill_group(&mut g, group, &held);
        g.held.clear();
        walk.clear();
        walk.extend([me, pet, target, mouseover, npc]);
        walk.extend(g.party.iter().chain(&g.party_pets).copied());
        walk.extend(g.raid.iter().chain(&g.raid_pets).copied());
        while let Some(guid) = walk.pop() {
            if guid == 0 || g.held.contains_key(&guid) {
                continue;
            }
            if let Some(unit) = held(guid).filter(|s| s.is_unit()) {
                let next = unit.0.unit_target().unwrap_or(0);
                g.held.insert(guid, next);
                walk.push(next);
            }
        }
        script.set_unit_guids(&g);
        self.guids = g;
        self.walk = walk;
    }
}

#[cfg(test)]
mod tests {
    use benilla_protocol::messages::{
        member_status, GroupMemberEntry, ObjectFields, PartyMemberStatsInfo,
    };
    use benilla_ui::script::{AuraState, UiScript};
    use bevy::prelude::*;

    use crate::client_state::ClientState;
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

    /// The resolver's feed on a bare app with a VM, us, and a party of one, `MEMBER`, with no
    /// record.
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
                f:SetScript("OnEvent", function() table.insert(SEEN, arg1) end)
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
        stats_of(app, MEMBER, full, info);
    }

    fn stats_of(app: &mut App, guid: u64, full: bool, info: PartyMemberStatsInfo) {
        app.world_mut()
            .resource_mut::<GroupState>()
            .apply_stats(guid, full, info);
    }

    /// The list of the unit `token` names through the pushed resolver inputs, as the lent host's
    /// walk reads it at a getter's call ([`super::super::AuraLive`]), its roster record standing
    /// in for a unit not held.
    fn list_of(app: &mut App, token: &str) -> Option<benilla_ui::script::UnitAuraList> {
        use bevy::ecs::system::RunSystemOnce;
        let guid = app
            .world()
            .non_send_resource::<UiScript>()
            .unit_guids_view()
            .guid_of(token)?;
        app.world_mut()
            .run_system_once(
                move |views: super::super::AuraViews,
                      relations: crate::target::RelationWorld,
                      me: Query<&ObjectStore, With<SelfPlayer>>,
                      index: Res<GuidIndex>,
                      stores: Query<&ObjectStore>,
                      group: Option<Res<GroupState>>| {
                    let store = index.0.get(&guid).and_then(|&e| stores.get(e).ok());
                    views.list(
                        guid,
                        store,
                        true,
                        group.as_deref(),
                        &relations,
                        me.iter().next(),
                    )
                },
            )
            .expect("the walk runs as a one-shot system")
    }

    /// What `UnitDebuff` (`harmful`) or `UnitBuff` enumerates for `token`, index 1 up to the nil:
    /// the half, the buffs through the unit gate, past the vision test, `raid` keeping the
    /// `raidFilter` verdicts alone.
    fn walked(app: &mut App, token: &str, harmful: bool, raid: bool) -> Vec<AuraState> {
        let Some(list) = list_of(app, token) else {
            return Vec::new();
        };
        let gate = harmful || list.buffs_visible;
        list.auras
            .into_iter()
            .filter(|a| gate && a.helpful != harmful && !a.vision_hidden)
            .filter(|a| !raid || a.raid_listed)
            .collect()
    }

    /// The spell ids [`walked`] enumerates for `token`.
    fn listed(app: &mut App, token: &str, harmful: bool) -> Vec<i64> {
        walked(app, token, harmful, false)
            .into_iter()
            .map(|a| i64::from(a.spell_id))
            .collect()
    }

    fn seen(app: &mut App) -> Vec<String> {
        app.world_mut()
            .non_send_resource::<UiScript>()
            .eval::<Vec<String>>("local s = SEEN SEEN = {} return s")
            .unwrap()
    }

    /// The player's slots list under its guid as any held unit's: `UnitBuff("player")` walks the
    /// fields in slot order (`0x51960f`), not the buff cache's arrival order.
    #[test]
    fn the_player_token_reads_the_slots_in_slot_order() {
        let mut app = party_app();
        let me = app.world().resource::<GuidIndex>().0[&ME];
        let seat = |app: &mut App, fields: &[(u16, u32)]| {
            app.world_mut()
                .entity_mut(me)
                .insert(ObjectStore(ObjectFields::from_pairs(fields)));
            app.update();
        };
        // Stance lands first, in slot 1; then Mark, in slot 0.
        seat(
            &mut app,
            &[(FLAGS, 0x0800_0000), (AURA + 1, 2457), (AURAFLAGS, 0x90)],
        );
        seat(
            &mut app,
            &[
                (FLAGS, 0x0800_0000),
                (AURA, 1126),
                (AURA + 1, 2457),
                (AURAFLAGS, 0x99),
            ],
        );
        assert_eq!(listed(&mut app, "player", false), [1126, 2457]);
    }

    /// A set `raidFilter` counts the buffs the player could cast, off the books as they stand: a
    /// spell learnt rebuilds the verdicts (`0x4b2603`).
    #[test]
    fn a_raid_filter_follows_the_book() {
        let mut app = party_app();
        let row = |id: u32| {
            (
                id,
                benilla_formats::SpellDisplay {
                    id,
                    icon: Some(format!("Spell_{id}")),
                    ..Default::default()
                },
            )
        };
        app.world_mut().insert_resource(crate::ui_action::Spells {
            catalog: benilla_formats::SpellCatalog::from_displays([row(1243), row(1126)].into()),
            ..crate::ui_action::Spells::empty_for_tests()
        });
        let mut actions = crate::ui_action::PlayerActions::default();
        actions.spells.insert(1243);
        app.world_mut().insert_resource(actions);
        stream(
            &mut app,
            MEMBER,
            &[
                (FLAGS, 0x0800_0000),
                (AURA, 1126),
                (AURA + 1, 1243),
                (AURAFLAGS, 0x0000_0088),
            ],
        );
        let kept = |app: &mut App| -> Vec<String> {
            walked(app, "party1", false, true)
                .into_iter()
                .filter_map(|a| a.icon)
                .collect()
        };
        app.update();
        assert_eq!(kept(&mut app), ["Spell_1243"]);
        assert_eq!(listed(&mut app, "party1", false), [1126, 1243]);
        app.world_mut()
            .resource_mut::<crate::ui_action::PlayerActions>()
            .spells
            .insert(1126);
        app.update();
        assert_eq!(kept(&mut app), ["Spell_1126", "Spell_1243"]);
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
        assert_eq!(walked(&mut app, "party1", true, false)[0].count, 1);

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

    /// The list writes the record's online bit too (`0x4e8361`-`0x4e837b`), so a status-only
    /// `SMSG_GROUP_LIST` for a member out of view takes their last auras: `UnitBuff` and
    /// `UnitDebuff` answer nil behind a clear bit (`0x519741`, `0x519b31`), and an online list brings them back.
    #[test]
    fn a_status_only_list_takes_an_out_of_view_members_auras_and_an_online_one_returns_them() {
        let mut app = party_app();
        stats(
            &mut app,
            true,
            PartyMemberStatsInfo {
                status: Some(member_status::ONLINE),
                auras: Some(vec![(0, 1126)]),
                auras_negative: Some(vec![(33, 589)]),
                ..Default::default()
            },
        );
        app.update();
        assert_eq!(listed(&mut app, "party1", false), [1126]);
        assert_eq!(listed(&mut app, "party1", true), [589]);

        let relist = |app: &mut App, status: u8| {
            let member = GroupMemberEntry {
                name: "Brisca".into(),
                guid: MEMBER,
                status,
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
            app.update();
        };
        relist(&mut app, member_status::OFFLINE);
        // The record kept the auras, the clear bit hides them.
        assert!(listed(&mut app, "party1", false).is_empty());
        assert!(listed(&mut app, "party1", true).is_empty());

        relist(&mut app, member_status::ONLINE);
        assert_eq!(listed(&mut app, "party1", false), [1126]);
        assert_eq!(listed(&mut app, "party1", true), [589]);
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
    }

    // == The pet token ==

    /// `OBJECT_FIELD_TYPE`, and the player's and a unit's type masks.
    const OBJECT_TYPE: u16 = 2;
    const TYPE_PLAYER: u32 = 0x19;
    const TYPE_UNIT: u32 = 0x9;
    /// `UNIT_FIELD_CHARM`, `UNIT_FIELD_CHARMEDBY` and `UNIT_FIELD_CREATEDBY`, two-field guids.
    const CHARM: u16 = 6;
    const CHARMEDBY: u16 = 10;
    const CREATEDBY: u16 = 14;
    /// `UNIT_FLAG_PLAYER_CONTROLLED`, which players and their pets carry.
    const PLAYER_CONTROLLED: u32 = 0x8;
    /// A unit we charm.
    const THRALL: u64 = 0xF130_0000_0000_0055;

    /// `field` and the next one holding `guid`.
    fn guid_at(field: u16, guid: u64) -> [(u16, u32); 2] {
        [(field, guid as u32), (field + 1, (guid >> 32) as u32)]
    }

    /// Our descriptor replaced by these fields.
    fn seat_me(app: &mut App, fields: &[(u16, u32)]) {
        let me = app.world().resource::<GuidIndex>().0[&ME];
        app.world_mut()
            .entity_mut(me)
            .insert(ObjectStore(ObjectFields::from_pairs(fields)));
    }

    /// The pet bar's cached guid (`[0xb714a0]`).
    fn bar(app: &mut App, guid: u64) {
        app.world_mut()
            .resource_mut::<crate::ui_pet::PetBar>()
            .spells
            .pet_guid = guid;
    }

    /// `"pet"` names our `UNIT_FIELD_CHARM`, else our `UNIT_FIELD_SUMMON`, off our own descriptor
    /// (`0x515a5e`-`0x515a6e`), never the pet bar's guid: its list is that unit's, a bar naming
    /// another unit changes nothing, and with neither field the token names nobody.
    #[test]
    fn the_pet_token_lists_our_charm_else_our_summon_not_the_bar() {
        let mut app = party_app();
        let open = (FLAGS, 0x0800_0000);
        stream(&mut app, PET, &[open, (AURA, 1126), (AURAFLAGS, 0x9)]);
        stream(&mut app, THRALL, &[open, (AURA, 21562), (AURAFLAGS, 0x9)]);
        bar(&mut app, THRALL);
        seat_me(&mut app, &guid_at(SUMMON, PET));
        app.update();
        assert_eq!(listed(&mut app, "pet", false), [1126], "the summon");

        bar(&mut app, PET);
        let mut both = guid_at(SUMMON, PET).to_vec();
        both.extend(guid_at(CHARM, THRALL));
        seat_me(&mut app, &both);
        app.update();
        assert_eq!(listed(&mut app, "pet", false), [21562], "the charm first");

        seat_me(&mut app, &[]);
        app.update();
        assert!(
            listed(&mut app, "pet", false).is_empty(),
            "no charm and no summon name nobody, whatever the bar holds"
        );
    }

    /// The pet's buffs pass `UnitBuff`'s unit gate as any unit's (`0x5195aa`-`0x5195f7`): our own
    /// pet passes `CanAssist` (`0x6066f0`), and while our `UNIT_FIELD_CHARMEDBY` is set
    /// (`0x5195eb`-`0x5195f7`) its buffs are nil. `UnitDebuff` has no gate.
    #[test]
    fn the_pets_buffs_close_while_we_are_charmed() {
        let mut app = party_app();
        stream(
            &mut app,
            PET,
            &[
                (OBJECT_TYPE, TYPE_UNIT),
                (FLAGS, PLAYER_CONTROLLED),
                guid_at(CREATEDBY, ME)[0],
                guid_at(CREATEDBY, ME)[1],
                (AURA, 1126),
                (AURA + 33, 589),
                (AURAFLAGS, 0x9),
                (AURAFLAGS + 4, 0x80),
            ],
        );
        bar(&mut app, PET);
        let me = |charmer: u64| {
            let mut f = vec![(OBJECT_TYPE, TYPE_PLAYER), (FLAGS, PLAYER_CONTROLLED)];
            f.extend(guid_at(SUMMON, PET));
            f.extend(guid_at(CHARMEDBY, charmer));
            f
        };
        seat_me(&mut app, &me(0));
        app.update();
        assert_eq!(
            listed(&mut app, "pet", false),
            [1126],
            "we assist our own pet"
        );
        assert_eq!(listed(&mut app, "pet", true), [589]);

        seat_me(&mut app, &me(THRALL));
        app.update();
        assert!(
            listed(&mut app, "pet", false).is_empty(),
            "charmed, we list nobody's buffs, our pet's included"
        );
        assert_eq!(listed(&mut app, "pet", true), [589], "the debuffs stay");
    }

    // == The vision test ==

    /// `PLAYER_FIELD_BYTES2`, whose byte 1 holds the auras the player can see (`+0x10c1`).
    const BYTES2: u16 = 1260;
    /// 26869 Amorous, whose `RequiredAuraVision` is 1, and an aura with none.
    const AMOROUS: u32 = 26869;

    /// A catalog of Amorous, needing vision 1, and Power Word: Fortitude, needing none.
    fn vision_catalog(app: &mut App) {
        let row = |id: u32, required_aura_vision: u32| {
            (
                id,
                benilla_formats::SpellDisplay {
                    id,
                    icon: Some(format!("Spell_{id}")),
                    required_aura_vision,
                    ..Default::default()
                },
            )
        };
        app.world_mut().insert_resource(crate::ui_action::Spells {
            catalog: benilla_formats::SpellCatalog::from_displays(
                [row(AMOROUS, 1), row(1243, 0)].into(),
            ),
            ..crate::ui_action::Spells::empty_for_tests()
        });
    }

    /// `IsAuraDisplayable` (`0x519860`) hides an aura whose `RequiredAuraVision` is set unless
    /// bit `value - 1` of the active player's `PLAYER_FIELD_BYTES2` byte 1 is (`0x51989c`-
    /// `0x5198b8`), on every unit's rows, the player's own included (`0x51966d`); the record walk
    /// runs it too (`0x51979f`). The player's buff bar does not (`0x4e4170`).
    #[test]
    fn an_aura_needing_vision_shows_only_while_the_player_has_it() {
        let mut app = party_app();
        vision_catalog(&mut app);
        let open = (FLAGS, 0x0800_0000);
        stream(
            &mut app,
            MEMBER,
            &[open, (AURA, AMOROUS), (AURA + 1, 1243), (AURAFLAGS, 0x99)],
        );
        let me = |vision: u32| {
            [
                open,
                (AURA, AMOROUS),
                (AURAFLAGS, 0x9),
                (BYTES2, vision << 8),
            ]
        };
        seat_me(&mut app, &me(0));
        app.update();
        assert_eq!(listed(&mut app, "party1", false), [1243], "Amorous hidden");
        assert!(listed(&mut app, "player", false).is_empty(), "on us too");
        let bar_shows = |app: &mut App| {
            let me = app.world().resource::<GuidIndex>().0[&ME];
            let store = app.world().get::<ObjectStore>(me).expect("our store");
            let catalog = app.world().resource::<crate::ui_action::Spells>();
            let mut cache = Vec::new();
            super::super::rebuild(&mut cache, store, Some(&catalog.catalog), 0.0);
            cache.iter().map(|c| c.spell_id).collect::<Vec<_>>()
        };
        assert_eq!(
            bar_shows(&mut app),
            [AMOROUS],
            "the buff bar runs no vision test"
        );

        // Bit 0 of byte 1 is vision 1 (Detect Amore).
        seat_me(&mut app, &me(1));
        app.update();
        assert_eq!(
            listed(&mut app, "party1", false),
            [i64::from(AMOROUS), 1243]
        );
        assert_eq!(listed(&mut app, "player", false), [i64::from(AMOROUS)]);
        // Bit 1 alone is another vision.
        seat_me(&mut app, &me(2));
        app.update();
        assert_eq!(listed(&mut app, "party1", false), [1243]);
    }

    /// A member out of view reads its record's block, which the vision test filters as well
    /// (`0x51979f`).
    #[test]
    fn a_records_aura_needing_vision_is_hidden_without_it() {
        let mut app = party_app();
        vision_catalog(&mut app);
        stats(
            &mut app,
            true,
            PartyMemberStatsInfo {
                status: Some(member_status::ONLINE),
                auras: Some(vec![(0, AMOROUS as u16), (1, 1243)]),
                ..Default::default()
            },
        );
        app.update();
        assert_eq!(listed(&mut app, "party1", false), [1243]);
    }

    // == The raid tokens ==

    /// In our subgroup, so `party1` and `raid1`.
    const MATE: u64 = 0x2001;
    /// Subgroup 2, `raid4` only.
    const FAR: u64 = 0x2004;
    const FAR_PET: u64 = 0xF140_0000_0000_0099;

    /// A raid of five: `MATE` (subgroup 1), two in subgroup 2 and `FAR` in subgroup 3 in wire
    /// order, then us (row 5, subgroup 1); none streamed and none with a record.
    fn raid_app() -> App {
        let mut app = party_app();
        relist_raid(&mut app, member_status::ONLINE);
        app.update();
        app
    }

    /// The raid's list as `raid_app` sends it, with `MATE`'s status byte as given.
    fn relist_raid(app: &mut App, mate: u8) {
        let member = |name: &str, guid, status, subgroup| GroupMemberEntry {
            name: name.into(),
            guid,
            status,
            flags: subgroup,
        };
        let online = member_status::ONLINE;
        app.world_mut().resource_mut::<GroupState>().apply_list(
            crate::ui_party::GROUPTYPE_RAID,
            0,
            vec![
                member("Mate", MATE, mate, 0),
                member("Two", 0x2002, online, 1),
                member("Three", 0x2003, online, 1),
                member("Far", FAR, online, 2),
            ],
            ME,
            None,
            Some(ME),
        );
    }

    fn online(auras_negative: Vec<(u8, u16)>) -> PartyMemberStatsInfo {
        PartyMemberStatsInfo {
            status: Some(member_status::ONLINE),
            auras_negative: Some(auras_negative),
            ..Default::default()
        }
    }

    /// A raid member in view reads its own `UNIT_FIELD_AURA` slots under `raidN`, whatever its
    /// subgroup, and its buffs keep the unit gate.
    #[test]
    fn a_live_raid_members_auras_come_off_its_descriptor() {
        let mut app = raid_app();
        stream(
            &mut app,
            FAR,
            &[
                (AURA, 1126),
                (AURA + 33, 589),
                (AURAFLAGS, 0x0000_0008),
                (AURAFLAGS + 4, 0x0000_0080),
            ],
        );
        app.update();
        assert_eq!(listed(&mut app, "raid4", true), [589]);
        assert!(
            listed(&mut app, "raid4", false).is_empty(),
            "no factions, so `CanAssist` fails and the gate drops the buffs"
        );
        for token in ["party1", "party2", "raid6"] {
            assert!(listed(&mut app, token, true).is_empty(), "{token}");
        }
    }

    /// Out of range, `raidN` walks the member's record, which vmangos keeps current for every raid
    /// member we do not see; `raidpetN` walks the pet block the same record names.
    #[test]
    fn an_out_of_range_raid_member_and_its_pet_read_the_record() {
        let mut app = raid_app();
        stats_of(
            &mut app,
            FAR,
            true,
            PartyMemberStatsInfo {
                pet_guid: Some(FAR_PET),
                pet_auras: Some(vec![(1, 1126)]),
                pet_auras_negative: Some(vec![(33, 770)]),
                ..online(vec![(32, 589), (35, 11976)])
            },
        );
        app.update();
        assert_eq!(listed(&mut app, "raid4", true), [589, 11976]);
        assert_eq!(walked(&mut app, "raid4", true, false)[0].count, 1);
        assert_eq!(listed(&mut app, "raidpet4", false), [1126]);
        assert_eq!(listed(&mut app, "raidpet4", true), [770]);
        assert!(
            listed(&mut app, "raidpet3", true).is_empty(),
            "a petless member's raidpet token names nobody"
        );
    }

    /// The feed fires nothing: `UNIT_AURA` is the aura fields' watches and the stats packet's
    /// record diff (`crate::field_watch`), so neither a record's new block nor a token's first look
    /// at one reaches a handler from here; the walk reads each record as it stands.
    #[test]
    fn the_feed_fires_nothing_and_the_walk_reads_each_record_as_it_stands() {
        let mut app = raid_app();
        stats_of(&mut app, MATE, true, online(vec![(32, 589)]));
        stats_of(&mut app, FAR, true, online(vec![(32, 770)]));
        app.update();
        assert!(seen(&mut app).is_empty());
        assert_eq!(listed(&mut app, "raid4", true), [770]);

        stats_of(&mut app, MATE, false, online(vec![(33, 11976)]));
        app.update();
        assert!(seen(&mut app).is_empty());
        assert_eq!(listed(&mut app, "raid1", true), [589, 11976]);
        assert_eq!(listed(&mut app, "party1", true), [589, 11976]);
    }

    /// The raid roster's twin of the list write (`0x4ba947`-`0x4ba960`): a status-only raid list
    /// hides an out-of-view row's auras under `raidN` and its party slot alike, and an online list
    /// returns them.
    #[test]
    fn a_status_only_raid_list_takes_an_out_of_view_rows_auras_and_an_online_one_returns_them() {
        let mut app = raid_app();
        stats_of(&mut app, MATE, true, online(vec![(32, 589)]));
        app.update();
        assert_eq!(listed(&mut app, "raid1", true), [589]);
        assert_eq!(listed(&mut app, "party1", true), [589]);

        relist_raid(&mut app, member_status::OFFLINE);
        app.update();
        assert!(listed(&mut app, "raid1", true).is_empty());
        assert!(listed(&mut app, "party1", true).is_empty());

        relist_raid(&mut app, member_status::ONLINE);
        app.update();
        assert_eq!(listed(&mut app, "raid1", true), [589]);
        assert_eq!(listed(&mut app, "party1", true), [589]);
    }

    /// A raid token past the roster, a raid token outside a raid, and an offline member's pet
    /// all answer nil.
    #[test]
    fn a_raid_token_naming_nobody_answers_nil() {
        let mut app = party_app();
        stats(&mut app, true, online(vec![(32, 589)]));
        app.update();
        assert_eq!(listed(&mut app, "party1", true), [589]);
        for token in ["raid1", "raid2"] {
            assert!(
                listed(&mut app, token, true).is_empty(),
                "a party is no raid: {token}"
            );
        }

        let mut app = raid_app();
        stats_of(
            &mut app,
            FAR,
            true,
            PartyMemberStatsInfo {
                status: Some(member_status::OFFLINE),
                pet_guid: Some(FAR_PET),
                pet_auras_negative: Some(vec![(33, 770)]),
                auras_negative: Some(vec![(32, 589)]),
                ..Default::default()
            },
        );
        app.update();
        for token in ["raid4", "raidpet4", "raid6", "raid40", "raidpet40"] {
            assert!(listed(&mut app, token, true).is_empty(), "{token}");
        }
    }

    /// The last row, 5, is us: the player's own list, as `"player"` answers it.
    #[test]
    fn our_raid_row_mirrors_the_player_list() {
        let mut app = raid_app();
        let me = app.world_mut().resource::<GuidIndex>().0[&ME];
        *app.world_mut().get_mut::<ObjectStore>(me).unwrap() =
            ObjectStore(ObjectFields::from_pairs(&[
                (AURA + 32, 589),
                (AURAFLAGS + 4, 0x0000_0008),
            ]));
        app.update();
        assert_eq!(listed(&mut app, "player", true), [589]);
        assert_eq!(listed(&mut app, "raid5", true), [589]);
    }

    // == Any token the resolver parses ==

    /// `OBJECT_FIELD_TYPE`; a unit's typemask is `0x9`, a player's `0x19`.
    const TYPE: u16 = 2;
    /// `UNIT_FIELD_TARGET`'s low dword; the high half is the next index.
    const TARGET: u16 = 16;
    const MOB: u64 = 0xF130_0000_0000_0001;
    const ADD: u64 = 0xF130_0000_0000_0002;

    /// A unit of typemask `ty` targeting `target`, with one debuff in slot 32.
    fn unit_fields(ty: u32, target: u64, debuff: u32) -> Vec<(u16, u32)> {
        vec![
            (TYPE, ty),
            (TARGET, target as u32),
            (TARGET + 1, (target >> 32) as u32),
            (AURA + 32, debuff),
            (AURAFLAGS + 4, 0x0000_0008),
        ]
    }

    /// We target `MOB`, which targets party1, who targets `ADD`, which targets us; the mouse is over
    /// `ADD`, and our pet targets `MOB`. Each carries its own debuff: us 6788, `MOB` 589, party1
    /// 11976, `ADD` 770.
    fn chain_app() -> App {
        let mut app = party_app();
        app.init_resource::<crate::target::Hovered>();
        let me = app.world_mut().resource::<GuidIndex>().0[&ME];
        let mut fields = unit_fields(0x19, MOB, 6788);
        fields.extend(guid_at(SUMMON, PET));
        *app.world_mut().get_mut::<ObjectStore>(me).unwrap() =
            ObjectStore(ObjectFields::from_pairs(&fields));
        stream(&mut app, MOB, &unit_fields(0x9, MEMBER, 589));
        stream(&mut app, MEMBER, &unit_fields(0x19, ADD, 11976));
        stream(&mut app, ADD, &unit_fields(0x9, ME, 770));
        stream(&mut app, PET, &unit_fields(0x9, MOB, 1));
        let (mob, add) = {
            let index = &app.world().resource::<GuidIndex>().0;
            (index[&MOB], index[&ADD])
        };
        let mut selection = app.world_mut().resource_mut::<crate::target::Selection>();
        (selection.target, selection.guid) = (Some(mob), Some(MOB));
        let mut hovered = app.world_mut().resource_mut::<crate::target::Hovered>();
        (hovered.target, hovered.guid) = (Some(add), Some(ADD));
        publish_mouseover(&mut app, Some(ADD));
        app.update();
        app
    }

    /// The world mouseover as the publisher (`0x492890`) writes it to `[0xb4e2c8]`.
    fn publish_mouseover(app: &mut App, guid: Option<u64>) {
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .publish_world_mouseover(guid);
    }

    /// `UnitDebuff` reads the list of whatever unit the token names, through the resolver
    /// (`0x515970`, called at `0x519542`): the mouseover, a member's target, any depth of `target`
    /// chain, in any case, and the player's own list at the end of a chain that comes back to us.
    #[test]
    fn every_token_reads_the_list_of_the_unit_it_names() {
        let mut app = chain_app();
        for (token, want) in [
            ("mouseover", 770),
            ("MouseOver", 770),
            ("party1target", 770),
            ("PARTY1TARGET", 770),
            ("target", 589),
            ("targettarget", 11976),
            ("targettargettarget", 770),
            ("targettargettargettarget", 6788),
            ("pettarget", 589),
            ("playertarget", 589),
            ("mouseovertargettarget", 589),
            ("PLAYER", 6788),
            ("Player", 6788),
        ] {
            assert_eq!(listed(&mut app, token, true), [want], "{token}");
        }
        assert_eq!(
            listed(&mut app, "PLAYER", true),
            listed(&mut app, "player", true)
        );
    }

    /// A chain's or the mouseover's list moving fires nothing, and the walk reads it as it stands.
    #[test]
    fn a_chains_list_moving_fires_nothing() {
        let mut app = chain_app();
        assert!(seen(&mut app).is_empty(), "a first look fires nothing");

        // `ADD`'s list moves: it is the mouseover, party1's target and three hops off ours.
        let add = app.world().resource::<GuidIndex>().0[&ADD];
        let mut fields = unit_fields(0x9, ME, 770);
        fields.push((AURA + 33, 589));
        fields[4].1 = 0x0000_0088;
        *app.world_mut().get_mut::<ObjectStore>(add).unwrap() =
            ObjectStore(ObjectFields::from_pairs(&fields));
        app.update();
        assert!(seen(&mut app).is_empty());
        assert_eq!(listed(&mut app, "mouseover", true), [770, 589]);
        assert_eq!(listed(&mut app, "party1target", true), [770, 589]);
    }

    /// A nearer GameObject wins the pick, and the publisher writes its guid, which the resolver
    /// rejects as a unit (`0x515bd9 je`): the unit behind it is no longer the mouseover.
    #[test]
    fn a_nearer_gameobject_takes_the_mouseover_from_the_unit_behind_it() {
        let mut app = chain_app();
        assert_eq!(listed(&mut app, "mouseover", true), [770]);
        app.world_mut()
            .resource_mut::<crate::target::Hovered>()
            .distance = 10.0;
        let chest = app.world_mut().spawn_empty().id();
        app.insert_resource(crate::target::HoveredObject {
            target: Some(chest),
            guid: Some(0xF110_0000_0000_0004),
            distance: 5.0,
        });
        publish_mouseover(&mut app, Some(0xF110_0000_0000_0004));
        app.update();
        assert!(listed(&mut app, "mouseover", true).is_empty());
        assert!(listed(&mut app, "mouseovertarget", true).is_empty());
    }

    /// A hovered plate retired after the pick: its leave (`0x7cb8b0`) publishes the empty
    /// mouseover, while the pick still names the plate's unit. The resolver reads the published
    /// pair (`0x515bb6`, `0x515bdb`), so in the next frame `"mouseover"` and its chains name
    /// nobody.
    #[test]
    fn a_retired_hovered_plate_names_no_mouseover_in_the_next_frame() {
        let mut app = chain_app();
        assert_eq!(listed(&mut app, "mouseover", true), [770]);
        assert_eq!(listed(&mut app, "mouseovertarget", true), [6788]);
        app.world_mut()
            .resource_mut::<crate::target::Hovered>()
            .plate = true;
        publish_mouseover(&mut app, None);
        app.update();
        assert!(listed(&mut app, "mouseover", true).is_empty());
        assert!(listed(&mut app, "mouseovertarget", true).is_empty());
        assert!(listed(&mut app, "mouseovertargettarget", true).is_empty());
    }

    /// A raid member's target that is out of view but on the roster reads its record, as
    /// `UnitDebuff`'s roster walk finds it (`0x519b25`); a hop off a unit not held is nobody.
    #[test]
    fn a_raid_members_target_reads_whatever_the_guid_holds() {
        let mut app = raid_app();
        stats_of(&mut app, FAR, true, online(vec![(32, 589)]));
        // `raid2` is 0x2002, in view and targeting `FAR`, which is not.
        stream(&mut app, 0x2002, &unit_fields(0x19, FAR, 770));
        app.update();
        assert_eq!(listed(&mut app, "raid2", true), [770]);
        assert_eq!(listed(&mut app, "raid2target", true), [589]);
        assert_eq!(listed(&mut app, "raid4", true), [589]);
        assert!(listed(&mut app, "raid4target", true).is_empty());
        assert!(listed(&mut app, "raid2targettarget", true).is_empty());
    }
}
