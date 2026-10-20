//! The protected-action gate `0x494a50` as the movement functions meet it: code that is not the
//! client's own is refused whatever input event runs, and says so with `MACRO_ACTION_FORBIDDEN`
//! (`RunScript`) or `ADDON_ACTION_FORBIDDEN` (an insecure addon, by name); the client's own code
//! passes inside an input event (a binding body, an edit box's enter script, a button's click) and
//! is refused silently outside one. Then the verbs behind tags 2, 4 and 10, which refuse
//! silently outside an input event and pass inside one.

use crate::bindings_xml;
use crate::script::{
    AddOnInfo, AddonSecurity, BindingInput, FiredInput, HeldInput, PetActionView, SpellBookState,
    SpellSlotView, SpellTabView, UiScript,
};

/// A one-addon install: `Walker.lua` defines the functions that call the gated verbs, `Walker.xml`
/// a button whose `OnClick` does, and `Bindings.xml` a binding. The core owns `JUMP`.
fn walker(security: AddonSecurity) -> UiScript {
    let mut s = UiScript::new().unwrap();
    let files = [
        (
            "Interface/AddOns/Walker/Walker.lua",
            "function Walker_Go() MoveForwardStart() end\n\
             function Walker_Spin() TurnLeftStart() PitchUpStart() end\n\
             function Walker_Shout() RunScript('Jump()') end\n\
             function Walker_Call(f) f() end",
        ),
        (
            "Interface/AddOns/Walker/Walker.xml",
            r#"<Ui><Script>function Walker_Inline() ToggleRun() end</Script>
            <Script file="Extra.lua"/>
            <Button name="WalkerButton"><Scripts>
                <OnClick>ToggleRun()</OnClick>
            </Scripts></Button></Ui>"#,
        ),
        (
            "Interface/AddOns/Walker/Extra.lua",
            "function Walker_Extra() Jump() end",
        ),
        (
            "Interface/AddOns/Walker/Bindings.xml",
            r#"<Bindings><Binding name="WALKERAUTORUN">ToggleAutoRun()</Binding></Bindings>"#,
        ),
    ];
    s.set_addon_chain_reader(Box::new(move |path| {
        files
            .iter()
            .find(|(p, _)| *p == path)
            .map(|(_, text)| text.as_bytes().to_vec())
    }));
    s.register_addons(
        vec![AddOnInfo {
            name: "Walker".into(),
            interface: 11200,
            load_on_demand: true,
            chain: true,
            security,
            enabled: true,
            files: vec!["Walker.lua".into(), "Walker.xml".into()],
            ..Default::default()
        }],
        None,
        None,
        None,
    );
    s.note_addon_info_reply(&[]);
    let core = bindings_xml::parse(
        r#"<Bindings><Binding name="JUMP">Jump()</Binding>
            <Binding name="WALKINLINE">Walker_Inline()</Binding></Bindings>"#,
    )
    .unwrap();
    s.register_bindings(&core);
    // What UIParent.lua registers for: one listener, the events in order with `arg1`.
    s.run(
        r#"
        Events = {}
        local f = CreateFrame("Frame")
        f:RegisterEvent("MACRO_ACTION_FORBIDDEN")
        f:RegisterEvent("ADDON_ACTION_FORBIDDEN")
        f:SetScript("OnEvent", function()
            table.insert(Events, event .. ":" .. tostring(arg1))
        end)
        assert(LoadAddOn("Walker") == 1)
    "#,
    )
    .unwrap();
    s
}

fn events(s: &UiScript) -> Vec<String> {
    s.eval("return Events").unwrap()
}

