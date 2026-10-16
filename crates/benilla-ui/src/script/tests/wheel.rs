//! The wheel walk (`0x7664f0`): one sweep over the wheel list's frames under the cursor, a frame
//! taking the notch only with an `OnMouseWheel` script, the world frame last.

use super::common::script;
use crate::script::{UiScript, WheelWalk};

/// A world frame over the whole 800x600 screen: the end of every walk, as the game builds it.
fn with_world() -> UiScript {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local w = CreateFrame("WorldFrame", "WorldFrame")
        w:SetPoint("BOTTOMLEFT", 0, 0) w:SetWidth(800) w:SetHeight(600)
        calls = {}
        function Pane(name, x, y, w, h, parent)
            local f = CreateFrame("Frame", name, parent)
            f:SetPoint("BOTTOMLEFT", x, y) f:SetWidth(w) f:SetHeight(h)
            return f
        end
        function Takes(f)
            f:EnableMouseWheel(true)
            f:SetScript("OnMouseWheel", function() table.insert(calls, this:GetName() .. ":" .. arg1) end)
        end
        "#,
    )
    .unwrap();
    s
}

fn fired(s: &UiScript) -> String {
    s.eval::<String>("return table.concat(calls, ',')").unwrap()
}

#[test]
fn a_notch_over_a_frame_with_no_handler_ends_at_the_world() {
    let mut s = with_world();
    s.run(
        r#"
        -- A mouse-enabled button on top of the world, as an action button is.
        local b = CreateFrame("Button", "Action")
        b:SetPoint("BOTTOMLEFT", 300, 200) b:SetWidth(100) b:SetHeight(100)
        b:SetFrameLevel(5)
        -- A wheel-enabled frame whose script slot is empty (`0x76c18d` finds nothing).
        Pane("Bare", 300, 200, 100, 100):EnableMouseWheel(true)
        -- A script that no `EnableMouseWheel` ever put in the list (`SetScript` enables nothing).
        local u = Pane("Unlisted", 300, 200, 100, 100)
        u:SetScript("OnMouseWheel", function() table.insert(calls, "Unlisted") end)
        "#,
    )
    .unwrap();
    s.resolve();
    assert_eq!(s.mouse_wheel(350.0, 250.0, 1.0), WheelWalk::World);
    assert_eq!(fired(&s), "");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

#[test]
fn a_frame_with_a_handler_takes_the_notch_and_the_world_never_sees_it() {
    let mut s = with_world();
    s.run(r#"Takes(Pane("Scroll", 300, 200, 100, 100))"#)
        .unwrap();
    s.resolve();
    assert_eq!(s.mouse_wheel(350.0, 250.0, 1.0), WheelWalk::Frame);
    assert_eq!(s.mouse_wheel(350.0, 250.0, -1.0), WheelWalk::Frame);
    assert_eq!(fired(&s), "Scroll:1,Scroll:-1");
    // Off the frame the notch is the world's.
    assert_eq!(s.mouse_wheel(50.0, 50.0, 1.0), WheelWalk::World);
    assert_eq!(fired(&s), "Scroll:1,Scroll:-1");
}

#[test]
fn a_wheel_frame_without_a_handler_does_not_hand_the_notch_to_an_off_cursor_parent() {
    let mut s = with_world();
    s.run(
        r#"
        local parent = Pane("Parent", 0, 0, 100, 100)
        Takes(parent)
        -- A child that sits outside its parent's rect and takes the wheel with no script.
        local child = Pane("Child", 500, 400, 100, 100, parent)
        child:EnableMouseWheel(true)
        "#,
    )
    .unwrap();
    s.resolve();
    assert_eq!(
        s.mouse_wheel(550.0, 450.0, 1.0),
        WheelWalk::World,
        "the parent is not under the cursor: the walk goes on to the world"
    );
    assert_eq!(fired(&s), "");
}

#[test]
fn a_wheel_frame_without_a_handler_passes_to_the_next_wheel_frame_under_the_cursor() {
    let mut s = with_world();
    s.run(
        r#"
        local below = Pane("Below", 300, 200, 200, 200)
        Takes(below)
        -- Above it (one level up) and under the same point, without a script.
        local above = Pane("Above", 300, 200, 100, 100, below)
        above:EnableMouseWheel(true)
        "#,
    )
    .unwrap();
    s.resolve();
    assert_eq!(s.mouse_wheel(350.0, 250.0, 1.0), WheelWalk::Frame);
    assert_eq!(fired(&s), "Below:1");
}

#[test]
fn the_walk_does_not_stop_at_a_mouse_enabled_frame_without_the_wheel() {
    let mut s = with_world();
    s.run(
        r#"
        Takes(Pane("Behind", 300, 200, 100, 100))
        local b = CreateFrame("Button", "Cover")
        b:SetPoint("BOTTOMLEFT", 300, 200) b:SetWidth(100) b:SetHeight(100)
        b:SetFrameLevel(9)
        b:SetFrameStrata("HIGH")
        "#,
    )
    .unwrap();
    s.resolve();
    assert_eq!(s.mouse_wheel(350.0, 250.0, 1.0), WheelWalk::Frame);
    assert_eq!(fired(&s), "Behind:1");
}

#[test]
fn strata_then_level_then_entry_order_pick_the_frame() {
    let mut s = with_world();
    s.run(
        r#"
        local a = Pane("First", 300, 200, 100, 100) Takes(a)
        local b = Pane("Second", 300, 200, 100, 100) Takes(b)
        "#,
    )
    .unwrap();
    s.resolve();
    s.mouse_wheel(350.0, 250.0, 1.0);
    assert_eq!(
        fired(&s),
        "First:1",
        "equal levels: the frame that entered first"
    );

    // A higher level wins whatever entered first.
    s.run("Second:SetFrameLevel(4)").unwrap();
    s.mouse_wheel(350.0, 250.0, 1.0);
    assert_eq!(fired(&s), "First:1,Second:1");

    // A higher stratum beats any level.
    s.run("First:SetFrameStrata('DIALOG')").unwrap();
    s.mouse_wheel(350.0, 250.0, 1.0);
    assert_eq!(fired(&s), "First:1,Second:1,First:1");
}

#[test]
fn a_bit_set_on_a_shown_frame_enters_behind_the_equals_already_there() {
    let mut s = with_world();
    s.run(
        r#"
        local a = Pane("Early", 300, 200, 100, 100)
        local b = Pane("Late", 300, 200, 100, 100)
        Takes(b)
        Takes(a) -- shown after `b`, but enabled after: it enters behind it
        "#,
    )
    .unwrap();
    s.resolve();
    s.mouse_wheel(350.0, 250.0, 1.0);
    assert_eq!(fired(&s), "Late:1");
}

#[test]
fn a_hidden_frame_is_not_offered_the_notch_until_it_shows() {
    let mut s = with_world();
    s.run(r#"local f = Pane("Gone", 300, 200, 100, 100) Takes(f) f:Hide()"#)
        .unwrap();
    s.resolve();
    assert_eq!(s.mouse_wheel(350.0, 250.0, 1.0), WheelWalk::World);
    s.run("Gone:Show()").unwrap();
    s.resolve();
    assert_eq!(s.mouse_wheel(350.0, 250.0, 1.0), WheelWalk::Frame);
    assert_eq!(fired(&s), "Gone:1");
}

#[test]
fn arg1_is_the_sign_of_the_notch_and_zero_counts_as_up() {
    let mut s = with_world();
    s.run(r#"Takes(Pane("Scroll", 300, 200, 100, 100))"#)
        .unwrap();
    s.resolve();
    for delta in [3.0, -0.25, 0.0] {
        s.mouse_wheel(350.0, 250.0, delta);
    }
    assert_eq!(fired(&s), "Scroll:1,Scroll:-1,Scroll:1");
}

#[test]
fn a_script_on_the_world_frame_takes_the_notch_before_the_binding() {
    let mut s = with_world();
    s.run(r#"Takes(WorldFrame)"#).unwrap();
    s.resolve();
    assert_eq!(s.mouse_wheel(50.0, 50.0, 1.0), WheelWalk::Frame);
    assert_eq!(fired(&s), "WorldFrame:1");
}

#[test]
fn without_a_world_frame_an_unhandled_notch_is_declined() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    assert_eq!(s.mouse_wheel(50.0, 50.0, 1.0), WheelWalk::Declined);
}

#[test]
fn an_xml_on_mouse_wheel_script_joins_the_walk_and_the_scrolling_kinds_are_born_outside_it() {
    let mut s = with_world();
    let doc = crate::framexml::parse(
        r#"<Ui>
            <Frame name="XmlPane"><Size><AbsDimension x="100" y="100"/></Size>
              <Anchors><Anchor point="BOTTOMLEFT"><Offset><AbsDimension x="300" y="200"/></Offset></Anchor></Anchors>
              <Scripts><OnMouseWheel>table.insert(calls, "XmlPane:" .. arg1)</OnMouseWheel></Scripts>
            </Frame>
            <ScrollFrame name="XmlScroll"><Size><AbsDimension x="100" y="100"/></Size>
              <Anchors><Anchor point="BOTTOMLEFT"><Offset><AbsDimension x="500" y="200"/></Offset></Anchor></Anchors>
            </ScrollFrame>
        </Ui>"#,
    )
    .unwrap();
    let report = crate::loader::load(&s, &doc, &|_| None);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    s.resolve();
    assert_eq!(s.mouse_wheel(350.0, 250.0, 1.0), WheelWalk::Frame);
    assert_eq!(fired(&s), "XmlPane:1");
    // A ScrollFrame whose script is set from Lua is not in the list: its constructor takes no
    // wheel (`0x786a40`).
    s.run(
        r#"XmlScroll:SetScript("OnMouseWheel", function() table.insert(calls, "XmlScroll") end)"#,
    )
    .unwrap();
    assert_eq!(s.mouse_wheel(550.0, 250.0, 1.0), WheelWalk::World);
    assert_eq!(fired(&s), "XmlPane:1");
}

#[test]
fn a_world_frame_with_no_binding_passes_the_notch_to_a_wheel_frame_behind_it() {
    let mut s = with_world();
    s.run(
        r#"
        Takes(Pane("Behind", 300, 200, 100, 100))
        "#,
    )
    .unwrap();
    // Lua cannot name the WORLD stratum; put the frame there, entered after the world frame.
    {
        let mut model = s.model_mut();
        let behind = model
            .arena
            .iter_frames()
            .find(|(_, f)| f.name.as_deref() == Some("Behind"))
            .map(|(h, _)| h)
            .unwrap();
        model.arena.frame_mut(behind).unwrap().strata = crate::order::Strata::World;
    }
    s.resolve();
    assert_eq!(
        s.mouse_wheel_bound(350.0, 250.0, 1.0, true),
        WheelWalk::World
    );
    assert_eq!(
        fired(&s),
        "",
        "a bound chord ends the walk at the world frame"
    );
    assert_eq!(
        s.mouse_wheel_bound(350.0, 250.0, 1.0, false),
        WheelWalk::Frame
    );
    assert_eq!(fired(&s), "Behind:1");
    assert_eq!(
        s.mouse_wheel_bound(50.0, 50.0, 1.0, false),
        WheelWalk::Declined,
        "nothing behind it under the cursor"
    );
}
