use super::*;
use crate::net::GuidIndex;
use benilla_protocol::messages::{TradeStatus, TradeStatusExtended};
use benilla_ui::script::{TradeSideState, TradeState};

/// Both offers as the getters resolve them, every slot read off `items` and `names`.
fn rows(s: &TradeSession, items: &Items, names: &NameCache) -> TradeState {
    let side = |offer: &TradeOffer, own: bool| TradeSideState {
        slots: offer.slots.map(|slot| {
            slot.map(|it| match own {
                true => resolve_own_slot(&it, items, None, names, crate::items::RollCatalogs::NONE),
                false => resolve_slot(&it, items, None, names, crate::items::RollCatalogs::NONE),
            })
        }),
        gold: offer.gold,
    };
    TradeState {
        player: side(&s.our, true),
        target: side(&s.their, false),
    }
}

fn wire_item(entry: u32) -> TradeItem {
    TradeItem {
        entry,
        display_id: 42,
        count: 3,
        wrapped: false,
        gift_creator: 0,
        perm_enchant: 0,
        creator: 0,
        charges: 0,
        suffix_factor: 0,
        random_prop_id: 0,
        lock_id: 0,
        max_durability: 0,
        durability: 0,
    }
}

/// The row carries the template's quality and InventoryType, which `GetTradePlayerItemInfo`
/// gates its fourth value on; an unanswered template leaves both empty.
#[test]
fn a_trade_row_carries_the_templates_quality_and_inventory_type() {
    let mut items = Items::default();
    let mut sword = crate::items::test_template("Sword");
    sword.quality = 4;
    sword.inventory_type = 13;
    items.insert_template(1234, Some(sword));
    items.insert_template(1235, Some(crate::items::test_template("Linen Cloth")));

    let row = resolve_slot(
        &wire_item(1234),
        &items,
        None,
        &NameCache::default(),
        crate::items::RollCatalogs::NONE,
    );
    assert_eq!((row.quality, row.inventory_type), (Some(4), 13));
    // The icon is the template's display's, the question mark when that row is missing, and
    // left to the getter while the template is unanswered.
    assert_eq!(
        row.texture.as_deref(),
        Some("Interface\\Icons\\INV_Misc_QuestionMark")
    );
    assert_eq!(
        resolve_slot(
            &wire_item(1236),
            &items,
            None,
            &NameCache::default(),
            crate::items::RollCatalogs::NONE
        )
        .texture,
        None
    );
    let row = resolve_slot(
        &wire_item(1235),
        &items,
        None,
        &NameCache::default(),
        crate::items::RollCatalogs::NONE,
    );
    assert_eq!((row.quality, row.inventory_type), (Some(1), 0));
    let row = resolve_slot(
        &wire_item(1236),
        &items,
        None,
        &NameCache::default(),
        crate::items::RollCatalogs::NONE,
    );
    assert_eq!((row.quality, row.inventory_type), (None, 0));
}

/// The partner's slot links from the wire's record as `GetTradeTargetItemLink` prints it
/// (`0x4c040a`-`0x4c0431` → `0x52adb0`): the permanent enchant (`+0x0c`, `0xb715c0`), the
/// random property id (`+0x30`, `0xb714b8`) and the suffix factor (`+0x2c`, `0xb715a0`).
#[test]
fn a_partner_slot_links_with_the_wires_enchant_roll_and_suffix() {
    let mut items = Items::default();
    let mut sword = crate::items::test_template("Bloodrazor");
    sword.quality = 3;
    items.insert_template(1234, Some(sword));
    let item = TradeItem {
        perm_enchant: 1900,
        random_prop_id: 862,
        suffix_factor: 77,
        ..wire_item(1234)
    };
    let props =
        crate::items::RandomProperties::new(benilla_formats::RandomPropertyCatalog::from_rows(
            [(
                862,
                benilla_formats::RandomProperty {
                    suffix: "of the Monkey".into(),
                    enchants: [0; 5],
                },
            )]
            .into_iter()
            .collect(),
        ));
    let rolls = crate::items::RollCatalogs {
        props: Some(&props),
        enchants: None,
    };
    let row = resolve_slot(&item, &items, None, &NameCache::default(), rolls);
    // The name joins the suffix as `GetTradeTargetItemInfo` names it (`0x4c01b8`).
    assert_eq!(row.name.as_deref(), Some("Bloodrazor of the Monkey"));
    assert_eq!(
        row.link.as_deref(),
        Some("|cff0070dd|Hitem:1234:1900:862:77|h[Bloodrazor of the Monkey]|h|r")
    );
}

/// Our own slot's item as `GetTradePlayerItemLink` links it, from the item object through
/// `0x52ae00` (`0x4c06bc`): the permanent enchant, the random property id and the seed.
#[test]
fn an_own_slot_links_from_the_item_object() {
    use bevy::ecs::system::RunSystemOnce;
    const GUID: u64 = 0x4000_0000_0000_0123;
    let mut world = World::new();
    world.init_resource::<GuidIndex>();
    let mut items = Items::default();
    items.insert_template(1234, Some(crate::items::test_template("Helm")));
    world.insert_resource(items);
    crate::items::test_spawn_item(
        &mut world,
        GUID,
        benilla_protocol::ObjectFields::from_pairs(&[(3, 1234), (22, 1900), (43, 7), (44, 862)]),
        false,
    );
    let item = world
        .run_system_once(|objects: Objects, items: Res<Items>| own_item(GUID, 1, &objects, &items))
        .unwrap()
        .expect("the item streams");
    let row = resolve_slot(
        &item,
        world.resource::<Items>(),
        None,
        &NameCache::default(),
        crate::items::RollCatalogs::NONE,
    );
    assert_eq!(
        row.link.as_deref(),
        Some("|cffffffff|Hitem:1234:1900:862:7|h[Helm]|h|r")
    );
}

/// Our own item with no cached record still links (`0x4c06bc` → `0x52ae00`): white
/// (`0x52aec5`) and nameless (`0x5d8b40`), with the object's enchant, roll and seed, the record's
/// flags reading 0 without it (`0x5da2a0`).
#[test]
fn an_own_slot_with_no_record_links_white_and_nameless() {
    use bevy::ecs::system::RunSystemOnce;
    const GUID: u64 = 0x4000_0000_0000_0123;
    let mut world = World::new();
    world.init_resource::<GuidIndex>();
    world.insert_resource(Items::default());
    crate::items::test_spawn_item(
        &mut world,
        GUID,
        benilla_protocol::ObjectFields::from_pairs(&[(3, 1234), (22, 1900), (43, 7), (44, 862)]),
        false,
    );
    let item = world
        .run_system_once(|objects: Objects, items: Res<Items>| own_item(GUID, 1, &objects, &items))
        .unwrap()
        .expect("the item streams");
    let row = resolve_own_slot(
        &item,
        world.resource::<Items>(),
        None,
        &NameCache::default(),
        crate::items::RollCatalogs::NONE,
    );
    assert_eq!(row.link, None);
    assert_eq!(
        row.uncached_link.as_deref(),
        Some("|cffffffff|Hitem:1234:1900:862:7|h[]|h|r")
    );
}

/// A wrapped gift's giver rides both columns: the partner's block carries the wire's wrapped
/// word and gift creator (`0x534559`, `0x534574`), our placed item's object the same; only a
/// wrapped item names it, a zero guid names no one, and a name not yet answered is `None`.
#[test]
fn a_wrapped_gifts_giver_reaches_both_columns() {
    let mut items = Items::default();
    items.insert_template(1234, Some(crate::items::test_template("Gift")));
    let mut names = NameCache::default();
    names.insert_player(0x7, "Santa".to_string(), None);
    let gift = |wrapped, giver| TradeItem {
        wrapped,
        gift_creator: giver,
        ..wire_item(1234)
    };
    let mut s = TradeSession::default();
    s.begin(0x9);
    s.open_window();
    s.place_own_item(1, gift(true, 0x7));
    s.place_own_item(2, gift(false, 0x7));
    s.place_own_item(3, gift(true, 0));
    s.place_own_item(4, gift(true, 0x8));
    s.their.slots[0] = Some(gift(true, 0x7));
    s.their.slots[1] = Some(gift(false, 0x7));
    let state = rows(&s, &items, &names);
    let giver = |side: &TradeSideState, i: usize| {
        let row = side.slots[i].as_ref().expect("filled");
        (row.wrapped, row.gift_creator.clone())
    };
    assert_eq!(giver(&state.target, 0), (true, Some("Santa".into())));
    assert_eq!(
        giver(&state.target, 1),
        (false, None),
        "unwrapped asks nothing"
    );
    assert_eq!(giver(&state.player, 0), (true, Some("Santa".into())));
    assert_eq!(giver(&state.player, 1), (false, None));
    assert_eq!(
        giver(&state.player, 2),
        (true, None),
        "a zero guid names no one"
    );
    assert_eq!(giver(&state.player, 3), (true, None), "a name in flight");
}

