//! The game-state aura bindings — **two dialects over one list** (decisions 0255, 0257).
//!
//! Same engine-free seam as [`super::unit`]: the app pushes a per-token, **already ordered** list of
//! [`AuraState`] via [`UiScript::set_auras`], and these globals index it. This module deliberately
//! knows nothing about slots, caches, or descriptors — the *order* of the list is the app's to decide,
//! because the two orderings the reference uses are not derivable from a snapshot:
//!
//! - the player's own bar walks a densely packed cache in **insertion order** (`GetPlayerBuff`), and
//! - any other unit is read straight off its aura array, ascending slot (`UnitBuff`/`UnitDebuff`).
//!
//! Decision 0257 resolves that split toward the player-bar law for the local player under every token,
//! since the Era API has one aura function where 1.12 had two.
//!
//! # The two dialects
//!
//! **Era** — `UnitAura`/`UnitBuff`/`UnitDebuff`/`CancelUnitBuff`: a token plus a **1-based** index
//! *within the sign-filtered list*, terminated by a bare `nil`. This is the shape our own transcribed
//! frames use.
//!
//! **1.12** — the `GetPlayerBuff*` family plus `CancelPlayerBuff`: the player only, no token, a
//! **0-based** index, and — the part no documentation gets right — `GetPlayerBuff` does not return the
//! aura. It returns a **handle**: the aura's *physical position* in the cache, `-1` past the end. Every
//! sibling (`GetPlayerBuffTexture`, `…TimeLeft`, `…DispelType`, `…Applications`, `CancelPlayerBuff`,
//! and `GameTooltip:SetPlayerBuff`) takes that position, **not** the loop counter. So the family is a
//! two-step: enumerate to get a position, then read the position.
//!
//! That two-step is what makes the index space *absolute*: the position a filtered enumeration hands
//! back is the same number an unfiltered one would, which is exactly what the corpus relies on
//! (`CT_BuffMod/CT_BuffFrame.lua:73-79` obtains an index under `"HARMFUL"` and compares it for
//! equality against one obtained under `"HELPFUL|HARMFUL"`; `Zorlen/Zorlen.lua:2798` intersects the
//! filtered and unfiltered walks at the same counter).
//!
//! Because [`AuraState`] lists are pushed in the app's display order and the player's list **is** the
//! reference's `0xbc6040` insertion-ordered cache (`benilla::ui_aura`), a cache position is simply a
//! 0-based index into `model.auras["player"]`. No second structure exists, and none is needed.
//!
//! # Byte-verified signatures (the 1.12.1 binary)
//!
//! Read off the disassembly, not off a wiki — a later client changed every one of these shapes, and a
//! wrong shape fails *silently*.
//!
//! | verb | addr | returns | miss |
//! |---|---|---|---|
//! | `GetPlayerBuff(index [, "filter"])` | `0x4e45d0` | **always 2** numbers: `pos`, `untilCancelled` | `(-1, 0)` |
//! | `GetPlayerBuffTexture(pos)` | `0x4e4740` | 1: icon path or nil | nil |
//! | `GetPlayerBuffDispelType(pos)` | `0x4e4800` | 1: dispel-class name or nil | nil |
//! | `GetPlayerBuffApplications(pos)` | `0x4e48b0` | 1: number | **`1`** |
//! | `GetPlayerBuffTimeLeft(pos)` | `0x4e4930` | 1: **seconds** | `0` |
//! | `CancelPlayerBuff(pos)` | `0x4e49a0` | **0** values | — |
//!
//! The traps, each with the site that pins it:
//!
//! - **`-1`, never nil.** `GetPlayerBuff` seeds its out-parameter to `-1` (`mov DWORD PTR [ebp-4],
//!   0xffffffff`, `0x4e46ea`) and pushes it unconditionally, then pushes `record+0xc` or `0`
//!   (`0x4e46f6`/`0x4e4728`), returning `mov eax,0x2` on **both** paths. The corpus terminates on the
//!   number — `while GetPlayerBuff(counter) >= 0 do` (`_LazyPig/LazyPig.lua:1174`) — so a nil past the
//!   end is `attempt to compare nil with number`, and *zero* returns is the same crash.
//! - **0-based.** `lua_tonumber` → `__ftol` → used raw, with **no `dec`** (`0x4e460a`-`0x4e4616`);
//!   contrast `UnitBuff 0x519500`, which does `dec eax` at `0x519579`. Every enumerating call site in
//!   the corpus starts at `0`; the reference's own buttons are `BuffButton0`..`BuffButton23` with
//!   `id="0"` upward (`ref-BuffFrame.xml:159`).
//! - **The default filter is `HELPFUL|HARMFUL`, not `HELPFUL`.** `mov esi,0x3` at `0x4e4618`, kept
//!   when the `lua_isstring` check on arg 2 fails. This is the one that would have failed silently:
//!   `Zorlen/Zorlen.lua:2797-2801` walks *unfiltered* and feeds each index to
//!   `GetPlayerBuffDispelType` looking for `"Poison"`/`"Disease"` (`Zorlen_Other.lua:94,98`) — a
//!   debuff-only concept. Under a `HELPFUL` default that check never matches, and nothing errors.
//! - **`untilCancelled` is the number `0`/`1`.** Five corpus sites compare it to the *number* `1`
//!   (`CT_BuffMod/CT_BuffFrame.lua:175,232`; `_LazyPig/LazyPig.lua:2071`; `Zorlen/Zorlen.lua:2800`;
//!   `ElkBuffBar/ElkBuffBar.lua:502`), as does `ref-BuffFrame.lua:124`. A boolean breaks all six:
//!   `true ~= 1`.
//! - **`PASSIVE` is not a token here.** The parser knows exactly four (`stricmp` against `0x84bc34`
//!   `HELPFUL`=`0x1`, `0x84bc2c` `HARMFUL`=`0x2`, `0x84bc20` `CANCELABLE`=`0x10`, `0x84bc10`
//!   `NOT_CANCELABLE`=`0x20`; `0x4e4661`-`0x4e46cf`). A `"PASSIVE"` string does exist in the binary at
//!   `0x806a30`, but `0x4e45d0` never references it — and no corpus addon passes it.
//! - **The mask starts at ZERO once a filter string is given** (`xor esi,esi`, `0x4e4639`). So
//!   `GetPlayerBuff(0, "CANCELABLE")` matches *nothing* — neither sign bit is set — where
//!   [`super::unit`]-style parsing would have defaulted the sign to helpful. Delimiters are
//!   `" |"` (`0x84bc3c`), i.e. space **or** pipe.
//!
//! **Stated gaps, not hidden.** `source` (the caster) is always nil and `duration`/`expirationTime`
//! are `0` for every unit but the player: the 1.12 wire carries no aura caster at all, and no duration
//! for anyone but yourself (byte-verified, decision 0257 B6/B10). The reference's own target and party
//! frames show no timers for the same reason.

