use bevy::ecs::system::RunSystemOnce;

use super::*;
use crate::local_state::test_env::{EnvGuard, ENV_LOCK};
use crate::net::{ClientCommand, NetCommands};
use crate::query_cache::Ask;

/// An answer's body, built field by field in the wire's order.
#[derive(Default)]
struct Body(Vec<u8>);

impl Body {
    fn u32(mut self, v: u32) -> Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn zeros(mut self, n: usize) -> Self {
        self.0.extend(std::iter::repeat_n(0, 4 * n));
        self
    }
    fn f32(self, v: f32) -> Self {
        self.u32(v.to_bits())
    }
    fn i32(self, v: i32) -> Self {
        self.u32(v as u32)
    }
    fn str(mut self, s: &str) -> Self {
        self.0.extend_from_slice(s.as_bytes());
        self.0.push(0);
        self
    }
    fn u8(mut self, v: u8) -> Self {
        self.0.push(v);
        self
    }
}

const CREATURE: u32 = 69;
const GAME_OBJECT: u32 = 1617;
const ITEM: u32 = 2589;
const NPC_TEXT: u32 = 4437;
const QUEST: u32 = 783;
const PAGE: u32 = 15;
const ITEM_TEXT: u32 = 301;

/// `SMSG_CREATURE_QUERY_RESPONSE`: the names, the subname, seven dwords and two flag bytes.
fn creature_answer(entry: u32) -> Vec<u8> {
    Body::default()
        .u32(entry)
        .str("Stable Master Kitrik")
        .str("")
        .str("")
        .str("")
        .str("Stable Master")
        .u32(0x10)
        .u32(7)
        .u32(0)
        .u32(1)
        .u32(0)
        .u32(5796)
        .u32(533)
        .u8(1)
        .u8(0)
        .0
}

fn game_object_answer() -> Vec<u8> {
    Body::default()
        .u32(GAME_OBJECT)
        .u32(3)
        .u32(310)
        .str("Copper Vein")
        .str("")
        .str("")
        .str("")
        .str("")
        .i32(38)
        .zeros(23)
        .0
}

fn item_answer() -> Vec<u8> {
    Body::default()
        .u32(ITEM)
        .u32(7)
        .u32(5)
        .str("Linen Cloth")
        .str("")
        .str("")
        .str("")
        // Display id through container slots, then the stats, damage and resistance blocks, the
        // delay, ammo and range, five empty spell blocks and the bonding.
        .zeros(96)
        .str("")
        .zeros(14)
        .0
}

fn item_name_answer() -> Vec<u8> {
    Body::default().u32(ITEM).str("Linen Cloth").0
}

fn npc_text_answer() -> Vec<u8> {
    let mut b = Body::default()
        .u32(NPC_TEXT)
        .f32(1.0)
        .str("Greetings, $N.")
        .str("")
        .zeros(7);
    for _ in 1..8 {
        b = b.f32(0.0).str("").str("").zeros(7);
    }
    b.0
}

fn quest_answer(title: &str) -> Vec<u8> {
    Body::default()
        .u32(QUEST)
        .zeros(14 + 8 + 12 + 4)
        .str(title)
        .str("Kill the kobolds.")
        .str("Details.")
        .str("")
        .zeros(16)
        .str("")
        .str("")
        .str("")
        .str("")
        .0
}

fn page_answer() -> Vec<u8> {
    Body::default().u32(PAGE).str("Page one.").u32(0).0
}

fn item_text_answer() -> Vec<u8> {
    Body::default().u32(ITEM_TEXT).str("Dear $N,").0
}

