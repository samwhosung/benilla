//! `GetActionBarToggles` / `SetActionBarToggles` — the four extra action bars' visibility, as one
//! server-owned nibble, byte-verified against the 1.12.1 binary.
//!
//! ## The shape, and why it is not the shape it looks like
//!
//! Four booleans in, four booleans out — but **the getter and the setter do not talk to each
//! other**. `SetActionBarToggles 0x4e76e0` packs its four arguments into a stack byte, posts
//! `CMSG_SET_ACTIONBAR_TOGGLES` and returns zero values; every store in its body is into its own
//! `ebp` frame. `GetActionBarToggles 0x4e7660` reads the live descriptor at
//! `[[player+0xe68]+0x102a]` — `PLAYER_FIELD_BYTES` byte 2 — on every call, with no cache. The
//! only writer of that cell in the whole image is the generic `SMSG_UPDATE_OBJECT` value-apply
//! (`0x466590`), and **nothing is notified when it lands**: all 49 field-change
//! registrations at `0x468070` were enumerated and none sits at an offset ≥ `0x1000`.
//!
//! So `SetActionBarToggles(1)` followed immediately by `GetActionBarToggles()` returns the **old**
//! value, for a whole round trip. That is not a bug to paper over — it is the mechanism, and it is
//! why [`super::UiScript::set_action_bar_toggles`] (the app's descriptor push) is the *only* thing
//! that moves our copy. Contrast [`super::worn_display`], whose setter updates its belief
//! optimistically because its wire verb is a blind flip that would otherwise mis-compute.
//!
//! The reference UI does not feel the lag because it never asks: it keeps `SHOW_MULTI_ACTIONBAR_1..4`
//! as Lua globals, updates them on the checkbox click, and reads the binding exactly once — in
//! `UIParent.lua`'s `PLAYER_ENTERING_WORLD` handler. Our Lua layer copies that split.
//!
//! ## Four bits, and only four
//!
//! The setter's loop runs `i = 0..3` (`0x4e770e cmp esi,4`) and ORs `1 << i` for argument `i + 1`;
//! the accumulator is written by exactly two instructions image-wide — the `mov BYTE [ebp-0x4],0`
//! that zeroes it (`0x4e76eb`) and the loop's `or` (`0x4e7709`). Two consequences we reproduce
//! exactly:
//!
//! - A **fifth argument is silently dropped**. The shipped `UIOptionsFrame_Save` passes
//!   `ALWAYS_SHOW_MULTIBARS` as a fifth; the binding never fetches it, so it never reaches the byte
//!   or the wire (and, consistently, it is the one option of the five that FrameXML saves locally).
//! - A `Set` **destroys the high nibble**. It starts from zero, so whatever `0x10..0x80` the server
//!   happened to hold is overwritten with 0 by the next post. The getter never tests those bits
//!   either, so the client's view of this field is genuinely 4-bit.
//!
//! The bit→bar meaning is a FrameXML convention, not the binary's — the engine stores four
//! unnamed bits and the names live in the UI layer.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::bool_or_default;
use super::Model;

/// How many toggle bits the bindings read and write — `0x4e770e cmp esi,4` in the setter,
/// `0x4e76cd mov eax,4` in the getter. Not a UI choice: four is what the binary does, in both
/// directions, and bits above it are unreachable from Lua.
const ACTION_BAR_TOGGLE_BITS: u32 = 4;

impl super::UiScript {
    /// Push the server's `PLAYER_FIELD_BYTES` byte 2 — the app calls this on the descriptor edge,
    /// and it is the **only** way this value moves (see the module doc). Mirrors
    /// [`Self::set_combo_points`], the other PRIVATE byte out of the same dword.
    pub fn set_action_bar_toggles(&mut self, toggles: u8) {
        self.model_mut().action_bar_toggles = Some(toggles);
    }

    /// What the VM last had pushed into it, or `None` if nothing ever has — the app's own read,
    /// and the tests'. `None` and `Some(0)` are indistinguishable to Lua by design: with no local
    /// player the reference's chain fails soft and the getter returns four `nil`s, which is exactly
    /// what a zero byte returns (`0x4e7660`).
    pub fn action_bar_toggles(&self) -> Option<u8> {
        self.model_ref().action_bar_toggles
    }

    /// Drain the `CMSG_SET_ACTIONBAR_TOGGLES` payloads queued since the last call — one per
    /// `SetActionBarToggles` call, in order.
    ///
    /// A list rather than a "latest wins" slot because the binding gates **nothing**: it has no
    /// did-it-change test (unlike `ShowHelm`/`ShowCloak`) and no connection check of its own, so
    /// two calls in a frame are two packets. Dropping one here would be an optimisation the real
    /// client does not make.
    pub fn take_action_bar_toggle_sends(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.model_mut().action_bar_toggle_sends)
    }
}

