//! `GetWeaponEnchantInfo` (`0x4c9790`): the two temporary weapon enchantments, which
//! `BuffFrame_Enchant_OnUpdate` (`BuffFrame.lua:162-233`) polls every frame.
//!
//! No arguments; six values on every path (`0x4c993a`, `0x4c995b`, `0x4c998f`), main hand
//! (equipment slot 15, `0x4c97c3`) then off hand (slot 16, `0x4c988b`):
//!
//! | value | present | absent |
//! |---|---|---|
//! | `has*Enchant` | the number 1 (`0x6f3810`) | nil (`0x6f37f0`) |
//! | `*Expiration` | milliseconds remaining (`0x5d9d00`) | nil |
//! | `*Charges` | the charges | 0, never nil (`0x4c985a`) |
//!
//! Each weapon's `ITEM_FIELD_ENCHANTMENT` slot 1 triple is read raw (`0x83a3c8`): a nonzero id is
//! the whole gate (`0x4c97f8`), with no DBC lookup, so the totem imbues, which the tooltip's
//! enchant list drops, still show. The duration dword only gates the expiration (`0x4c981b`); the
//! value is `max(0, deadline - now)` on a deadline `0x5d9cc0` parks from the seconds in
//! `SMSG_ITEM_ENCHANT_TIME_UPDATE`, so the host subtracts on read and an elapsed timer is 0.
//!
//! The reference's expiration is nil only when the duration dword is 0; ours is nil until the
//! first `SMSG_ITEM_ENCHANT_TIME_UPDATE`, which follows the field within a frame or two.

use mlua::{Lua, MultiValue, Value};

use super::Model;

/// One weapon's temporary enchantment (`ITEM_FIELD_ENCHANTMENT` slot 1); the app builds one only
/// for a nonzero enchant id, the reference's gate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WeaponEnchant {
    /// Milliseconds left, `max(0, deadline - now)`: `Some(0)` once elapsed, where the reference
    /// returns 0 and the row draws "0 s"; `None` is no timer at all.
    pub remaining_ms: Option<u64>,
    /// The charges dword, 0 for none.
    pub charges: u32,
}

impl super::UiScript {
    /// Push the two weapons' temporary enchantments, `None` for no item or no enchant. Pushed every
    /// frame: `remaining_ms` is live, and the reference recomputes it per call.
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
            // Main hand then off hand, three values each: always six.
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

    /// `BuffFrame.lua:163` destructures all six at once.
    #[test]
    fn get_weapon_enchant_info_returns_six_values_on_every_path() {
        let mut s = UiScript::new().unwrap();

        // Nothing enchanted: six nils, not zero values.
        assert_eq!(s.arity("GetWeaponEnchantInfo()").unwrap(), 6);
        assert!(s
            .eval::<bool>(
                "local a, b, c, d, e, f = GetWeaponEnchantInfo() \
                 return a == nil and b == nil and c == nil and d == nil and e == nil and f == nil"
            )
            .unwrap());
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
        // The number 1, not a boolean: `true ~= 1`.
        assert!(s
            .eval::<bool>("local a = GetWeaponEnchantInfo() return a == 1")
            .unwrap());
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

        // An elapsed timer is the number 0, which the row's `if ( expiration )` takes as true.
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

        // The reference reads no arguments and has no usage string, so a surplus one is ignored.
        assert_eq!(
            s.arity("GetWeaponEnchantInfo(16, \"nonsense\")").unwrap(),
            6
        );
    }

    /// `BuffFrame.lua:212` divides by 1000 before its `BUFF_WARNING_TIME` test.
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
