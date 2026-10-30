//! The reputation getters as a listener reads them, packet by packet. Each reputation message's
//! handler writes the slots, edits the pane's list and signals `UPDATE_FACTION` inside itself
//! (`SMSG_SET_FACTION_STANDING` `0x4d5760` ends at `0x4d5984`, `SMSG_SET_FACTION_ATWAR` `0x4d56b0`
//! at `0x4d56f6`, `SMSG_SET_FACTION_VISIBLE` `0x4d5710` sorts at `0x4d574a`), and the getters read
//! the slots at their calls: `GetFactionInfo` the standing, the flag byte and the player's watched
//! field (`0x4d65ac`, `0x4d65ea`, `0x4d6613`), `GetWatchedFactionInfo` the watched slot's (`0x4d6820`),
//! `GetNumFactions` the list the sorts left (`0xb73764`). So two messages in one frame signal
//! twice, and each listener reads its own message's state.

use benilla_formats::{faction_flags as flag, FactionCatalog};
use benilla_protocol::field::{FIELD_UNIT_HEALTH, FIELD_UNIT_MAXHEALTH};
use benilla_protocol::{EntityKind, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender};

use super::unit_view_tests::{created, values, wire_packets};
use crate::net::ClientCommand;

const ME: u64 = 0x7;
/// The creature the unit tests' client targets.
const WOLF: u64 = 0xF130_0000_0044_0001;
/// `UNIT_FIELD_BYTES_0` of a male human warrior.
const BYTES_0: u16 = 36;
const HUMAN_WARRIOR: u32 = 1 | (1 << 8);
/// `PLAYER_FIELD_WATCHED_FACTION_INDEX`.
const WATCHED: u16 = 1261;
/// `UNIT_FIELD_FLAGS` and its player-controlled bit.
const UNIT_FLAGS: u16 = 46;
const PLAYER_CONTROLLED: u32 = 0x8;

/// The factions and their slots: Alliance, the header, over Stormwind, Ironforge and Darnassus.
const ALLIANCE: (u32, u32) = (469, 11);
const STORMWIND: (u32, u32) = (72, 19);
const IRONFORGE: (u32, u32) = (47, 20);
const DARNASSUS: (u32, u32) = (69, 21);

/// The whole client, headless, in the world, with its frame run.
struct Client {
    app: App,
    inbound: Sender<SessionEvent>,
    _sent: Receiver<ClientCommand>,
}

impl Client {
    /// One frame of the client's, with `packets` in the inbound channel, each one packet.
    fn frame(&mut self, packets: Vec<Vec<SessionEvent>>) {
        for ev in wire_packets(packets) {
            self.inbound.send(ev).expect("the inbound channel");
        }
        self.app.update();
    }

    /// `chunk`'s answer through the lent host when `live`, else off the snapshot alone.
    fn eval<T: benilla_ui::mlua::FromLuaMulti>(&mut self, live: bool, chunk: &str) -> T {
        if live {
            crate::script_calls::with_hosted_vm(self.app.world_mut(), |s| {
                s.expect("a VM").eval::<T>(chunk).expect("runs")
            })
        } else {
            crate::script_calls::eval_in_vm(self.app.world(), chunk)
        }
    }

    /// What the spy heard since the last call.
    fn seen(&mut self) -> Vec<String> {
        self.eval(false, "local s = Seen Seen = {} return s")
    }
}

/// The catalog: the header and three cities, a human's base 4000 in each.
fn catalog() -> FactionCatalog {
    let human = 1;
    let mut catalog = FactionCatalog::from_rows(Default::default(), Default::default());
    for ((id, slot), team, name) in [
        (ALLIANCE, 0, "Alliance"),
        (STORMWIND, ALLIANCE.0, "Stormwind"),
        (IRONFORGE, ALLIANCE.0, "Ironforge"),
        (DARNASSUS, ALLIANCE.0, "Darnassus"),
    ] {
        catalog = catalog
            .with_reputation(id, slot as i32, human, 4000)
            .with_faction_text(id, team, name, "");
    }
    catalog
}