/// A crafted item's maker rides both columns: the partner's block carries the wire's creator
/// (`+0x410`, `0x53453e`), our placed item its object's `ITEM_FIELD_CREATOR` (`0x52e247`); a
/// zero guid names no one, and a name not yet answered keeps its guid for the build to ask.
#[test]
fn a_crafted_items_maker_reaches_both_columns() {
    let mut items = Items::default();
    items.insert_template(1234, Some(crate::items::test_template("Bag")));
    let mut names = NameCache::default();
    names.insert_player(0x7, "Tailor".to_string(), None);
    let made = |creator| TradeItem {
        creator,
        ..wire_item(1234)
    };
    let mut s = TradeSession::default();
    s.begin(0x9);
    s.open_window();
    s.place_own_item(1, made(0x7));
    s.their.slots[0] = Some(made(0x7));
    s.their.slots[1] = Some(made(0));
    s.their.slots[2] = Some(made(0x8));
    let state = rows(&s, &items, &names);
    let maker = |side: &TradeSideState, i: usize| {
        let row = side.slots[i].as_ref().expect("filled");
        (row.creator.clone(), row.creator_guid)
    };
    assert_eq!(maker(&state.player, 0), (Some("Tailor".into()), 0x7));
    assert_eq!(maker(&state.target, 0), (Some("Tailor".into()), 0x7));
    assert_eq!(
        maker(&state.target, 1),
        (None, 0),
        "a zero guid names no one"
    );
    assert_eq!(maker(&state.target, 2), (None, 0x8), "a name in flight");
}

#[test]
fn open_close_state_machine() {
    let mut s = TradeSession::default();
    assert!(!s.is_open());

    s.initiate(0x1234, false);
    assert_eq!(s.partner, Some(0x1234));
    assert!(!s.is_open());
    assert_eq!(s.cell(), None);

    s.open_window();
    assert!(s.is_open());
    assert_eq!(s.cell(), Some(0x1234));

    s.close_window();
    assert!(!s.is_open());
    assert_eq!(s.partner, None);
    assert_eq!(s.cell(), None);
}

/// The offers live in tables only an open clears (`0x4bf549`-`0x4bf5ef`): a close, a new
/// initiate and an accepted request keep them for the getters, and the open clears them, money
/// and the slot 7 spells with them (`0x5d4825`-`0x5d4864`).
#[test]
fn only_an_open_clears_the_offers() {
    let mut s = TradeSession::default();
    s.begin(0x1);
    s.open_window();
    s.place_own_item(1, wire_item(2589));
    s.set_own_gold(300);
    s.their.slots[0] = Some(wire_item(1234));
    s.their.enchant_spell_id = 13898;
    s.close_window();
    s.initiate(0x2, false);
    s.begin(0x2);
    assert!(s.our.slots[0].is_some() && s.their.slots[0].is_some());
    assert_eq!((s.our.gold, s.their.enchant_spell_id), (300, 13898));
    s.open_window();
    assert!(s.our.slots[0].is_none() && s.their.slots[0].is_none());
    assert_eq!((s.our.gold, s.their.enchant_spell_id), (0, 0));
}

#[test]
fn extended_fills_the_right_side_and_resets_accepts() {
    let mut s = TradeSession::default();
    let mut lane = TradeLane::default();
    s.begin(0x1);
    s.open_window();
    s.partner_accepted(&mut lane);
    assert!(s.their_accept);

    let mut slots = [None; TRADE_SLOT_COUNT];
    slots[0] = Some(wire_item(2589));
    let ext = TradeStatusExtended {
        their_window: true,
        sequence: 7,
        flag: 7,
        gold: 500,
        enchant_spell_id: 0,
        slots,
    };
    s.set_offer(&ext, &mut lane);
    assert!(s.their.slots[0].is_some());
    assert_eq!(s.their.gold, 500);
    assert_eq!(s.our.gold, 0);
    assert!(!s.their_accept, "an offer change resets accepts");

    let our_ext = TradeStatusExtended {
        their_window: false,
        sequence: 7,
        flag: 7,
        gold: 999,
        enchant_spell_id: 0,
        slots: [None; TRADE_SLOT_COUNT],
    };
    s.set_offer(&our_ext, &mut lane);
    assert_eq!(s.our.gold, 999);
    assert_eq!(s.their.gold, 500, "the other side is untouched");
}

/// `0x4bf2b0` in its order: both accepts dropped, each partner slot whose record moved, the
/// spells and the money; an own-side status compares the partner's records too, so its own
/// slots signal nothing but an emptied one's invalidation, which comes first (`0x5d4ad3`).
#[test]
fn an_extended_status_queues_the_compares_signals_in_order() {
    let mut s = TradeSession::default();
    let mut lane = TradeLane::default();
    s.begin(0x1);
    s.open_window();
    s.accept();
    s.partner_accepted(&mut lane);
    lane.take();
    let mut theirs = [None; TRADE_SLOT_COUNT];
    theirs[0] = Some(wire_item(1234));
    theirs[2] = Some(wire_item(1235));
    s.set_offer(
        &TradeStatusExtended {
            their_window: true,
            sequence: 7,
            flag: 7,
            gold: 500,
            enchant_spell_id: 0,
            slots: theirs,
        },
        &mut lane,
    );
    assert_eq!(
        lane.take(),
        [
            TradeEvent::Accept(false, true),
            TradeEvent::Unaccept,
            TradeEvent::Accept(false, false),
            TradeEvent::TargetItem(0),
            TradeEvent::TargetItem(2),
            TradeEvent::TargetMoney,
        ]
    );
    // The same records again, the display aside, which the compare skips: nothing.
    theirs[0] = Some(TradeItem {
        display_id: 99,
        ..wire_item(1234)
    });
    let ext = TradeStatusExtended {
        their_window: true,
        sequence: 7,
        flag: 7,
        gold: 500,
        enchant_spell_id: 0,
        slots: theirs,
    };
    s.set_offer(&ext, &mut lane);
    assert!(lane.take().is_empty());
    // A count that moved signals its slot.
    theirs[2] = Some(TradeItem {
        count: 9,
        ..wire_item(1235)
    });
    s.set_offer(
        &TradeStatusExtended {
            slots: theirs,
            ..ext
        },
        &mut lane,
    );
    assert_eq!(lane.take(), [TradeEvent::TargetItem(2)]);

    // Our own side: an emptied slot invalidates, a new one signals nothing, then our money.
    s.place_own_item(1, wire_item(2589));
    let mut ours = [None; TRADE_SLOT_COUNT];
    ours[1] = Some(wire_item(2590));
    s.set_offer(
        &TradeStatusExtended {
            their_window: false,
            sequence: 7,
            flag: 7,
            gold: 40,
            enchant_spell_id: 0,
            slots: ours,
        },
        &mut lane,
    );
    assert_eq!(
        lane.take(),
        [TradeEvent::Invalidate(0), TradeEvent::PlayerMoney]
    );
}

#[test]
fn accept_glow_tracks_both_sides() {
    let mut s = TradeSession::default();
    let mut lane = TradeLane::default();
    s.begin(0x1);
    s.open_window();
    s.accept();
    s.partner_accepted(&mut lane);
    assert_eq!((s.our_accept, s.their_accept), (true, true));
    s.back_to_trade(&mut lane);
    assert_eq!((s.our_accept, s.their_accept), (false, false));
}

#[test]
fn own_offer_is_tracked_optimistically() {
    let mut s = TradeSession::default();
    s.begin(0x7);
    s.open_window();
    s.accept();
    s.partner_accepted(&mut TradeLane::default());
    assert_eq!((s.our_accept, s.their_accept), (true, true));

    s.place_own_item(2, wire_item(1234));
    assert!(s.our.slots[1].is_some(), "our slot 2 shows the placed item");
    assert_eq!(
        (s.our_accept, s.their_accept),
        (true, true),
        "our edit leaves both accepts: only `0x4bf230` and `0x4bf280` write them, on the \
         server's `BACK_TO_TRADE` or status"
    );

    s.set_own_gold(500);
    assert_eq!(s.our.gold, 500);

    s.clear_own_item(2);
    assert!(s.our.slots[1].is_none(), "clearing empties the slot");
}