/// Register the two action-bar-toggle globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetActionBarToggles() -> four values, each the NUMBER 1 or nil (`0x6f3810` pushes tag 3 with
    // the double 1.0; `0x6f37f0` pushes tag 0). Never booleans — callers write `if toggle then`,
    // and the WoW `1`-or-`nil` idiom is what the reference's own `SHOW_MULTI_ACTIONBAR_n` globals
    // then hold. Return N tests bit `1 << (N-1)`, the exact inverse of the setter's map.
    g.set(
        "GetActionBarToggles",
        lua.create_function(|lua, ()| {
            let byte = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model.action_bar_toggles.unwrap_or(0)
            };
            let bit = |i: u32| {
                if byte & (1 << i) != 0 {
                    Value::Number(1.0)
                } else {
                    Value::Nil
                }
            };
            Ok((bit(0), bit(1), bit(2), bit(3)))
        })?,
    )?;

    // SetActionBarToggles(a, b, c, d) -> nothing. Four argument slots, no more: a fifth is never
    // fetched (`0x4e76e0`), which matters because the shipped Options panel passes one.
    //
    // The arguments are read through `0x6f1c10`, NOT Lua truthiness ([`bool_or_default`]) — the
    // panel that calls this hands option bindings the strings "0"/"1", and `"0"` is truthy in Lua.
    // The default at this call site is FALSE (`0x4e76f4 push 0x0`), so an omitted argument is off.
    //
    // Nothing local is written: the byte we hold is the server's, and only the server's push moves
    // it (module doc). A `Set` that never reaches the server therefore leaves the UI showing the
    // truth rather than a lie.
    g.set(
        "SetActionBarToggles",
        lua.create_function(|lua, args: MultiValue| {
            let args: Vec<Value> = args.into_iter().collect();
            let mut packed = 0u8;
            for i in 0..ACTION_BAR_TOGGLE_BITS {
                if bool_or_default(args.get(i as usize), false) {
                    packed |= 1 << i;
                }
            }
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.action_bar_toggle_sends.push(packed);
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::UiScript;

    /// The argument→bit map, read straight off `0x4e7703 shl al,cl` with `cl = argIndex − 1`.
    #[test]
    fn the_four_arguments_pack_into_bits_0_through_3() {
        let mut s = UiScript::new().unwrap();
        for (call, want) in [
            ("SetActionBarToggles(1)", 0x01),
            ("SetActionBarToggles(nil, 1)", 0x02),
            ("SetActionBarToggles(nil, nil, 1)", 0x04),
            ("SetActionBarToggles(nil, nil, nil, 1)", 0x08),
            ("SetActionBarToggles(1, 1, 1, 1)", 0x0f),
            ("SetActionBarToggles()", 0x00),
        ] {
            s.run(call).unwrap();
            assert_eq!(
                s.take_action_bar_toggle_sends(),
                vec![want],
                "{call} packs {want:#04x}"
            );
        }
    }

    /// The coercion is `0x6f1c10`, not `if arg then` — and the case that proves it is the one the
    /// reference's own Options panel produces. `"0"` is truthy in Lua 5.0/5.1; here it is OFF.
    #[test]
    fn the_arguments_are_coerced_the_binarys_way_not_luas() {
        let mut s = UiScript::new().unwrap();

        s.run(r#"SetActionBarToggles("1", "0", "1", "0")"#).unwrap();
        assert_eq!(
            s.take_action_bar_toggle_sends(),
            vec![0x05],
            r#"the strings the panel passes: "0" is FALSE, where Lua truthiness says true"#
        );

        // The number arm TRUNCATES toward zero (`0x40a2b0`, RC = chop) — it does not round.
        s.run("SetActionBarToggles(0.5, -0.9, 1.5, 0)").unwrap();
        assert_eq!(
            s.take_action_bar_toggle_sends(),
            vec![0x04],
            "fractions inside (-1, 1) truncate to 0 and read false; 1.5 truncates to 1"
        );

        // The keyword and letter arms.
        s.run(r#"SetActionBarToggles("on", "off", "enabled", "disabled")"#)
            .unwrap();
        assert_eq!(s.take_action_bar_toggle_sends(), vec![0x05]);
        s.run(r#"SetActionBarToggles("true", "false", "yes", "no")"#)
            .unwrap();
        assert_eq!(s.take_action_bar_toggle_sends(), vec![0x05]);

        // Booleans, the spelling our own Lua uses.
        s.run("SetActionBarToggles(true, false, true, false)")
            .unwrap();
        assert_eq!(s.take_action_bar_toggle_sends(), vec![0x05]);

        // The default arm (`push 0x0` at this call site) — a table, and an unrecognised string.
        s.run(r#"SetActionBarToggles({}, "maybe", 1, 1)"#).unwrap();
        assert_eq!(
            s.take_action_bar_toggle_sends(),
            vec![0x0c],
            "both take the default, which is FALSE here"
        );
    }

    /// The shipped `UIOptionsFrame_Save` calls this with **five** arguments
    /// (`…, ALWAYS_SHOW_MULTIBARS`). The binding's loop stops at four and never fetches the fifth,
    /// so it reaches neither the byte nor the wire — the reason that option is the only one of the
    /// five FrameXML saves locally.
    #[test]
    fn a_fifth_argument_is_accepted_and_dropped() {
        let mut s = UiScript::new().unwrap();
        s.run("SetActionBarToggles(nil, nil, nil, nil, 1)").unwrap();
        assert_eq!(s.take_action_bar_toggle_sends(), vec![0x00]);
        s.run("SetActionBarToggles(1, 1, 1, 1, 1, 1, 1)").unwrap();
        assert_eq!(
            s.take_action_bar_toggle_sends(),
            vec![0x0f],
            "still four bits — the accumulator can only ever hold 0x00..0x0f"
        );
    }

    /// A `Set` starts from **zero** and ORs at most four bits, so any high nibble the server held
    /// is destroyed by the next post. Faithful, not tidy: it is what two accumulator writes
    /// image-wide amount to.
    #[test]
    fn a_set_destroys_whatever_the_server_held_above_the_low_nibble() {
        let mut s = UiScript::new().unwrap();
        s.set_action_bar_toggles(0xf5);
        s.run("SetActionBarToggles(1, 0, 0, 0)").unwrap();
        assert_eq!(
            s.take_action_bar_toggle_sends(),
            vec![0x01],
            "0xf5's high nibble is gone — the accumulator never saw it"
        );
    }

    /// The getter reads the **app-pushed descriptor byte**, tests only bits 0..3, and returns the
    /// NUMBER 1 or nil — never a boolean, never `0`.
    #[test]
    fn the_getter_returns_four_ones_or_nils_from_the_pushed_byte() {
        let mut s = UiScript::new().unwrap();

        // Nothing pushed: four nils, and no error. Indistinguishable from a zero byte, exactly as
        // "no local player" is in the reference (`0x4e7660`).
        assert!(s
            .eval::<bool>(
                "local a,b,c,d = GetActionBarToggles() \
                 return a == nil and b == nil and c == nil and d == nil"
            )
            .unwrap());
        assert_eq!(s.arity("GetActionBarToggles()").unwrap(), 4);

        s.set_action_bar_toggles(0x0a);
        assert!(s
            .eval::<bool>(
                "local a,b,c,d = GetActionBarToggles() \
                 return a == nil and b == 1 and c == nil and d == 1"
            )
            .unwrap());
        assert!(
            s.eval::<bool>("local a,b = GetActionBarToggles() return type(b) == 'number'")
                .unwrap(),
            "the NUMBER 1 (0x6f3810 pushes tag 3 / double 1.0), never the boolean true"
        );

        // Bits 0x10..0x80 are loaded but never tested — a byte of 0xf0 reads as all four off.
        s.set_action_bar_toggles(0xf0);
        assert!(s
            .eval::<bool>(
                "local a,b,c,d = GetActionBarToggles() \
                 return a == nil and b == nil and c == nil and d == nil"
            )
            .unwrap());
    }

    /// Round trip: what the setter packs is what the getter reads back — but only once the value
    /// has travelled through the app's push, because the setter deliberately does not touch the
    /// local copy. The stale read in the middle is the mechanism, not an oversight (`0x466590`,
    /// the only writer).
    #[test]
    fn the_setter_does_not_touch_the_local_copy_so_the_read_lags_a_round_trip() {
        let mut s = UiScript::new().unwrap();
        s.set_action_bar_toggles(0x00);
        s.run("SetActionBarToggles(1, 1, 0, 0)").unwrap();
        assert_eq!(
            s.action_bar_toggles(),
            Some(0x00),
            "still the server's value — the setter wrote nothing local"
        );
        assert!(s
            .eval::<bool>("local a = GetActionBarToggles() return a == nil")
            .unwrap());

        let sent = s.take_action_bar_toggle_sends();
        assert_eq!(sent, vec![0x03]);
        // The server echoes it back through UPDATE_OBJECT; now the getter agrees.
        s.set_action_bar_toggles(sent[0]);
        assert!(s
            .eval::<bool>(
                "local a,b,c,d = GetActionBarToggles() \
                 return a == 1 and b == 1 and c == nil and d == nil"
            )
            .unwrap());
    }

    /// No did-it-change gate and no connection test: two calls in a frame are two packets, in
    /// order. The binding returns zero Lua values either way, so nothing upstream can tell.
    #[test]
    fn every_call_queues_a_packet_and_returns_nothing() {
        let mut s = UiScript::new().unwrap();
        s.run("SetActionBarToggles(1) SetActionBarToggles(1) SetActionBarToggles(nil)")
            .unwrap();
        assert_eq!(s.take_action_bar_toggle_sends(), vec![0x01, 0x01, 0x00]);
        assert_eq!(s.arity("SetActionBarToggles(1)").unwrap(), 0);
        assert_eq!(s.take_action_bar_toggle_sends(), vec![0x01]);
    }
}
