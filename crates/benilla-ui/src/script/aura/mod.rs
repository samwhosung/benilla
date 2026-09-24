//! The aura bindings, over the per-token lists the app pushes through [`UiScript::set_auras`] in
//! display order: the player's in the reference's insertion-ordered cache (`0xbc6040`), kept by
//! `benilla::ui_aura`, any other unit's by ascending aura slot. The player's list keeps cache order
//! under every token, where the reference's `UnitBuff("player", i)` reads by slot.
//!
//! `UnitAura`, `UnitBuff`, `UnitDebuff` and `CancelUnitBuff` take a token and a 1-based index into
//! the sign-filtered list; the getters answer nil past the end. The 1.12 `GetPlayerBuff`
//! ([`player_buff`]) takes a 0-based index and returns a cache position, the same number under
//! every filter, which its siblings and `GameTooltip:SetPlayerBuff` take in place of the counter.
//!
//! `source` is always nil, and only the player's auras carry a duration: the 1.12 wire has no aura
//! caster and sends durations for the player alone, so the stock target frame shows no timers.

use mlua::{Lua, MultiValue, Value};

use super::Model;

mod player_buff;

/// One aura on one unit, as the app's [`crate::script::UiScript::set_auras`] feed pushes it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AuraState {
    /// `Spell.dbc` id, `UnitAura`'s `spellId`.
    pub spell_id: u32,
    /// The spell's name; `None` when the catalog lacks the id.
    pub name: Option<String>,
    /// The icon's extensionless MPQ path (`Interface\Icons\…`).
    pub icon: Option<String>,
    /// Stack count, at least 1; the reference shows it only above 1.
    pub count: u8,
    /// `"Magic"`, `"Curse"`, `"Disease"`, `"Poison"` or `None`; the debuff border tints by it.
    pub debuff_type: Option<String>,
    /// Seconds, from the last apply or refresh; 0 when permanent or on a unit not the player.
    pub duration: f64,
    /// The `GetTime()` instant it runs out; 0 when [`Self::duration`] is 0.
    pub expiration_time: f64,
    /// A buff rather than a debuff: the `HELPFUL`/`HARMFUL` filter's test.
    pub helpful: bool,
    /// The wire's `AFLAG_CANCELABLE`: the `CANCELABLE`/`NOT_CANCELABLE` filter's test.
    pub cancelable: bool,
    /// `GetPlayerBuff`'s second return, cache record `+0xc`: no finite duration to show. Derived
    /// from the spell (`0x4e452e`-`0x4e45c5`), not from `expiration_time`, so it holds before any
    /// `SMSG_UPDATE_AURA_DURATION` and a permanent aura never shows a `0 s` timer.
    pub until_cancelled: bool,
    /// `AttributesEx & 0x4` (`SPELL_ATTR_EX_IS_CHANNELED`), [`cancel_authorized`]'s second arm.
    pub channeled: bool,
}

/// The player's active tracking aura, the reference's tracking global (`0xbc6378`): the cache
/// rebuild records a spell with a tracking effect (`0x2c`, `0x2d`, `0x97`) there instead of in
/// the display cache, and `GetTrackingTexture` (`0x4e4a20`) reads it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrackingState {
    /// `Spell.dbc` id, the `CMSG_CANCEL_AURA` payload of `CancelTrackingBuff`.
    pub spell_id: u32,
    /// The spell's name; `None` when the catalog lacks the id.
    pub name: Option<String>,
    /// The icon's extensionless MPQ path, `GetTrackingTexture`'s return.
    pub icon: Option<String>,
    /// The wire's `AFLAG_CANCELABLE` on the aura's slot, `CancelTrackingBuff`'s gate.
    pub cancelable: bool,
}

/// `UnitAura`'s `filter`: `|`-separated tokens, the sign defaulting to `HELPFUL` as in the Era API.
struct Filter {
    helpful: bool,
    cancelable: Option<bool>,
}

impl Filter {
    fn parse(spec: Option<&str>) -> Self {
        let spec = spec.unwrap_or("");
        let has = |t: &str| spec.split('|').any(|s| s.trim().eq_ignore_ascii_case(t));
        Self {
            helpful: !has("HARMFUL"),
            cancelable: match (has("CANCELABLE"), has("NOT_CANCELABLE")) {
                (true, false) => Some(true),
                (false, true) => Some(false),
                // Both or neither: no constraint; the reference never passes both.
                _ => None,
            },
        }
    }

