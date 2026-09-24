//! [`super::feed::feed_actions`] as a real Bevy system through a real Lua VM — the seam where the
//! action bar's slot identity is resolved and pushed.
//!
//! What these pin is the **landed-template redisplay** (decision 0660): an ITEM slot's icon needs
//! an item template that arrives asynchronously, so the resolve that first touches a cold entry is
//! the one that ISSUES the ask-once query and necessarily reads back nothing. The regression these
//! guard is a question mark that never goes away — the fresh character's food button.
//!
//! `run_system_once` is deliberately NOT used: the feed's memory is a `Local`, which that helper
//! rebuilds per call, so a two-frame test has to run a registered system across two `app.update()`s.

use std::collections::HashMap;

use benilla_formats::{ItemDisplay, ItemDisplayCatalog};
use benilla_protocol::messages::{ActionButton, ItemSpellEntry, ACTION_KIND_ITEM};
use benilla_ui::script::UiScript;
use bevy::prelude::*;

use super::feed::{feed_actions, MISSING_ITEM_ICON};
use super::{CastErrors, MountErrors, PetTameFailures, PlayerActions, UiErrorKeys, UiErrorTexts};
use crate::entities::ItemDisplays;
use crate::items::{test_template, Items};
use crate::net::{ClientCommand, NetCommands};

/// The fresh human warrior's default bar (vmangos `playercreateinfo_action`): Tough Jerky, item
/// **117**, on wire slot **83** (the Battle Stance bonus page) — Lua action id 84.
const JERKY: u32 = 117;
const JERKY_SLOT: u8 = 83;
const JERKY_ACTION: u32 = JERKY_SLOT as u32 + 1;
/// Tough Jerky's real display id and the icon that display carries (`ItemDisplayInfo.dbc`).
const JERKY_DISPLAY: u32 = 2473;
const JERKY_ICON: &str = "Interface\\Icons\\INV_Misc_Food_16";

/// An app with the feed registered and the fresh character's food button on the bar — but a cold
/// template cache, exactly as at login.
fn app_with_food_on_the_bar() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
    let (tx, rx) = crossbeam_channel::unbounded();
    let mut app = App::new();
    let mut actions = PlayerActions::default();
    actions.buttons.insert(
        JERKY_SLOT,
        ActionButton {
            slot: JERKY_SLOT,
            action: JERKY,
            kind: ACTION_KIND_ITEM,
        },
    );
    // SMSG_ACTION_BUTTONS' own arm sets this; the feed's first pass is what clears it.
    actions.dirty = true;

    let displays = HashMap::from([(
        JERKY_DISPLAY,
        ItemDisplay {
            icon: Some(JERKY_ICON.to_string()),
            ..Default::default()
        },
    )]);

    app.insert_resource(actions)
        .init_resource::<Items>()
        .init_resource::<crate::net::GuidIndex>()
        .init_resource::<CastErrors>()
        .init_resource::<MountErrors>()
        .init_resource::<PetTameFailures>()
        .init_resource::<UiErrorKeys>()
        .init_resource::<UiErrorTexts>()
        // The cast-failure combat-log line (1703) rides the same drain.
        .init_resource::<crate::ui_chat::ChatLog>()
        .init_resource::<crate::sound::MessageSounds>()
        .insert_resource(ItemDisplays::icons_for_tests(
            ItemDisplayCatalog::from_displays(displays),
        ))
        .insert_resource(NetCommands(tx));
    app.insert_non_send_resource(UiScript::new().unwrap());
    app.add_systems(Update, feed_actions);
    (app, rx)
}

/// What the VM believes the food button's icon is — the exact read `ActionBar.xml` does before it
/// falls back to `BENILLA_FALLBACK_ICON` (the question mark).
fn fed_texture(app: &mut App) -> Option<String> {
    app.world_mut()
        .non_send_resource::<UiScript>()
        .eval::<Option<String>>(&format!("return GetActionTexture({JERKY_ACTION})"))
        .unwrap()
}

/// What the VM answers `ActionBar.xml`'s Count gate — the ref's 1/nil, so `None` is "no".
fn fed_consumable(app: &mut App) -> Option<i64> {
    app.world_mut()
        .non_send_resource::<UiScript>()
        .eval::<Option<i64>>(&format!("return IsConsumableAction({JERKY_ACTION})"))
        .unwrap()
}