/// The wire's list: the header and Stormwind met, the other cities not; Stormwind at war with
/// nothing.
fn initial_list() -> Vec<(u8, i32)> {
    let mut slots = vec![(0u8, 0i32); 64];
    slots[ALLIANCE.1 as usize] = (flag::HEADER | flag::VISIBLE, 0);
    slots[STORMWIND.1 as usize] = (flag::VISIBLE, 0);
    slots[IRONFORGE.1 as usize] = (0, 0);
    slots[DARNASSUS.1 as usize] = (0, 0);
    slots
}

/// The client in the world as a human warrior with the list landed ahead of the create, as the
/// server orders it, and a spy whose line for each `UPDATE_FACTION` carries the number of rows,
/// Stormwind's standing, rank and at-war flag, and the watched faction's name and standing.
fn client() -> Client {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    app.insert_resource(bevy::ecs::error::DefaultErrorHandler(
        bevy::ecs::error::ignore,
    ));
    let (tx, sent) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    let (inbound, rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetEvents::for_test(rx));
    app.update();
    app.insert_resource(crate::target::Factions::from_catalog(catalog()));
    let script = UiScript::new().expect("a VM");
    script
        .run(
            r#"
            Seen = {}
            local f = CreateFrame("Frame")
            f:RegisterEvent("UPDATE_FACTION")
            f:SetScript("OnEvent", function()
                local storm = "-"
                for i = 1, GetNumFactions() do
                    local name, _, id, _, _, value, war, _, header = GetFactionInfo(i)
                    if name == "Stormwind" then
                        storm = value .. "/" .. id .. "/" .. tostring(war)
                    end
                end
                local watched, _, _, _, wvalue = GetWatchedFactionInfo()
                table.insert(Seen, GetNumFactions() .. " " .. storm .. " "
                    .. tostring(watched) .. "/" .. wvalue)
            end)
            "#,
        )
        .unwrap();
    app.insert_non_send_resource(script);
    app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
    benilla_world::world_point::init_world_point_resources(app.world_mut());
    app.world_mut().resource_mut::<crate::net::SelfGuid>().0 = Some(ME);
    let mut c = Client {
        app,
        inbound,
        _sent: sent,
    };
    c.frame(vec![vec![SessionEvent::Reputations {
        standings: initial_list(),
    }]]);
    c.frame(vec![vec![created(
        ME,
        EntityKind::Player,
        &[
            (FIELD_UNIT_HEALTH, 100),
            (FIELD_UNIT_MAXHEALTH, 100),
            (BYTES_0, HUMAN_WARRIOR),
            (WATCHED, u32::MAX),
        ],
    )]]);
    c.frame(vec![]);
    c.frame(vec![]);
    c.seen();
    c
}

fn delta(slot: u32, standing: i32) -> SessionEvent {
    SessionEvent::ReputationDelta {
        standings: vec![(slot, standing)],
    }
}

/// Two `SMSG_SET_FACTION_STANDING`s in one frame: each handler stores its standing and signals
/// `UPDATE_FACTION` at its end (`0x4d5984`), so the first listener reads Stormwind at 4100 and
/// Friendly, the second at 9100 and Honored.
#[test]
fn each_standing_listener_reads_its_own_messages_standing() {
    let mut c = client();
    assert_eq!(
        c.eval::<String>(
            true,
            "local _, _, _, _, _, v = GetFactionInfo(2) return v .. ''"
        ),
        "4000",
        "the list as the login left it: Stormwind under the Alliance header"
    );
    c.frame(vec![
        vec![delta(STORMWIND.1, 100)],
        vec![delta(STORMWIND.1, 5100)],
    ]);
    assert_eq!(
        c.seen(),
        ["2 4100/5/nil nil/0", "2 9100/6/nil nil/0"],
        "one line per message, each as its message left the slot"
    );
}