/// A macro or a `/script` line is `RunScript`'s chunk: every gated call it makes signals
/// `MACRO_ACTION_FORBIDDEN` with no argument, once each, and does nothing. Pitch's pair is gated
/// too (`0x51406a`-`0x5140fa`).
#[test]
fn a_script_runs_movement_into_the_macro_event() {
    let mut s = walker(AddonSecurity::Insecure);
    s.run(r#"RunScript("MoveForwardStart() Jump() PitchUpStart() TurnOrActionStop()")"#)
        .unwrap();
    assert_eq!(events(&s), ["MACRO_ACTION_FORBIDDEN:nil"; 4]);
    assert!(s.take_binding_input().is_empty());
    // What the macro calls is the macro's: a stock function it names does not clean it.
    s.run(r#"function StockWalk() ToggleAutoRun() end RunScript("StockWalk()")"#)
        .unwrap();
    assert_eq!(events(&s).len(), 5);
    assert!(s.take_binding_input().is_empty());
}

/// An insecure addon's own code, a handler it declares in XML and a binding body it registers are
/// all its code however they are reached: the event names it.
#[test]
fn an_insecure_addons_code_is_refused_with_its_name() {
    let mut s = walker(AddonSecurity::Insecure);
    // A file's function, called from a chunk of the client's own.
    s.run("Walker_Go() Walker_Spin()").unwrap();
    // An XML handler, fired as the engine does.
    s.run("WalkerButton:GetScript('OnClick')()").unwrap();
    // A binding body, run by the key's own runner: the input event does not clean it.
    assert!(s.execute_binding("WALKERAUTORUN", true).unwrap());
    assert_eq!(events(&s), ["ADDON_ACTION_FORBIDDEN:Walker"; 5]);
    assert!(s.take_binding_input().is_empty());
    // A chunk of a macro the addon runs is the macro's; a stock key's body is the client's.
    s.run("Walker_Shout()").unwrap();
    assert_eq!(events(&s).last().unwrap(), "MACRO_ACTION_FORBIDDEN:nil");
    assert!(s.execute_binding("JUMP", true).unwrap());
    assert_eq!(events(&s).len(), 6);
    assert_eq!(
        s.take_binding_input(),
        vec![BindingInput::Fire(FiredInput::Jump)]
    );
}

/// A secure addon loads under context 0 (`0x51f387`): its code is the client's own, so it is
/// refused silently outside a binding body, and moves inside one.
#[test]
fn a_secure_addons_code_is_the_clients_own() {
    let mut s = walker(AddonSecurity::Secure);
    s.run("Walker_Go() WalkerButton:GetScript('OnClick')()")
        .unwrap();
    assert!(events(&s).is_empty());
    assert!(s.take_binding_input().is_empty());
    assert!(s.execute_binding("WALKERAUTORUN", true).unwrap());
    assert!(events(&s).is_empty());
    assert_eq!(
        s.take_binding_input(),
        vec![BindingInput::Fire(FiredInput::ToggleAutoRun)]
    );
    // A binding body that calls the secure addon's function moves as well.
    s.run_binding("Walker_Go()").unwrap();
    assert_eq!(
        s.take_binding_input(),
        vec![BindingInput::Start(HeldInput::MoveForward)]
    );
}

/// The client's own code outside any input event is refused with no event at all.
#[test]
fn the_clients_own_code_outside_a_binding_is_refused_silently() {
    let mut s = walker(AddonSecurity::Insecure);
    s.run("MoveForwardStart() Jump() TurnOrActionStart() PitchDownStop()")
        .unwrap();
    assert!(events(&s).is_empty());
    assert!(s.take_binding_input().is_empty());
}

/// The event is signalled with the guard `[0xb4e3e4]` set (`0x494acc`): a handler that calls a
/// gated function is refused without a second event.
#[test]
fn a_refusal_does_not_signal_itself() {
    let s = walker(AddonSecurity::Insecure);
    s.run(
        r#"
        local g = CreateFrame("Frame")
        g:RegisterEvent("ADDON_ACTION_FORBIDDEN")
        g:SetScript("OnEvent", function()
            Probe = (Probe or 0) + 1
            Walker_Go()
        end)
        Walker_Go()
    "#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<i64>("return Probe").unwrap(),
        1,
        "the handler ran once"
    );
    assert_eq!(events(&s), ["ADDON_ACTION_FORBIDDEN:Walker"]);
    // The guard is released: the next refusal signals again.
    s.run("Walker_Go()").unwrap();
    assert_eq!(events(&s).len(), 2);
}

/// A folder the registry does not know is no addon's: nothing runs from an unregistered addon,
/// and benilla's own layer names its chunks `@Interface\AddOns\benilla\…`. Its calls are the
/// client's, refused silently outside a binding body.
#[test]
fn a_folder_the_registry_lacks_is_the_clients_own() {
    let mut s = walker(AddonSecurity::Insecure);
    let name = crate::script::addon_chunk_name("Ghost", "Ghost.lua");
    s.run_chunk_named(b"function Ghost_Go() Jump() end Ghost_Go()", &name)
        .unwrap();
    assert!(events(&s).is_empty());
    assert!(s.take_binding_input().is_empty());
    // The registered one beside it still is.
    let name = crate::script::addon_chunk_name("walker", "Other.lua");
    s.run_chunk_named(b"Jump()", &name).unwrap();
    assert_eq!(events(&s), ["ADDON_ACTION_FORBIDDEN:Walker"]);
}

/// A function an insecure addon's XML defines in an inline `<Script>` block, or in a file the
/// XML names, is its code: the chunk is named over the install path, `<path>:<Scripts>` with no `@`
/// for the block (`0x871074`), and a stock key's binding that calls it is refused with the
/// addon's name. A secure addon's is the client's.
#[test]
fn an_inline_script_block_of_an_insecure_addon_is_its_code() {
    let mut s = walker(AddonSecurity::Insecure);
    assert!(s.execute_binding("WALKINLINE", true).unwrap());
    assert_eq!(events(&s), ["ADDON_ACTION_FORBIDDEN:Walker"]);
    assert!(s.take_binding_input().is_empty());
    // A Lua file the XML names is named over the install path too.
    s.run("Walker_Extra()").unwrap();
    assert_eq!(events(&s).len(), 2);

    let mut s = walker(AddonSecurity::Secure);
    assert!(s.execute_binding("WALKINLINE", true).unwrap());
    assert!(events(&s).is_empty());
    assert_eq!(
        s.take_binding_input(),
        vec![BindingInput::Fire(FiredInput::ToggleRun)]
    );
}

// ── The input events the gate reads ─────────────────────────────────────────────────────────────

/// An edit box, a frame and a button whose scripts all run the global `Body`, over the client's
/// own code. A script of the client's own is context 0, which tag 0 passes only inside an input
/// event.
fn event_frames(s: &mut UiScript) {
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local up = CreateFrame("Frame", "UIParent")
        up:SetPoint("BOTTOMLEFT", 0, 0); up:SetPoint("TOPRIGHT", 0, 0)
        local e = CreateFrame("EditBox", "GateEdit", up)
        e:SetPoint("BOTTOMLEFT", 0, 0); e:SetWidth(100); e:SetHeight(20)
        for _, script in ipairs({"OnEnterPressed", "OnEscapePressed", "OnTabPressed"}) do
            e:SetScript(script, function() Body() end)
        end
        local f = CreateFrame("Frame", "GateFrame", up)
        f:SetPoint("BOTTOMLEFT", 200, 0); f:SetWidth(100); f:SetHeight(100); f:EnableMouse(true)
        f:SetScript("OnMouseDown", function() Body() end)
        f:SetScript("OnMouseUp", function() Body() end)
        local b = CreateFrame("Button", "GateButton", up)
        b:SetPoint("BOTTOMLEFT", 400, 0); b:SetWidth(100); b:SetHeight(100)
        b:RegisterForClicks("LeftButtonUp")
        b:SetScript("OnMouseDown", function() Body() end)
        b:SetScript("OnMouseUp", function() Body() end)
        b:SetScript("OnClick", function() Body() end)
        local d = CreateFrame("Button", "GateDouble", up)
        d:SetPoint("BOTTOMLEFT", 600, 0); d:SetWidth(100); d:SetHeight(100)
        d:RegisterForClicks("LeftButtonUp")
        d:SetScript("OnDoubleClick", function() Body() end)
        local g = CreateFrame("Button", "GateDown", up)
        g:SetPoint("BOTTOMLEFT", 0, 300); g:SetWidth(100); g:SetHeight(100)
        g:RegisterForClicks("LeftButtonDown")
        g:SetScript("OnClick", function() Body() end)
    "#,
    )
    .unwrap();
    s.resolve();
}

fn click(s: &mut UiScript, x: f32, y: f32) {
    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_button(x, y, "LeftButton", false);
}

/// Tag 0 for the client's own code: inside each of the three input events it passes, and outside
/// them it is refused with nothing signalled. Each event is raised around exactly the code the
/// reference raises it around: the enter script and not the escape or tab script (`0x77b438`), the
/// click and not the mouse-down or mouse-up script that runs before it (`0x779275`, `0x779352`),
/// and not `Click()` (`0x782826`).
#[test]
fn the_clients_own_movement_passes_inside_each_input_event() {
    let mut s = walker(AddonSecurity::Insecure);
    event_frames(&mut s);
    s.run("Body = function() MoveForwardStart() end").unwrap();
    let moved = |s: &mut UiScript| s.take_binding_input().len();

    // The enter script, and only it.
    s.run("GateEdit:SetFocus()").unwrap();
    assert!(s.key_input("ESCAPE"));
    assert!(s.key_input("TAB"));
    assert_eq!(moved(&mut s), 0, "escape and tab run no input event");
    assert!(s.key_input("ENTER"));
    assert_eq!(moved(&mut s), 1, "the enter script is one");
    s.run("GateEdit:ClearFocus()").unwrap();

    // A plain frame's mouse scripts are not.
    click(&mut s, 250.0, 50.0);
    assert_eq!(
        moved(&mut s),
        0,
        "OnMouseDown and OnMouseUp are no input event"
    );

    // A button: the click is, and the mouse scripts before it are not. One press and release runs
    // OnMouseDown (refused), OnMouseUp (refused) and OnClick (passes).
    click(&mut s, 450.0, 50.0);
    assert_eq!(moved(&mut s), 1, "only OnClick of the three");
    // A Down registration clicks on the press.
    s.mouse_button(50.0, 350.0, "LeftButton", true);
    assert_eq!(moved(&mut s), 1, "a Down click");
    s.mouse_button(50.0, 350.0, "LeftButton", false);
    // OnDoubleClick, in place of the second OnClick (`0x77938d`).
    click(&mut s, 650.0, 50.0);
    assert_eq!(moved(&mut s), 0, "a single click runs no OnDoubleClick");
    click(&mut s, 650.0, 50.0);
    assert_eq!(moved(&mut s), 1, "the double click is an input event too");

    // `Click()` raises nothing.
    s.run("GateButton:Click()").unwrap();
    assert_eq!(moved(&mut s), 0, "Click() runs the slot with no flag");

    // Every event ends with its code, and after a handler that raised.
    s.run("Body = function() error('boom') end").unwrap();
    click(&mut s, 450.0, 50.0);
    assert!(s.key_input("ENTER"));
    s.run("Body = nil MoveForwardStart()").unwrap();
    assert_eq!(moved(&mut s), 0, "none stays raised");
    assert!(events(&s).is_empty());
}

/// The same events leave tag 0 refused for other code: an insecure addon's handler, and a macro,
/// are refused with their event whatever input runs.
#[test]
fn tainted_movement_is_refused_inside_every_input_event() {
    let mut s = walker(AddonSecurity::Insecure);
    event_frames(&mut s);
    s.run("Body = function() Walker_Call(function() MoveForwardStart() end) end")
        .unwrap();
    s.run("GateEdit:SetFocus()").unwrap();
    assert!(s.key_input("ENTER"));
    click(&mut s, 450.0, 50.0);
    assert!(s.execute_binding("WALKINLINE", true).unwrap());
    s.run("Body = function() RunScript('MoveForwardStart()') end")
        .unwrap();
    assert!(s.key_input("ENTER"));
    click(&mut s, 450.0, 50.0);
    assert!(s.take_binding_input().is_empty());
    // The addon's body refuses at the enter script, the three scripts of one click on the button
    // (OnMouseDown, OnMouseUp, OnClick) and the binding; the macro's at the first four.
    let names: Vec<String> = events(&s)
        .iter()
        .map(|e| e.split(':').next().unwrap().to_string())
        .collect();
    let mut want = vec!["ADDON_ACTION_FORBIDDEN".to_string(); 5];
    want.extend(vec!["MACRO_ACTION_FORBIDDEN".to_string(); 4]);
    assert_eq!(names, want);
    assert!(events(&s)[..5].iter().all(|e| e.ends_with(":Walker")));
}

// ── Tags 2, 4 and 10 ────────────────────────────────────────────────────────────────────────────

/// One verb behind a silent arm: the chunk that calls it, and whether a refused call still queues
/// itself with its flag cleared (the call decides what to do with it when it is applied) or queues
/// nothing.
struct Verb {
    chunk: &'static str,
    /// The refused call is queued with `may_cast`/`may_pick` false, for the app to apply.
    queued: bool,
}

const fn gone(chunk: &'static str) -> Verb {
    Verb {
        chunk,
        queued: false,
    }
}

const fn flagged(chunk: &'static str) -> Verb {
    Verb {
        chunk,
        queued: true,
    }
}

/// Every Lua verb whose reference path reaches a tag-2, tag-4 or tag-10 caller of `0x494a50` that
/// this fixture can set up. The census: `TryCastSpell`'s callers (`CastBookSpell`, `UseActionSlot`,
/// `CGItem::Use`, the shapeshift, duel, craft and trade-skill casts, `Stuck`), `PressPetAction`'s,
/// `TogglePetSlotAutocast`, `SendSpellAutocast`, `SendPetAbandon`, `SpellTargetUnit`,
/// `SpellStopTargeting`, `SpellStopCasting`, `CGGameUI::TargetNearest` (and the attack validator's
/// pick that reaches it), `CGGameUI::Reload` and `BindSpellTargetObject`, which `SelectTarget`
/// reaches from every selecting verb while the cursor is up.
const VERBS: &[Verb] = &[
    flagged(r#"CastSpellByName("Fireball")"#),
    flagged(r#"CastSpell(1, BOOKTYPE_SPELL)"#),
    flagged("UseAction(1)"),
    flagged("CastShapeshiftForm(1)"),
    gone("SpellStopCasting()"),
    gone("SpellStopTargeting()"),
    gone(r#"SpellTargetUnit("player")"#),
    gone("CastPetAction(1)"),
    gone("PetAttack()"),
    gone("PetFollow()"),
    gone("PetWait()"),
    gone("PetPassiveMode()"),
    gone("PetDefensiveMode()"),
    gone("PetAggressiveMode()"),
    gone("PetDismiss()"),
    gone("PetAbandon()"),
    gone("TogglePetAutocast(2)"),
    gone("ToggleSpellAutocast(1, BOOKTYPE_PET)"),
    gone(r#"StartDuel("Bob")"#),
    gone(r#"StartDuelUnit("target")"#),
    gone("Stuck()"),
    gone("ReloadUI()"),
    gone("TargetNearestEnemy()"),
    gone("TargetNearestFriend()"),
    gone("TargetNearestPartyMember()"),
    gone("TargetNearestRaidMember()"),
    flagged("AttackTarget()"),
    // `SelectTarget` binds the unit to the cast with the cursor up, behind tag 2 (`0x6e5ba5`); the
    // verbs queue either way, the cursor being the app's to see when it applies them.
    flagged(r#"TargetUnit("player")"#),
    flagged(r#"AssistUnit("player")"#),
    flagged("TargetLastEnemy()"),
    flagged(r#"TargetByName("Bob")"#),
    flagged(r#"AssistByName("Bob")"#),
    flagged("TargetLastTarget()"),
    gone(r#"DropItemOnUnit("pet")"#),
    flagged("BindEnchant()"),
    flagged("ReplaceEnchant()"),
];

/// A VM with everything [`VERBS`] needs to do something: a book with Fireball, a form, a pet bar
/// and book, a cast in flight, a targeting cursor and a Down-and-Up button for the events.
fn verbs_world(security: AddonSecurity) -> UiScript {
    let mut s = walker(security);
    s.keep_calls();
    event_frames(&mut s);
    s.set_spellbook(SpellBookState {
        tabs: vec![SpellTabView {
            name: "Fire".into(),
            texture: None,
            offset: 0,
            num_spells: 1,
        }],
        slots: vec![SpellSlotView {
            spell_id: 133,
            name: "Fireball".into(),
            ..Default::default()
        }],
    });
    s.set_pet_book(crate::script::PetBookState {
        token: Some("PET".into()),
        slots: vec![SpellSlotView {
            spell_id: 2649,
            name: "Growl".into(),
            autocast: Some((true, false)),
            packed: 0x8000_0000 | 2649,
            ..Default::default()
        }],
    });
    // `StartDuel("Bob")` needs a name the resolver finds: the active player, and Bob in the party.
    s.set_unit(
        "player",
        Some(crate::script::UnitState {
            exists: true,
            has_object: true,
            ..Default::default()
        }),
    );
    s.set_party(crate::script::PartyState {
        members: vec![crate::script::PartyMemberInfo {
            name: "Bob".into(),
            guid: 0xB0B,
        }],
        ..Default::default()
    });
    let slot = |name: &str, autocast| PetActionView {
        name: Some(name.into()),
        autocast_allowed: autocast,
        packed: 0x8000_0000 | 2649,
        ..Default::default()
    };
    s.set_pet_actions(
        true,
        true,
        true,
        vec![slot("Claw", false), slot("Growl", true)],
    );
    s.set_shapeshift_forms(vec![crate::script::ShapeshiftFormView {
        spell_id: 2457,
        name: "Battle Stance".into(),
        castable: true,
        ..Default::default()
    }]);
    s.set_action(
        1,
        Some(crate::script::ActionSlot {
            kind: 0x00,
            action: 133,
            ..Default::default()
        }),
    );
    s.set_cast_view(crate::script::CastView {
        slot: Some(133),
        ..Default::default()
    });
    s.set_spell_targeting(true);
    s
}

/// Everything the verbs queue, as text: the call stream and each side queue.
fn effects(s: &mut UiScript) -> Vec<String> {
    let mut out: Vec<String> = s
        .take_script_calls()
        .into_iter()
        .map(|c| format!("{c:?}"))
        .collect();
    // A macro's replay of the events is not what these tests compare.
    for e in &mut out {
        if let Some(i) = e.find(", input: InputEvents {") {
            let end = e[i..].find('}').map_or(e.len(), |j| i + j + 1);
            e.replace_range(i..end, "");
        }
    }
    out.extend(s.take_session_requests().iter().map(|r| format!("{r:?}")));
    out.extend(s.take_duel_requests().iter().map(|r| format!("{r:?}")));
    out.extend(
        s.take_drop_item_on_unit()
            .iter()
            .map(|t| format!("drop {t}")),
    );
    out.extend(
        s.take_pet_autocast_toggles()
            .iter()
            .map(|i| format!("autocast {i}")),
    );
    out.extend(
        s.take_pet_spell_autocasts()
            .iter()
            .map(|i| format!("book autocast {i}")),
    );
    let (abandons, dismisses) = s.take_pet_gives_up();
    out.extend((0..abandons).map(|_| "abandon".to_string()));
    out.extend((0..dismisses).map(|_| "dismiss".to_string()));
    out.extend((0..s.take_stuck_casts()).map(|_| "stuck".to_string()));
    out
}

/// What a refused call leaves: nothing, or for a call that is applied later, the same call with its
/// flag cleared.
fn refused(verb: &Verb, passed: &[String]) -> Vec<String> {
    if !verb.queued {
        return Vec::new();
    }
    passed
        .iter()
        .map(|e| {
            e.replace("may_cast: true", "may_cast: false")
                .replace("may_pick: true", "may_pick: false")
        })
        .collect()
}

/// Run `check` on every verb and fail naming all that went wrong, so a single run says which gates
/// a mutant of the code leaves unasked.
fn each_verb(check: impl Fn(&Verb)) {
    let failed: Vec<&str> = VERBS
        .iter()
        .filter(|v| std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check(v))).is_err())
        .map(|v| v.chunk)
        .collect();
    assert!(failed.is_empty(), "verbs that went wrong: {failed:#?}");
}

/// Tags 2, 4 and 10 pass the client's own code anywhere, so the stock UI never meets a refusal, and
/// refuse all other code outside an input event, silently: no event, no queue entry.
#[test]
fn spell_target_and_reload_verbs_refuse_tainted_code_silently() {
    each_verb(|verb| {
        {
            let mut s = verbs_world(AddonSecurity::Insecure);
            s.run(verb.chunk).unwrap();
            let passed = effects(&mut s);
            assert!(
                !passed.is_empty(),
                "{}: the client's own call did nothing",
                verb.chunk
            );

            // A macro (`RunScript`) and an insecure addon's function: refused, no event.
            for caller in [
                format!("RunScript({:?})", verb.chunk),
                format!("Walker_Call(function() {} end)", verb.chunk),
            ] {
                s.run(&caller).unwrap();
                assert_eq!(
                    effects(&mut s),
                    refused(verb, &passed),
                    "{}: from {caller}",
                    verb.chunk
                );
                assert!(events(&s).is_empty(), "{}: signalled a refusal", verb.chunk);
            }
        }
    });
}

/// Inside a key's body, an enter script and a click, the same tainted calls go through.
#[test]
fn spell_target_and_reload_verbs_pass_tainted_code_inside_each_input_event() {
    each_verb(|verb| {
        let mut s = verbs_world(AddonSecurity::Insecure);
        s.run(verb.chunk).unwrap();
        let passed = effects(&mut s);
        let addon = format!(
            "Body = function() Walker_Call(function() {} end) end",
            verb.chunk
        );
        let macro_ = format!("Body = function() RunScript({:?}) end", verb.chunk);
        for body in [&addon, &macro_] {
            s.run(body).unwrap();
            s.run("GateEdit:SetFocus()").unwrap();
            assert!(s.key_input("ENTER"));
            assert_eq!(
                effects(&mut s),
                passed,
                "{}: enter script, {body}",
                verb.chunk
            );
            s.run("GateEdit:ClearFocus()").unwrap();

            // The button's OnMouseDown and OnMouseUp run the body first, outside an event, and
            // OnClick last, inside one.
            click(&mut s, 450.0, 50.0);
            let mut want = [refused(verb, &passed), refused(verb, &passed)].concat();
            want.extend(passed.clone());
            assert_eq!(effects(&mut s), want, "{}: click, {body}", verb.chunk);
            s.mouse_button(50.0, 350.0, "LeftButton", true);
            assert_eq!(
                effects(&mut s),
                passed,
                "{}: down click, {body}",
                verb.chunk
            );
            s.mouse_button(50.0, 350.0, "LeftButton", false);
            effects(&mut s);

            let chunk = body.replacen("Body = ", "return (", 1) + ")()";
            s.run_binding(&chunk).unwrap();
            assert_eq!(effects(&mut s), passed, "{}: binding, {body}", verb.chunk);

            // Not by the frame's mouse scripts, escape or tab, nor `Click()`: five calls, none
            // of them inside an input event.
            click(&mut s, 250.0, 50.0);
            s.run("GateEdit:SetFocus()").unwrap();
            assert!(s.key_input("ESCAPE"));
            assert!(s.key_input("TAB"));
            s.run("GateEdit:ClearFocus() GateButton:Click()").unwrap();
            assert_eq!(
                effects(&mut s),
                std::iter::repeat_n(refused(verb, &passed), 5)
                    .flatten()
                    .collect::<Vec<_>>(),
                "{}: frame mouse scripts, escape, tab and Click() raise no event",
                verb.chunk
            );
        }
    });
}