#[test]
fn status_codes_are_stable() {
    assert_eq!(TradeStatus::OpenWindow.code(), 2);
    assert_eq!(TradeStatus::Accept.code(), 4);
    assert_eq!(TradeStatus::BackToTrade.code(), 7);
}

/// An app with the trade's handlers and lane, a live VM and `0x7` named in the cache. The lane
/// fires through [`drain`].
fn feed_app() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
    let (tx, rx) = crossbeam_channel::unbounded();
    let mut app = App::new();
    let mut names = NameCache::default();
    names.insert_player(0x7, "Grubbis".to_string(), None);
    app.init_resource::<TradeSession>()
        .init_resource::<Items>()
        .init_resource::<crate::ui_action::UiErrorKeys>()
        .init_resource::<GuidIndex>()
        .init_resource::<crate::item_lane::ItemLane>()
        .init_resource::<crate::ui_chat::ChatLog>()
        .init_resource::<crate::sound::MessageSounds>()
        .insert_resource(names)
        .insert_resource(NetCommands(tx));
    // The calls are kept, not applied: this app has no host, and its tests read the VM.
    let mut script = UiScript::new().unwrap();
    script.keep_calls();
    app.insert_non_send_resource(script);
    app.add_latched_window::<TradeSession>();
    net::register(&mut app);
    (app, rx)
}

/// What the handlers left, applied, then the trade lane fired, as the drain fires it.
fn drain(app: &mut App) {
    app.world_mut().flush();
    lane::fire_trade_lane(app.world_mut());
}

/// `chunk`'s answer with the trade's live reads lent, as the host lends them.
fn eval_live<T: benilla_ui::mlua::FromLuaMulti>(app: &mut App, chunk: &str) -> T {
    struct Lent<'a, 'w>(&'a mut TradeLive<'w>);
    impl benilla_ui::script::LiveReader for Lent<'_, '_> {
        fn read_live(&mut self, ask: &mut benilla_ui::script::live::LiveAsk<'_>) -> bool {
            crate::script_calls::live::LiveFamily::answer(self.0, ask)
        }
    }
    let world = app.world_mut();
    let mut script = world.remove_non_send_resource::<UiScript>().expect("a VM");
    let mut state = bevy::ecs::system::SystemState::<TradeLive>::new(world);
    let mut live = state.get(world);
    let r = script.reading(&mut Lent(&mut live), |s| s.eval::<T>(chunk).expect("runs"));
    world.insert_non_send_resource(script);
    r
}

/// The getters' record asks the VM kept, made as the host makes them inside the call
/// ([`ask_trade_record`]).
fn ask_kept_records(app: &mut App) {
    let world = app.world_mut();
    let entries = world
        .non_send_resource_mut::<UiScript>()
        .take_trade_record_asks();
    for entry in entries {
        ask_trade_record(
            world.resource::<Items>(),
            world.resource::<NetCommands>(),
            entry,
            0,
        );
    }
}

/// The server's status `status`, through the handler table, then the lane.
fn status(app: &mut App, status: TradeStatus) {
    crate::net::handlers::dispatch(
        app.world_mut(),
        vec![benilla_protocol::SessionEvent::TradeStatus { status }],
    );
    drain(app);
}

/// The server's extended status `ext`, through the handler table, then the lane.
fn extended(app: &mut App, ext: TradeStatusExtended) {
    crate::net::handlers::dispatch(
        app.world_mut(),
        vec![benilla_protocol::SessionEvent::TradeStatusExtended {
            state: Box::new(ext),
        }],
    );
    drain(app);
}

/// A trade accepted with `partner` and its `OPEN_WINDOW`, the window holding the latch.
fn open_with(app: &mut App, partner: u64) {
    app.world_mut()
        .resource_mut::<TradeSession>()
        .begin(partner);
    status(app, TradeStatus::OpenWindow);
}

/// The number of `TRADE_REQUEST_CANCEL`s a spy frame has seen.
fn cancels_seen(app: &mut App) -> i64 {
    app.world_mut()
        .non_send_resource::<UiScript>()
        .eval::<i64>("return BENILLA_TEST_TRADE_CANCELS or 0")
        .unwrap()
}

#[test]
fn canceled_fires_trade_request_cancel_and_the_siblings_do_not() {
    let (mut app, _rx) = feed_app();
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(
            r#"
            local f = CreateFrame("Frame")
            f:RegisterEvent("TRADE_REQUEST_CANCEL")
            f:SetScript("OnEvent", function()
                BENILLA_TEST_TRADE_CANCELS = (BENILLA_TEST_TRADE_CANCELS or 0) + 1
            end)
            "#,
        )
        .unwrap();
    assert_eq!(cancels_seen(&mut app), 0, "nothing has cancelled anything");

    // `COMPLETE` and `CLOSE_WINDOW` close without the signal.
    for closing in [
        TradeStatus::Complete,
        TradeStatus::CloseWindow {
            result: 0,
            side: 0,
            item_limit_category: 0,
        },
    ] {
        open_with(&mut app, 0x7);
        status(&mut app, closing);
        assert_eq!(
            cancels_seen(&mut app),
            0,
            "closing the window is not itself a cancel signal"
        );
    }

    open_with(&mut app, 0x7);
    status(&mut app, TradeStatus::Canceled);
    assert_eq!(cancels_seen(&mut app), 1, "CANCELED signals it, once");
    drain(&mut app);
    assert_eq!(cancels_seen(&mut app), 1, "and only once");
}

/// The reference prints the line from its own sender (`0x5d4031`): a cached name at once
/// (`0x5d4042`), with no object test; row `0xb9` is chat.
#[test]
fn initiating_a_trade_says_so() {
    let (mut app, rx) = feed_app();
    app.world_mut()
        .resource_scope(|world, mut trade: Mut<TradeSession>| {
            trade.initiate_send(
                0x7,
                false,
                world.resource::<NetCommands>(),
                world.resource::<NameCache>(),
                world.resource::<crate::ui_action::UiErrorKeys>(),
            );
        });
    assert!(matches!(
        rx.try_recv(),
        Ok(ClientCommand::InitiateTrade { target: 0x7 })
    ));
    assert_eq!(
        app.world()
            .resource::<crate::ui_action::UiErrorKeys>()
            .snapshot(),
        vec![crate::ui_action::UiError::s(
            "ERR_INITIATE_TRADE_S",
            "Grubbis".to_string()
        )],
        "0x7 is cached, not a streamed object: the initiate line needs no object"
    );
    assert_eq!(
        benilla_ui::messages::kind_of("ERR_INITIATE_TRADE_S"),
        benilla_ui::messages::MsgKind::Chat,
    );
}

/// `initiate_send` on `who` in `app`'s world.
fn initiate(app: &mut App, who: u64) {
    app.world_mut()
        .resource_scope(|world, mut trade: Mut<TradeSession>| {
            trade.initiate_send(
                who,
                false,
                world.resource::<NetCommands>(),
                world.resource::<NameCache>(),
                world.resource::<crate::ui_action::UiErrorKeys>(),
            );
        });
}

/// The name answer for `who`, `name` empty for a refusal, as the cache files it and runs the
/// trade's callbacks.
fn name_answer(app: &mut App, who: u64, name: &str) {
    use bevy::ecs::system::RunSystemOnce;
    let name = name.to_string();
    app.world_mut()
        .run_system_once(
            move |mut names: ResMut<NameCache>, mut answers: TradeNameAnswers| {
                names.insert_player(who, name.clone(), None);
                answers.answered(who, &names);
            },
        )
        .unwrap();
}

/// A miss keeps the callback `0x5d4080`, which the answer runs: a found name shows the line on
/// the lane at the answer's packet (`0x5d40ca`), a close in between notwithstanding; a refusal
/// spends it silently (`0x5d40a0`).
#[test]
fn the_initiate_line_waits_for_the_name_and_survives_the_close() {
    let (mut app, _rx) = feed_app();
    initiate(&mut app, 0x99); // no name cached
    assert!(
        app.world()
            .resource::<crate::ui_action::UiErrorKeys>()
            .is_empty(),
        "no name, no line, and never a line with an unfilled %s"
    );
    app.world_mut()
        .resource_mut::<TradeSession>()
        .close_window();
    name_answer(&mut app, 0x99, "Skarrid");
    assert_eq!(
        app.world_mut().resource_mut::<TradeLane>().take(),
        [TradeEvent::Line(crate::ui_action::UiError::s(
            "ERR_INITIATE_TRADE_S",
            "Skarrid"
        ))]
    );
    name_answer(&mut app, 0x99, "Skarrid");
    assert!(
        app.world_mut()
            .resource_mut::<TradeLane>()
            .take()
            .is_empty(),
        "one callback, spent"
    );

    initiate(&mut app, 0x98);
    name_answer(&mut app, 0x98, "");
    assert!(
        app.world_mut()
            .resource_mut::<TradeLane>()
            .take()
            .is_empty(),
        "a refusal shows nothing"
    );
    assert!(app
        .world()
        .resource::<TradeSession>()
        .initiate_waits
        .is_empty());
}

