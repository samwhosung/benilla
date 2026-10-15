//! The protected-action gate `0x494a50` as the movement functions meet it: code that is not the
//! client's own is refused whatever input event runs, and says so with `MACRO_ACTION_FORBIDDEN`
//! (`RunScript`) or `ADDON_ACTION_FORBIDDEN` (an insecure addon, by name); the client's own code
//! passes inside a binding body and is refused silently outside one.

use crate::bindings_xml;
use crate::script::{AddOnInfo, AddonSecurity, BindingInput, FiredInput, HeldInput, UiScript};

/// A one-addon install: `Walker.lua` defines the functions that call the gated verbs, `Walker.xml`
/// a button whose `OnClick` does, and `Bindings.xml` a binding. The core owns `JUMP`.
fn walker(security: AddonSecurity) -> UiScript {
    let mut s = UiScript::new().unwrap();
    let files = [
        (
            "Interface/AddOns/Walker/Walker.lua",
            "function Walker_Go() MoveForwardStart() end\n\
             function Walker_Spin() TurnLeftStart() PitchUpStart() end\n\
             function Walker_Shout() RunScript('Jump()') end",
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
