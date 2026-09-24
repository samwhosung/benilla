//! `GetWeaponEnchantInfo` — the two **temporary weapon enchantment** slots, as virtual buff slots.
//!
//! This is the whole data path behind the reference's `TemporaryEnchantFrame`
//! (`BuffFrame.lua:162-233`): the row polls this verb every frame, shows an icon per enchanted
//! weapon, and counts its expiry down. Eight corpus addons call it as well.
//!
//! # The byte-verified signature (`0x4c9790`)
//!
//! Zero arguments — the function never touches the Lua stack for input; `edi` is the state. **Six**
//! return values on **every** path (`mov eax,0x6` at `0x4c993a`, `0x4c995b` and `0x4c998f`, the
//! three exits), in the order FrameXML destructures them:
//!
//! ```text
//! hasMainHandEnchant, mainHandExpiration, mainHandCharges,
//! hasOffHandEnchant,  offHandExpiration,  offHandCharges
//! ```
//!
//! | value | shape | absent |
//! |---|---|---|
//! | `has*Enchant` | the **number 1** (`push 0x3ff00000; push 0x0` — the double 1.0 — through the number-push at `0x6f3810`, `0x4c97fc`/`0x4c98c3`) | **nil** (`0x6f37f0`, the nil-push, three of them at `0x4c9873` / `0x4c9944`) |
//! | `*Expiration` | a number of **MILLISECONDS remaining** (`0x5d9d00(item, 1)` → `fild` → number-push, `0x4c9828`-`0x4c983f`) | **nil** |
//! | `*Charges` | a number (the enchantment triple's third dword) | the number **0**, never nil (`mov DWORD PTR [ebp-0x4],0x0; fild`, `0x4c985a`) |
//!
//! Milliseconds, not seconds: `ref-BuffFrame.lua:189`/`:212` divide by 1000 before displaying, and
//! `0x5d9d00` is the same live remaining-time reader `benilla::items::Countdowns::enchant_remaining_ms`
//! mirrors. It is *remaining* time, recomputed per call against a deadline — never
//! an absolute stamp.
//!
//! # Which slots, and which enchantment
//!
//! Main hand is fetched first, from **equipment slot 15** (`push 0xf`, `0x4c97c3`), off hand from
//! **16** (`push 0x10`, `0x4c988b`) — the client's 0-based `EQUIPMENT_SLOT_*` numbering, so the
//! live-API `GetInventorySlotInfo` ids one higher (16 `MainHandSlot`, 17 `SecondaryHandSlot`) are
//! what the frame passes to `GetInventoryItemTexture`.
//!
//! Per weapon the binary reads three consecutive dwords off the item's descriptor —
//! `[D+0x4c]` the enchant **id** (`test eax,eax; je` = the entire has-an-enchant gate, `0x4c97f8`),
//! `[D+0x50]` the **duration**, `[D+0x54]` the **charges** — and the `push 0x1` handed to
//! `0x5d9d00` names the enchantment slot: **1 = TEMP**. The identity is read off the client's own
//! field-metadata table, not inferred: `0x83a3c8` = `"ITEM_FIELD_ENCHANTMENT"`, index `0x10`, size
//! `0x15`, with `0x83a2d4`/`0x83a2e8` fixing `D = O + 0x18` — so the triple is fields 25/26/27,
//! slot 1.
//!
//! **The gate is the raw id and nothing else** — no DBC lookup, no display filter. That is why the
//! app feeds this from [`benilla_protocol`]'s `item_enchant(1)` rather than from the tooltip-shaped
//! [`super::EnchantView`] list, which drops an id the `SpellItemEnchantment` catalog cannot name
//! *and* the whole `Flags & 0x2` print-no-line family — the totem weapon imbues,
//! i.e. precisely the enchants this row exists to show.
//!
//! # The expiration is NOT the wire field
//!
//! The trap: `[D+0x50]`, the item's own enchantment **duration** dword, is read *only* as a
//! non-zero presence gate (`0x4c981b test ecx,ecx`) — **its value is never returned**. The number
//! comes from a client-local **absolute deadline** at `item+0x324[slot]`, written only by
//! `0x5d9cc0` out of `SMSG_ITEM_ENCHANT_TIME_UPDATE` as `now_ms + seconds*1000`, and read back as
//! `max(0, deadline − now)` (`0x5d9d00`: `0x5d9d15` clock, `0x5d9d2a sub`). **The wire carries
//! seconds; this API returns milliseconds.** So a host stores a deadline and subtracts on read — it
//! never ticks a countdown. `benilla::items::Countdowns` does exactly that, and
//! `Countdowns::enchant_deadline_ms` is the read that keeps an *elapsed* timer as the number `0` rather
//! than collapsing it to "no timer", which is the distinction `BuffFrame_Enchant_OnUpdate` draws
//! "0 s" from.
//!
//! **Our one named gap.** The reference's nil-expiration branch is `[D+0x50] == 0`; ours is "no
//! deadline was ever parked for this slot". They differ for exactly one state — an enchant whose
//! duration field is set but whose `SMSG_ITEM_ENCHANT_TIME_UPDATE` has not arrived yet, where the
//! reference answers the number `0` and we answer nil. Closing it needs an
//! `item_enchant_duration(slot)` accessor beside `item_enchant`/`item_enchant_charges` in
//! `benilla_protocol` (the triple's middle field, deliberately unexposed there because the tooltip
//! never reads it). The server sends the packet in the same breath as the field, so
//! the window is a frame or two.