/// A request's status through the handler table: the ladder answers inside it.
fn request(app: &mut App, from: u64) {
    crate::net::handlers::dispatch(
        app.world_mut(),
        vec![benilla_protocol::SessionEvent::TradeStatus {
            status: TradeStatus::BeginTrade { partner: from },
        }],
    );
}

/// The lines on the trade lane, taking every item.
fn lines(app: &mut App) -> Vec<crate::ui_action::UiError> {
    app.world_mut()
        .resource_mut::<TradeLane>()
        .take()
        .into_iter()
        .filter_map(|e| match e {
            TradeEvent::Line(line) => Some(line),
            _ => None,
        })
        .collect()
}

#[test]
fn a_request_that_survives_the_ladder_is_accepted() {
    let (mut app, rx) = request_app(false, false, true);
    request(&mut app, 0x7);

    assert!(
        matches!(rx.try_recv(), Ok(ClientCommand::BeginTrade)),
        "nothing refused it, so the client answers inside the status, and the server opens \
         both windows"
    );
    let trade = app.world().resource::<TradeSession>();
    assert_eq!(trade.partner, Some(0x7));
    assert!(
        !trade.initiated,
        "a trade we did not start never sets the initiate latch"
    );
    assert!(
        lines(&mut app).is_empty(),
        "and an accepted request says nothing: only leg 8 speaks"
    );
}

#[test]
fn block_trades_refuses_and_says_so() {
    let (mut app, rx) = request_app(true, false, true);
    request(&mut app, 0x7);

    assert!(matches!(rx.try_recv(), Ok(ClientCommand::BusyTrade)));
    assert_eq!(
        lines(&mut app),
        vec![crate::ui_action::UiError::s(
            "ERR_TRADE_BLOCKED_S",
            "Grubbis".to_string()
        )],
        "the refusal raises the reference's own `0x496720(0xbb, name)`"
    );
    assert_eq!(
        benilla_ui::messages::kind_of("ERR_TRADE_BLOCKED_S"),
        benilla_ui::messages::MsgKind::Chat,
    );
}

/// Leg 8 names the initiator through `GetUnitName` (`0x4bf7d6` → `0x609210`), which on a miss
/// asks the cache and answers `UNKNOWNOBJECT` at once: the refusal waits for nothing.
#[test]
fn block_trades_names_an_unknown_initiator_unknown() {
    let (mut app, rx) = request_app(true, false, true);
    request(&mut app, 0x9);
    let sent: Vec<ClientCommand> = rx.try_iter().collect();
    assert!(
        sent.iter().any(|c| matches!(c, ClientCommand::BusyTrade)),
        "{sent:?}"
    );
    assert!(
        sent.iter()
            .any(|c| matches!(c, ClientCommand::NameQuery { guid: 0x9 })),
        "the miss asks: {sent:?}"
    );
    assert_eq!(
        lines(&mut app),
        vec![crate::ui_action::UiError::args(
            "ERR_TRADE_BLOCKED_S",
            vec![crate::ui_action::FillArg::UnknownObject]
        )]
    );
}

#[test]
fn the_silent_legs_refuse_without_a_word() {
    for (cinematic, controlled) in [(true, true), (false, false)] {
        let (mut app, rx) = request_app(false, cinematic, controlled);
        request(&mut app, 0x7);
        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::BusyTrade)),
            "cinematic={cinematic} controlled={controlled}: refused as busy"
        );
        assert!(
            lines(&mut app).is_empty(),
            "cinematic={cinematic} controlled={controlled}: a silent leg says nothing"
        );
    }
}

/// The leg reads `[0xb4b3e4]` (`0x4bf79c`), written for our own character alone: a possessed
/// creature stuck under fear leaves it set, and the request is answered.
#[test]
fn a_stuck_possessed_creature_does_not_make_us_busy() {
    let (mut app, rx) = request_app(false, false, true);
    app.world_mut().insert_resource(crate::player::MoverLock {
        control_lost: true,
        ..Default::default()
    });
    request(&mut app, 0x7);
    assert!(
        matches!(rx.try_recv(), Ok(ClientCommand::BeginTrade)),
        "auto-accepted, not busy"
    );
}

#[test]
fn an_open_auction_house_refuses_the_request() {
    let (mut app, rx) = request_app(false, false, true);
    app.world_mut()
        .resource_mut::<crate::ui_auction::AuctionOpen>()
        .auctioneer = Some(0x1234);
    request(&mut app, 0x7);
    assert!(matches!(rx.try_recv(), Ok(ClientCommand::BusyTrade)));
}

/// The ignore list is asked before the initiator's object (`0x4bf750` before `0x4bf779`), so an
/// ignored initiator is refused as ignored whether or not it streams.
#[test]
fn an_ignored_initiator_is_refused_as_ignored() {
    for streamed in [true, false] {
        let (mut app, rx) = request_app(false, false, true);
        app.world_mut()
            .resource_mut::<crate::ui_social::SocialState>()
            .set_ignores_for_test(vec![0x7]);
        if !streamed {
            app.world_mut().resource_mut::<GuidIndex>().0.remove(&0x7);
        }
        request(&mut app, 0x7);
        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::IgnoreTrade)),
            "streamed={streamed}: the ignore leg does NOT answer busy"
        );
        assert!(lines(&mut app).is_empty());
    }
}

#[test]
fn a_dead_or_ghost_initiator_is_refused() {
    // The `UNIT_FIELD_HEALTH`, `UNIT_FIELD_MAXHEALTH` and `PLAYER_FLAGS` field ids, and
    // `PLAYER_FLAGS_GHOST`.
    const HEALTH: u16 = 22;
    const MAXHEALTH: u16 = 28;
    const PLAYER_FLAGS: u16 = 190;
    const GHOST: u32 = 0x10;
    for (label, fields) in [
        ("dead", vec![(HEALTH, 0u32), (MAXHEALTH, 100)]),
        (
            "ghost",
            vec![(HEALTH, 1u32), (MAXHEALTH, 100), (PLAYER_FLAGS, GHOST)],
        ),
    ] {
        let (mut app, rx) = request_app(false, false, true);
        let entity = app.world().resource::<GuidIndex>().0[&0x7];
        app.world_mut().entity_mut(entity).insert(ObjectStore(
            benilla_protocol::messages::ObjectFields::from_pairs(&fields),
        ));
        request(&mut app, 0x7);
        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::BusyTrade)),
            "{label}: refused as busy"
        );
    }
}

#[test]
fn two_legs_answer_with_no_packet_at_all() {
    let answered = |rx: &crossbeam_channel::Receiver<ClientCommand>| {
        rx.try_iter().any(|c| {
            matches!(
                c,
                ClientCommand::BusyTrade
                    | ClientCommand::IgnoreTrade
                    | ClientCommand::BeginTrade
                    | ClientCommand::CancelTrade
            )
        })
    };

    // Leg 1: we asked somebody else first, and the request arrives over it.
    let (mut app, rx) = request_app(false, false, true);
    app.world_mut()
        .resource_mut::<TradeSession>()
        .initiate(0x9, false);
    request(&mut app, 0x7);
    assert!(
        !answered(&rx),
        "an initiate of our own in flight drops the request in silence"
    );
    let trade = app.world().resource::<TradeSession>();
    assert_eq!(trade.partner, Some(0x9), "…and our own trade survives it");

    // Leg 3: nothing of that guid is streamed.
    let (mut app, rx) = request_app(false, false, true);
    app.world_mut().resource_mut::<GuidIndex>().0.remove(&0x7);
    request(&mut app, 0x7);
    assert!(
        !answered(&rx),
        "an unresolvable initiator drops the request in silence"
    );
}

