//! [`super::feed::feed_actions`]'s refusals as a registered system through a real Lua VM, and the
//! consumable test its getter reads.

use std::collections::HashMap;

use benilla_protocol::messages::ItemSpellEntry;
use benilla_ui::script::UiScript;
use bevy::prelude::*;

use super::feed::feed_actions;
use super::{PlayerActions, UiError, UiErrorKeys};
use crate::items::{test_template, Items};
use crate::net::{ClientCommand, NetCommands};

/// The feed with an empty bar and a cold template cache, as at login.
fn app_with_an_empty_bar() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
    let (tx, rx) = crossbeam_channel::unbounded();
    let mut app = App::new();
    app.insert_resource(PlayerActions::default())
        .init_resource::<crate::ui_action::ActionSlots>()
        .init_resource::<Items>()
        .init_resource::<crate::net::GuidIndex>()
        .init_resource::<UiErrorKeys>()
        // The cast-failure combat-log line rides the same drain.
        .init_resource::<crate::ui_chat::ChatLog>()
        .init_resource::<crate::sound::MessageSounds>()
        .insert_resource(NetCommands(tx));
    app.insert_non_send_resource(UiScript::new().unwrap());
    // The ui-error lane fired alone after the feed, as the drain fires it after every feed.
    app.add_systems(Update, (feed_actions, super::fire_ui_error_lane).chain());
    (app, rx)
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