/// Two `SMSG_SET_FACTION_ATWAR`s in one frame: the first sets the bit, the second clears it
/// (`0x4d56d9`), and each signals at its end (`0x4d56f6`).
#[test]
fn each_at_war_listener_reads_its_own_messages_bit() {
    let mut c = client();
    let at_war = |flags| SessionEvent::ReputationAtWar {
        list_id: STORMWIND.1,
        flags,
    };
    c.frame(vec![vec![at_war(flag::AT_WAR)], vec![at_war(0)]]);
    assert_eq!(
        c.seen(),
        ["2 4000/5/1 nil/0", "2 4000/5/nil nil/0"],
        "the bit as each message left it"
    );
}

/// Two `SMSG_SET_FACTION_VISIBLE`s in one frame: each lists its slot and sorts (`0x4d5738`,
/// `0x4d574a`), then signals, so the first listener counts one new row and the second two.
#[test]
fn each_visible_listener_counts_the_rows_its_own_message_listed() {
    let mut c = client();
    let before: i64 = c.eval(true, "return GetNumFactions()");
    c.frame(vec![
        vec![SessionEvent::ReputationVisible {
            list_id: IRONFORGE.1,
        }],
        vec![SessionEvent::ReputationVisible {
            list_id: DARNASSUS.1,
        }],
    ]);
    assert_eq!(
        c.seen(),
        [
            format!("{} 4000/5/nil nil/0", before + 1),
            format!("{} 4000/5/nil nil/0", before + 2),
        ],
        "the list as each message's sort left it"
    );
}

/// Two descriptor updates in one frame move the watched faction, and each signals
/// `UPDATE_FACTION` for its change (`0x5ddcce`): `GetWatchedFactionInfo` reads the field at the
/// call (`0x4d6820`), so each listener names the faction its own update watched.
#[test]
fn each_watched_listener_reads_its_own_updates_faction() {
    let mut c = client();
    c.frame(vec![
        vec![delta(STORMWIND.1, 100)],
        vec![values(ME, &[(WATCHED, STORMWIND.1)])],
        vec![delta(STORMWIND.1, 300)],
        vec![values(ME, &[(WATCHED, 0)])],
        vec![SessionEvent::ReputationVisible {
            list_id: IRONFORGE.1,
        }],
    ]);
    let seen = c.seen();
    assert_eq!(seen.len(), 5, "{seen:?}");
    assert!(seen[0].ends_with("nil/0"), "{seen:?}");
    assert!(seen[1].ends_with("Stormwind/4100"), "{seen:?}");
    assert!(seen[2].ends_with("Stormwind/4300"), "{seen:?}");
    assert!(seen[3].ends_with("nil/0"), "{seen:?}");
    assert!(seen[4].ends_with("nil/0"), "{seen:?}");
}

/// With no host lent the getters answer from the snapshot the list last took: the same rows,
/// after a standing, a watch and an at-war message (each message hands the lane its snapshot).
#[test]
fn the_snapshot_answers_what_the_lent_host_does() {
    let mut c = client();
    let chunk = "local t = {} for i = 1, GetNumFactions() do \
                 local _, _, _, _, _, _, war = GetFactionInfo(i) \
                 t[i] = table.concat({GetFactionInfo(i)}, ',', 1, 6) .. tostring(war) end \
                 local n, _, _, _, v = GetWatchedFactionInfo() t[0] = tostring(n) .. v return t";
    c.frame(vec![
        vec![delta(STORMWIND.1, 100)],
        vec![values(ME, &[(WATCHED, STORMWIND.1)])],
        vec![SessionEvent::ReputationAtWar {
            list_id: STORMWIND.1,
            flags: flag::AT_WAR,
        }],
    ]);
    let live: Vec<String> = c.eval(true, chunk);
    let snapshot: Vec<String> = c.eval(false, chunk);
    assert_eq!(live, snapshot);
    assert_eq!(live.len(), 2, "{live:?}");
}