fn request_app(
    block_trades: bool,
    cinematic: bool,
    controlled: bool,
) -> (App, crossbeam_channel::Receiver<ClientCommand>) {
    let (tx, rx) = crossbeam_channel::unbounded();
    let mut app = App::new();
    let mut names = NameCache::default();
    // Named, so leg 8's line carries the name.
    names.insert_player(0x7, "Grubbis".to_string(), None);
    names.insert_player(0x8, "Skarrid".to_string(), None);
    app.init_resource::<TradeSession>()
        .init_resource::<crate::ui_social::SocialState>()
        .init_resource::<crate::ui_auction::AuctionOpen>()
        .init_resource::<GuidIndex>()
        .init_resource::<crate::player::Player>()
        .init_resource::<crate::player::OwnControlLost>()
        .insert_resource(names)
        .insert_resource(NetCommands(tx))
        .insert_resource(BlockTrades(block_trades))
        .insert_resource(if cinematic {
            crate::cinematic::Cinematic::playing_for_test()
        } else {
            crate::cinematic::Cinematic::default()
        });
    app.world_mut()
        .resource_mut::<crate::player::OwnControlLost>()
        .0 = !controlled;
    // Streamed and alive, since leg 3 drops a request from a guid that resolves to nothing.
    for guid in [0x7u64, 0x8, 0x9] {
        let alive = benilla_protocol::ObjectFields::from_pairs(&[(22, 100)]);
        let e = app.world_mut().spawn(ObjectStore(alive)).id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(guid, e);
    }
    app.add_latched_window::<TradeSession>();
    net::register(&mut app);
    (app, rx)
}

/// `0x5d49ba`: an own-side status whose sequence is below the own-edit counter is dropped
/// before the store, the slot invalidations and the accept reset. The counter is 1 at the
/// open and one more per edit we send; vmangos writes 7, so the seventh edit makes its
/// statuses stale. The partner's column is never dropped, and a new open starts over.
#[test]
fn a_stale_own_side_status_is_dropped_whole() {
    let (tx, _rx) = crossbeam_channel::unbounded();
    let commands = NetCommands(tx);
    let ext = |their_window: bool, sequence: u32, gold: u32| TradeStatusExtended {
        their_window,
        sequence,
        flag: 7,
        gold,
        enchant_spell_id: 0,
        slots: [None; TRADE_SLOT_COUNT],
    };
    let mut lane = TradeLane::default();
    let mut s = TradeSession::default();
    s.begin(0x1);
    s.open_window();
    s.place_own_item(2, wire_item(1234));
    s.accept();
    s.partner_accepted(&mut lane);
    let edit = |s: &mut TradeSession| {
        s.send_edit(&commands, ClientCommand::SetTradeGold { copper: 1 });
    };
    let invalidated = |lane: &mut TradeLane| -> Vec<u8> {
        lane.take()
            .into_iter()
            .filter_map(|e| match e {
                TradeEvent::Invalidate(i) => Some(i),
                _ => None,
            })
            .collect()
    };

    // Six edits make the counter 7: a status with sequence 7 is not below it.
    for _ in 0..6 {
        edit(&mut s);
    }
    s.set_offer(&ext(false, 7, 111), &mut lane);
    assert_eq!(
        s.our.gold, 111,
        "sequence 7 against a counter of 7 is applied"
    );
    assert!(s.our.slots[1].is_none() && invalidated(&mut lane) == [1]);

    // The seventh makes it 8: the same status is now stale.
    s.place_own_item(2, wire_item(1234));
    s.accept();
    s.partner_accepted(&mut lane);
    lane.take();
    edit(&mut s);
    s.set_offer(&ext(false, 7, 222), &mut lane);
    assert_eq!(s.our.gold, 111, "the stale status left our gold alone");
    assert!(s.our.slots[1].is_some(), "and the placement in flight");
    assert!(lane.take().is_empty(), "and signalled nothing");
    assert!(s.we_accepted() && s.their_accept, "and both accepts");

    // The partner's column carries no sequence check.
    s.set_offer(&ext(true, 0, 333), &mut lane);
    assert_eq!(s.their.gold, 333);

    // A newer sequence passes, and a new open starts the count over.
    s.set_offer(&ext(false, 8, 444), &mut lane);
    assert_eq!(s.our.gold, 444);
    s.open_window();
    s.set_offer(&ext(false, 1, 555), &mut lane);
    assert_eq!(s.our.gold, 555);
}

/// An app with the trade window open and the backpack's slot 1 offered in trade slot 1, the
/// placement in the session as the host's apply leaves it, and a spy that lists the trade-close
/// and lock events in order.
fn offered_app() -> App {
    let (mut app, _rx) = feed_app();
    open_with(&mut app, 0x7);
    app.world_mut()
        .resource_mut::<TradeSession>()
        .place_own_item(1, wire_item(2589));
    let mut vm = app.world_mut().non_send_resource_mut::<UiScript>();
    let mut bag = benilla_ui::script::ContainerState::default();
    bag.slots.insert(
        1,
        benilla_ui::script::ContainerSlot {
            item_id: 2589,
            guid: LINEN,
            count: 1,
            ..Default::default()
        },
    );
    vm.set_container(0, Some(bag));
    vm.run(
        r#"PickupContainerItem(0, 1) ClickTradeButton(1)
           SEEN = {}
           local f = CreateFrame("Frame")
           for _, e in { "TRADE_REQUEST_CANCEL", "ITEM_LOCK_CHANGED", "TRADE_CLOSED",
                         "TRADE_PLAYER_ITEM_CHANGED" } do
               f:RegisterEvent(e)
           end
           f:SetScript("OnEvent", function() table.insert(SEEN, event) end)"#,
    )
    .unwrap();
    app
}

fn seen(app: &mut App) -> Vec<String> {
    app.world_mut()
        .non_send_resource::<UiScript>()
        .eval("local s = SEEN SEEN = {} return s")
        .unwrap()
}

fn bag_locked(app: &mut App) -> bool {
    app.world_mut()
        .non_send_resource::<UiScript>()
        .eval("local _, _, l = GetContainerItemInfo(0, 1) return l == 1")
        .unwrap()
}

/// `CANCELED` fires `TRADE_REQUEST_CANCEL` (`0x4bf832`), then unlocks the offered items
/// (`0x4bf837`), then runs the close core while the partner holds the latch: `TRADE_CLOSED`
/// twice (`0x4bf522`, once inside `0x493365`'s re-entry) and `CMSG_CANCEL_TRADE` after each
/// (`0x4bf527`). A local `CloseTrade` (`0x4bfdc0`) runs the same core without unlocking, which
/// waits for the server's `CANCELED`; that close finds no window and only cancels again.
#[test]
fn a_canceled_trade_unlocks_between_its_signals_and_a_local_close_does_not() {
    let (tx, rx) = crossbeam_channel::unbounded();
    let mut app = offered_app();
    app.insert_resource(NetCommands(tx.clone()));
    status(&mut app, TradeStatus::Canceled);
    assert_eq!(
        seen(&mut app),
        [
            "TRADE_REQUEST_CANCEL",
            "ITEM_LOCK_CHANGED",
            "TRADE_CLOSED",
            "TRADE_CLOSED"
        ]
    );
    assert!(!bag_locked(&mut app));
    assert_eq!(cancels(&rx), 2, "the re-entered core's and the outer one's");
    assert_eq!(
        app.world()
            .resource::<crate::ui_session::InteractLatch>()
            .guid(),
        None
    );

    let mut app = offered_app();
    app.insert_resource(NetCommands(tx));
    // `CloseTrade` (`0x4bfdc0` → `0x4bf4e0(0, 0)`), as the latch runs it outside a call.
    close_core(app.world_mut());
    drain(&mut app);
    assert_eq!(seen(&mut app), ["TRADE_CLOSED", "TRADE_CLOSED"]);
    assert_eq!(cancels(&rx), 2);
    assert!(bag_locked(&mut app), "held until the server answers");
    status(&mut app, TradeStatus::Canceled);
    assert_eq!(
        seen(&mut app),
        ["TRADE_REQUEST_CANCEL", "ITEM_LOCK_CHANGED"]
    );
    assert!(!bag_locked(&mut app));
    assert_eq!(
        cancels(&rx),
        1,
        "the core's cancel goes out with no window open"
    );
}