/// The resolved texts end to end into the stock `UIErrorsFrame`: an `SMSG_NOTIFICATION` draws red,
/// an `SMSG_AREA_TRIGGER_MESSAGE` yellow.
#[test]
fn pre_resolved_lines_land_on_the_errors_frame_in_the_arms_colour() {
    benilla_formats::wow_data_or_skip!();
    let (mut app, _rx) = app_with_an_empty_bar();
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
    let texts = app.world().resource::<UiErrorKeys>();
    texts.error_text("GM mode is ON".to_string());
    texts.info_text("You must be at least level 58 to enter.".to_string());
    app.update();

    assert!(
        app.world().resource::<UiErrorKeys>().texts().is_empty(),
        "the lane drains the queue"
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

/// A login notice queued while the loading cover is up waits for the reveal: the reference's
/// cover drops at the player's create (`0x5debbd`), ahead of the notice, so its `UIErrorsFrame`
/// hold is never spent under the cover.
#[test]
fn a_notice_under_the_loading_cover_fires_at_the_reveal() {
    let (mut app, _rx) = app_with_an_empty_bar();
    app.insert_resource(crate::loading_screen::LoadingScreen::test_covering());
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(
            r#"
            BENILLA_TEST_ERRORS = {}
            local f = CreateFrame("Frame")
            f:RegisterEvent("UI_ERROR_MESSAGE")
            f:SetScript("OnEvent", function() table.insert(BENILLA_TEST_ERRORS, arg1) end)
            "#,
        )
        .unwrap();
    let fired = |app: &mut App| {
        app.world_mut()
            .non_send_resource::<UiScript>()
            .eval::<Vec<String>>("return BENILLA_TEST_ERRORS")
            .unwrap()
    };

    app.world()
        .resource::<UiErrorKeys>()
        .error_text("GM mode is ON".to_string());
    app.update();
    app.update();
    assert!(fired(&mut app).is_empty(), "nothing fires under the cover");
    assert_eq!(
        app.world().resource::<UiErrorKeys>().texts().len(),
        1,
        "the notice stays queued"
    );

    app.insert_resource(crate::loading_screen::LoadingScreen::default());
    app.update();
    assert_eq!(
        fired(&mut app),
        ["GM mode is ON"],
        "the reveal fires it once"
    );
    assert!(app.world().resource::<UiErrorKeys>().texts().is_empty());
}

/// The cast reports' lane ([`crate::ui_cast::signal_reports`]) over a VM, with `reports` queued
/// and `spells` as the catalog.
fn report_feed(spells: crate::ui_action::Spells, reports: crate::ui_cast::CastReports) -> App {
    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut app = App::new();
    app.add_message::<crate::net::FieldChanged>();
    app.insert_resource(reports)
        .insert_resource(spells)
        .init_resource::<Items>()
        .init_resource::<crate::net::GuidIndex>()
        .init_resource::<crate::ui_chat::ChatLog>()
        .init_resource::<crate::sound::MessageSounds>()
        .init_resource::<crate::spell::PendingCast>()
        .init_resource::<crate::spell::QueuedMeleeSpell>()
        .init_resource::<crate::spell::targeting::SpellTargeting>()
        .init_resource::<crate::spell::ActiveChannel>()
        .init_resource::<crate::spell::AutoRepeatActive>()
        .init_resource::<crate::creature_anim::UnitEdits>()
        .add_message::<crate::player::StandStateRequest>()
        .insert_resource(NetCommands(tx));
    app.insert_non_send_resource(UiScript::new().unwrap());
    app.add_systems(Update, crate::ui_cast::signal_reports);
    app
}

/// A validator rung's own line (`0x496720` from `0x6098a2`, `0x609930`, `0x6e6ab4`) shows as its
/// catalog record does, its `%s` filled: `ERR_SPELL_ALREADY_KNOWN_S` with the taught spell's name,
/// and `ERR_TAME_FAILED` with the `PETTAME_TOOMANY` string, each a `UI_ERROR_MESSAGE`.
#[test]
fn a_validator_line_shows_as_its_message_does() {
    use crate::ui_action::{FillArg, Spells, UiError};
    let mut reports = crate::ui_cast::CastReports::default();
    reports.line(UiError {
        key: "ERR_SPELL_ALREADY_KNOWN_S",
        args: vec![FillArg::S("Fireball".into())],
    });
    reports.line(UiError {
        key: "ERR_TAME_FAILED",
        args: vec![FillArg::Global("PETTAME_TOOMANY".into())],
    });
    let mut app = report_feed(Spells::empty_for_tests(), reports);
    app.world_mut()
        .non_send_resource::<UiScript>()
        .run(
            r#"ERR_SPELL_ALREADY_KNOWN_S = "You already know %s.";
               ERR_TAME_FAILED = "%s.";
               PETTAME_TOOMANY = "You have too many pets already";
               SEEN = {}
               local f = CreateFrame("Frame")
               f:RegisterEvent("UI_ERROR_MESSAGE")
               f:SetScript("OnEvent", function() table.insert(SEEN, arg1) end)"#,
        )
        .unwrap();
    app.update();
    let seen = app
        .world_mut()
        .non_send_resource::<UiScript>()
        .eval::<Vec<String>>("return SEEN")
        .unwrap();
    assert_eq!(
        seen,
        [
            "You already know Fireball.",
            "You have too many pets already."
        ]
    );
}

/// `HandleCastFailed 0x6e1a00` logs through `0x62c360`; `HandlePetCastFailed 0x6e8eb0` calls only
/// the packet readers and `0x496720`, with no log line and no error sound. Both entries name the
/// same spell, so only the caster differs.
#[test]
fn a_pets_refused_cast_writes_no_combat_log_line() {
    use crate::ui_action::{CastFail, Spells};
    use benilla_formats::{SpellCatalog, SpellDisplay};

    const GROWL: u32 = 2649;

    let mut reports = crate::ui_cast::CastReports::default();
    // 0x5f ROOTED has a string on both tables, so neither entry drops for want of one.
    reports.refused(CastFail::server(GROWL, 0x5F, None));
    reports.pet_refused(GROWL, 0x5F);
    let mut app = report_feed(
        Spells {
            catalog: SpellCatalog::from_displays(HashMap::from([(
                GROWL,
                SpellDisplay {
                    name: "Growl".into(),
                    ..Default::default()
                },
            )])),
            ..Spells::empty_for_tests()
        },
        reports,
    );
    // Only what the log line needs, its template and its reason; the red line's text has its own
    // tests.
    app.world_mut()
        .non_send_resource::<UiScript>()
        .run(
            r#"SPELLFAILCASTSELF = "You fail to cast %s: %s."
               SPELL_FAILED_ROOTED = "You are unable to move";"#,
        )
        .unwrap();
    app.update();

    assert_eq!(
        app.world()
            .resource::<crate::ui_chat::ChatLog>()
            .pending_len(),
        1,
        "the player's failure logs and the pet's does not"
    );
}

/// `0x62c360` logs no refusal of a spell flagged `Attributes & 0x180` (`0x62c4bd`), though the
/// red line still shows.
#[test]
fn a_refused_spell_flagged_not_to_log_writes_no_combat_log_line() {
    use crate::ui_action::{CastFail, Spells};
    use benilla_formats::{SpellCatalog, SpellDisplay};

    let logged = |attributes| {
        let mut reports = crate::ui_cast::CastReports::default();
        reports.refused(CastFail::server(1, 0x5F, None));
        let mut app = report_feed(
            Spells {
                catalog: SpellCatalog::from_displays(HashMap::from([(
                    1,
                    SpellDisplay {
                        name: "Growl".into(),
                        attributes,
                        ..Default::default()
                    },
                )])),
                ..Spells::empty_for_tests()
            },
            reports,
        );
        app.world_mut()
            .non_send_resource::<UiScript>()
            .run(
                r#"SPELLFAILCASTSELF = "You fail to cast %s: %s."
                   SPELL_FAILED_ROOTED = "You are unable to move";"#,
            )
            .unwrap();
        app.update();
        app.world()
            .resource::<crate::ui_chat::ChatLog>()
            .pending_len()
    };
    assert_eq!(logged(0), 1);
    assert_eq!(logged(0x100), 0, "SPELL_ATTR_DO_NOT_LOG");
    assert_eq!(logged(0x80), 0, "SPELL_ATTR_DO_NOT_DISPLAY");
}

/// `DisplayError` formats the default errorId through `ERR_SPELL_FAILED_S` (`0x4967d7`); an empty
/// one shows nothing (`0x4945b0` drops an empty text), and the log still writes the reason
/// (`0x6e21e2`-`0x6e21fc` run whatever was shown).
#[test]
fn an_empty_default_format_shows_nothing_and_still_logs() {
    use crate::ui_action::{CastFail, Spells};
    use benilla_formats::{SpellCatalog, SpellDisplay};

    let lines = |format: &str| {
        let mut reports = crate::ui_cast::CastReports::default();
        reports.refused(CastFail::server(1, 0x5F, None));
        let mut app = report_feed(
            Spells {
                catalog: SpellCatalog::from_displays(HashMap::from([(
                    1,
                    SpellDisplay {
                        name: "Growl".into(),
                        ..Default::default()
                    },
                )])),
                ..Spells::empty_for_tests()
            },
            reports,
        );
        app.world_mut()
            .non_send_resource::<UiScript>()
            .run(&format!(
                r#"SPELLFAILCASTSELF = "You fail to cast %s: %s."
                SPELL_FAILED_ROOTED = "You are unable to move"
                ERR_SPELL_FAILED_S = "{format}"
                SEEN = {{}}
                local f = CreateFrame("Frame")
                f:RegisterEvent("UI_ERROR_MESSAGE")
                f:SetScript("OnEvent", function() table.insert(SEEN, arg1) end)"#
            ))
            .unwrap();
        app.update();
        let seen = app
            .world_mut()
            .non_send_resource::<UiScript>()
            .eval::<Vec<String>>("return SEEN")
            .unwrap();
        let logged = app
            .world()
            .resource::<crate::ui_chat::ChatLog>()
            .pending_len();
        (seen, logged)
    };
    assert_eq!(lines("%s"), (vec!["You are unable to move".to_string()], 1));
    assert_eq!(
        lines("Failed: %s"),
        (vec!["Failed: You are unable to move".to_string()], 1)
    );
    assert_eq!(lines(""), (vec![], 1));
}

/// `0x6e6a20` resolves `PETTAME_<reason>` first and passes the string to `DisplayError(0xee)`,
/// so `ERR_TAME_FAILED` ("%s.") prints the sentence, not the key; a missing string reads as `""`
/// (`FrameScript_GetText 0x703bf0`), and the line still shows.
#[test]
fn a_tame_failure_composes_the_reason_string_into_err_tame_failed() {
    let (mut app, _rx) = app_with_an_empty_bar();
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

    // The line the handler queues for a reason ([`crate::ui_pet`]'s `pet_tame_failure`).
    let tame = |reason: u8| {
        UiError::args(
            "ERR_TAME_FAILED",
            vec![super::FillArg::Global(
                benilla_protocol::messages::pet_tame_failure_key(reason).to_string(),
            )],
        )
    };
    // 9 is `PETTAME_TOOHIGHLEVEL`; 12, a vmangos value past the reference's `reason - 1 > 0xa`
    // bound, takes the default arm.
    app.world().resource::<UiErrorKeys>().push(tame(9));
    app.world().resource::<UiErrorKeys>().push(tame(12));
    app.update();

    assert!(
        app.world().resource::<UiErrorKeys>().is_empty(),
        "the lane drains the queue"
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

    // A reason whose `PETTAME_` string is not loaded fills `%s` with `""`.
    app.world().resource::<UiErrorKeys>().push(tame(1));
    app.update();
    assert_eq!(
        line(&mut app, 3).as_deref(),
        Some("."),
        "a missing reason string reads as empty, as GetText answers it"
    );
}

/// Every first pass through the failure handler `0x6e1a00`, a local refusal included, disarms the
/// tradeskill repeat when it names the armed spell (`0x6e1a3c`-`0x6e1a55`); another spell's does
/// not.
#[test]
fn a_refusal_of_the_armed_craft_disarms_the_batch() {
    use crate::ui_action::CastFail;
    let mut app = report_feed(
        crate::ui_action::Spells::empty_for_tests(),
        crate::ui_cast::CastReports::default(),
    );
    {
        let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
        script.set_trade_skill(Some(benilla_ui::script::TradeSkillState {
            line: 197,
            line_name: "Tailoring".into(),
            rank: 1,
            max_rank: 75,
            recipes: vec![benilla_ui::script::TradeSkillRecipe {
                group: None,
                spell_id: 2963,
                name: "Bolt of Linen Cloth".into(),
                difficulty: benilla_ui::script::TradeSkillDifficulty::Trivial,
                num_available: 5,
                icon: None,
                min_made: 1,
                max_made: 1,
                cooldown: None,
                product_item: 2996,
                product_inv_type: 0,
                product_item_level: 0,
                reagents: vec![],
                tools: vec![],
                icon_pending: None,
                tools_pending: Vec::new(),
            }],
        }));
        script.keep_calls();
        script.run("DoTradeSkill(2, 3)").unwrap();
    }
    let repeat = |app: &App| {
        app.world()
            .non_send_resource::<UiScript>()
            .trade_skill_repeat()
    };
    app.world_mut()
        .resource_mut::<crate::ui_cast::CastReports>()
        .refused(CastFail::local(133, 0x0c));
    app.update();
    assert_eq!(repeat(&app), (2963, 3), "another spell's refusal");
    app.world_mut()
        .resource_mut::<crate::ui_cast::CastReports>()
        .refused(CastFail::local(2963, 0x0c));
    app.update();
    assert_eq!(repeat(&app), (0, 3), "the armed craft's refusal disarms");
}