use mlua::{Lua, MultiValue, Value};

use super::Model;

/// One weapon's **temporary** enchantment (`ITEM_FIELD_ENCHANTMENT` slot 1), as the app reads it
/// off the equipped item object. Its mere existence is `has*Enchant`: the app only builds one when
/// the enchant id is nonzero, which is the binary's own gate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WeaponEnchant {
    /// Milliseconds left on the enchant — `max(0, deadline − now)` off the parked
    /// `SMSG_ITEM_ENCHANT_TIME_UPDATE` deadline, so **`Some(0)` for an elapsed timer** (the
    /// reference returns the number 0 there, and the row draws "0 s" and pulses). `None` is the
    /// reference's nil expiration: no timer on this enchant at all.
    pub remaining_ms: Option<u64>,
    /// The triple's charges dword — `0` for an enchant that carries none, never absent.
    pub charges: u32,
}

impl super::UiScript {
    /// Push the two weapons' temporary enchantments (`None` = no item, or no enchant on it).
    ///
    /// **Pushed every frame, not on change**: `remaining_ms` is a live countdown, so folding it
    /// into a change-gated snapshot would either fire that snapshot's event every frame or freeze
    /// the timer. The reference recomputes it per call for the same reason.
    pub fn set_weapon_enchants(
        &mut self,
        main_hand: Option<WeaponEnchant>,
        off_hand: Option<WeaponEnchant>,
    ) {
        let mut model = self.model_mut();
        model.weapon_enchants = [main_hand, off_hand];
    }
}

/// Register `GetWeaponEnchantInfo`.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    lua.globals().set(
        "GetWeaponEnchantInfo",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let mut out = Vec::with_capacity(6);
            // Main hand then off hand, three values each — the arity is fixed at six whatever is
            // equipped, so a caller's `local a, b, c, d, e, f = …` never shifts.
            for hand in model.weapon_enchants {
                match hand {
                    Some(e) => {
                        out.push(Value::Integer(1));
                        out.push(
                            e.remaining_ms
                                .map_or(Value::Nil, |ms| Value::Number(ms as f64)),
                        );
                        out.push(Value::Integer(i64::from(e.charges)));
                    }
                    None => out.extend([Value::Nil, Value::Nil, Value::Nil]),
                }
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )
}

#[cfg(test)]
mod tests {
    use crate::script::{UiScript, WeaponEnchant};