/// A listener's `SetFactionInactive` writes the slot's flag byte inside its call (`0x4d612c`), and a
/// later message in the same frame read-modify-writes the byte as it then stands (`0x4d56d9`,
/// `0x4d5fc0`): the verb's bit survives the at-war packet.
#[test]
fn a_verbs_flag_bit_survives_a_later_packets_write() {
    let mut c = client();
    c.eval::<()>(
        false,
        r#"
        local g = CreateFrame("Frame")
        local done
        g:RegisterEvent("UPDATE_FACTION")
        g:SetScript("OnEvent", function()
            if done then return end
            done = true
            for i = 1, GetNumFactions() do
                if (GetFactionInfo(i)) == "Stormwind" then SetFactionInactive(i) end
            end
        end)
        "#,
    );
    let at_war = |flags| SessionEvent::ReputationAtWar {
        list_id: STORMWIND.1,
        flags,
    };
    c.frame(vec![vec![at_war(0)], vec![at_war(flag::AT_WAR)]]);
    let flags = c
        .app
        .world()
        .resource::<crate::net::Reputations>()
        .flags(STORMWIND.1 as usize);
    assert_eq!(
        flags,
        flag::VISIBLE | flag::INACTIVE | flag::AT_WAR,
        "{flags:#x}"
    );
}

/// A `/reload` makes a new VM mid-session: the cascade's first events, `PLAYER_ENTERING_WORLD`
/// and the quest log's rebuild (`0x49096a`, `0x4909a1`), read the list and the standings the
/// client kept (`0xb73764`, `0x4d6370`), ahead of the cascade's own rebuild (`0x4909d3`).
#[test]
fn a_reloaded_vm_reads_the_list_and_standings_in_the_first_cascade_events() {
    let mut c = client();
    c.frame(vec![vec![delta(STORMWIND.1, 100)]]);
    let fresh = UiScript::new().expect("a VM");
    fresh
        .run(
            r#"
            Seen = {}
            local f = CreateFrame("Frame")
            f:RegisterEvent("PLAYER_ENTERING_WORLD")
            f:SetScript("OnEvent", function()
                local _, _, _, _, _, value = GetFactionInfo(2)
                table.insert(Seen, event .. " " .. GetNumFactions() .. " " .. tostring(value))
            end)
            "#,
        )
        .unwrap();
    c.app.insert_non_send_resource(fresh);
    crate::script_calls::with_hosted_vm(c.app.world_mut(), |s| {
        s.expect("a VM").fire_world_enter();
    });
    assert_eq!(c.seen(), ["PLAYER_ENTERING_WORLD 2 4100"]);
}

/// Two `SMSG_SET_FACTION_ATWAR`s in one frame, and the relation getters beside them: the bit is
/// read live (`0x60640c` through `0x4d61b0`), so the first listener finds the guard of the faction
/// hostile and attackable, the second at peace.
#[test]
fn each_at_war_listener_reads_the_reaction_its_own_message_left() {
    use super::unit_view_tests as units;
    use benilla_formats::FactionTemplate;
    use benilla_protocol::field::FIELD_UNIT_FACTIONTEMPLATE;
    use std::collections::HashMap;

    let mut app = units::client();
    let template = |faction| FactionTemplate {
        faction,
        ..Default::default()
    };
    let catalog = FactionCatalog::from_rows(
        HashMap::from([(1, template(1)), (11, template(STORMWIND.0))]),
        HashMap::new(),
    )
    .with_reputation(STORMWIND.0, STORMWIND.1 as i32, 1, 4000)
    .with_faction_text(STORMWIND.0, 0, "Stormwind", "");
    app.insert_resource(crate::target::Factions::from_catalog(catalog));
    app.world_mut()
        .non_send_resource::<UiScript>()
        .run(
            r#"
            local f = CreateFrame("Frame")
            f:RegisterEvent("UPDATE_FACTION")
            f:SetScript("OnEvent", function()
                table.insert(Seen, "UPDATE_FACTION:" .. tostring(UnitReaction("player", "target")) .. ":"
                    .. tostring(UnitCanAttack("player", "target")))
            end)
            "#,
        )
        .unwrap();
    let at_war = |flags| SessionEvent::ReputationAtWar {
        list_id: STORMWIND.1,
        flags,
    };
    // A human warrior of the player faction, and the guard of Stormwind as the target.
    let mut slots = vec![(0u8, 0i32); 64];
    slots[STORMWIND.1 as usize] = (flag::VISIBLE, 0);
    units::frame(
        &mut app,
        vec![
            SessionEvent::Reputations { standings: slots },
            values(
                ME,
                &[
                    (BYTES_0, HUMAN_WARRIOR),
                    (FIELD_UNIT_FACTIONTEMPLATE, 1),
                    (UNIT_FLAGS, PLAYER_CONTROLLED),
                ],
            ),
            values(WOLF, &[(FIELD_UNIT_FACTIONTEMPLATE, 11)]),
        ],
    );
    units::frame(&mut app, vec![]);
    assert_eq!(
        units::frame(&mut app, vec![at_war(flag::AT_WAR), at_war(0)]),
        ["UPDATE_FACTION:2:1", "UPDATE_FACTION:5:nil"],
        "hostile after the first message, at peace after the second"
    );
}