use mlua::{Lua, MultiValue, Value};

use super::Model;

mod player_buff;

/// One aura on one unit, as the Era `UnitAura` family reports it. Plain data (no mlua handles, no ECS
/// types), pushed by the app's [`crate::script::UiScript::set_auras`] feed each frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AuraState {
    /// `Spell.dbc` id (`UnitAura`'s `spellId`).
    pub spell_id: u32,
    /// The spell's name, or `None` when the catalog doesn't know the id (`UnitAura` returns nil).
    pub name: Option<String>,
    /// The icon's extensionless MPQ path (`Interface\Icons\…`).
    pub icon: Option<String>,
    /// Stack count, ≥ 1 (`UnitAura`'s `count`). The reference shows the number only above 1.
    pub count: u8,
    /// `"Magic"`/`"Curse"`/`"Disease"`/`"Poison"`, or `None` for an undispellable class — the
    /// `debuffType` return, which the debuff border tints by.
    pub debuff_type: Option<String>,
    /// The full duration in seconds as the last apply/refresh reported it; `0.0` = no duration
    /// (a permanent aura, or any aura on a unit that isn't the player).
    pub duration: f64,
    /// The `GetTime()`-based instant this aura runs out; `0.0` when [`Self::duration`] is `0.0`.
    /// Lua counts down against it, exactly as `CastingBar.xml` does for a cast.
    pub expiration_time: f64,
    /// A buff (helpful) rather than a debuff — the `HELPFUL`/`HARMFUL` filter's discriminator.
    pub helpful: bool,
    /// Whether right-clicking it will be honoured (the wire's `AFLAG_CANCELABLE`) — the
    /// `CANCELABLE`/`NOT_CANCELABLE` filter's discriminator.
    pub cancelable: bool,
    /// **`GetPlayerBuff`'s second return** — the reference's cache record `+0xc`: "this aura has no
    /// finite duration to display."
    ///
    /// DBC-derived at cache-build time, and deliberately **not** the same question as
    /// `expiration_time == 0.0`: it is computed from the spell, so it is right on the frame an aura
    /// appears, before any `SMSG_UPDATE_AURA_DURATION` has arrived. That is the whole point of it —
    /// `ref-BuffFrame.lua:124` returns *before* calling `GetPlayerBuffTimeLeft` when it is `1`, so a
    /// permanent aura never flickers a `0 s` timer. The app fills it (`benilla::ui_aura`) from the
    /// byte-verified derivation at `0x4e452e`-`0x4e45c5`; see there for the two clauses.
    pub until_cancelled: bool,
    /// `Spell.dbc` `AttributesEx & 0x4` (`SPELL_ATTR_EX_IS_CHANNELED`) — the **second** arm of the
    /// cancel gate, and the only one that can authorise cancelling a *negative* aura. See
    /// [`cancel_authorized`].
    pub channeled: bool,
}