/// Both offers live in tables only an open clears (`0x4bf549`-`0x4bf5ef`); a close clears none,
/// and the item getters test no open state (`0x4c015a`, `0x4c04b3`), so after it the getters read
/// the last offer of both sides until the next open. The money is the exception: the statuses
/// that end a trade zero both offers before the dispatcher (`0x5d4884`, `0x5d48eb`), and our
/// getter also tests the partner cell (`0x4c0706`).
#[test]
fn both_offers_answer_after_the_close_and_until_the_next_open() {
    for closing in [
        TradeStatus::Complete,
        TradeStatus::Canceled,
        TradeStatus::CloseWindow {
            result: 0,
            side: 0,
            item_limit_category: 0,
        },
    ] {
        let mut app = offered_app();
        {
            let mut items = app.world_mut().resource_mut::<Items>();
            items.insert_template(2589, Some(crate::items::test_template("Linen Cloth")));
            items.insert_template(1234, Some(crate::items::test_template("Iron Sword")));
        }
        let mut theirs = [None; TRADE_SLOT_COUNT];
        theirs[0] = Some(wire_item(1234));
        app.world_mut()
            .resource_mut::<TradeSession>()
            .set_own_gold(300);
        extended(
            &mut app,
            TradeStatusExtended {
                their_window: true,
                sequence: 7,
                flag: 7,
                gold: 500,
                enchant_spell_id: 0,
                slots: theirs,
            },
        );
        let reads = "return tostring((GetTradePlayerItemInfo(1))) .. '/' .. \
                     tostring((GetTradeTargetItemInfo(1))) .. '/' .. GetPlayerTradeMoney() \
                     .. '/' .. GetTargetTradeMoney()";
        assert_eq!(
            eval_live::<String>(&mut app, reads),
            "Linen Cloth/Iron Sword/300/500",
            "open"
        );

        status(&mut app, closing);
        assert_eq!(
            eval_live::<String>(&mut app, reads),
            "Linen Cloth/Iron Sword/0/0",
            "after {closing:?}"
        );
        let holding = |app: &App| {
            app.world()
                .non_send_resource::<UiScript>()
                .trade_slot_holding(LINEN)
        };
        assert_eq!(holding(&app), Some((0, 0, 1)), "our item table outlives it");

        open_with(&mut app, 0x7);
        assert_eq!(
            eval_live::<String>(&mut app, reads),
            "nil/nil/0/0",
            "the open cleared both"
        );
        assert_eq!(holding(&app), None, "and the item table");
    }
}

/// A partner's slot looks its record up with the partner cell `[0xb71728]` as the owner id
/// (`0x4c016c`), which the open sets from the trade's partner (`0x4bf549`): no unit stands for it.
#[test]
fn a_partner_slot_asks_with_the_partner_cell_as_its_owner() {
    use benilla_ui::script::RecordAsk;
    let (mut app, _rx) = feed_app();
    open_with(&mut app, 0x7);
    let mut theirs = [None; TRADE_SLOT_COUNT];
    theirs[0] = Some(wire_item(4306));
    extended(
        &mut app,
        TradeStatusExtended {
            their_window: true,
            sequence: 7,
            flag: 7,
            gold: 0,
            enchant_spell_id: 0,
            slots: theirs,
        },
    );
    eval_live::<()>(&mut app, "GetTradeTargetItemInfo(1)");
    assert_eq!(
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .take_record_asks(),
        [RecordAsk::TradeItem {
            entry: 4306,
            owner: 0x7
        }]
    );
}

/// The slot rows only read their record; the getters' lookups ask, with the callback
/// `0x4c0340` (`0x4c0191`). A refused record is asked by no frame after it, only by the next
/// getter call.
#[test]
fn a_refused_slot_record_is_asked_by_the_next_getter_never_by_a_frame() {
    let (mut app, rx) = feed_app();
    open_with(&mut app, 0x7);
    let mut theirs = [None; TRADE_SLOT_COUNT];
    theirs[0] = Some(wire_item(4306));
    extended(
        &mut app,
        TradeStatusExtended {
            their_window: true,
            sequence: 7,
            flag: 7,
            gold: 0,
            enchant_spell_id: 0,
            slots: theirs,
        },
    );
    assert!(
        crate::query_cache::queries(&rx).is_empty(),
        "the row only reads"
    );
    let getter = |app: &mut App| {
        eval_live::<()>(app, "GetTradeTargetItemInfo(1)");
        ask_kept_records(app);
    };
    getter(&mut app);
    assert_eq!(crate::query_cache::queries(&rx).len(), 1);
    {
        let mut items = app.world_mut().resource_mut::<Items>();
        items.insert_template(4306, None);
        items.take_trade_waiters(4306);
    }
    for _ in 0..600 {
        app.update();
        drain(&mut app);
    }
    assert!(
        crate::query_cache::queries(&rx).is_empty(),
        "no frame asks a refusal"
    );
    getter(&mut app);
    assert_eq!(
        crate::query_cache::queries(&rx).len(),
        1,
        "the next getter asks again"
    );
}

/// The initiate looks the name up as it is raised (`0x5d4031`, callback `0x5d4080`). A refused
/// name is asked by no frame after it, only by the next initiate that names its player.
#[test]
fn a_refused_line_name_is_asked_by_the_next_line_never_by_a_frame() {
    let (mut app, rx) = feed_app();
    initiate(&mut app, 0x9);
    assert_eq!(crate::query_cache::queries(&rx).len(), 1);
    name_answer(&mut app, 0x9, "");
    for _ in 0..600 {
        app.update();
        drain(&mut app);
    }
    assert!(
        crate::query_cache::queries(&rx).is_empty(),
        "no frame asks a refusal"
    );
    initiate(&mut app, 0x9);
    assert_eq!(
        crate::query_cache::queries(&rx).len(),
        1,
        "the next line asks again"
    );
}

/// `ONLY_CONJURED` invalidates the slot (`0x4bf9fd` into `0x4bfbd0`): the item unlocks and
/// `TRADE_PLAYER_ITEM_CHANGED` fires once; so does an own-side extended status that empties a
/// filled slot (`0x5d4b33`).
#[test]
fn a_bounced_offer_unlocks_its_item_and_signals_the_slot_once() {
    let mut app = offered_app();
    seen(&mut app);
    status(&mut app, TradeStatus::OnlyConjured { slot: 0 });
    assert_eq!(
        seen(&mut app),
        ["ITEM_LOCK_CHANGED", "TRADE_PLAYER_ITEM_CHANGED"]
    );
    assert!(!bag_locked(&mut app));

    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run("ClearCursor() PickupContainerItem(0, 1) ClickTradeButton(1)")
        .unwrap();
    app.world_mut()
        .resource_mut::<TradeSession>()
        .place_own_item(1, wire_item(2589));
    seen(&mut app);
    extended(
        &mut app,
        TradeStatusExtended {
            their_window: false,
            sequence: 7,
            flag: 7,
            gold: 0,
            enchant_spell_id: 0,
            slots: [None; TRADE_SLOT_COUNT],
        },
    );
    assert_eq!(
        seen(&mut app),
        ["ITEM_LOCK_CHANGED", "TRADE_PLAYER_ITEM_CHANGED"]
    );
    assert!(!bag_locked(&mut app));
}

/// [`feed_app`] with the sweep's two trade legs.
fn sweep_trade_app() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
    use crate::ui_script::SweepApp;
    let (mut app, rx) = feed_app();
    app.add_sweep_leg(SweepLeg::Trade, cancel_trade_on_leave_world)
        .add_sweep_leg(SweepLeg::Interaction, crate::ui_session::dispatch);
    (app, rx)
}

fn cancels(rx: &crossbeam_channel::Receiver<ClientCommand>) -> usize {
    rx.try_iter()
        .filter(|c| matches!(c, ClientCommand::CancelTrade))
        .count()
}

const EVENTS: [&str; 3] = ["TRADE_SHOW", "TRADE_CLOSED", "PLAYER_LEAVING_WORLD"];

/// A `/reload` with a trade open: the trade leg cancels (`0x490adf` → `0x5d4310`), and past
/// `PLAYER_LEAVING_WORLD` the interaction close shuts the window in the dying VM and cancels
/// again (`0x493365` → `0x4bf4e0`); the new VM opens nothing.
#[test]
fn a_reload_cancels_the_trade_closes_it_and_the_new_vm_opens_nothing() {
    use crate::ui_script::sweep_test::{reload_in_place, seen, seen_in, spied_vm};
    let (mut app, rx) = sweep_trade_app();
    app.insert_non_send_resource(spied_vm(&EVENTS));
    open_with(&mut app, 0x7);
    assert_eq!(seen_in(&app), ["TRADE_SHOW"]);
    let _ = rx.try_iter().count();

    let dying = reload_in_place(&mut app, &EVENTS);
    assert_eq!(
        seen(&dying),
        ["TRADE_SHOW", "PLAYER_LEAVING_WORLD", "TRADE_CLOSED"]
    );
    assert_eq!(cancels(&rx), 2, "the leg's and the interaction close's");
    assert!(!app.world().resource::<TradeSession>().is_open());
    app.update();
    drain(&mut app);
    assert!(seen_in(&app).is_empty(), "the new VM has no trade to show");
}