    fn matches(&self, a: &AuraState) -> bool {
        a.helpful == self.helpful && self.cancelable.is_none_or(|c| a.cancelable == c)
    }
}

/// The cancel gate of `CancelUnitBuff` and `CancelPlayerBuff` (`0x4e49a0`, `0x4e49fb`-`0x4e4a14`):
/// a helpful aura with `AFLAG_CANCELABLE` (vmangos: unless `SPELL_ATTR_NO_AURA_CANCEL`), or any
/// aura of a channeled spell, the only negative aura that cancels (breaking a channel on you).
pub(super) fn cancel_authorized(a: &AuraState) -> bool {
    (a.helpful && a.cancelable) || a.channeled
}

/// The 1.12 tuple, not a prefix of the Era one: `UnitBuff` returns `(texture, applications)`
/// (`0x519500`) and `UnitDebuff` adds `dispelType` (`0x5198f0`), as stock `TargetFrame.lua:287-290`
/// reads them.
fn returns_1121(lua: &Lua, a: &AuraState, with_dispel_type: bool) -> mlua::Result<MultiValue> {
    let icon = match &a.icon {
        Some(t) => Value::String(lua.create_string(t)?),
        None => Value::Nil,
    };
    let mut out = vec![icon, Value::Integer(i64::from(a.count))];
    if with_dispel_type {
        out.push(match &a.debuff_type {
            Some(t) => Value::String(lua.create_string(t)?),
            None => Value::Nil,
        });
    }
    Ok(MultiValue::from_vec(out))
}

/// Which return tuple a getter pushes.
#[derive(Clone, Copy)]
enum Shape {
    /// `UnitAura`'s ten values, the Era signature.
    Era,
    /// `UnitBuff`'s two.
    Buff,
    /// `UnitDebuff`'s three.
    Debuff,
}

fn returns(lua: &Lua, a: &AuraState) -> mlua::Result<MultiValue> {
    let s = |v: &Option<String>| -> mlua::Result<Value> {
        Ok(match v {
            Some(t) => Value::String(lua.create_string(t)?),
            None => Value::Nil,
        })
    };
    Ok(MultiValue::from_vec(vec![
        s(&a.name)?,
        s(&a.icon)?,
        Value::Integer(i64::from(a.count)),
        s(&a.debuff_type)?,
        Value::Number(a.duration),
        Value::Number(a.expiration_time),
        Value::Nil, // source: the 1.12 wire carries no aura caster
        Value::Nil, // isStealable: Spellsteal is TBC
        Value::Nil, // nameplateShowPersonal
        Value::Integer(i64::from(a.spell_id)),
    ]))
}

/// The `index`-th (1-based) aura of `token` passing `filter`, in pushed order; out of range is a
/// bare nil, the loop terminator.
fn nth_aura(
    lua: &Lua,
    token: &Option<String>,
    index: i64,
    filter: &Filter,
    shape: Shape,
) -> mlua::Result<MultiValue> {
    if index < 1 {
        return Ok(MultiValue::new());
    }
    let hit = {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        token
            .as_ref()
            .and_then(|t| model.auras.get(t))
            .and_then(|list| {
                list.iter()
                    .filter(|a| filter.matches(a))
                    .nth((index - 1) as usize)
                    .cloned()
            })
    };
    match hit {
        Some(a) => match shape {
            Shape::Era => returns(lua, &a),
            Shape::Buff => returns_1121(lua, &a, false),
            Shape::Debuff => returns_1121(lua, &a, true),
        },
        None => Ok(MultiValue::new()),
    }
}

impl super::UiScript {
    /// Push or clear a token's aura list, in display order: cache order for the player, ascending
    /// aura slot for anyone else.
    pub fn set_auras(&mut self, token: &str, auras: Option<Vec<AuraState>>) {
        let mut model = self.model_mut();
        match auras {
            Some(a) => {
                model.auras.insert(token.to_string(), a);
            }
            None => {
                model.auras.remove(token);
            }
        }
    }

    /// Push or clear the player's active [`TrackingState`].
    pub fn set_tracking(&mut self, tracking: Option<TrackingState>) {
        self.model_mut().tracking = tracking;
    }