/// The player's active tracking aura — the engine mirror of the reference's tracking global
/// (`DAT_00bc6378`): during the aura-cache rebuild the client *excludes* a tracking-effect spell
/// (Find Minerals, Track Beasts, …) from the display cache and records it here instead;
/// `GetTrackingTexture` is the one reader (`0x4e4a20`, the
/// `{0x2c,0x2d,0x97}` effect test). Pushed by the app's aura feed via [`UiScript::set_tracking`];
/// `None` = no tracking active (the minimap frame hides).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrackingState {
    /// `Spell.dbc` id — `CancelTrackingBuff`'s `CMSG_CANCEL_AURA` payload.
    pub spell_id: u32,
    /// The spell's name, or `None` when the catalog doesn't know the id (tooltip fallback line).
    pub name: Option<String>,
    /// The icon's extensionless MPQ path — `GetTrackingTexture`'s return (`nil` when unknown).
    pub icon: Option<String>,
    /// The wire's `AFLAG_CANCELABLE` on the tracking aura's slot — `CancelTrackingBuff`'s gate,
    /// same client-side check as `CancelUnitBuff`'s.
    pub cancelable: bool,
}

/// The parsed `filter` argument: a `|`-separated token set. Absent or sign-less defaults to
/// `HELPFUL`, matching the live API (`UnitAura(unit, i)` enumerates buffs).
struct Filter {
    helpful: bool,
    cancelable: Option<bool>,
}

impl Filter {
    fn parse(spec: Option<&str>) -> Self {
        let spec = spec.unwrap_or("");
        let has = |t: &str| spec.split('|').any(|s| s.trim().eq_ignore_ascii_case(t));
        Self {
            // HARMFUL wins only if named; everything else (including "CANCELABLE" alone) is HELPFUL.
            helpful: !has("HARMFUL"),
            cancelable: match (has("CANCELABLE"), has("NOT_CANCELABLE")) {
                (true, false) => Some(true),
                (false, true) => Some(false),
                // Both or neither: no cancelable constraint (the reference never passes both).
                _ => None,
            },
        }
    }

    fn matches(&self, a: &AuraState) -> bool {
        a.helpful == self.helpful && self.cancelable.is_none_or(|c| a.cancelable == c)
    }
}