/// With no trade, the leg still cancels and nothing closes; a worldport's surviving VM hears
/// `TRADE_CLOSED` once when one was open.
#[test]
fn the_trade_leg_cancels_with_no_trade_and_a_worldport_closes_once() {
    use crate::ui_script::sweep_test::{seen_in, spied_vm, sweep_in_place};
    let (mut app, rx) = sweep_trade_app();
    app.insert_non_send_resource(spied_vm(&EVENTS));
    sweep_in_place(&mut app);
    assert_eq!(
        cancels(&rx),
        1,
        "`0x5d4310` sends whether or not a trade is open"
    );
    assert_eq!(seen_in(&app), ["PLAYER_LEAVING_WORLD"]);

    open_with(&mut app, 0x7);
    sweep_in_place(&mut app);
    app.update();
    drain(&mut app);
    assert_eq!(
        seen_in(&app),
        [
            "PLAYER_LEAVING_WORLD",
            "TRADE_SHOW",
            "PLAYER_LEAVING_WORLD",
            "TRADE_CLOSED"
        ]
    );
}

/// The offered linen (backpack slot 1, our trade slot 1) as a live item object of ours, with
/// our player, the item lane's reads, and the spy's list emptied.
fn resignal_app() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
    use benilla_protocol::field::{FIELD_ITEM_STACK_COUNT, FIELD_OBJECT_ENTRY};
    use benilla_protocol::ObjectFields;
    let (tx, rx) = crossbeam_channel::unbounded();
    let mut app = offered_app();
    app.insert_resource(NetCommands(tx));
    let world = app.world_mut();
    crate::items::test_spawn_item(
        world,
        LINEN,
        ObjectFields::from_pairs(&[(FIELD_OBJECT_ENTRY, 2589), (FIELD_ITEM_STACK_COUNT, 1)]),
        false,
    );
    world.spawn(crate::net::SelfPlayer);
    crate::item_lane::seat_item_lane(world);
    seen(&mut app);
    (app, rx)
}

/// The offered linen's guid in [`offered_app`].
const LINEN: u64 = 0x4000_0000_0000_2589;

/// `notice`'s watcher run on the item lane, as its packet readies it, then a frame and the lane.
fn notify(app: &mut App, notice: crate::item_watch::ItemNotice) {
    queue(app, notice);
    app.update();
    crate::item_lane::fire_item_lane(app.world_mut());
}

/// `notice`'s watcher run on the item lane, with no signals.
fn queue(app: &mut App, notice: crate::item_watch::ItemNotice) {
    app.world_mut()
        .resource_mut::<crate::item_lane::ItemLane>()
        .push(crate::item_watch::Run {
            notice: Some(notice),
            signals: Vec::new(),
            ..Default::default()
        });
}

/// `0x4bfb10` from the destroy: the slot holding the item clears on the wire and in the VM,
/// and signals once; nothing unlocks, the lock having gone with the item.
#[test]
fn a_destroyed_offer_clears_its_slot_and_tells_the_server() {
    let (mut app, rx) = resignal_app();
    notify(
        &mut app,
        crate::item_watch::ItemNotice::Destroyed {
            guid: LINEN,
            entry: 2589,
        },
    );
    let sent: Vec<ClientCommand> = rx.try_iter().collect();
    assert!(
        sent.iter()
            .any(|c| matches!(c, ClientCommand::ClearTradeItem { trade_slot: 0 })),
        "CMSG_CLEAR_TRADE_ITEM for slot 1: {sent:?}"
    );
    assert_eq!(seen(&mut app), ["TRADE_PLAYER_ITEM_CHANGED"]);
    assert_eq!(
        app.world().resource::<TradeSession>().own_edits,
        2,
        "the clear is one of our edits (`0x5d44bc`)"
    );
    let name: Option<String> = eval_live(&mut app, "return (GetTradePlayerItemInfo(1))");
    assert_eq!(name, None, "the slot is empty");
    // Another item's destroy, or the same one again, touches no slot.
    notify(
        &mut app,
        crate::item_watch::ItemNotice::Destroyed {
            guid: LINEN,
            entry: 2589,
        },
    );
    assert!(rx.try_iter().next().is_none());
    assert!(seen(&mut app).is_empty());
    assert_eq!(app.world().resource::<TradeSession>().own_edits, 2);
}

/// vmangos sends the traded items' destroys before `TRADE_COMPLETE`, and the destroy releases
/// the item at its packet (`0x4674cf` → `0x464920`), so the slot clears and signals while the
/// window is still open: by `TRADE_CLOSED` the slot is empty, once, and the later scheduled
/// scan finds nothing left to read.
#[test]
fn a_traded_away_item_clears_its_slot_before_trade_closed() {
    let (mut app, rx) = resignal_app();
    // The packets of one drain: the destroy's notice, then the status, with no frame between.
    queue(
        &mut app,
        crate::item_watch::ItemNotice::Destroyed {
            guid: LINEN,
            entry: 2589,
        },
    );
    status(&mut app, TradeStatus::Complete);
    assert_eq!(
        seen(&mut app),
        ["TRADE_PLAYER_ITEM_CHANGED", "TRADE_CLOSED", "TRADE_CLOSED"],
        "the clear signals ahead of the close"
    );
    let read: Option<String> = eval_live(&mut app, "return (GetTradePlayerItemInfo(1))");
    assert_eq!(read, None, "and the slot the close's getters read is empty");
    app.update();
    crate::item_lane::fire_item_lane(app.world_mut());
    let sent: Vec<ClientCommand> = rx.try_iter().collect();
    assert_eq!(
        sent.iter()
            .filter(|c| matches!(c, ClientCommand::ClearTradeItem { .. }))
            .count(),
        1,
        "scanned once: {sent:?}"
    );
    assert!(seen(&mut app).is_empty());
}

/// The placed item is a live object: its WRAPPED flag and `ITEM_FIELD_GIFTCREATOR` reach the
/// slot when the watchers place it again (`0x52b789`, `0x52e32a`), the giver by name.
#[test]
fn a_wrapped_offer_placed_again_carries_its_giver() {
    use benilla_protocol::field::FIELD_ITEM_STACK_COUNT;
    use benilla_protocol::ObjectFields;
    let (mut app, _rx) = resignal_app();
    app.world_mut()
        .resource_mut::<NameCache>()
        .insert_player(0x77, "Santa".to_string(), None);
    let linen = app.world().resource::<GuidIndex>().0[&LINEN];
    // ITEM_FIELD_GIFTCREATOR is the guid at fields 12 and 13, ITEM_FIELD_FLAGS field 21.
    app.world_mut()
        .get_mut::<ObjectStore>(linen)
        .unwrap()
        .0
        .merge(ObjectFields::from_pairs(&[
            (FIELD_ITEM_STACK_COUNT, 2),
            (12, 0x77),
            (13, 0),
            (21, 0x8),
        ]));
    notify(
        &mut app,
        crate::item_watch::ItemNotice::StackCount {
            guid: LINEN,
            entry: 2589,
        },
    );
    let world = app.world();
    let state = rows(
        world.resource::<TradeSession>(),
        world.resource::<Items>(),
        world.resource::<NameCache>(),
    );
    let row = state.player.slots[0].as_ref().expect("the slot is filled");
    assert_eq!(
        (row.wrapped, row.gift_creator.as_deref()),
        (true, Some("Santa"))
    );
}