    /// Drain the queued cancels, one `CMSG_CANCEL_AURA` per spell id: the server cancels by spell,
    /// never by slot.
    pub fn take_cancel_aura_requests(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().cancel_aura_requests)
    }
}

/// Register the aura and tracking globals, the `GetPlayerBuff` family from [`player_buff`].
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // UnitAura(unit, index [, filter]), filter defaulting to HELPFUL, with the Era tuple: not a
    // 1.12 verb.
    g.set(
        "UnitAura",
        lua.create_function(
            |lua, (token, index, filter): (Option<String>, i64, Option<String>)| {
                nth_aura(
                    lua,
                    &token,
                    index,
                    &Filter::parse(filter.as_deref()),
                    Shape::Era,
                )
            },
        )?,
    )?;

    // UnitBuff(unit, index [, raidFilter]) and UnitDebuff: the verb fixes the sign, `0x519500`
    // reading aura slots 0..31 and `0x5198f0` slots 32..47. A non-zero `raidFilter` keeps only
    // buffs the player can cast (`0x4b3870`) or debuffs they can dispel (`0x4b3920`); it is
    // accepted, not applied, as `AuraState` holds neither fact, so the pet frame's buff row and the
    // dispellable-debuff rows show every aura.
    g.set(
        "UnitBuff",
        lua.create_function(
            |lua, (token, index, _raid_filter): (Option<String>, i64, Option<Value>)| {
                nth_aura(
                    lua,
                    &token,
                    index,
                    &Filter::parse(Some("HELPFUL")),
                    Shape::Buff,
                )
            },
        )?,
    )?;
    g.set(
        "UnitDebuff",
        lua.create_function(
            |lua, (token, index, _raid_filter): (Option<String>, i64, Option<Value>)| {
                nth_aura(
                    lua,
                    &token,
                    index,
                    &Filter::parse(Some("HARMFUL")),
                    Shape::Debuff,
                )
            },
        )?,
    )?;

    // CancelUnitBuff(unit, index [, filter]): not a 1.12 verb, the Era name for the reference's
    // `CancelPlayerBuff`. It queues the aura's spell id, what `CMSG_CANCEL_AURA` carries; an aura
    // the gate refuses is a silent no-op, as in the reference.
    g.set(
        "CancelUnitBuff",
        lua.create_function(
            |lua, (token, index, filter): (Option<String>, i64, Option<String>)| {
                let spec = match filter {
                    Some(f) => format!("HELPFUL|{f}"),
                    None => "HELPFUL".to_string(),
                };
                let f = Filter::parse(Some(&spec));
                let hit = {
                    let model = lua.app_data_ref::<Model>().expect("model app_data");
                    token
                        .as_ref()
                        .filter(|_| index >= 1)
                        .and_then(|t| model.auras.get(t))
                        .and_then(|list| {
                            list.iter()
                                .filter(|a| f.matches(a))
                                .nth((index - 1) as usize)
                                .cloned()
                        })
                };
                if let Some(a) = hit.filter(cancel_authorized) {
                    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                    model.cancel_aura_requests.push(a.spell_id);
                }
                Ok(())
            },
        )?,
    )?;

    player_buff::install(lua)?;

    // GetTrackingTexture() (`0x4e4a20`): the tracking spell's icon or nil, which shows or hides
    // the stock `MiniMapTrackingFrame` (`Minimap.xml:150-159`).
    g.set(
        "GetTrackingTexture",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.tracking.as_ref().and_then(|t| t.icon.clone()))
        })?,
    )?;

    // CancelTrackingBuff() (`0x4e4a80`), the tracking frame's right-click: queues the tracking
    // spell's id behind its `AFLAG_CANCELABLE`; with no tracking it is a no-op.
    g.set(
        "CancelTrackingBuff",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            if let Some(t) = model.tracking.as_ref().filter(|t| t.cancelable) {
                let id = t.spell_id;
                model.cancel_aura_requests.push(id);
            }
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::{AuraState, TrackingState, UiScript};

    fn aura(spell_id: u32, name: &str, helpful: bool, cancelable: bool) -> AuraState {
        AuraState {
            spell_id,
            name: Some(name.into()),
            icon: Some(format!("Interface\\Icons\\Spell_{spell_id}")),
            count: 1,
            debuff_type: None,
            duration: 0.0,
            expiration_time: 0.0,
            helpful,
            cancelable,
            until_cancelled: false,
            channeled: false,
        }
    }

    #[test]
    fn unit_aura_enumerates_the_pushed_order_not_the_spell_id_order() {
        let mut s = UiScript::new().unwrap();
        s.set_auras(
            "player",
            Some(vec![
                aura(2457, "Battle Stance", true, true),
                aura(1126, "Mark of the Wild", true, true),
                aura(589, "Shadow Word: Pain", false, false),
            ]),
        );
        assert_eq!(
            s.eval::<String>(r#"return (UnitAura("player", 1))"#)
                .unwrap(),
            "Battle Stance"
        );
        // `UnitBuff` and `UnitDebuff` return the icon first, the 1.12 shape.
        assert_eq!(
            s.eval::<String>(r#"return (UnitBuff("player", 2))"#)
                .unwrap(),
            "Interface\\Icons\\Spell_1126"
        );
        // The debuff is index 1 of its own filter, not index 3.
        assert_eq!(
            s.eval::<String>(r#"return (UnitDebuff("player", 1))"#)
                .unwrap(),
            "Interface\\Icons\\Spell_589"
        );
        assert!(s
            .eval::<bool>(r#"return UnitBuff("player", 3) == nil"#)
            .unwrap());
        assert!(s
            .eval::<bool>(r#"return UnitDebuff("player", 2) == nil"#)
            .unwrap());
        assert!(s
            .eval::<bool>(r#"return UnitAura("target", 1) == nil"#)
            .unwrap());
        assert!(s
            .eval::<bool>(r#"return UnitAura("player", 0) == nil"#)
            .unwrap());
    }

    #[test]
    fn unit_aura_defaults_to_helpful_and_honours_the_cancelable_tokens() {
        let mut s = UiScript::new().unwrap();
        s.set_auras(
            "player",
            Some(vec![
                aura(2457, "Battle Stance", true, true),
                aura(9999, "Sealed", true, false), // helpful, not cancelable
                aura(589, "Pain", false, false),
            ]),
        );
        // No filter means HELPFUL.
        assert_eq!(
            s.eval::<i64>(
                r#"local n = 0 for i=1,10 do if UnitAura("player", i) then n = n + 1 end end return n"#
            )
            .unwrap(),
            2
        );
        assert_eq!(
            s.eval::<String>(r#"return (UnitAura("player", 1, "HARMFUL"))"#)
                .unwrap(),
            "Pain"
        );
        assert_eq!(
            s.eval::<String>(r#"return (UnitAura("player", 1, "HELPFUL|CANCELABLE"))"#)
                .unwrap(),
            "Battle Stance"
        );
        assert_eq!(
            s.eval::<String>(r#"return (UnitAura("player", 1, "HELPFUL|NOT_CANCELABLE"))"#)
                .unwrap(),
            "Sealed"
        );
        // A bare CANCELABLE keeps the helpful default.
        assert_eq!(
            s.eval::<String>(r#"return (UnitAura("player", 1, "CANCELABLE"))"#)
                .unwrap(),
            "Battle Stance"
        );
    }

    #[test]
    fn unit_aura_returns_the_era_tuple_with_the_unknowable_fields_nil() {
        let mut s = UiScript::new().unwrap();
        let mut a = aura(589, "Shadow Word: Pain", false, false);
        a.count = 3;
        a.debuff_type = Some("Magic".into());
        a.duration = 18.0;
        a.expiration_time = 1042.5;
        s.set_auras("target", Some(vec![a]));

        let (name, icon, count, dtype, dur, expiry, spell) = s
            .eval::<(String, String, i64, String, f64, f64, i64)>(
                r#"local n, i, c, d, du, e, src, st, np, sid = UnitAura("target", 1, "HARMFUL")
                   assert(src == nil and st == nil and np == nil, "unknowable fields must be nil")
                   return n, i, c, d, du, e, sid"#,
            )
            .unwrap();
        assert_eq!(name, "Shadow Word: Pain");
        assert_eq!(icon, "Interface\\Icons\\Spell_589");
        assert_eq!((count, dtype.as_str()), (3, "Magic"));
        assert_eq!((dur, expiry), (18.0, 1042.5));
        assert_eq!(spell, 589);
    }

    #[test]
    fn unit_buff_and_unit_debuff_return_the_1121_tuple_not_the_era_one() {
        let mut s = UiScript::new().unwrap();
        let mut buff = aura(1126, "Mark of the Wild", true, true);
        buff.count = 1;
        let mut debuff = aura(589, "Shadow Word: Pain", false, false);
        debuff.count = 3;
        debuff.debuff_type = Some("Magic".into());
        s.set_auras("target", Some(vec![buff, debuff]));

        let (icon, count, third) = s
            .eval::<(String, i64, Option<String>)>(
                r#"local a, b, c = UnitBuff("target", 1) return a, b, c"#,
            )
            .unwrap();
        assert_eq!(icon, "Interface\\Icons\\Spell_1126");
        assert_eq!(count, 1);
        assert_eq!(third, None, "UnitBuff returns two values, never a third");

        let (icon, count, dispel, fourth) = s
            .eval::<(String, i64, String, Option<String>)>(
                r#"local a, b, c, d = UnitDebuff("target", 1) return a, b, c, d"#,
            )
            .unwrap();
        assert_eq!(icon, "Interface\\Icons\\Spell_589");
        assert_eq!((count, dispel.as_str()), (3, "Magic"));
        assert_eq!(
            fourth, None,
            "UnitDebuff returns three values, never a fourth"
        );

        // Stock `TargetFrame.lua:287-297`'s read, verbatim.
        assert!(s
            .eval::<bool>(
                r#"local d, stack, dtype = UnitDebuff("target", 1)
                   return stack > 1 and dtype == "Magic""#
            )
            .unwrap());

        // The raidFilter flag is accepted and not applied.
        assert_eq!(
            s.eval::<String>(r#"return (UnitDebuff("target", 1, 1))"#)
                .unwrap(),
            "Interface\\Icons\\Spell_589"
        );
    }

    #[test]
    fn cancel_unit_buff_queues_the_spell_id_and_refuses_a_non_cancelable_aura() {
        let mut s = UiScript::new().unwrap();
        s.set_auras(
            "player",
            Some(vec![
                aura(2457, "Battle Stance", true, true),
                aura(9999, "Sealed", true, false),
                aura(589, "Pain", false, false),
            ]),
        );
        assert!(s.take_cancel_aura_requests().is_empty());

        // Only the cancelable buff queues, by spell id; the others are silent no-ops.
        s.eval::<()>(r#"CancelUnitBuff("player", 1)"#).unwrap();
        s.eval::<()>(r#"CancelUnitBuff("player", 2)"#).unwrap();
        s.eval::<()>(r#"CancelUnitBuff("player", 9)"#).unwrap();
        assert_eq!(s.take_cancel_aura_requests(), vec![2457]);
        assert!(s.take_cancel_aura_requests().is_empty());
    }

    #[test]
    fn tracking_bindings_read_the_pushed_state_and_cancel_by_spell_id() {
        let mut s = UiScript::new().unwrap();
        assert!(s
            .eval::<bool>("return GetTrackingTexture() == nil")
            .unwrap());
        s.eval::<()>("CancelTrackingBuff()").unwrap();
        assert!(s.take_cancel_aura_requests().is_empty());

        s.set_tracking(Some(TrackingState {
            spell_id: 2580,
            name: Some("Find Minerals".into()),
            icon: Some("Interface\\Icons\\Trade_Mining".into()),
            cancelable: true,
        }));
        assert_eq!(
            s.eval::<String>("return GetTrackingTexture()").unwrap(),
            "Interface\\Icons\\Trade_Mining"
        );
        s.eval::<()>("CancelTrackingBuff()").unwrap();
        assert_eq!(s.take_cancel_aura_requests(), vec![2580]);

        s.set_tracking(Some(TrackingState {
            spell_id: 2580,
            cancelable: false,
            ..Default::default()
        }));
        s.eval::<()>("CancelTrackingBuff()").unwrap();
        assert!(s.take_cancel_aura_requests().is_empty());

        s.set_tracking(None);
        assert!(s
            .eval::<bool>("return GetTrackingTexture() == nil")
            .unwrap());
    }

    #[test]
    fn set_auras_none_clears_the_token() {
        let mut s = UiScript::new().unwrap();
        s.set_auras("player", Some(vec![aura(2457, "Stance", true, true)]));
        assert!(s
            .eval::<bool>(r#"return UnitAura("player", 1) ~= nil"#)
            .unwrap());
        s.set_auras("player", None);
        assert!(s
            .eval::<bool>(r#"return UnitAura("player", 1) == nil"#)
            .unwrap());
    }
}
