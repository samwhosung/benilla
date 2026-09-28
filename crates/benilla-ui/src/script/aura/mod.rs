//! The aura bindings, over the lists the app pushes in display order: the player's in the
//! reference's insertion-ordered cache (`0xbc6040`), kept by `benilla::ui_aura`, every other unit's
//! by guid, in ascending aura slot. A token resolves to its guid as the reference's resolver
//! `0x515970` does ([`super::UnitGuids`]), so `"mouseover"`, `"party1target"` and `"PLAYER"` read
//! the list of the unit they name, and a token naming the player reads the player's list in cache
//! order, where the reference's `UnitBuff("player", i)` reads by slot.
//!
//! `UnitBuff` and `UnitDebuff` take a token and a 1-based index into the sign-filtered list and
//! answer nothing past the end. The 1.12 `GetPlayerBuff`
//! ([`player_buff`]) takes a 0-based index and returns a cache position, the same number under
//! every filter, which its siblings and `GameTooltip:SetPlayerBuff` take in place of the counter.
//!
//! `source` is always nil, and only the player's auras carry a duration: the 1.12 wire has no aura
//! caster and sends durations for the player alone, so the stock target frame shows no timers.

use mlua::{Lua, MultiValue, Value};

use super::Model;

mod player_buff;

/// One aura on one unit, as the app's feed pushes it ([`crate::script::UiScript::set_unit_auras`],
/// [`crate::script::UiScript::set_player_auras`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AuraState {
    /// `Spell.dbc` id, what `CMSG_CANCEL_AURA` names and the app's icon lookup keys on.
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

/// The cancel gate of `CancelPlayerBuff` (`0x4e49a0`, `0x4e49fb`-`0x4e4a14`):
/// a helpful aura with `AFLAG_CANCELABLE` (vmangos: unless `SPELL_ATTR_NO_AURA_CANCEL`), or any
/// aura of a channeled spell, the only negative aura that cancels (breaking a channel on you).
pub(super) fn cancel_authorized(a: &AuraState) -> bool {
    (a.helpful && a.cancelable) || a.channeled
}

/// The 1.12 tuple: `UnitBuff` returns `(texture, applications)` (`0x519500`) and `UnitDebuff` adds
/// `dispelType` (`0x5198f0`), as stock `TargetFrame.lua:287-290` reads them.
fn returns(lua: &Lua, a: &AuraState, with_dispel_type: bool) -> mlua::Result<MultiValue> {
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

/// The list of the unit `token` names, through the resolver (`0x515970`, which `UnitBuff` calls at
/// `0x519542`): the player's cache list for the player, else that guid's; `None` for nobody or a
/// unit with no list. A token the resolver does not recognise raises `Unknown unit name`.
pub(crate) fn auras_of<'m>(model: &'m Model, token: &str) -> mlua::Result<Option<&'m [AuraState]>> {
    let guids = &model.unit_guids;
    Ok(guids.guid_of(token)?.and_then(|g| {
        if g == guids.player {
            Some(model.player_auras.as_slice())
        } else {
            model.unit_auras.get(&g).map(Vec::as_slice)
        }
    }))
}

/// The `index`-th (1-based) buff (`helpful`) or debuff of `token`, in pushed order; out of range
/// answers nothing, the loop terminator.
fn nth_aura(
    lua: &Lua,
    token: &Option<String>,
    index: i64,
    helpful: bool,
) -> mlua::Result<MultiValue> {
    // The token resolves before the index is read (`0x519542`, then `0x51957a`), so a bad token
    // raises at any index.
    let hit = {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        match token {
            Some(t) => auras_of(&model, t)?
                .filter(|_| index >= 1)
                .and_then(|list| {
                    list.iter()
                        .filter(|a| a.helpful == helpful)
                        .nth((index - 1) as usize)
                        .cloned()
                }),
            None => None,
        }
    };
    match hit {
        Some(a) => returns(lua, &a, !helpful),
        None => Ok(MultiValue::new()),
    }
}

impl super::UiScript {
    /// Push the player's aura list in the cache's order (`0xbc6040`), durations joined.
    pub fn set_player_auras(&mut self, auras: Vec<AuraState>) {
        self.model_mut().player_auras = auras;
    }

    /// Push or clear a unit's aura list by guid, in ascending aura slot; any token naming the unit
    /// reads it. The player's is [`Self::set_player_auras`].
    pub fn set_unit_auras(&mut self, guid: u64, auras: Option<Vec<AuraState>>) {
        let mut model = self.model_mut();
        match auras {
            Some(a) => {
                model.unit_auras.insert(guid, a);
            }
            None => {
                model.unit_auras.remove(&guid);
            }
        }
    }

    /// Drop every aura list, the player's with the rest: the session's end.
    pub fn clear_auras(&mut self) {
        let mut model = self.model_mut();
        model.player_auras.clear();
        model.unit_auras.clear();
    }

