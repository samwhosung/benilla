//! The stance bar's four bindings, which `BonusActionBarFrame.lua`'s `ShapeshiftBar_*` calls, over
//! the form list `0xb71108` (count `0xb71104`) read at the call through the lent host
//! ([`ShapeshiftList`], [`ShapeshiftForm`]), else off the copy the app pushes; flags answer 1/nil
//! and the cooldown is the reference's `(start, duration, enable)`.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi;
use super::live::{ask, LiveQuery};
use super::Model;

/// The form list's spells as they stand, in bar order: `0xb71108`, which every binding indexes
/// (`0x4b4590`, `0x4b45c0`, `0x4b49a0`, `0x4b4810`).
#[derive(Clone, Copy, Debug)]
pub struct ShapeshiftList;

impl LiveQuery for ShapeshiftList {
    type Answer = Vec<u32>;
}

/// One form-list entry, 0-based, as `GetShapeshiftFormInfo` reads it at the call (`0x4b45c0`):
/// `None` for its four nils past the list (`0x4b4633`). A host with no active player declines, so
/// the copy answers the list's spell: in game nothing pushes one, and its absence is the four nils
/// of `0x4b4639`. The view's cooldown is unread: `GetShapeshiftFormCooldown` asks the spell's own
/// ([`super::CooldownQuery`]).
#[derive(Clone, Copy, Debug)]
pub struct ShapeshiftForm(pub usize);

impl LiveQuery for ShapeshiftForm {
    type Answer = Option<ShapeshiftFormView>;
}

/// One stance-bar button, resolved by the app.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShapeshiftFormView {
    /// The form spell a click queues.
    pub spell_id: u32,
    /// The icon texture path; `None` shows the slot's fallback.
    pub texture: Option<String>,
    /// The spell name, `GetShapeshiftFormInfo`'s second return.
    pub name: String,
    /// The player's current form (the checked ring).
    pub active: bool,
    /// Castable now; the reference greys the icon to 0.4 when not.
    pub castable: bool,
    /// `(start_ms on the GetTime clock, duration_ms, enabled)`, as an action's cooldown.
    pub cooldown: Option<(i64, u32, bool)>,
}

/// A pushed form with its cooldown in `GetTime` seconds.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct StoredShapeshiftForm {
    pub(crate) view: ShapeshiftFormView,
    /// `(start_s, duration_s, enabled)`.
    pub(crate) cooldown: Option<(f64, f64, bool)>,
}

impl super::UiScript {
    /// How many forms the list holds now, as `GetNumShapeshiftForms` reads it (`0x4b4590`): the
    /// lent host's, else the copy's.
    pub fn shapeshift_form_count(&self) -> usize {
        ask(self.lua(), &ShapeshiftList)
            .map_or_else(|| self.model_ref().shapeshift_forms.len(), |l| l.len())
    }

    /// Replace the form list, in bar order, for a VM with no host to ask; the app fires
    /// `UPDATE_SHAPESHIFT_FORMS` itself.
    pub fn set_shapeshift_forms(&mut self, forms: Vec<ShapeshiftFormView>) {
        let mut model = self.model_mut();
        model.shapeshift_forms = forms
            .into_iter()
            .map(|view| {
                let cooldown = view.cooldown.map(|(start_ms, duration_ms, enabled)| {
                    (
                        start_ms as f64 / 1000.0,
                        f64::from(duration_ms) / 1000.0,
                        enabled,
                    )
                });
                StoredShapeshiftForm { view, cooldown }
            })
            .collect();
    }

    /// Take the `CastShapeshiftForm` calls out of the call stream, as form spells; the app casts
    /// each, or cancels it when it is the active form.
    pub fn take_shapeshift_casts(&mut self) -> Vec<u32> {
        self.take_calls_where(|c| match c {
            super::ScriptCall::CastShapeshiftForm { spell_id, .. } => Some(*spell_id),
            _ => None,
        })
    }
}

/// A 1-based button index, as the reference indexes, 0-based.
fn slot(i: u32) -> Option<usize> {
    usize::try_from(i.checked_sub(1)?).ok()
}

/// A 1-based button index into the copy.
fn form_at(model: &Model, i: u32) -> Option<&StoredShapeshiftForm> {
    model.shapeshift_forms.get(slot(i)?)
}

/// The copy's entry for 1-based button `i`: the one holding the host's list's spell there, else
/// the copy's own `i`th.
fn copy_at<'m>(lua: &Lua, model: &'m Model, i: u32) -> Option<&'m StoredShapeshiftForm> {
    match ask(lua, &ShapeshiftList) {
        Some(list) => {
            let spell = *list.get(slot(i)?)?;
            model
                .shapeshift_forms
                .iter()
                .find(|f| f.view.spell_id == spell)
        }
        None => form_at(model, i),
    }
}

