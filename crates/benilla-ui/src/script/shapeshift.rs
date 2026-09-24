//! The stance bar's four bindings, which `BonusActionBarFrame.lua`'s `ShapeshiftBar_*` calls, over
//! a form list the app resolves and pushes; flags answer 1/nil and the cooldown is the reference's
//! `(start, duration, enable)`.

use mlua::{Lua, MultiValue, Value};

use super::Model;

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
    /// Replace the form list, in bar order; the app fires `UPDATE_SHAPESHIFT_FORMS` itself.
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

    /// Drain the form spells `CastShapeshiftForm` queued; the app casts each, or cancels it when it
    /// is the active form.
    pub fn take_shapeshift_casts(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().shapeshift_casts)
    }
}

/// A 1-based button index, as the reference indexes.
fn form_at(model: &Model, i: u32) -> Option<&StoredShapeshiftForm> {
    usize::try_from(i.checked_sub(1)?)
        .ok()
        .and_then(|n| model.shapeshift_forms.get(n))
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    g.set(
        "GetNumShapeshiftForms",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.shapeshift_forms.len() as i64)
        })?,
    )?;

    g.set(
        "GetShapeshiftFormInfo",
        lua.create_function(|lua, i: u32| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let Some(form) = form_at(&model, i) else {
                return Ok(MultiValue::from_vec(vec![Value::Nil]));
            };
            let texture = match &form.view.texture {
                Some(t) => Value::String(lua.create_string(t)?),
                None => Value::Nil,
            };
            let flag = |b: bool| if b { Value::Integer(1) } else { Value::Nil };
            Ok(MultiValue::from_vec(vec![
                texture,
                Value::String(lua.create_string(&form.view.name)?),
                flag(form.view.active),
                flag(form.view.castable),
            ]))
        })?,
    )?;

    // As `GetActionCooldown`: an elapsed or absent cooldown answers `(0, 0, 1)`, so a re-feed never
    // replays the sweep.
    g.set(
        "GetShapeshiftFormCooldown",
        lua.create_function(|lua, i: u32| {
            let now: f64 = lua.globals().get("__benilla_now").unwrap_or(0.0);
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(match form_at(&model, i).and_then(|f| f.cooldown) {
                Some((start, duration, enabled)) if start + duration > now || !enabled => {
                    (start, duration, i32::from(enabled))
                }
                _ => (0.0, 0.0, 1),
            })
        })?,
    )?;

    g.set(
        "CastShapeshiftForm",
        lua.create_function(|lua, i: u32| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            if let Some(spell_id) = form_at(&model, i).map(|f| f.view.spell_id) {
                model.shapeshift_casts.push(spell_id);
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
        s.set_shapeshift_forms(forms());

        s.run("CastShapeshiftForm(2)").unwrap();
        s.run("CastShapeshiftForm(9)").unwrap(); // out of range: no-op
        assert_eq!(s.take_shapeshift_casts(), vec![71]);
        assert!(s.take_shapeshift_casts().is_empty(), "drain empties");
    }
}