/// The whole bug in one test: the first resolve issues the query and can only show the
/// placeholder; the frame the answer lands, the slot re-resolves and the real icon arrives.
/// Before decision 0660 the second half never happened — the button kept the question mark for the
/// whole session, until some unrelated bar edit re-dirtied the feed.
#[test]
fn a_landed_item_template_redisplays_the_action_slot() {
    let (mut app, rx) = app_with_food_on_the_bar();

    app.update();
    assert_eq!(
        fed_texture(&mut app).as_deref(),
        Some(MISSING_ITEM_ICON),
        "the first resolve of a cold entry IS the ask, so it can only show the reference's own \
         placeholder — and never nil, since ref FrameXML HIDES the icon on a nil texture (0666)"
    );
    assert!(
        rx.try_iter()
            .any(|c| matches!(c, ClientCommand::ItemQuery { entry, .. } if entry == JERKY)),
        "…and it must have asked the server for the template"
    );

    // Nothing else changes: no bar edit, no new spell, no local pickup — only the answer arriving.
    let mut info = test_template("Tough Jerky");
    info.display_info_id = JERKY_DISPLAY;
    app.world_mut()
        .resource_mut::<Items>()
        .insert_template(JERKY, Some(info));

    app.update();
    assert_eq!(
        fed_texture(&mut app).as_deref(),
        Some(JERKY_ICON),
        "the landed template redisplays the slot — the fresh character's food loses its question mark"
    );
}

/// The Count fontstring's **gate** rides the same landed template as the icon (decision 1301).
///
/// `IsConsumableAction 0x4e5250` reads nothing but the slot's item template, so it moves exactly
/// when the icon does. Fed from the per-frame *state* map it could not: that feed runs `.after`
/// this one and fires no event of its own for the flag, so the `ACTIONBAR_SLOT_CHANGED` that
/// repaints the button always carried the previous frame's answer — and at login the previous
/// frame had no template at all. The button kept its icon and lost its stack number for the whole
/// session (the director's report, 2026-08-14). This is 0660's race again, one field over, which
/// is why the guard belongs beside it: the two are one push or they are two bugs.
#[test]
fn a_landed_item_template_also_lands_the_consumable_gate() {
    let (mut app, _rx) = app_with_food_on_the_bar();

    app.update();
    assert_eq!(
        fed_consumable(&mut app),
        None,
        "a cold template cannot answer the gate — the ask is still in flight"
    );

    // Tough Jerky's real row (vmangos `item_template` 117, read 2026-08-14): one ON_USE block,
    // spell 433 Food, SpellCharges **-1** — the destroy-on-use sign `is_consumable` tests.
    let mut info = test_template("Tough Jerky");
    info.display_info_id = JERKY_DISPLAY;
    info.spells = vec![ItemSpellEntry {
        index: 0,
        spell_id: 433,
        trigger: 0,
        charges: -1,
        cooldown_ms: -1,
        category: 0,
        category_cooldown_ms: -1,
    }];
    app.world_mut()
        .resource_mut::<Items>()
        .insert_template(JERKY, Some(info));

    app.update();
    assert_eq!(
        fed_texture(&mut app).as_deref(),
        Some(JERKY_ICON),
        "the icon lands (the control — this half never broke)"
    );
    assert_eq!(
        fed_consumable(&mut app),
        Some(1),
        "…and the gate lands with it, on the same push, so the repaint that follows paints a count"
    );
}

