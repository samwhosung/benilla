//! The world frame's right press, as its hook (`0x492c20`) runs it.

use super::*;
use benilla_ui::script::UiScript;

/// A right mouse-down in the world ends repair mode (`0x492c68`); the same press on a UI frame,
/// or one that drops a held payload (`0x492b50`), leaves it standing.
#[test]
fn a_right_press_in_the_world_ends_repair_mode_and_one_on_the_ui_does_not() {
    use benilla_ui::script::MerchantState;

    // One right press with repair mode armed; returns whether repair mode survived it.
    let press = |over_ui: bool, payload_held: bool| {
        let mut world = World::new();
        world.insert_resource(State::new(crate::client_state::ClientState::InWorld));
        world.insert_resource(crate::ui_script::CursorPayloadHeld(payload_held));
        world.init_resource::<Messages<WorldRightPress>>();
        world.init_resource::<crate::spell::targeting::SpellTargeting>();
        let mut script = UiScript::new().unwrap();
        script.set_merchant(Some(MerchantState {
            can_repair: true,
            ..MerchantState::default()
        }));
        script.run("ShowRepairCursor()").unwrap();
        assert!(script.repair_mode(), "the repair vendor arms repair mode");
        world.insert_non_send_resource(script);
        if world_press(&world, over_ui) {
            world.write_message(WorldRightPress);
            run(
                &mut world,
                crate::ui_merchant::end_repair_mode_on_right_press,
            );
        }
        world.non_send_resource::<UiScript>().repair_mode()
    };

    assert!(
        !press(false, false),
        "a right press in the world ends repair mode"
    );
    assert!(
        press(true, false),
        "a right press on a UI frame never reaches the world's hook"
    );
    assert!(
        press(false, true),
        "a press while a payload is held skips the repair reset"
    );
}