/// The spell on 1-based button `i` now, the host's list, else the copy's; `None` past the list.
fn form_spell(lua: &Lua, i: u32) -> Option<u32> {
    match ask(lua, &ShapeshiftList) {
        Some(list) => list.get(slot(i)?).copied(),
        None => form_at(&lua.app_data_ref::<Model>().expect("model app_data"), i)
            .map(|f| f.view.spell_id),
    }
}

/// A cooldown in `GetTime` seconds: `(start, duration, enabled)`.
type Seconds = (f64, f64, bool);

/// Button `i`'s spell and its cooldown in `GetTime` seconds now: the host's list with the spell's
/// own cooldown off the player's list (`0x6e2ea0`, item 0), else the copy's entry; `None` past the
/// list.
fn form_cooldown(lua: &Lua, i: u32) -> Option<(u32, Option<Seconds>)> {
    let Some(list) = ask(lua, &ShapeshiftList) else {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        let form = form_at(&model, i)?;
        return Some((form.view.spell_id, form.cooldown));
    };
    let spell = *list.get(slot(i)?)?;
    let cooldown = super::cooldown_read::read_live(lua, super::CooldownQuery::Spell(spell))
        .unwrap_or_else(|| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            model
                .shapeshift_forms
                .iter()
                .find(|f| f.view.spell_id == spell)
                .and_then(|f| f.cooldown)
        });
    Some((spell, cooldown))
}

/// Button `i`'s spell, its name and its cooldown now, for `GameTooltip:SetShapeshift`
/// (`0x532920`): the list read at the call (`0x5329f9`, `0x532a21`) and the cooldown off the
/// player's list (`0x532a26` → `0x6e2ea0`), each the host's, else the copy's; `None` past the
/// list.
pub(super) fn tooltip_form(lua: &Lua, i: u32) -> Option<(u32, Option<String>, Option<Seconds>)> {
    let (spell, cooldown) = form_cooldown(lua, i)?;
    let name = match slot(i).and_then(|n| ask(lua, &ShapeshiftForm(n))) {
        Some(form) => form.map(|f| f.name),
        None => copy_at(
            lua,
            &lua.app_data_ref::<Model>().expect("model app_data"),
            i,
        )
        .map(|f| f.view.name.clone()),
    };
    Some((spell, name, cooldown))
}