/// One `SMSG_SET_FACTION_STANDING` entry in the reference's order (`0x4d57f5`-`0x4d5896`): the
/// standing is stored, then the "increased by" line is raised (`0x4d6330`, `0x62c5f0`), then the
/// flag byte moves and the rank line follows (`0x62c670`). So the `CHAT_MSG_COMBAT_FACTION_CHANGE`
/// listener reads the new standing, and a second message's lines read the second.
#[test]
fn the_chat_listeners_read_the_standing_their_line_follows() {
    let mut c = client();
    c.eval::<()>(
        false,
        r#"
        FACTION_STANDING_INCREASED = "Your %s reputation has increased by %d."
        FACTION_STANDING_CHANGED = "You are now %s with %s."
        FACTION_STANDING_LABEL5 = "Friendly"
        FACTION_STANDING_LABEL6 = "Honored"
        local g = CreateFrame("Frame")
        g:RegisterEvent("CHAT_MSG_COMBAT_FACTION_CHANGE")
        g:RegisterEvent("CHAT_MSG_SYSTEM")
        g:SetScript("OnEvent", function()
            local standing
            for i = 1, GetNumFactions() do
                local name, _, _, _, _, value = GetFactionInfo(i)
                if name == "Stormwind" then standing = value end
            end
            table.insert(Seen, event .. " " .. standing)
        end)
        "#,
    );
    c.frame(vec![
        vec![delta(STORMWIND.1, 100)],
        vec![delta(STORMWIND.1, 5100)],
    ]);
    let heard: Vec<String> = c
        .seen()
        .into_iter()
        .filter(|l| l.starts_with("CHAT_MSG"))
        .collect();
    assert_eq!(
        heard,
        [
            "CHAT_MSG_COMBAT_FACTION_CHANGE 4100",
            "CHAT_MSG_COMBAT_FACTION_CHANGE 9100",
            "CHAT_MSG_SYSTEM 9100",
        ],
        "each line reads its message's standing, the rank line after the increase"
    );
}

/// Each standing listener reads its own message's standing, and the friends the packets ahead of
/// it left: a friend list between the two messages, which `GetNumFriends` reads at the call
/// (`0x5ad000`), counts none for the first and Ann for the second.
#[test]
fn each_standing_listener_reads_its_own_span() {
    let mut c = client();
    let ann = super::unit_view_tests::one_friend(&mut c.app);
    c.eval::<()>(
        false,
        r#"local g = CreateFrame("Frame")
        g:RegisterEvent("UPDATE_FACTION")
        g:SetScript("OnEvent", function() table.insert(Seen, "friends " .. GetNumFriends()) end)"#,
    );
    c.frame(vec![
        vec![delta(STORMWIND.1, 100)],
        vec![ann],
        vec![delta(STORMWIND.1, 5100)],
    ]);
    assert_eq!(
        c.seen(),
        [
            "2 4100/5/nil nil/0",
            "friends 0",
            "2 9100/6/nil nil/0",
            "friends 1"
        ]
    );
}