/// A roster with a pick in flight on `realm`.
fn roster(realm: &str) -> Roster {
    let mut roster = Roster::with_pending_pick(
        vec![benilla_protocol::Character {
            guid: 0x2a,
            name: "Probeseven".into(),
            race: 1,
            class: 1,
            gender: 0,
            skin: 0,
            face: 0,
            hair_style: 0,
            hair_color: 0,
            facial_hair: 0,
            level: 1,
            new_character: false,
            zone: 0,
            map: 0,
            position: benilla_protocol::wire::Vector3d {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            flags: 0,
            equipment: [benilla_protocol::CharEnumItem::default(); 19],
            pet_display_id: 0,
            pet_level: 0,
            pet_family: 0,
        }],
        0x2a,
    );
    roster.realm = Some(benilla_protocol::RealmInfo {
        name: realm.into(),
        address: "localhost:8085".into(),
        population: 0.0,
        characters: 1,
        realm_type: 0,
        flags: 0,
        category: 1,
        id: 1,
    });
    roster
}

/// A state folder of the test's own, with the process's environment held still.
struct Home {
    dir: std::path::PathBuf,
    _home: EnvGuard,
    _capture: EnvGuard,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl Home {
    fn new(name: &str) -> Self {
        let lock = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = std::env::temp_dir().join(format!("benilla-wdb-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Self {
            _capture: EnvGuard::unset("WOW_CAPTURE"),
            _home: EnvGuard::set("BENILLA_HOME", dir.to_str().expect("a utf-8 temp path")),
            dir,
            _lock: lock,
        }
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// One login's worth of a saved store: the store, its file and the realm it plays on.
struct Session<T: SavedCache> {
    app: App,
    _store: PhantomData<T>,
}

impl<T: SavedCache> Session<T> {
    /// A fresh client process logging in on `realm`: the store starts empty and the realm's file
    /// is loaded.
    fn start(realm: &str) -> Self {
        let mut app = App::new();
        app.insert_resource(roster(realm))
            .init_resource::<T>()
            .init_resource::<WdbFile<T>>();
        app.world_mut()
            .run_system_once(load::<T>)
            .expect("the load runs");
        Self {
            app,
            _store: PhantomData,
        }
    }

    /// An answer off the wire: its bytes, then its decoded events filed as the live handler files
    /// them, the order the IO thread hands them on in.
    fn answer(&mut self, body: Vec<u8>) {
        let world = self.app.world_mut();
        let packet = benilla_protocol::messages::parse_server(T::ANSWER, &body).expect("parses");
        world.resource_mut::<WdbFile<T>>().keep(body);
        for ev in benilla_protocol::decode(packet) {
            world.resource_mut::<T>().install(ev);
        }
    }

    /// A debounced write between frames.
    fn save(&mut self) {
        self.app
            .world_mut()
            .run_system_once(save_on_exit::<T>)
            .expect("the write runs");
    }

    /// The process exits.
    fn exit(mut self) {
        self.app
            .world_mut()
            .run_system_once(save_on_exit::<T>)
            .expect("the exit write runs");
    }

    fn store(&mut self) -> Mut<'_, T> {
        self.app.world_mut().resource_mut::<T>()
    }
}

/// The queries `read` sends against a store loaded in a new session, and whether it found the
/// record.
fn asks_in_next_session<T: SavedCache>(
    body: Vec<u8>,
    read: impl FnOnce(&mut T, &NetCommands) -> bool,
) -> (bool, Vec<ClientCommand>) {
    let _home = Home::new(T::FILE);
    let mut first = Session::<T>::start("Hydraxian Waterlords");
    first.answer(body);
    first.exit();

    let mut next = Session::<T>::start("Hydraxian Waterlords");
    let (tx, rx) = crossbeam_channel::unbounded();
    let commands = NetCommands(tx);
    let found = read(&mut next.store(), &commands);
    (found, crate::query_cache::queries(&rx))
}

/// The reference saves these stores (`0x554b40` and kin push 1) and a hit sends nothing
/// (`0x5586ca`): a record answered in one session is served without a query in the next.
#[test]
fn an_answer_from_one_session_is_served_without_a_query_in_the_next() {
    let (found, asked) = asks_in_next_session::<NameCache>(creature_answer(CREATURE), |n, c| {
        n.resolve_creature(CREATURE, 0, Ask::Event(c)) == Some("Stable Master Kitrik")
    });
    assert!(found && asked.is_empty(), "creature: {asked:?}");

    let (found, asked) =
        asks_in_next_session::<GameObjectTemplates>(game_object_answer(), |g, c| {
            g.resolve_name(GAME_OBJECT, Ask::Event(c)) == Some("Copper Vein")
        });
    assert!(found && asked.is_empty(), "game object: {asked:?}");

    let (found, asked) = asks_in_next_session::<ItemNames>(item_name_answer(), |n, c| {
        n.name(ITEM, Ask::Event(c)) == Some("Linen Cloth")
    });
    assert!(found && asked.is_empty(), "item name: {asked:?}");

    let (found, asked) = asks_in_next_session::<Items>(item_answer(), |i, c| {
        i.template(ITEM, 0, Ask::Event(c))
            .is_some_and(|t| t.name == "Linen Cloth")
    });
    assert!(found && asked.is_empty(), "item: {asked:?}");

    let (found, asked) = asks_in_next_session::<GossipState>(npc_text_answer(), |g, _| {
        // `true` would owe the text query.
        let owed = g.open_menu(0xf130_0000_0000_0001, NPC_TEXT, Vec::new(), Vec::new(), 0);
        !owed && g.greeting.as_deref() == Some("Greetings, $N.")
    });
    assert!(found && asked.is_empty(), "npc text: {asked:?}");

    let (found, asked) =
        asks_in_next_session::<QuestLog>(quest_answer("The Kobold Menace"), |q, c| {
            q.template(QUEST, Ask::Event(c))
                .is_some_and(|t| t.title == "The Kobold Menace")
        });
    assert!(found && asked.is_empty(), "quest: {asked:?}");

    let (found, asked) = asks_in_next_session::<PageTexts>(page_answer(), |p, c| {
        p.page(PAGE, 0, Ask::Event(c))
            .is_some_and(|p| p.text == "Page one.")
    });
    assert!(found && asked.is_empty(), "page text: {asked:?}");

    let (found, asked) = asks_in_next_session::<MailOpen>(item_text_answer(), |m, c| {
        m.bodies
            .lookup(ITEM_TEXT, || {
                let _ = c.0.send(ClientCommand::ItemTextQuery {
                    text_id: ITEM_TEXT,
                    mail_id: 1,
                });
            })
            .is_some_and(|t| t == "Dear $N,")
    });
    assert!(found && asked.is_empty(), "item text: {asked:?}");
}

/// The creature record survives whole, an absent subname included: `Some("")` would paint an
/// empty tooltip line.
#[test]
fn a_saved_creature_record_comes_back_field_for_field() {
    let _home = Home::new("creature-fields");
    let mut first = Session::<NameCache>::start("R");
    first.answer(creature_answer(CREATURE));
    let mut boar = creature_answer(70);
    // The subname is the fifth string: blank it.
    let at = boar
        .windows(14)
        .position(|w| w == b"Stable Master\0")
        .expect("the subname");
    boar.drain(at..at + 13);
    first.answer(boar);
    first.exit();

    let mut next = Session::<NameCache>::start("R");
    let names = next.store();
    let rec = names.creature_record(CREATURE).expect("the record");
    assert_eq!(rec.name, "Stable Master Kitrik");
    assert_eq!(rec.subname.as_deref(), Some("Stable Master"));
    assert_eq!((rec.creature_type, rec.rank, rec.type_flags), (7, 1, 0x10));
    assert_eq!((rec.civilian, rec.racial_leader), (true, false));
    assert_eq!((rec.display_id, rec.pet_spell_list), (533, 5796));
    assert_eq!(names.creature_record(70).expect("the boar").subname, None);
}

/// No guid- or spawn-keyed name reaches the disk: the name and pet-name stores are built with
/// persistence off (`0x554cd0`, `0x554e10`).
#[test]
fn player_and_pet_names_are_never_saved() {
    let _home = Home::new("identities");
    let mut first = Session::<NameCache>::start("R");
    first.answer(creature_answer(CREATURE));
    first
        .store()
        .insert_player(0x11, "Moamtester".into(), Some((1, 2, 0)));
    first.store().insert_pet(7, "Rexpet".into());
    first.exit();

    let path = crate::local_state::template_cache_path("R", NameCache::FILE).expect("a path");
    let bytes = std::fs::read(&path).expect("the file");
    let has = |needle: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);
    assert!(has(b"Stable Master Kitrik"));
    assert!(!has(b"Moamtester") && !has(b"Rexpet"));

    let mut next = Session::<NameCache>::start("R");
    assert_eq!(next.store().resolve_unit(0x11, None, Ask::Read), None);
}

/// The load compares every header field and keeps nothing on a difference (`0x557390`-
/// `0x55742e`); the realm is ours.
#[test]
fn a_header_from_another_realm_build_locale_or_format_discards_the_file() {
    let body = creature_answer(CREATURE);
    let tag = NameCache::TAG;
    let good = encode(
        tag,
        "Hydraxian Waterlords",
        [(CREATURE, body.as_slice())].into_iter(),
    );
    assert_eq!(
        decode(&good, tag, "Hydraxian Waterlords"),
        Some(vec![(CREATURE, body.clone())])
    );
    assert_eq!(decode(&good, tag, "Another Realm"), None, "another realm");
    assert_eq!(
        decode(&good, GameObjectTemplates::TAG, "Hydraxian Waterlords"),
        None
    );
    for (dword, what) in [(0, "tag"), (1, "build"), (2, "locale"), (3, "format")] {
        let mut bad = good.clone();
        bad[4 * dword] ^= 1;
        assert_eq!(
            decode(&bad, tag, "Hydraxian Waterlords"),
            None,
            "another {what}"
        );
    }
    assert_eq!(
        good[4..8],
        0x16f3u32.to_le_bytes(),
        "the build the header carries"
    );
    // A short read keeps nothing (`0x557665`).
    assert_eq!(
        decode(&good[..good.len() - 12], tag, "Hydraxian Waterlords"),
        None
    );
    assert_eq!(decode(&[], tag, "Hydraxian Waterlords"), None);

    // At the load: the other realm's file is left, and its records are not served.
    let _home = Home::new("realm");
    let mut there = Session::<NameCache>::start("Hydraxian Waterlords");
    there.answer(creature_answer(CREATURE));
    there.exit();
    let elsewhere =
        crate::local_state::template_cache_path("Hydraxian Waterlords", "creaturecache")
            .expect("a path");
    let here =
        crate::local_state::template_cache_path("Lightbringer", "creaturecache").expect("a path");
    std::fs::create_dir_all(here.parent().unwrap()).unwrap();
    std::fs::copy(&elsewhere, &here).unwrap();
    let mut next = Session::<NameCache>::start("Lightbringer");
    assert!(next.store().creature_record(CREATURE).is_none());
}

/// A realm change in one process saves the realm left and serves none of its records on the new
/// one.
#[test]
fn a_realm_change_saves_the_old_realm_and_starts_from_the_new_ones_file() {
    let _home = Home::new("realm-change");
    let mut session = Session::<NameCache>::start("Hydraxian Waterlords");
    session.answer(creature_answer(CREATURE));
    session.app.insert_resource(roster("Lightbringer"));
    session
        .app
        .world_mut()
        .run_system_once(load::<NameCache>)
        .expect("the load runs");
    assert!(session.store().creature_record(CREATURE).is_none());
    session.exit();

    let mut back = Session::<NameCache>::start("Hydraxian Waterlords");
    assert!(back.store().creature_record(CREATURE).is_some());
}

/// A refusal frees the record (`0x556f35`), so it is not saved and the next session asks again.
#[test]
fn an_eviction_re_asks_in_the_next_session() {
    let _home = Home::new("eviction");
    let mut first = Session::<NameCache>::start("R");
    first.answer(creature_answer(CREATURE));
    first.exit();

    let mut second = Session::<NameCache>::start("R");
    assert!(second.store().creature_record(CREATURE).is_some());
    second.answer(Body::default().u32(CREATURE | 0x8000_0000).0);
    assert!(second.store().creature_record(CREATURE).is_none());
    second.exit();

    let mut third = Session::<NameCache>::start("R");
    let (tx, rx) = crossbeam_channel::unbounded();
    let commands = NetCommands(tx);
    assert_eq!(
        third
            .store()
            .resolve_creature(CREATURE, 0, Ask::Event(&commands)),
        None
    );
    assert!(matches!(
        crate::query_cache::queries(&rx)[..],
        [ClientCommand::CreatureQuery {
            entry: CREATURE,
            ..
        }]
    ));
}

/// A quest's empty title is a refusal (`0x563150`): it frees a saved record too.
#[test]
fn a_quest_refused_with_an_empty_title_is_dropped_from_the_file() {
    let _home = Home::new("quest-refusal");
    let mut first = Session::<QuestLog>::start("R");
    first.answer(quest_answer("The Kobold Menace"));
    first.exit();
    let mut second = Session::<QuestLog>::start("R");
    assert!(second.store().template(QUEST, Ask::Read).is_some());
    second.answer(quest_answer(""));
    second.exit();
    let mut third = Session::<QuestLog>::start("R");
    assert!(third.store().template(QUEST, Ask::Read).is_none());
}

/// The saved stores outlive a disconnect and the world entry's mail init (`0x4acb10`), which
/// touch no `DBCache`; only a store with persistence off is cleared (`0x555740`).
#[test]
fn a_disconnect_keeps_the_saved_quest_and_item_text_records() {
    let mut quests = QuestLog::default();
    let template = benilla_protocol::messages::parse_server(
        QuestLog::ANSWER,
        &quest_answer("The Kobold Menace"),
    )
    .expect("parses");
    for ev in benilla_protocol::decode(template) {
        quests.install(ev);
    }
    quests.clear_session();
    assert!(quests.template(QUEST, Ask::Read).is_some());

    let mut mail = MailOpen::default();
    mail.bodies.insert(ITEM_TEXT, "Dear $N,".into());
    mail.clear_session();
    assert_eq!(
        mail.bodies.get(ITEM_TEXT).map(String::as_str),
        Some("Dear $N,")
    );
}

/// The guild and petition stores are built with persistence off (`0x554d20`, `0x554e60`), so the
/// world entry clears them (`0x5557c0`, `0x55580c`).
#[test]
fn the_session_only_stores_are_empty_after_world_entry() {
    let mut app = App::new();
    app.init_resource::<Time>().add_plugins(WdbPlugin);
    app.world_mut()
        .resource_mut::<GuildState>()
        .land_for_test(9, "Wardens");
    app.world_mut()
        .resource_mut::<PetitionRecords>()
        .answer_for_test(benilla_protocol::messages::PetitionQueryResponse {
            petition_id: 7,
            name: "Legacy".into(),
            ..Default::default()
        });
    assert!(app.world().resource::<GuildState>().identity_known(9));
    assert!(app.world().resource::<PetitionRecords>().holds_for_test(7));

    crate::net::handlers::dispatch(
        app.world_mut(),
        vec![SessionEvent::Connected {
            self_guid: 0x2a,
            name: "Probeseven".into(),
            billing: Default::default(),
            tutorial_flags: None,
            addon_info: None,
        }],
    );

    assert!(!app.world().resource::<GuildState>().identity_known(9));
    assert!(!app.world().resource::<PetitionRecords>().holds_for_test(7));
}

/// The IO thread hands on the bytes of exactly the saved stores' answers.
#[test]
fn the_saved_answers_are_the_saved_stores_answers() {
    for opcode in [0x61, 0x5f, 0x2c5, 0x58, 0x180, 0x5d, 0x5b, 0x244] {
        assert!(is_saved_answer(opcode), "{opcode:#x}");
    }
    // The names, guild, pet-name and petition answers.
    for opcode in [0x51, 0x55, 0x53, 0x1c7] {
        assert!(!is_saved_answer(opcode), "{opcode:#x}");
    }
}

/// An answer's record and its bytes drained in two frames, with a write between: the bytes still
/// reach the file.
#[test]
fn bytes_that_land_after_a_write_still_reach_the_file() {
    let _home = Home::new("split");
    let mut first = Session::<NameCache>::start("R");
    let body = creature_answer(CREATURE);
    let packet = benilla_protocol::messages::parse_server(NameCache::ANSWER, &body).unwrap();
    for ev in benilla_protocol::decode(packet) {
        first.store().install(ev);
    }
    first.save();
    first
        .app
        .world_mut()
        .resource_mut::<WdbFile<NameCache>>()
        .keep(body);
    first.exit();

    let mut next = Session::<NameCache>::start("R");
    assert!(next.store().creature_record(CREATURE).is_some());
}

/// The creature names an earlier build kept in `cache/<realm>.tsv` go when the realm's stores
/// load.
#[test]
fn the_load_removes_the_earlier_creature_file() {
    let home = Home::new("tsv");
    let old = home.dir.join("cache").join("Hydraxian_Waterlords.tsv");
    std::fs::create_dir_all(old.parent().unwrap()).unwrap();
    std::fs::write(
        &old,
        "benilla-namecache\t3\t5875\tenGB\tHydraxian Waterlords\n",
    )
    .unwrap();
    let _session = Session::<NameCache>::start("Hydraxian Waterlords");
    assert!(!old.exists());
}

/// Whether a fresh process on realm `R` holds `key` in the store `T` loads.
fn held_next_session<T: SavedCache>(key: u32) -> bool {
    Session::<T>::start("R").store().store().get(key).is_some()
}

/// Every saved answer from its packet through the IO thread's emission ([`crate::net::io::inbound`])
/// and the live client's handler table to the store's file, and back in the next process.
#[test]
fn each_answer_reaches_its_own_stores_file_through_the_live_handlers() {
    let _home = Home::new("live");
    let answers = [
        (NameCache::ANSWER, creature_answer(CREATURE)),
        (GameObjectTemplates::ANSWER, game_object_answer()),
        (ItemNames::ANSWER, item_name_answer()),
        (Items::ANSWER, item_answer()),
        (GossipState::ANSWER, npc_text_answer()),
        (QuestLog::ANSWER, quest_answer("The Kobold Menace")),
        (PageTexts::ANSWER, page_answer()),
        (MailOpen::ANSWER, item_text_answer()),
    ];
    let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
    app.insert_resource(roster("R"));
    fn load_all(world: &mut World) {
        world.run_system_once(load::<NameCache>).unwrap();
        world.run_system_once(load::<GameObjectTemplates>).unwrap();
        world.run_system_once(load::<ItemNames>).unwrap();
        world.run_system_once(load::<Items>).unwrap();
        world.run_system_once(load::<GossipState>).unwrap();
        world.run_system_once(load::<QuestLog>).unwrap();
        world.run_system_once(load::<PageTexts>).unwrap();
        world.run_system_once(load::<MailOpen>).unwrap();
    }
    load_all(app.world_mut());
    for (opcode, body) in answers {
        let packet = benilla_protocol::messages::parse_server(opcode, &body).expect("parses");
        let events = crate::net::io::inbound(opcode, benilla_protocol::decode(packet), body);
        crate::net::handlers::dispatch(app.world_mut(), events);
    }
    let world = app.world_mut();
    world.run_system_once(save_on_exit::<NameCache>).unwrap();
    world
        .run_system_once(save_on_exit::<GameObjectTemplates>)
        .unwrap();
    world.run_system_once(save_on_exit::<ItemNames>).unwrap();
    world.run_system_once(save_on_exit::<Items>).unwrap();
    world.run_system_once(save_on_exit::<GossipState>).unwrap();
    world.run_system_once(save_on_exit::<QuestLog>).unwrap();
    world.run_system_once(save_on_exit::<PageTexts>).unwrap();
    world.run_system_once(save_on_exit::<MailOpen>).unwrap();
    drop(app);

    assert!(held_next_session::<NameCache>(CREATURE), "creature");
    assert!(
        held_next_session::<GameObjectTemplates>(GAME_OBJECT),
        "game object"
    );
    assert!(held_next_session::<ItemNames>(ITEM), "item name");
    assert!(held_next_session::<Items>(ITEM), "item");
    assert!(held_next_session::<GossipState>(NPC_TEXT), "npc text");
    assert!(held_next_session::<QuestLog>(QUEST), "quest");
    assert!(held_next_session::<PageTexts>(PAGE), "page text");
    assert!(held_next_session::<MailOpen>(ITEM_TEXT), "item text");
}
