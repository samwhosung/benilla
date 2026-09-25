//! [`super::feed::feed_actions`] as a registered system through a real Lua VM. Not
//! `run_system_once`: the feed's memory is a `Local`, which that helper rebuilds per call.

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

/// A new human warrior's food button (vmangos `playercreateinfo_action`): Tough Jerky on wire
/// slot 83, the Battle Stance page.
const JERKY: u32 = 117;
const JERKY_SLOT: u8 = 83;
const JERKY_ACTION: u32 = JERKY_SLOT as u32 + 1;
/// Tough Jerky's display id and its `ItemDisplayInfo.dbc` icon.
const JERKY_DISPLAY: u32 = 2473;
const JERKY_ICON: &str = "Interface\\Icons\\INV_Misc_Food_16";

/// The feed with the food button on the bar and a cold template cache, as at login.
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
    // As `SMSG_ACTION_BUTTONS` sets it; the feed's first pass clears it.
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
        // The cast-failure combat-log line rides the same drain.
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

/// The food button's `GetActionTexture`, as `ActionButton_Update` reads it
/// (`ActionButton.lua:157`); a nil hides the icon.
fn fed_texture(app: &mut App) -> Option<String> {
    app.world_mut()
        .non_send_resource::<UiScript>()
        .eval::<Option<String>>(&format!("return GetActionTexture({JERKY_ACTION})"))
        .unwrap()
}

/// The food button's `IsConsumableAction`, the count gate in `ActionButton_UpdateCount`
/// (`ActionButton.lua:287`): 1 or nil.
fn fed_consumable(app: &mut App) -> Option<i64> {
    app.world_mut()
        .non_send_resource::<UiScript>()
        .eval::<Option<i64>>(&format!("return IsConsumableAction({JERKY_ACTION})"))
        .unwrap()
}

/// The first resolve asks for the template and shows the placeholder; the answer re-resolves.
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

/// `IsConsumableAction 0x4e5250` reads only the item template, so it rides the icon's push: the
/// `ACTIONBAR_SLOT_CHANGED` repaint reads both.
#[test]
fn a_landed_item_template_also_lands_the_consumable_gate() {
    let (mut app, _rx) = app_with_food_on_the_bar();

    app.update();
    assert_eq!(
        fed_consumable(&mut app),
        None,
        "a cold template cannot answer the gate — the ask is still in flight"
    );

    // Tough Jerky's row (vmangos `item_template` 117): one ON_USE block, spell 433, charges -1.
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

/// `IsConsumableAction 0x4e5250`'s two clauses; the item class is neither.
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

    // A mount: class 15, InventoryType 0, one ON_USE block with charges 0 (not used up).
    let mut mount = test_template("Red Skeletal Horse");
    mount.class = 15;
    mount.spells = vec![block(0, 0)];
    assert!(!mount.is_consumable(), "a mount has no stack to show");

    // A potion: its ON_USE block's -1 charges decide, not its class 0.
    let mut potion = test_template("Minor Healing Potion");
    potion.spells = vec![block(0, -1)];
    assert!(potion.is_consumable());

    // Class 0 with no ON_USE block is not consumable.
    let classless = test_template("Trade Good");
    assert!(!classless.is_consumable());

    // Ammo (24) and thrown (25) always count, charges or not.
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

/// A negative answer advances the epoch once and keeps the placeholder.
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

/// `GetActionTexture`'s macro arm (`0x4e6bf9`) shows the macro's own icon, never the bound
/// spell's; the macro-table generation, beside `dirty` and the template epoch, re-resolves it.
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
        // The cast-failure combat-log line rides the same drain.
        .init_resource::<crate::ui_chat::ChatLog>()
        .init_resource::<crate::sound::MessageSounds>()
        .insert_resource(NetCommands(tx));
    let mut script = UiScript::new().unwrap();
    script.set_macros(MacroState {
        account: vec![MacroView {
            name: "Ambush".into(),
            texture: Some("Interface\\Icons\\Ability_Ambush".into()),
            // The bound spell is not what the icon shows.
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

    // A rename leaves the slot's texture, kind and id unchanged, so a value diff alone would
    // fire nothing; the feed must re-fire the slot anyway.
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
    // A frame with nothing moved fires nothing more.
    app.update();
    assert_eq!(events(&mut app), 1);
}

/// `UiErrorTexts` end to end into the stock `UIErrorsFrame`: an `SMSG_NOTIFICATION` draws red,
/// an `SMSG_AREA_TRIGGER_MESSAGE` yellow.
#[test]
fn pre_resolved_lines_land_on_the_errors_frame_in_the_arms_colour() {
    benilla_formats::wow_data_or_skip!();
    let (mut app, _rx) = app_with_food_on_the_bar();
    {
        let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
        script.set_screen_size(1024.0, 768.0);
        // Stock files, read from the install.
        for file in [
            "Interface\\FrameXML\\Fonts.xml",
            "Interface\\FrameXML\\UIErrorsFrame.xml",
        ] {
            crate::ui_script::load_ui_for_test(&script, file);
        }
    }

    // What the net drain queues for one `.gm on`, plus a refused portal.
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
            // `AddMessage` quantizes each channel to a byte (`ftol(v*255 + 0.5)`): 0.1 is 26/255.
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

/// `HandleCastFailed 0x6e1a00` logs through `0x62c360`; `HandlePetCastFailed 0x6e8eb0` calls only
/// the packet readers and `0x496720`, with no log line and no error sound. Both entries name the
/// same spell, so only the caster differs.
#[test]
fn a_pets_refused_cast_writes_no_combat_log_line() {
    use crate::ui_action::{CastFail, Caster, Spells};
    use benilla_formats::{SpellCatalog, SpellDisplay};

    const GROWL: u32 = 2649;

    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut app = App::new();
    let mut errors = CastErrors::default();
    // 0x5f ROOTED has a string on both tables, so neither entry drops for want of one.
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
    // Only what the log line needs; the red line's text has its own tests.
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

/// `0x6e6a20` resolves `PETTAME_<reason>` first and passes the string to `DisplayError(0xee)`,
/// so `ERR_TAME_FAILED` ("%s.") prints the sentence, not the key.
#[test]
fn a_tame_failure_composes_the_reason_string_into_err_tame_failed() {
    let (mut app, _rx) = app_with_food_on_the_bar();
    {
        let script = app.world_mut().non_send_resource_mut::<UiScript>();
        // The strings verbatim from 1.12's `GlobalStrings.lua`, and a recorder for the event.
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

    // 9 is `PETTAME_TOOHIGHLEVEL`; 12, a vmangos value past the reference's `reason - 1 > 0xa`
    // bound, takes the default arm.
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

    // A reason whose `PETTAME_` string is not loaded shows nothing, not a bare ".".
    app.world_mut().resource_mut::<PetTameFailures>().0.push(1);
    app.update();
    assert_eq!(
        line(&mut app, 3),
        None,
        "an unresolvable reason shows nothing"
    );
}