    /// **The arity, and the shapes that carry it.** Six values on every path — the reference's
    /// `mov eax,0x6` — with `nil` for an unenchanted weapon and the *number* 1 for an enchanted
    /// one. An implementation that returned `false`/`0`, or that returned early with three values
    /// when the off hand is empty, breaks `BuffFrame_Enchant_OnUpdate`'s single destructuring
    /// assignment and every corpus caller with it.
    #[test]
    fn get_weapon_enchant_info_returns_six_values_on_every_path() {
        let mut s = UiScript::new().unwrap();

        // Nothing equipped or nothing enchanted: six nils, not zero values.
        assert_eq!(s.arity("GetWeaponEnchantInfo()").unwrap(), 6);
        assert!(s
            .eval::<bool>(
                "local a, b, c, d, e, f = GetWeaponEnchantInfo() \
                 return a == nil and b == nil and c == nil and d == nil and e == nil and f == nil"
            )
            .unwrap());
        // The reference's own early-out branch, run for real.
        assert!(s
            .eval::<bool>("local m, _, _, o = GetWeaponEnchantInfo() return (not m) and (not o)")
            .unwrap());

        // Main hand only: an 8-minute Windfury with no charges.
        s.set_weapon_enchants(
            Some(WeaponEnchant {
                remaining_ms: Some(480_000),
                charges: 0,
            }),
            None,
        );
        assert_eq!(
            s.arity("GetWeaponEnchantInfo()").unwrap(),
            6,
            "still six with only one weapon enchanted"
        );
        let (has, expiry, charges) = s
            .eval::<(f64, f64, f64)>("local a, b, c = GetWeaponEnchantInfo() return a, b, c")
            .unwrap();
        assert_eq!(
            (has, expiry, charges),
            (1.0, 480_000.0, 0.0),
            "the number 1, MILLISECONDS, and a zero charge count"
        );
        // `has` is the number 1, not a boolean: `true ~= 1` would invert any `== 1` test.
        assert!(s
            .eval::<bool>("local a = GetWeaponEnchantInfo() return a == 1")
            .unwrap());
        // The off hand is still all nil.
        assert!(s
            .eval::<bool>(
                "local _, _, _, d, e, f = GetWeaponEnchantInfo() \
                 return d == nil and e == nil and f == nil"
            )
            .unwrap());

        // Both hands: a charged poison off hand, no countdown known on it.
        s.set_weapon_enchants(
            Some(WeaponEnchant {
                remaining_ms: Some(1_500),
                charges: 0,
            }),
            Some(WeaponEnchant {
                remaining_ms: None,
                charges: 42,
            }),
        );
        let (d, f) = s
            .eval::<(f64, f64)>("local _, _, _, d, _, f = GetWeaponEnchantInfo() return d, f")
            .unwrap();
        assert_eq!((d, f), (1.0, 42.0));
        assert!(
            s.eval::<bool>("local _, _, _, _, e = GetWeaponEnchantInfo() return e == nil")
                .unwrap(),
            "no parked deadline is a nil expiration, never a fabricated 0"
        );

        // ...and the state that is NOT nil: an elapsed timer. The reference returns the number 0
        // there (`max(0, deadline - now)` on a deadline in the past), and the enchant row's
        // `if ( expiration )` is TRUE for 0 in Lua — so it draws "0 s" and pulses the icon. A host
        // that collapsed "expired" into "no timer" would blank the last second of every enchant.
        s.set_weapon_enchants(
            Some(WeaponEnchant {
                remaining_ms: Some(0),
                charges: 0,
            }),
            None,
        );
        assert!(
            s.eval::<bool>("local _, e = GetWeaponEnchantInfo() return e == 0")
                .unwrap(),
            "an elapsed enchant is the NUMBER 0, not nil"
        );
        assert!(
            s.eval::<bool>("local _, e = GetWeaponEnchantInfo() return (e and true or false)")
                .unwrap(),
            "and 0 is truthy in Lua, which is what makes the row draw it"
        );

        // Zero arguments — and a surplus one is IGNORED, not an error. The reference reads nothing
        // off the stack and has no usage string at all (contrast its neighbour
        // `GetInventoryAlertStatus`, which has both), so an addon that guesses at a slot argument
        // still gets an answer rather than a raised error.
        assert_eq!(
            s.arity("GetWeaponEnchantInfo(16, \"nonsense\")").unwrap(),
            6
        );
    }

    /// The reference's own consumer arithmetic, run over the binding: `expiration/1000` is the
    /// number of seconds the row prints, and the `< BUFF_WARNING_TIME` flash test rides on it.
    /// Milliseconds is the whole point — a seconds return would make a 480 s enchant read as
    /// 0.48 s and flash from the moment it was applied.
    #[test]
    fn the_expiration_is_milliseconds_the_way_the_reference_divides_it() {
        let mut s = UiScript::new().unwrap();
        s.set_weapon_enchants(
            Some(WeaponEnchant {
                remaining_ms: Some(480_000),
                charges: 0,
            }),
            None,
        );
        assert_eq!(
            s.eval::<f64>("local _, e = GetWeaponEnchantInfo() return e / 1000")
                .unwrap(),
            480.0
        );
        assert!(
            s.eval::<bool>("local _, e = GetWeaponEnchantInfo() return (e / 1000) >= 31")
                .unwrap(),
            "eight minutes is nowhere near the 31s warning window"
        );
    }
}