    /// Push the unit-token resolver's inputs, copied only when they moved.
    pub fn set_unit_guids(&mut self, guids: &super::UnitGuids) {
        let mut model = self.model_mut();
        if model.unit_guids != *guids {
            model.unit_guids.clone_from(guids);
        }
    }

    /// Push or clear the player's active [`TrackingState`].
    pub fn set_tracking(&mut self, tracking: Option<TrackingState>) {
        self.model_mut().tracking = tracking;
    }

    /// The spell ids of the buffs (`helpful`) or debuffs of the unit `token` names, in the order
    /// `UnitBuff`/`UnitDebuff` enumerate them; empty for nobody and for a token the resolver
    /// refuses. No 1.12 verb answers an aura's spell id, so this is the host's read.
    pub fn aura_spell_ids(&self, token: &str, helpful: bool) -> Vec<u32> {
        let model = self.model_ref();
        auras_of(&model, token)
            .ok()
            .flatten()
            .map(|list| {
                list.iter()
                    .filter(|a| a.helpful == helpful)
                    .map(|a| a.spell_id)
                    .collect()
            })
            .unwrap_or_default()
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

    // UnitBuff(unit, index [, raidFilter]) and UnitDebuff: the verb fixes the sign, `0x519500`
    // reading aura slots 0..31 and `0x5198f0` slots 32..47. A non-zero `raidFilter` keeps only
    // buffs the player can cast (`0x4b3870`) or debuffs they can dispel (`0x4b3920`); it is
    // accepted, not applied, as `AuraState` holds neither fact, so the pet frame's buff row and the
    // dispellable-debuff rows show every aura.
    g.set(
        "UnitBuff",
        lua.create_function(
            |lua, (token, index, _raid_filter): (Option<String>, i64, Option<Value>)| {
                nth_aura(lua, &token, index, true)
            },
        )?,
    )?;
    g.set(
        "UnitDebuff",
        lua.create_function(
            |lua, (token, index, _raid_filter): (Option<String>, i64, Option<Value>)| {
                nth_aura(lua, &token, index, false)
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
    use crate::script::{AuraState, TrackingState, UiScript, UnitGuids};

    const ME: u64 = 0x10;
    const TARGET: u64 = 0xF130_0000_0000_0001;

    /// A VM whose resolver names us as `player` and `TARGET` as `target`.
    fn script() -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.set_unit_guids(&UnitGuids {
            player: ME,
            target: TARGET,
            ..Default::default()
        });
        s
    }

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
    fn unit_buff_enumerates_the_pushed_order_not_the_spell_id_order() {
        let mut s = script();
        s.set_player_auras(vec![
            aura(2457, "Battle Stance", true, true),
            aura(1126, "Mark of the Wild", true, true),
            aura(589, "Shadow Word: Pain", false, false),
        ]);
        assert_eq!(
            s.eval::<String>(r#"return (UnitBuff("player", 1))"#)
                .unwrap(),
            "Interface\\Icons\\Spell_2457"
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
            .eval::<bool>(r#"return UnitBuff("target", 1) == nil"#)
            .unwrap());
        assert_eq!(
            s.arity(r#"UnitBuff("player", 0)"#).unwrap(),
            0,
            "a miss answers nothing"
        );
    }

    #[test]
    fn unit_buff_and_unit_debuff_return_the_1121_tuple() {
        let mut s = script();
        let mut buff = aura(1126, "Mark of the Wild", true, true);
        buff.count = 1;
        let mut debuff = aura(589, "Shadow Word: Pain", false, false);
        debuff.count = 3;
        debuff.debuff_type = Some("Magic".into());
        s.set_unit_auras(TARGET, Some(vec![buff, debuff]));

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
    fn tracking_bindings_read_the_pushed_state_and_cancel_by_spell_id() {
        let mut s = script();
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
    fn an_emptied_player_list_answers_nil() {
        let mut s = script();
        s.set_player_auras(vec![aura(2457, "Stance", true, true)]);
        assert!(s
            .eval::<bool>(r#"return UnitBuff("player", 1) ~= nil"#)
            .unwrap());
        s.set_player_auras(Vec::new());
        assert!(s
            .eval::<bool>(r#"return UnitBuff("player", 1) == nil"#)
            .unwrap());
    }

    const MOB: u64 = 0xF130_0000_0000_0002;
    const P1: u64 = 0x21;
    const P1_TARGET: u64 = 0xF130_0000_0000_0003;

    /// We target `TARGET`, which targets party1, who targets `P1_TARGET`, which targets us; the mouse
    /// is over `MOB`. Each unit's list holds a buff and a debuff whose spell ids, and so icons, are
    /// its own: `base + 1` and `base + 2`.
    fn group_script() -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.set_unit_guids(&UnitGuids {
            player: ME,
            target: TARGET,
            mouseover: MOB,
            party: [P1, 0, 0, 0],
            held: [
                (ME, TARGET),
                (TARGET, P1),
                (P1, P1_TARGET),
                (MOB, 0),
                (P1_TARGET, ME),
            ]
            .into_iter()
            .collect(),
            ..Default::default()
        });
        s.set_player_auras(vec![
            aura(2457, "Battle Stance", true, true),
            aura(1126, "Mark of the Wild", true, true),
            aura(11976, "Strike", false, false),
        ]);
        for (guid, base, name) in [
            (TARGET, 100, "on the target"),
            (MOB, 200, "on the mouseover"),
            (P1, 300, "on party1"),
            (P1_TARGET, 400, "on party1's target"),
        ] {
            s.set_unit_auras(
                guid,
                Some(vec![
                    aura(base + 1, name, true, true),
                    aura(base + 2, name, false, false),
                ]),
            );
        }
        s
    }

    /// The icon a read answers, the list's identity.
    fn icon_of(s: &UiScript, call: &str) -> Option<String> {
        s.eval::<Option<String>>(&format!("return ({call})"))
            .unwrap()
    }

    /// Every token reads the list of the unit the resolver names (`0x515970` at `0x519542`): the
    /// mouseover, a group member's target and any depth of `target` chain.
    #[test]
    fn a_token_reads_the_list_of_the_unit_it_names() {
        let s = group_script();
        for (token, base) in [
            ("mouseover", 200),
            ("party1target", 400),
            ("targettarget", 300),
            ("TargetTarget", 300),
            ("targettargettarget", 400),
            ("party1", 300),
            ("PARTY1", 300),
        ] {
            assert_eq!(
                icon_of(&s, &format!(r#"UnitBuff("{token}", 1)"#)),
                Some(format!("Interface\\Icons\\Spell_{}", base + 1)),
                "{token}"
            );
            assert_eq!(
                icon_of(&s, &format!(r#"UnitDebuff("{token}", 1)"#)),
                Some(format!("Interface\\Icons\\Spell_{}", base + 2)),
                "{token}"
            );
        }
        assert!(s
            .eval::<bool>(r#"return UnitBuff("party1target", 2) == nil"#)
            .unwrap());
    }

    /// A token naming the player, in any case or down a chain, reads the player's list in cache
    /// order, as `"player"` does.
    #[test]
    fn a_token_naming_the_player_reads_the_player_list() {
        let s = group_script();
        for token in [
            "player",
            "PLAYER",
            "Player",
            "targettargettargettarget",
            "party1TARGETtarget",
        ] {
            assert_eq!(
                icon_of(&s, &format!(r#"UnitBuff("{token}", 2)"#)).as_deref(),
                Some("Interface\\Icons\\Spell_1126"),
                "{token}"
            );
        }
        assert!(s
            .eval::<bool>(
                r#"local a, n = UnitBuff("PLAYER", 1)
                   local b, m = UnitBuff("player", 1)
                   local c = UnitDebuff("targettargettargettarget", 1)
                   return a == b and n == m and a == "Interface\\Icons\\Spell_2457"
                       and c == "Interface\\Icons\\Spell_11976""#
            )
            .unwrap());
    }

    /// A recognised token naming nobody is nil; a token none of the resolver's nine compares match
    /// raises `Unknown unit name: %s` (`0x515c14`), at any index, as the resolver runs first.
    #[test]
    fn an_unknown_token_raises_and_a_recognised_empty_one_is_nil() {
        let s = group_script();
        for call in [
            r#"UnitBuff("party2", 1)"#,
            r#"UnitDebuff("raid1target", 1)"#,
            r#"UnitBuff("mouseovertargettarget", 1)"#,
            r#"UnitBuff("playerfoo", 1)"#,
            r#"UnitBuff("pet", 1)"#,
            r#"UnitBuff("", 1)"#,
        ] {
            assert!(
                s.eval::<bool>(&format!("return {call} == nil")).unwrap(),
                "{call}"
            );
        }
        for call in [
            r#"UnitBuff("bogus", 1)"#,
            r#"UnitDebuff("focus", 1)"#,
            r#"UnitDebuff("npctarget", 1)"#,
            r#"UnitBuff("bogus", 0)"#,
        ] {
            let err = s.eval::<()>(call).unwrap_err().to_string();
            assert!(err.contains("Unknown unit name: "), "{call}: {err}");
        }
    }

    /// `GameTooltip:SetUnitBuff` resolves as `UnitBuff` does (`0x534b8b`).
    #[test]
    fn set_unit_buff_resolves_the_token() {
        let s = group_script();
        s.run(
            r#"local a = CreateFrame("Button", "TF1")
               a:SetPoint("CENTER", 0, 0) a:SetWidth(10) a:SetHeight(10)
               TT = CreateFrame("GameTooltip", "TT")
               TT:SetOwner(a, "ANCHOR_BOTTOMRIGHT")
               TT:SetUnitDebuff("MouseOver", 1)"#,
        )
        .unwrap();
        assert_eq!(
            s.eval::<String>("return TTTextLeft1:GetText()").unwrap(),
            "on the mouseover"
        );
        let err = s
            .run(r#"TT:SetUnitBuff("bogus", 1)"#)
            .unwrap_err()
            .to_string();
        assert!(err.contains("Unknown unit name: bogus"), "{err}");
    }
}