/// `IsConsumableAction 0x4e5250` — the gate's own law
/// ([`benilla_protocol::ItemInfo::is_consumable`], fed into [`benilla_ui::script::ActionSlot`]).
/// The director's B201 is the mount row: an on-use item with no charges wore a stack number under
/// it because we tested `Class == 0` instead of the reference's two clauses (decision 0926 §3).
#[test]
fn is_consumable_is_ammo_thrown_or_a_negative_charge_use_block() {
    let block = |trigger: u32, charges: i32| ItemSpellEntry {
        index: 0,
        spell_id: 439,
        trigger,
        charges,
        cooldown_ms: -1,
        category: 0,
        category_cooldown_ms: -1,
    };

    // The report: a mount. Class 15 Miscellaneous, InventoryType 0, one ON_USE block whose
    // SpellCharges is 0 — the item is not destroyed by using it.
    let mut mount = test_template("Red Skeletal Horse");
    mount.class = 15;
    mount.spells = vec![block(0, 0)];
    assert!(!mount.is_consumable(), "a mount has no stack to show");

    // A potion: Class 0, but that is not what decides it — the ON_USE block's -1 charges is.
    let mut potion = test_template("Minor Healing Potion");
    potion.spells = vec![block(0, -1)];
    assert!(potion.is_consumable());

    // …and Class 0 alone (a conjured-water-shaped template with no on-use block at all) is
    // NOT enough, which is exactly what the old `class == 0` read got wrong in reverse.
    let classless = test_template("Trade Good");
    assert!(!classless.is_consumable());

    // The InventoryType clause, both members — ammo and thrown always count, charges or not.
    for inv in [24u32, 25] {
        let mut ammo = test_template("Rough Arrow");
        ammo.inventory_type = inv;
        assert!(ammo.is_consumable(), "InventoryType {inv} is consumable");
    }
    let mut trinket = test_template("Trinket");
    trinket.inventory_type = 12;
    assert!(!trinket.is_consumable());

    // An ON_EQUIP proc with negative charges is not an ON_USE block: the trigger must be 0.
    let mut proc_item = test_template("Proc Weapon");
    proc_item.spells = vec![block(1, -1)];
    assert!(!proc_item.is_consumable());
}

/// The epoch is a *change* gate, not a per-frame re-resolve: once the answer has landed and been
/// fed, an idle frame re-resolves nothing. (Guards the obvious over-correction — turning the feed
/// into an every-frame rebuild of all 120 slots.)
#[test]
fn a_quiet_frame_after_the_answer_re_resolves_nothing() {
    let (mut app, _rx) = app_with_food_on_the_bar();
    app.update();
    let mut info = test_template("Tough Jerky");
    info.display_info_id = JERKY_DISPLAY;
    app.world_mut()
        .resource_mut::<Items>()
        .insert_template(JERKY, Some(info));
    app.update();

    let before = app.world().resource::<Items>().template_epoch();
    app.update();
    assert_eq!(
        app.world().resource::<Items>().template_epoch(),
        before,
        "an idle frame lands no template, so the gate stays shut"
    );
    assert_eq!(
        fed_texture(&mut app).as_deref(),
        Some(JERKY_ICON),
        "…and the fed icon is unchanged"
    );
}

/// A NEGATIVE answer (the server does not know the entry) also advances the epoch — it is a real
/// transition for anything that waits on the ask — but it resolves to no icon, so the slot keeps
/// the fallback. Pins that the gate cannot spin: one re-resolve, then quiet.
#[test]
fn an_unknown_entry_answers_once_and_settles() {
    let (mut app, _rx) = app_with_food_on_the_bar();
    app.update();
    app.world_mut()
        .resource_mut::<Items>()
        .insert_template(JERKY, None);
    app.update();

    assert_eq!(
        fed_texture(&mut app).as_deref(),
        Some(MISSING_ITEM_ICON),
        "an unknown entry has no display, which is the resolver's OTHER route to the placeholder"
    );
    assert!(
        app.world()
            .resource::<Items>()
            .template_answered_unknown(JERKY),
        "the negative is cached — the feed must not re-ask on the next resolve"
    );
}

