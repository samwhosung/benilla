//! The shapeshift/stance bar seam — the ref's four bindings
//! (`GetNumShapeshiftForms`/`GetShapeshiftFormInfo`/`GetShapeshiftFormCooldown`/
//! `CastShapeshiftForm`, consumed by `BonusActionBarFrame.lua`'s `ShapeshiftBar_*` family) over
//! an app-pushed form list, the [`super::spellbook`]/[`super::action`] two-way shape: the app
//! resolves everything (which known spells are forms, their icon/name/active/castable/cooldown —
//! the mechanism lives app-side), pushes a snapshot
//! ([`super::UiScript::set_shapeshift_forms`]), and drains the click intents
//! ([`super::UiScript::take_shapeshift_casts`]) onto the wire. The engine holds no form
//! KNOWLEDGE — a form is "a spell id, a texture, a name, two bits, and a cooldown triple".
//!
//! Return conventions are the 1.12 API's own, matching [`super::action`]: 1/nil booleans, and
//! the cooldown pushed as `(start_ms on the GetTime clock, duration_ms, enabled)` (the
//! [`super::action::ActionState::cooldown`] pattern — absolute start, app-converted at feed
//! time) so `GetShapeshiftFormCooldown` answers the ref's `(start, duration, enable)`.

use mlua::{Lua, MultiValue, Value};

use super::Model;

/// One stance-bar button, fully resolved by the app before pushing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShapeshiftFormView {
    /// The form spell (the app resolves a queued cast/cancel from it at drain time).
    pub spell_id: u32,
    /// The icon texture path; `None` shows the slot's fallback.
    pub texture: Option<String>,
    /// The spell name (`GetShapeshiftFormInfo`'s second return — the ref's tooltip/debug read).
    pub name: String,
    /// This form is the player's CURRENT form (the checked ring).
    pub active: bool,
    /// Castable right now (the ref greys the icon 0.4 when not).
    pub castable: bool,
    /// The form spell's cooldown as `(start_ms on the GetTime clock, duration_ms, enabled)`
    /// ([`super::action::ActionState::cooldown`]'s exact shape); `None` = no cooldown.
    pub cooldown: Option<(i64, u32, bool)>,
}

/// [`ShapeshiftFormView`] as stored: the cooldown converted to the `GetTime` clock at push time
/// (the [`super::action::StoredActionState`] pattern).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct StoredShapeshiftForm {
    pub(crate) view: ShapeshiftFormView,
    /// `(start_s, duration_s, enabled)` in `GetTime` seconds; `None` = no cooldown.
    pub(crate) cooldown: Option<(f64, f64, bool)>,
}

impl super::UiScript {
    /// Push the whole form list (bar order = list order), replacing whatever was there. A bare
    /// setter — firing `UPDATE_SHAPESHIFT_FORMS` (and the state-refresh events the transcribed
    /// bar listens to) is the app's own diff-and-fire job, mirroring `set_spellbook`.
    pub fn set_shapeshift_forms(&mut self, forms: Vec<ShapeshiftFormView>) {
        let mut model = self.model_mut();
        // The cooldown arrives with its absolute start already on the `GetTime` clock (ms) —
        // storing is a pure unit conversion, the same seam shape as
        // [`super::UiScript::set_action_state`].
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

    /// Drain the form spell ids `CastShapeshiftForm` queued since the last call. Whether a queued
    /// id becomes a cast or a cancel (clicking the ACTIVE form) is the app's call at drain time.
    pub fn take_shapeshift_casts(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().shapeshift_casts)
    }
}

/// The 1-based button index → stored form, the ref's own indexing.
fn form_at(model: &Model, i: u32) -> Option<&StoredShapeshiftForm> {
    usize::try_from(i.checked_sub(1)?)
        .ok()
        .and_then(|n| model.shapeshift_forms.get(n))
}

/// Register the shapeshift globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    g.set(
        "GetNumShapeshiftForms",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.shapeshift_forms.len() as i64)
        })?,
    )?;

    // GetShapeshiftFormInfo(i) → texture, name, isActive (1/nil), isCastable (1/nil); out of
    // range → a single nil (the spellbook bindings' out-of-range shape).
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

    // GetShapeshiftFormCooldown(i) → start, duration, enable — GetActionCooldown's exact triple
    // and elapsed-goes-cold rule (an elapsed/absent cooldown answers (0, 0, 1) so a re-feed never
    // replays the sweep).
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

    // CastShapeshiftForm(i) — queues the form's spell id; an out-of-range index is a no-op.
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

    /// A warrior mid-career: Battle Stance (active, castable), Defensive Stance (castable, on a
    /// 1.5 s cooldown started at GetTime 9.4 s).
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
        // Form 2: not active (nil), castable (1).
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

        // No cooldown → the cold (0, 0, 1).
        assert_eq!(
            s.eval::<(f64, f64, i32)>("return GetShapeshiftFormCooldown(1)")
                .unwrap(),
            (0.0, 0.0, 1)
        );
        // The pushed absolute start (9400 ms) reads back verbatim in seconds.
        let (start, duration, enable) = s
            .eval::<(f64, f64, i32)>("return GetShapeshiftFormCooldown(2)")
            .unwrap();
        assert!((start - 9.4).abs() < 1e-9, "start {start}");
        assert!((duration - 1.5).abs() < 1e-9);
        assert_eq!(enable, 1);

        // Past the end the read goes cold.
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