/// `0x4bfb60` from the stack and enchantment watchers: the same item placed again from its
/// position, the slot showing the live count, and one signal; a placement of an item that
/// is no longer an object sends and signals nothing (`0x5d43da`).
#[test]
fn a_restacked_offer_is_placed_again_with_its_live_count() {
    use benilla_protocol::field::FIELD_ITEM_STACK_COUNT;
    use benilla_protocol::ObjectFields;
    let (mut app, rx) = resignal_app();
    let linen = app.world().resource::<GuidIndex>().0[&LINEN];
    app.world_mut()
        .get_mut::<ObjectStore>(linen)
        .unwrap()
        .0
        .merge(ObjectFields::from_pairs(&[(FIELD_ITEM_STACK_COUNT, 7)]));
    notify(
        &mut app,
        crate::item_watch::ItemNotice::StackCount {
            guid: LINEN,
            entry: 2589,
        },
    );
    let sent: Vec<ClientCommand> = rx.try_iter().collect();
    assert!(
        sent.iter().any(|c| matches!(
            c,
            ClientCommand::SetTradeItem {
                trade_slot: 0,
                bag: 255,
                slot: 23
            }
        )),
        "CMSG_SET_TRADE_ITEM from backpack slot 1: {sent:?}"
    );
    assert_eq!(seen(&mut app), ["TRADE_PLAYER_ITEM_CHANGED"]);
    let count: i64 = eval_live(
        &mut app,
        "local _, _, n = GetTradePlayerItemInfo(1) return n",
    );
    assert_eq!(count, 7, "the slot reads the live stack");
    assert_eq!(
        app.world().resource::<TradeSession>().own_edits,
        2,
        "the placement is one of our edits (`0x5d4442`)"
    );
    notify(
        &mut app,
        crate::item_watch::ItemNotice::Enchantment {
            guid: LINEN,
            entry: 2589,
        },
    );
    assert_eq!(rx.try_iter().count(), 1, "the enchantment places it again");
    assert_eq!(seen(&mut app), ["TRADE_PLAYER_ITEM_CHANGED"]);
    assert_eq!(app.world().resource::<TradeSession>().own_edits, 3);
    app.world_mut().resource_mut::<GuidIndex>().0.remove(&LINEN);
    notify(
        &mut app,
        crate::item_watch::ItemNotice::StackCount {
            guid: LINEN,
            entry: 2589,
        },
    );
    assert!(rx.try_iter().next().is_none(), "no item, no send");
    assert!(seen(&mut app).is_empty());
    assert_eq!(app.world().resource::<TradeSession>().own_edits, 3);
}

/// Each item handler recounts the action bar before it scans the trade (`0x5d8798` before
/// `0x5d87a7`, `0x5d94e1` before `0x5d94f0`, `0x5d9afd` before `0x5d9b0c`), in its run on the item
/// lane.
#[test]
fn the_trade_re_signal_follows_the_bar_recount_in_the_watchers_run() {
    let (mut app, _rx) = resignal_app();
    let world = app.world_mut();
    let mut vm = world.non_send_resource_mut::<UiScript>();
    vm.set_action(
        4,
        Some(benilla_ui::script::ActionSlot {
            kind: benilla_protocol::messages::ACTION_KIND_ITEM,
            action: 2589,
            ..Default::default()
        }),
    );
    vm.run(
        "f = CreateFrame('Frame') f:RegisterEvent('ACTIONBAR_SLOT_CHANGED') \
            f:SetScript('OnEvent', function() table.insert(SEEN, event) end)",
    )
    .unwrap();
    notify(
        &mut app,
        crate::item_watch::ItemNotice::Destroyed {
            guid: LINEN,
            entry: 2589,
        },
    );
    assert_eq!(
        seen(&mut app),
        ["ACTIONBAR_SLOT_CHANGED", "TRADE_PLAYER_ITEM_CHANGED"]
    );
}

/// The destroy's handler (`0x5d8660`) runs whole in its order: the bar recount (`0x5d8798`), the
/// trade scan (`0x5d87a7`), the quest log's ask for the item (`0x5d87b2`), then the slot's signal
/// (`0x5d87c1`).
#[test]
fn the_destroy_run_recounts_scans_asks_then_signals() {
    use benilla_ui::script::{QuestObjectiveView, QuestRecordView, QuestSlotView, QUEST_LOG_SLOTS};
    let (mut app, _rx) = resignal_app();
    let world = app.world_mut();
    let mut vm = world.non_send_resource_mut::<UiScript>();
    vm.set_action(
        4,
        Some(benilla_ui::script::ActionSlot {
            kind: benilla_protocol::messages::ACTION_KIND_ITEM,
            action: 2589,
            ..Default::default()
        }),
    );
    let mut record = QuestRecordView {
        title: "Cloth".into(),
        ..Default::default()
    };
    record.objectives[0] = QuestObjectiveView {
        item_id: 2589,
        item_count: 4,
        ..Default::default()
    };
    vm.seed_quest_record(77, record);
    vm.set_quest_log_player(true);
    let mut slots = [QuestSlotView::default(); QUEST_LOG_SLOTS];
    slots[0].quest_id = 77;
    vm.set_quest_slots(slots);
    vm.quest_log_changed();
    vm.run(
        "f = CreateFrame('Frame') \
         for _, e in ipairs({'ACTIONBAR_SLOT_CHANGED', 'QUEST_LOG_UPDATE', \
             'UNIT_INVENTORY_CHANGED'}) do f:RegisterEvent(e) end \
         f:SetScript('OnEvent', function() table.insert(SEEN, event) end)",
    )
    .unwrap();
    seen(&mut app);
    app.world_mut()
        .resource_mut::<crate::item_lane::ItemLane>()
        .push(crate::item_watch::Run {
            notice: Some(crate::item_watch::ItemNotice::Destroyed {
                guid: LINEN,
                entry: 2589,
            }),
            signals: vec![crate::field_watch::FieldNotice::Player {
                id: crate::field_watch::id::UNIT_INVENTORY_CHANGED,
            }],
            ..Default::default()
        });
    app.update();
    crate::item_lane::fire_item_lane(app.world_mut());
    assert_eq!(
        seen(&mut app),
        [
            "ACTIONBAR_SLOT_CHANGED",
            "TRADE_PLAYER_ITEM_CHANGED",
            "QUEST_LOG_UPDATE",
            "UNIT_INVENTORY_CHANGED"
        ]
    );
}

/// Slot 7's spell per side (`0x4bf2b0`): the partner's window carries the spell cast on our
/// slot 7, which repaints ours and names its enchantment; our window carries ours on theirs.
/// A resend of the same spell signals nothing, and the record compare ignores the name.
#[test]
fn the_enchant_slots_spell_signals_its_side_and_names_the_enchantment() {
    const FIERY: u32 = 13898;
    let (mut app, _rx) = feed_app();
    app.insert_resource(crate::ui_action::Spells {
        catalog: benilla_formats::SpellCatalog::from_displays(std::collections::HashMap::from([(
            FIERY,
            benilla_formats::SpellDisplay {
                name: "Enchant Weapon - Fiery Weapon".into(),
                ..Default::default()
            },
        )])),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    app.world_mut()
        .resource_mut::<Items>()
        .insert_template(2589, Some(crate::items::test_template("Linen Cloth")));
    open_with(&mut app, 0x7);
    let mut slots = [None; TRADE_SLOT_COUNT];
    slots[ENCHANT_SLOT] = Some(wire_item(2589));
    let ext = |their_window: bool, spell: u32| TradeStatusExtended {
        their_window,
        sequence: 7,
        flag: 7,
        gold: 0,
        enchant_spell_id: spell,
        slots,
    };
    // Both columns hold an item in slot 7, as their windows say.
    extended(&mut app, ext(true, 0));
    extended(&mut app, ext(false, 0));
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(
            r#"SEEN = {}
               local f = CreateFrame("Frame")
               f:RegisterEvent("TRADE_PLAYER_ITEM_CHANGED")
               f:RegisterEvent("TRADE_TARGET_ITEM_CHANGED")
               f:SetScript("OnEvent", function() table.insert(SEEN, event .. " " .. arg1) end)"#,
        )
        .unwrap();
    let enchantments = |app: &mut App| -> (Option<String>, Option<String>) {
        eval_live(
            app,
            "local _, _, _, _, p = GetTradePlayerItemInfo(7) \
             local _, _, _, _, _, t = GetTradeTargetItemInfo(7) return p, t",
        )
    };
    extended(&mut app, ext(true, FIERY));
    assert_eq!(seen(&mut app), ["TRADE_PLAYER_ITEM_CHANGED 7"]);
    assert_eq!(
        enchantments(&mut app),
        (Some("Enchant Weapon - Fiery Weapon".into()), None)
    );
    extended(&mut app, ext(true, FIERY));
    assert!(seen(&mut app).is_empty(), "the same spell again");
    extended(&mut app, ext(false, FIERY));
    assert_eq!(seen(&mut app), ["TRADE_TARGET_ITEM_CHANGED 7"]);
    assert_eq!(
        enchantments(&mut app),
        (
            Some("Enchant Weapon - Fiery Weapon".into()),
            Some("Enchant Weapon - Fiery Weapon".into())
        )
    );
    // Cleared on our side: the partner's slot 7 repaints and names nothing.
    extended(&mut app, ext(false, 0));
    assert_eq!(seen(&mut app), ["TRADE_TARGET_ITEM_CHANGED 7"]);
    assert_eq!(enchantments(&mut app).1, None);
}