/// A MACRO slot serves **the macro's own icon**, and follows an EDIT of that macro without any
/// bar-table change at all (decision 0983).
///
/// Two things are pinned. The icon rule: `GetActionTexture`'s macro arm (`0x4e6bf9`) builds the
/// macro record's own icon path and never touches the bound spell. And the *trigger* is the
/// macro-table generation — the third
/// input beside `dirty` and the item-template epoch — because renaming or re-iconing a macro moves
/// neither of those, and gating on them alone leaves a stale icon on the bar until some unrelated
/// edit happens to re-dirty the feed (exactly decision 0660's bug, one seam over).
#[test]
fn a_macro_slot_shows_the_macros_own_icon_and_follows_an_edit() {
    use benilla_protocol::messages::ACTION_KIND_MACRO;
    use benilla_ui::script::{MacroState, MacroView};

    const SLOT: u8 = 0;
    const ACTION: u32 = 1;
    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut app = App::new();
    let mut actions = PlayerActions::default();
    actions.buttons.insert(
        SLOT,
        ActionButton {
            slot: SLOT,
            action: 1, // macro index 1
            kind: ACTION_KIND_MACRO,
        },
    );
    actions.dirty = true;
    app.insert_resource(actions)
        .init_resource::<Items>()
        .init_resource::<crate::net::GuidIndex>()
        .init_resource::<CastErrors>()
        .init_resource::<MountErrors>()
        .init_resource::<PetTameFailures>()
        .init_resource::<UiErrorKeys>()
        .init_resource::<UiErrorTexts>()
        // The cast-failure combat-log line (1703) rides the same drain.
        .init_resource::<crate::ui_chat::ChatLog>()
        .init_resource::<crate::sound::MessageSounds>()
        .insert_resource(NetCommands(tx));
    let mut script = UiScript::new().unwrap();
    script.set_macros(MacroState {
        account: vec![MacroView {
            name: "Ambush".into(),
            texture: Some("Interface\\Icons\\Ability_Ambush".into()),
            // The bound spell is deliberately NOT what the icon shows.
            body: "/cast Ambush".into(),
            local_only: false,
        }],
        character: Vec::new(),
    });
    app.insert_non_send_resource(script);
    app.add_systems(Update, feed_actions);

    app.update();
    let icon = |app: &mut App| {
        app.world_mut()
            .non_send_resource::<UiScript>()
            .eval::<Option<String>>(&format!("return GetActionTexture({ACTION})"))
            .unwrap()
    };
    assert_eq!(
        icon(&mut app).as_deref(),
        Some("Interface\\Icons\\Ability_Ambush"),
        "the MACRO's own icon"
    );

    // Re-icon the macro. Nothing touches `PlayerActions` — only the macro table moves.
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(r#"EditMacro(1, nil, "Interface\\Icons\\Spell_Fire_FlameBolt")"#)
        .unwrap();
    assert!(
        !app.world().resource::<PlayerActions>().dirty,
        "the precondition: the bar table is UNtouched by a macro edit"
    );

    app.update();
    assert_eq!(
        icon(&mut app).as_deref(),
        Some("Interface\\Icons\\Spell_Fire_FlameBolt"),
        "the generation gate re-resolved the slot"
    );

    // RENAME the macro (decision 1636). The slot's value — texture, kind, id — is byte-identical
    // after this, so a value diff alone would fire nothing and the bar would keep drawing the old
    // name line until some unrelated edit repainted it. The feed must re-fire the slot anyway.
    let events = |app: &mut App| {
        app.world_mut()
            .non_send_resource::<UiScript>()
            .eval::<i64>("return BENILLA_TEST_SLOT_EVENTS or 0")
            .unwrap()
    };
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(
            r#"
            local f = CreateFrame("Frame")
            f:RegisterEvent("ACTIONBAR_SLOT_CHANGED")
            f:SetScript("OnEvent", function()
                if arg1 == 1 then BENILLA_TEST_SLOT_EVENTS = (BENILLA_TEST_SLOT_EVENTS or 0) + 1 end
            end)
            EditMacro(1, "Shadowstep", nil)
            "#,
        )
        .unwrap();
    app.update();
    assert_eq!(
        events(&mut app),
        1,
        "a rename re-fires ACTIONBAR_SLOT_CHANGED for the slot"
    );
    assert_eq!(
        app.world_mut()
            .non_send_resource::<UiScript>()
            .eval::<Option<String>>(&format!("return GetActionText({ACTION})"))
            .unwrap()
            .as_deref(),
        Some("Shadowstep"),
        "and the repaint reads the new name"
    );
    // A frame with nothing moved fires nothing more — the re-fire is gated on the table moving.
    app.update();
    assert_eq!(events(&mut app), 1);
}

/// **The pre-resolved lines reach the frame, on the arm they asked for** — `UiErrorTexts` end to
/// end: the queue the net drain writes, through this feed, into the shipped `UIErrorsFrame`'s own
/// drawn quads.
///
/// This is the seam the GM-mode double line lived in. `SMSG_NOTIFICATION` used to be pushed into
/// the chat feed (a stand-in from before benilla had an errors frame), so vmangos answering
/// `.gm on` with both a `SendSysMessage` and a `SendNotification` printed the words twice. The
/// notice belongs here, red — and its `SMSG_AREA_TRIGGER_MESSAGE` sibling here, yellow.
#[test]
fn pre_resolved_lines_land_on_the_errors_frame_in_the_arms_colour() {
    benilla_formats::wow_data_or_skip!();
    let (mut app, _rx) = app_with_food_on_the_bar();
    {
        let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
        script.set_screen_size(1024.0, 768.0);
        // The errors frame is the reference's own file since 1751 window 14, so this reads both
        // stores through the one loader that speaks them.
        for file in [
            "Interface\\FrameXML\\Fonts.xml",
            "Interface\\FrameXML\\UIErrorsFrame.xml",
        ] {
            crate::ui_script::load_ui_for_test(&script, file);
        }
    }

    // What the net drain queues for one `.gm on` toggle, plus a refused portal.
    let mut texts = app.world_mut().resource_mut::<UiErrorTexts>();
    texts.error("GM mode is ON".to_string());
    texts.info("You must be at least level 58 to enter.".to_string());
    app.update();

    assert!(
        app.world().resource::<UiErrorTexts>().0.is_empty(),
        "the feed drains the queue"
    );

    let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
    assert!(
        script.errors().is_empty(),
        "VM errors: {:?}",
        script.errors()
    );
    script.resolve();
    let mut drawn: Vec<(String, [f32; 4])> = script
        .extract()
        .iter()
        .filter_map(|q| match &q.content {
            benilla_ui::script::QuadContent::Text {
                text: Some(t),
                color: Some(c),
                ..
            } if !t.is_empty() => Some((t.clone(), *c)),
            _ => None,
        })
        .collect();
    drawn.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        drawn,
        [
            // `AddMessage` byte-quantizes every channel (`ftol(v*255 + 0.5)`), so 0.1 draws as
            // 26/255 — the same arithmetic `ui_script::errors_tests` pins.
            (
                "GM mode is ON".to_string(),
                [1.0, 26.0 / 255.0, 26.0 / 255.0, 1.0]
            ),
            (
                "You must be at least level 58 to enter.".to_string(),
                [1.0, 1.0, 0.0, 1.0],
            ),
        ],
        "red UI_ERROR_MESSAGE for the notice, yellow UI_INFO_MESSAGE for the area trigger"
    );
}

