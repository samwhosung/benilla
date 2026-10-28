//! The world frame's right press, as its hook (`0x492c20`) runs it.

use super::*;
use benilla_ui::script::UiScript;

/// A right mouse-down the world frame takes runs the hook's legs whatever the cursor holds: repair
/// mode ends (`0x492c68`) and a pending gift wrap clears (`0x492c44`, `0x492c54`). The same press
/// on a UI frame never reaches the hook.
#[test]
fn a_right_press_in_the_world_ends_repair_mode_and_a_wrap_and_one_on_the_ui_does_not() {
    use benilla_ui::script::MerchantState;

    // One right press with repair mode and a wrap armed; returns whether each survived it.
    let press = |over_ui: bool, payload_held: bool| {
        let mut world = World::new();
        world.insert_resource(State::new(crate::client_state::ClientState::InWorld));
        world.init_resource::<crate::spell::targeting::SpellTargeting>();
        let mut script = UiScript::new().unwrap();
        script.set_merchant(Some(MerchantState {
            can_repair: true,
            ..MerchantState::default()
        }));
        script.run("ShowRepairCursor()").unwrap();
        assert!(script.repair_mode(), "the repair vendor arms repair mode");
        script.arm_gift_wrap(0, 6);
        assert!(script.gift_wrap_armed().is_some(), "the paper arms a wrap");
        if payload_held {
            use benilla_ui::script::{CursorMoney, CursorPayload, HeldCursor};
            script.seed_held_cursor(HeldCursor::holding(CursorPayload::Money(CursorMoney {
                copper: 1,
            })));
        }
        world.insert_non_send_resource(script);
        right_press_hook(&mut world, over_ui);
        let script = world.non_send_resource::<UiScript>();
        (script.repair_mode(), script.gift_wrap_armed().is_some())
    };

    assert_eq!(
        press(false, false),
        (false, false),
        "a right press in the world ends repair mode and the wrap"
    );
    assert_eq!(
        press(true, false),
        (true, true),
        "a right press on a UI frame never reaches the world's hook"
    );
    assert_eq!(
        press(false, true),
        (false, false),
        "the hook tests no payload"
    );
}