/// May this aura be cancelled? **One law, two entry points** — `CancelUnitBuff` (the Era name) and
/// `CancelPlayerBuff` (the 1.12 name) are the same verb over the same cache, so they share the gate
/// and the queue rather than growing a second mechanism.
///
/// Byte-verified, `CancelPlayerBuff 0x4e49a0` (`0x4e49fb`-`0x4e4a14`) — an **OR** of two arms:
///
/// 1. the **wire** arm: the aura sits in the positive half (`record.slot < 0x20`, i.e. helpful) AND
///    carries `AFLAG_CANCELABLE` (`record+0xa & 0x1`), which vmangos sets iff the aura is positive
///    and the spell lacks `SPELL_ATTR_NO_AURA_CANCEL`; OR
/// 2. the **DBC** arm: the spell has `AttributesEx & 0x4` (`SPELL_ATTR_EX_IS_CHANNELED`) — the only
///    way a *negative* aura becomes cancellable, which is how you break a channel being held on you.
///
/// Neither arm alone is the gate; reading only the first (the shape `CancelUnitBuff` shipped with)
/// silently refuses the channeled case.
pub(super) fn cancel_authorized(a: &AuraState) -> bool {
    (a.helpful && a.cancelable) || a.channeled
}

/// `UnitAura`'s return tuple for a hit. Order and arity follow the Classic Era signature:
/// `name, icon, count, debuffType, duration, expirationTime, source, isStealable,
/// nameplateShowPersonal, spellId`. The three we cannot know on this wire (`source`, `isStealable`,
/// `nameplateShowPersonal`) return nil — an addon that reads them sees "unknown", which is honest,
/// rather than a fabricated value.
/// The **1.12** return tuple, which is a different shape and not a prefix of the Era one: the first
/// value is the TEXTURE, not the name. `UnitBuff` returns `(texture, applications)` (`0x519500`) and
/// `UnitDebuff` returns `(texture, applications, dispelType)` (`0x5198f0`) — both are what the
/// reference's own FrameXML reads
/// (`TargetFrame.lua:287-290` binds `debuff, debuffStack, debuffType` and then
/// `SetTexture(debuff)`). Decision 1818, which also records why the Era shape was serving nobody.
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

/// Which return shape a getter wants — the two are not compatible, see [`returns_1121`].
#[derive(Clone, Copy)]
enum Shape {
    /// `UnitAura`'s ten values. Era-only: the name does not exist in 1.12 FrameXML at all.
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
        Value::Nil, // source — no aura caster exists on the 1.12 wire, for any unit (0257 B10)
        Value::Nil, // isStealable — Spellsteal is TBC
        Value::Nil, // nameplateShowPersonal
        Value::Integer(i64::from(a.spell_id)),
    ]))
}

/// The shared body of `UnitAura`/`UnitBuff`/`UnitDebuff`: take the `index`-th (1-based) aura of
/// `unit` that passes `filter`, in the order the app pushed them. Out of range → a bare nil, the
/// live API's "no more auras" terminator that every `for i = 1, N` loop breaks on.
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
    /// Push (or clear) a unit token's aura list, **in display order** — the app decides that order
    /// (decision 0257): the maintained insertion-order cache for the local player, ascending aura
    /// slot for anyone else. `None` (or an empty list) makes every `UnitAura(token, i)` return nil.
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

    /// Push (or clear) the player's active tracking aura ([`TrackingState`] doc) — the app's aura
    /// feed derives it from the same walk that excludes tracking spells from the display lists.
    pub fn set_tracking(&mut self, tracking: Option<TrackingState>) {
        self.model_mut().tracking = tracking;
    }

    /// Drain the spell ids `CancelUnitBuff` queued since the last call — the app sends one
    /// `CMSG_CANCEL_AURA` per id (the server cancels by spell, never by slot; decision 0257 B8).
    pub fn take_cancel_aura_requests(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().cancel_aura_requests)
    }
}