/// **A pet's refused cast is not written to the combat log** (decision 2033).
///
/// `HandleCastFailed 0x6e1a00` calls the log formatter `0x62c360` beside its `DisplayError`;
/// `HandlePetCastFailed 0x6e8eb0` calls neither it nor the error sound — its whole call set is the
/// two packet readers, `0x496720`, and the string plumbing behind it. Until this guard the drain
/// treated both queue entries alike, so a pet's refused Growl printed "You fail to cast Growl:
/// ..." in the log, worded as the player's own failure.
///
/// Driven through the real system with the two entries **side by side and the same spell**, so the
/// assert cannot pass by the line failing to build for some unrelated reason: one goes in, one
/// comes out.
#[test]
fn a_pets_refused_cast_writes_no_combat_log_line() {
    use crate::ui_action::{CastFail, Caster, Spells};
    use benilla_formats::{SpellCatalog, SpellDisplay};

    const GROWL: u32 = 2649;

    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut app = App::new();
    let mut errors = CastErrors::default();
    // 0x5f ROOTED: an override on BOTH tables, so neither entry can be dropped for want of a
    // string, and the red line still gets two lines — it is only the log that differs.
    errors.0.push(CastFail {
        spell_id: GROWL,
        reason: 0x5F,
        arg: None,
        caster: Caster::Player,
        redisplay: false,
    });
    errors.push_pet(GROWL, 0x5F);

    app.insert_resource(PlayerActions::default())
        .insert_resource(errors)
        .insert_resource(Spells {
            catalog: SpellCatalog::from_displays(HashMap::from([(
                GROWL,
                SpellDisplay {
                    name: "Growl".into(),
                    ..Default::default()
                },
            )])),
            ..Spells::empty_for_tests()
        })
        .init_resource::<Items>()
        .init_resource::<crate::net::GuidIndex>()
        .init_resource::<MountErrors>()
        .init_resource::<PetTameFailures>()
        .init_resource::<UiErrorKeys>()
        .init_resource::<UiErrorTexts>()
        .init_resource::<crate::ui_chat::ChatLog>()
        .init_resource::<crate::sound::MessageSounds>()
        .insert_resource(NetCommands(tx));
    let script = UiScript::new().unwrap();
    // Only the two strings the log line needs; the red line's own text is `cast_fail`'s business
    // and has its own tests.
    script
        .run(r#"SPELL_FAILED_ROOTED = "You are unable to move";"#)
        .unwrap();
    app.insert_non_send_resource(script);
    app.add_systems(Update, feed_actions);
    app.update();

    assert_eq!(
        app.world()
            .resource::<crate::ui_chat::ChatLog>()
            .pending_len(),
        1,
        "the player's failure logs and the pet's does not"
    );
}

/// **The taming refusal is TWO GlobalStrings lookups, in order** (decision 2039) — the one
/// message benilla shows whose argText is itself a key.
///
/// The reference's `0x6e6a20` resolves `PETTAME_<reason>` through the script VM first and hands
/// the resulting *string* to `DisplayError(0xee)`, so `ERR_TAME_FAILED` ("%s.") renders the
/// reason's sentence with a period after it. Getting the order wrong is not a compile error and
/// not a silent no-op — it would print the literal key on the red line — so it is pinned here at
/// both ends of the reason table's bounds check.
#[test]
fn a_tame_failure_composes_the_reason_string_into_err_tame_failed() {
    let (mut app, _rx) = app_with_food_on_the_bar();
    {
        let script = app.world_mut().non_send_resource_mut::<UiScript>();
        // The two strings verbatim from 1.12's `GlobalStrings.lua`, plus a recorder for the
        // event the sink fires — this test is about the composition, not the frame.
        script
            .run(
                r#"
                ERR_TAME_FAILED = "%s.";
                PETTAME_TOOHIGHLEVEL = "Creature is too high level for you to tame";
                PETTAME_UNKNOWNERROR = "Unknown taming error";
                BENILLA_TEST_LINES = {};
                local f = CreateFrame("Frame")
                f:RegisterEvent("UI_ERROR_MESSAGE")
                f:SetScript("OnEvent", function()
                    table.insert(BENILLA_TEST_LINES, arg1)
                end)
                "#,
            )
            .unwrap();
    }
    let line = |app: &mut App, i: usize| {
        app.world_mut()
            .non_send_resource::<UiScript>()
            .eval::<Option<String>>(&format!("return BENILLA_TEST_LINES[{i}]"))
            .unwrap()
    };

    // 9 = PETTAME_TOOHIGHLEVEL, the middle of the jump table; 12 is vmangos's own out-of-range
    // twelfth value, which the reference's `reason - 1 > 0xa` bound sends to the default arm.
    app.world_mut().resource_mut::<PetTameFailures>().0.push(9);
    app.world_mut().resource_mut::<PetTameFailures>().0.push(12);
    app.update();

    assert!(
        app.world().resource::<PetTameFailures>().0.is_empty(),
        "the feed drains the queue"
    );
    assert_eq!(
        line(&mut app, 1).as_deref(),
        Some("Creature is too high level for you to tame."),
        "the reason string fills ERR_TAME_FAILED's %s — not the key, and not the reason alone"
    );
    assert_eq!(
        line(&mut app, 2).as_deref(),
        Some("Unknown taming error."),
        "out of range takes the default arm rather than showing nothing"
    );

    // The reference's data-suppression face: a reason whose PETTAME string is not loaded shows
    // NOTHING rather than a bare "." — the inner lookup fails, so the whole line is suppressed.
    app.world_mut().resource_mut::<PetTameFailures>().0.push(1);
    app.update();
    assert_eq!(
        line(&mut app, 3),
        None,
        "an unresolvable reason shows nothing"
    );
}