/// The index of the two form getters: `lua_tonumber` (`0x6f3620`) compared with 0.0 (`0x4b45d8`,
/// `0x4b49b4`), so a zero, an absent or a non-number raises the binding's usage error (`0x846a6c`,
/// `0x846ab4`) before the table is read; the truncated value is then compared unsigned.
fn form_index(lua: &Lua, index: Option<Value>, usage: &'static str) -> mlua::Result<u32> {
    let n = crate::script::binding_abi::coerced_number(lua, index);
    if n == 0.0 {
        return Err(mlua::Error::RuntimeError(usage.into()));
    }
    Ok(crate::script::binding_abi::ftol(n) as u32)
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    g.set(
        "GetNumShapeshiftForms",
        lua.create_function(|lua, ()| {
            // `0x4b4590`: the count `0xb71104` as it stands.
            if let Some(list) = ask(lua, &ShapeshiftList) {
                return Ok(list.len() as i64);
            }
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.shapeshift_forms.len() as i64)
        })?,
    )?;

    g.set(
        "GetShapeshiftFormInfo",
        lua.create_function(|lua, index: Option<Value>| {
            let i = form_index(lua, index, "Usage: GetShapeshiftFormInfo(index)")?;
            // The entry as it stands, read at the call (`0x4b45c0`), else the copy's.
            let live = slot(i).and_then(|n| ask(lua, &ShapeshiftForm(n)));
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let form = match &live {
                Some(form) => form.as_ref(),
                None => copy_at(lua, &model, i).map(|f| &f.view),
            };
            let Some(form) = form else {
                // `0x4b47d9`: four nils.
                return Ok(super::binding_abi::tuple(&[None; 4]));
            };
            let texture = match &form.texture {
                Some(t) => Value::String(lua.create_string(t)?),
                None => Value::Nil,
            };
            let flag = |b: bool| if b { Value::Integer(1) } else { Value::Nil };
            Ok(MultiValue::from_vec(vec![
                texture,
                Value::String(lua.create_string(&form.name)?),
                flag(form.active),
                flag(form.castable),
            ]))
        })?,
    )?;

    // As `GetActionCooldown`: an elapsed or absent cooldown answers `(0, 0, 1)`, so a re-feed never
    // replays the sweep. The entry's spell queries the player's list inside the call (`0x4b49a0`
    // → `0x6e2ea0`, item 0), through the lent host; one past the list answers `(0, 0, 1)`.
    g.set(
        "GetShapeshiftFormCooldown",
        lua.create_function(|lua, index: Option<Value>| {
            let i = form_index(lua, index, "Usage: GetShapeshiftFormCooldown(index)")?;
            let now = crate::script::clock::now(lua);
            Ok(
                match form_cooldown(lua, i).and_then(|(_, cooldown)| cooldown) {
                    Some((start, duration, enabled)) if start + duration > now || !enabled => (
                        crate::script::clock::held_start(start, enabled, now),
                        duration,
                        i32::from(enabled),
                    ),
                    _ => (0.0, 0.0, 1),
                },
            )
        })?,
    )?;

    g.set(
        "CastShapeshiftForm",
        lua.create_function(|lua, i: Value| {
            // A bare `tonumber` compared with 0.0 (`0x4b4820`, `0x802970`): a zero, and so any
            // non-number, raises `Usage: CastShapeshiftForm(index)` (`0x846a90`, `0x4b4832`);
            // the slot is then `_ftol`'s (`0x4b4855`).
            let n = lua.coerce_number(i)?.unwrap_or(0.0);
            if n == 0.0 {
                return Err(mlua::Error::runtime("Usage: CastShapeshiftForm(index)"));
            }
            let i = binding_abi::ftol(n) as u32;
            // The form's cast is `TryCastSpell` (`0x4b4992` → `0x6e5a90`), behind the gate's tag 2;
            // the cancels of an active form (`0x4b496b`, `0x4b497c`) run ahead of it, so the call
            // is queued whatever the gate said and carries the answer to the cast alone.
            let may_cast = super::protected::gate(lua, super::protected::Tag::CAST);
            // The entry's spell as it stands (`0x4b4810` reads `0xb71108`).
            if let Some(spell_id) = form_spell(lua, i) {
                super::calls::submit(
                    lua,
                    super::ScriptCall::CastShapeshiftForm { spell_id, may_cast },
                )?;
            }
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::ShapeshiftFormView;
    use crate::script::UiScript;

    fn forms() -> Vec<ShapeshiftFormView> {
        vec![
            ShapeshiftFormView {
                spell_id: 2457,
                texture: Some("Interface\\Icons\\Ability_Warrior_OffensiveStance".into()),
                name: "Battle Stance".into(),
                active: true,
                castable: true,
                cooldown: None,
            },
            ShapeshiftFormView {
                spell_id: 71,
                texture: Some("Interface\\Icons\\Ability_Warrior_DefensiveStance".into()),
                name: "Defensive Stance".into(),
                active: false,
                castable: true,
                cooldown: Some((9400, 1500, true)),
            },
        ]
    }

    #[test]
    fn form_info_reads_and_out_of_range_nil() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.eval::<i64>("return GetNumShapeshiftForms()").unwrap(), 0);
        assert!(s
            .eval::<bool>("return GetShapeshiftFormInfo(1) == nil")
            .unwrap());

        s.set_shapeshift_forms(forms());
        assert_eq!(s.eval::<i64>("return GetNumShapeshiftForms()").unwrap(), 2);

        let (texture, name, active, castable) = s
            .eval::<(String, String, Option<i64>, Option<i64>)>("return GetShapeshiftFormInfo(1)")
            .unwrap();
        assert_eq!(
            (texture.as_str(), name.as_str(), active, castable),
            (
                "Interface\\Icons\\Ability_Warrior_OffensiveStance",
                "Battle Stance",
                Some(1),
                Some(1)
            )
        );
        assert!(s
            .eval::<bool>("local _, _, a, c = GetShapeshiftFormInfo(2) return a == nil and c == 1")
            .unwrap());
        assert!(s
            .eval::<bool>("return GetShapeshiftFormInfo(3) == nil")
            .unwrap());
    }

    #[test]
    fn cooldown_triple_stamps_to_the_vm_clock_and_goes_cold() {
        let mut s = UiScript::new().unwrap();
        s.tick(10.0); // GetTime == 10
        s.set_shapeshift_forms(forms());

        assert_eq!(
            s.eval::<(f64, f64, i32)>("return GetShapeshiftFormCooldown(1)")
                .unwrap(),
            (0.0, 0.0, 1)
        );
        let (start, duration, enable) = s
            .eval::<(f64, f64, i32)>("return GetShapeshiftFormCooldown(2)")
            .unwrap();
        assert!((start - 9.4).abs() < 1e-9, "start {start}");
        assert!((duration - 1.5).abs() < 1e-9);
        assert_eq!(enable, 1);

        s.tick(2.0); // now == 12 > 9.4 + 1.5
        assert_eq!(
            s.eval::<(f64, f64, i32)>("return GetShapeshiftFormCooldown(2)")
                .unwrap(),
            (0.0, 0.0, 1)
        );
    }

    #[test]
    fn cast_queues_the_form_spell_and_ignores_out_of_range() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.set_shapeshift_forms(forms());

        s.run("CastShapeshiftForm(2)").unwrap();
        s.run("CastShapeshiftForm(9)").unwrap(); // out of range: no-op
        assert_eq!(s.take_shapeshift_casts(), vec![71]);
        assert!(s.take_shapeshift_casts().is_empty(), "drain empties");
    }

    /// A held form answers the call's own now, whatever was pushed (`0x6e1517`).
    #[test]
    fn a_held_form_answers_the_polls_now() {
        let mut s = UiScript::new().unwrap();
        s.tick(10.0);
        let mut f = forms();
        f[1].cooldown = Some((0, 1, false));
        s.set_shapeshift_forms(f);
        let poll = |s: &UiScript| {
            s.eval::<(f64, f64, i32)>("return GetShapeshiftFormCooldown(2)")
                .unwrap()
        };
        assert_eq!(poll(&s), (10.0, 0.001, 0));
        s.tick(0.02);
        let (start, _, _) = poll(&s);
        assert_eq!(start, s.eval::<f64>("return GetTime()").unwrap());
        assert!((start - 10.02).abs() < 1e-9);
    }

    /// `CastShapeshiftForm` casts through `TryCastSpell` inside the call (`0x4b4992` → `0x6e5a90`).
    #[test]
    fn a_form_cast_applies_in_the_call_with_a_host_lent() {
        let mut s = UiScript::new().unwrap();
        s.set_shapeshift_forms(forms());
        assert_eq!(
            crate::script::host::test_support::applied_in_call(&mut s, "CastShapeshiftForm(2)"),
            vec![crate::script::ScriptCall::CastShapeshiftForm {
                spell_id: 71,
                may_cast: true
            }]
        );
    }

    /// Answers the form list and its entries off its own list, as of each ask, and keeps the
    /// calls it applies.
    struct Host(Vec<ShapeshiftFormView>, Vec<crate::script::ScriptCall>);

    impl crate::script::CallHost for Host {
        fn apply(
            &mut self,
            _: &mut UiScript,
            call: crate::script::ScriptCall,
        ) -> Option<crate::script::CallTail> {
            self.1.push(call);
            None
        }

        fn read_live(&mut self, ask: &mut crate::script::live::LiveAsk<'_>) -> bool {
            let forms = &self.0;
            ask.answer(|_: &super::ShapeshiftList| Some(forms.iter().map(|f| f.spell_id).collect()))
                || ask.answer(|q: &super::ShapeshiftForm| Some(forms.get(q.0).cloned()))
        }
    }

    /// With a host lent, every binding reads its list, not the copy: the count, an entry's
    /// values, a cast's spell, and four nils past it (`0x4b4590`, `0x4b45c0`, `0x4b4810`).
    #[test]
    fn the_bindings_read_the_lent_hosts_list() {
        let mut s = UiScript::new().unwrap();
        s.set_shapeshift_forms(forms());
        let mut host = Host(vec![forms().remove(1)], Vec::new());
        let chunk = "local _, name, active, castable = GetShapeshiftFormInfo(1) \
                     CastShapeshiftForm(1) \
                     return GetNumShapeshiftForms(), name, active, castable, \
                     GetShapeshiftFormInfo(2) == nil";
        let read = s.hosted(&mut host, |s| {
            s.eval::<(i64, String, Option<i64>, Option<i64>, bool)>(chunk)
                .unwrap()
        });
        assert_eq!(read, (1, "Defensive Stance".into(), None, Some(1), true));
        assert!(
            matches!(
                host.1[..],
                [crate::script::ScriptCall::CastShapeshiftForm { spell_id: 71, .. }]
            ),
            "{:?}",
            host.1
        );
        assert_eq!(
            s.eval::<i64>("return GetNumShapeshiftForms()").unwrap(),
            2,
            "no host, the copy"
        );
    }
}