/// Register `UnitAura`, `UnitBuff`, `UnitDebuff` and `CancelUnitBuff`, plus the 1.12 `GetPlayerBuff*`
/// family ([`player_buff`]).
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // UnitAura(unit, index [, filter]) — filter defaults to HELPFUL, like the live API. This one
    // keeps the Era tuple: it is not a 1.12 verb at all (zero occurrences in the reference's
    // FrameXML) and has zero call sites in the 110-addon corpus, so it costs nothing and stays the
    // Era-compat surface 0068 asked for.
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

    // UnitBuff(unit, index [, raidFilter]) / UnitDebuff(unit, index [, raidFilter]) — the 1.12
    // signature and the 1.12 RETURN SHAPE (decision 1818). The sign is fixed by the verb, not by a
    // filter word: `0x519500` reads aura slots 0..31 and `0x5198f0` reads 32..47.
    //
    // **The third argument is a `raidFilter` flag, not Era's filter string.** Non-zero enables ONE
    // extra per-slot predicate — castable-by-me for buffs (`0x4b3870`), dispellable-by-me for
    // debuffs (`0x4b3920`). We accept it and DO NOT apply it, because [`AuraState`] holds neither
    // fact and neither is derivable from the aura feed: castable-by-me needs the player's spellbook,
    // dispellable-by-me needs a class→dispel-type table joined to `debuff_type`. Inventing an answer
    // is the guess 1203 exists to prevent, so the flag is parked here in the open. Visible
    // consequence, so nobody has to rediscover it: the pet frame's buff row and the
    // dispellable-debuff rows show everything rather than only what the player can cast or dispel.
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

    // CancelUnitBuff(unit, index [, filter]) — the Era name for the reference's
    // `CancelPlayerBuff(buffIndex)`. Resolves the aura the same way the getters do, then queues its
    // SPELL ID for the app: the server's `CMSG_CANCEL_AURA` carries a spell, not a slot, and refuses
    // anything not cancelable. A non-cancelable aura (or a missing one) is a silent no-op, as in the
    // reference, where the client's own `AFLAG_CANCELABLE` gate never sends the packet.
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

    // GetTrackingTexture() — the reference's `0x4e4a20`: the icon path of the active tracking
    // spell (the tracking global's occupant), or nil when none — `MiniMapTrackingFrame`'s
    // show/hide + SetTexture driver on PLAYER_AURAS_CHANGED (ref-Minimap.xml l.150-159).
    g.set(
        "GetTrackingTexture",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.tracking.as_ref().and_then(|t| t.icon.clone()))
        })?,
    )?;

    // CancelTrackingBuff() — the reference's `0x4e4a80` (the tracking frame's right-click): cancel
    // the active tracking aura by SPELL id through the same `CMSG_CANCEL_AURA` queue as
    // `CancelUnitBuff`, behind the same client-side `AFLAG_CANCELABLE` gate. No tracking active
    // is a silent no-op.
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

    /// The list the app pushes IS the enumeration order — the binding never re-sorts it. This is the
    /// contract decision 0257 rests on: the player's insertion-ordered cache survives the seam.
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
        // Buffs, in the pushed order (not ascending spell id).
        assert_eq!(
            s.eval::<String>(r#"return (UnitAura("player", 1))"#)
                .unwrap(),
            "Battle Stance"
        );
        // `UnitBuff`/`UnitDebuff` enumerate the same list, but their first return is the ICON, not
        // the name (1.12's shape — decision 1818), so they are addressed by it here.
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
        // Past the end → nil, the loop terminator.
        assert!(s
            .eval::<bool>(r#"return UnitBuff("player", 3) == nil"#)
            .unwrap());
        assert!(s
            .eval::<bool>(r#"return UnitDebuff("player", 2) == nil"#)
            .unwrap());
        // An unknown token, and a zero/negative index, are the same "no aura" shape.
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
                aura(9999, "Sealed", true, false), // helpful but not cancelable
                aura(589, "Pain", false, false),
            ]),
        );
        // No filter ⇒ HELPFUL: two hits, the debuff invisible.
        assert_eq!(
            s.eval::<i64>(
                r#"local n = 0 for i=1,10 do if UnitAura("player", i) then n = n + 1 end end return n"#
            )
            .unwrap(),
            2
        );
        // HARMFUL names the sign explicitly.
        assert_eq!(
            s.eval::<String>(r#"return (UnitAura("player", 1, "HARMFUL"))"#)
                .unwrap(),
            "Pain"
        );
        // CANCELABLE / NOT_CANCELABLE partition the helpful set.
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
        // A bare CANCELABLE still means helpful (the sign defaults, it isn't cleared).
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

    /// The **1.12** shape, which is not a prefix of the Era one: `UnitBuff` → `(texture,
    /// applications)`, `UnitDebuff` → `(texture, applications, dispelType)`, and the FIRST value is
    /// the texture. Decision 1818; `0x519500`/`0x5198f0`. The
    /// reference's own FrameXML reads exactly this (`TargetFrame.lua:287-290`), and so does every
    /// one of the 184 call sites in the addon corpus.
    #[test]
    fn unit_buff_and_unit_debuff_return_the_1121_tuple_not_the_era_one() {
        let mut s = UiScript::new().unwrap();
        let mut buff = aura(1126, "Mark of the Wild", true, true);
        buff.count = 1;
        let mut debuff = aura(589, "Shadow Word: Pain", false, false);
        debuff.count = 3;
        debuff.debuff_type = Some("Magic".into());
        s.set_auras("target", Some(vec![buff, debuff]));

        // UnitBuff returns exactly TWO values, texture first.
        let (icon, count, third) = s
            .eval::<(String, i64, Option<String>)>(
                r#"local a, b, c = UnitBuff("target", 1) return a, b, c"#,
            )
            .unwrap();
        assert_eq!(icon, "Interface\\Icons\\Spell_1126");
        assert_eq!(count, 1);
        assert_eq!(third, None, "UnitBuff returns two values, never a third");

        // UnitDebuff returns THREE, the dispel type last.
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

        // The reference's own read, verbatim from TargetFrame.lua:287-297 — the comparison that
        // raised `attempt to compare number with string` while we returned the Era tuple.
        assert!(s
            .eval::<bool>(
                r#"local d, stack, dtype = UnitDebuff("target", 1)
                   return stack > 1 and dtype == "Magic""#
            )
            .unwrap());

        // The raidFilter argument is ACCEPTED and its predicate is not applied (1818): passing it
        // must not error, and must not change the answer.
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

        // A cancelable buff queues its SPELL id (not its index).
        s.eval::<()>(r#"CancelUnitBuff("player", 1)"#).unwrap();
        // A non-cancelable one is a silent no-op, as in the reference.
        s.eval::<()>(r#"CancelUnitBuff("player", 2)"#).unwrap();
        // So is an out-of-range index, and a debuff reached through the helpful filter.
        s.eval::<()>(r#"CancelUnitBuff("player", 9)"#).unwrap();
        assert_eq!(s.take_cancel_aura_requests(), vec![2457]);
        assert!(s.take_cancel_aura_requests().is_empty());
    }

    /// The tracking seam (the reference's `GetTrackingTexture`/`CancelTrackingBuff` pair over the
    /// tracking global): the pushed state is what the bindings read, the cancel queues the SPELL
    /// id behind the same `AFLAG_CANCELABLE` gate as `CancelUnitBuff`, and no tracking is nil +
    /// no-op.
    #[test]
    fn tracking_bindings_read_the_pushed_state_and_cancel_by_spell_id() {
        let mut s = UiScript::new().unwrap();
        // No tracking pushed yet: nil texture, cancel is a silent no-op.
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

        // A non-cancelable tracking aura refuses, exactly like CancelUnitBuff's gate.
        s.set_tracking(Some(TrackingState {
            spell_id: 2580,
            cancelable: false,
            ..Default::default()
        }));
        s.eval::<()>("CancelTrackingBuff()").unwrap();
        assert!(s.take_cancel_aura_requests().is_empty());

        // Cleared: back to nil (the minimap frame's hide branch).
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
