//! Player macros. 1.12 keeps them client-side with no opcode (`macros-cache.txt`), and
//! `CreateMacro` must return the new index at once, so the table lives here: the app seeds it
//! ([`UiScript::set_macros`]), saves it when [`UiScript::take_macros_dirty`] says, and pushes the
//! icon list ([`UiScript::set_macro_icons`]).
//!
//! Indices 1..=18 are the account macros and 19..=36 the character macros, each list dense from
//! its base (`MacroFrame.macroBase`). A macro's bound spell (`0x4e5a50`'s macro arm returns
//! `[rec+0x564]`) is the app's to derive: it needs the spell catalog and the player's book.

use mlua::{Lua, MultiValue, Value};

use super::cursor::{set_cursor, CursorMacro, CursorPayload};
use super::Model;
use crate::script::binding_abi::Idx;

/// Macros per tab (`Blizzard_MacroUI.lua:1`), and so the character tab's base.
pub const MAX_MACROS: usize = 18;

/// The name cap, `MacroPopupEditBox`'s `letters="16"`.
pub const MAX_MACRO_NAME: usize = 16;

/// The body cap, `MacroFrameText`'s `letters="255"`.
pub const MAX_MACRO_BODY: usize = 255;

/// One macro, as `GetMacroInfo` reports it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MacroView {
    pub name: String,
    /// The icon's texture path, kept by name because the reference saves the icon by name
    /// (`MACRO %d "%s" %s`, `0x44cb60`).
    pub texture: Option<String>,
    /// The macro's lines, `\n`-separated.
    pub body: String,
    /// `isLocal` and the `local` argument, carried but not saved here. The 1.12 client saves these
    /// to `macros-local.txt` beside `macros-cache.txt` (`0x45de74`, `0x45de88`); the stock UI never
    /// sets it.
    pub local_only: bool,
}

/// A macro record's cached cast, the reference's `([rec+0x564], [rec+0x568])` pair (`0x4efe00`):
/// the bound spell and the book it resolved from, which the app derives.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MacroBinding {
    /// The bound spell id; 0 for none, -1 for a `/cast` that did not resolve.
    pub spell: i32,
    /// The spell came from the pet's book (`0x4b3b51` writes 1), not the player's.
    pub pet_book: bool,
}

/// The macro table: the account and character lists.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MacroState {
    /// Indices 1..=18.
    pub account: Vec<MacroView>,
    /// Indices 19..=36.
    pub character: Vec<MacroView>,
}

/// A 1-based Lua macro index split into its list and 0-based position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MacroIndex {
    /// True for 19..=36.
    per_character: bool,
    pos: usize,
}

impl MacroIndex {
    /// `None` outside 1..=36; not checked against the lists, which [`MacroState::get`] does.
    fn split(index: usize) -> Option<Self> {
        let zero = index.checked_sub(1)?;
        match zero {
            0..MAX_MACROS => Some(Self {
                per_character: false,
                pos: zero,
            }),
            _ if zero < MAX_MACROS * 2 => Some(Self {
                per_character: true,
                pos: zero - MAX_MACROS,
            }),
            _ => None,
        }
    }

    fn lua_index(self) -> usize {
        self.pos + 1 + if self.per_character { MAX_MACROS } else { 0 }
    }
}

impl MacroState {
    /// The macro at a 1-based Lua index.
    pub fn get(&self, index: usize) -> Option<&MacroView> {
        let at = MacroIndex::split(index)?;
        self.list(at.per_character).get(at.pos)
    }

    fn list(&self, per_character: bool) -> &Vec<MacroView> {
        if per_character {
            &self.character
        } else {
            &self.account
        }
    }

    fn list_mut(&mut self, per_character: bool) -> &mut Vec<MacroView> {
        if per_character {
            &mut self.character
        } else {
            &mut self.account
        }
    }

    /// `GetMacroIndexByName`: case-insensitive, account list first; 0 on a miss, as in 1.12.
    pub fn index_by_name(&self, name: &str) -> usize {
        for per_character in [false, true] {
            for (i, m) in self.list(per_character).iter().enumerate() {
                if m.name.eq_ignore_ascii_case(name) {
                    return MacroIndex {
                        per_character,
                        pos: i,
                    }
                    .lua_index();
                }
            }
        }
        0
    }
}

/// Clamp to a count of characters, not bytes, as the edit boxes do, so a script cannot store more.
fn clamp_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

impl super::UiScript {
    /// Seed the whole table from the app's load. It does not mark the table dirty, or the load
    /// would trigger a save.
    pub fn set_macros(&mut self, state: MacroState) {
        let mut model = self.model_mut();
        model.macros = state;
        model.macros_dirty = false;
        model.macros_generation += 1;
    }

    /// The live table, for the app's save.
    pub fn macros(&self) -> MacroState {
        self.model_mut().macros.clone()
    }

    /// Push every macro's cached cast by 1-based index, replacing the last push.
    pub fn set_macro_bindings(&mut self, bindings: std::collections::HashMap<u32, MacroBinding>) {
        self.model_mut().macro_bindings = bindings;
    }

    /// Whether a script changed the table since the last call; the app then saves it and fires
    /// `UPDATE_MACROS` (`0x452460`).
    pub fn take_macros_dirty(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().macros_dirty)
    }

    /// Push the icon chooser's texture paths in grid order, built by the app from `SpellIcon.dbc`.
    pub fn set_macro_icons(&mut self, icons: Vec<String>) {
        self.model_mut().macro_icons = icons;
    }

    /// Bumped by every seed and every change, so a per-frame reader such as the action bar compares
    /// a `u64` instead of diffing the table. Not [`Self::take_macros_dirty`]: that drain has one
    /// owner, the save, and a second reader would steal its edge.
    pub fn macros_generation(&self) -> u64 {
        self.model_mut().macros_generation
    }
}

/// `CreateMacro(name, iconIndex, body, local, perCharacter)` (usage string `0x84cb74`): the new
/// 1-based index, or the error the client raises for the two failures (`0x84cbb4`, `0x84cbdc`), an
/// empty name and a full tab.
fn create_macro(
    model: &mut Model,
    name: &str,
    texture: Option<String>,
    body: &str,
    local_only: bool,
    per_character: bool,
) -> mlua::Result<usize> {
    // Tested on the first byte (`*name == 0`): a blank name is a name.
    if name.is_empty() {
        return Err(mlua::Error::RuntimeError(
            "CreateMacro() failed, no name specified".into(),
        ));
    }
    let name = clamp_chars(name, MAX_MACRO_NAME);
    let list = model.macros.list_mut(per_character);
    if list.len() >= MAX_MACROS {
        return Err(mlua::Error::RuntimeError(format!(
            "CreateMacro() failed, already have {MAX_MACROS} macros"
        )));
    }
    list.push(MacroView {
        name,
        texture,
        body: clamp_chars(body, MAX_MACRO_BODY),
        local_only,
    });
    let pos = list.len() - 1;
    model.macros_dirty = true;
    model.macros_generation += 1;
    Ok(MacroIndex { per_character, pos }.lua_index())
}

/// `EditMacro(index, name, icon, body, local)`: an omitted name, icon or body leaves its field
/// alone, since the stock UI calls it with disjoint halves, `(sel, name, icon)` from the rename
/// popup and `(sel, nil, nil, text)` from `MacroFrame_SaveMacro`. The `local` flag is always
/// written (`0x4f12dc`, `[macro+0x20]`), so an omitted one clears it.
fn edit_macro(
    model: &mut Model,
    index: usize,
    name: Option<String>,
    texture: Option<Option<String>>,
    body: Option<String>,
    local_only: bool,
) -> Option<usize> {
    let at = MacroIndex::split(index)?;
    let entry = model.macros.list_mut(at.per_character).get_mut(at.pos)?;
    if let Some(name) = name {
        let name = clamp_chars(name.trim(), MAX_MACRO_NAME);
        // A blank name is ignored; the stock popup cannot send one (`MacroPopupOkayButton_Update`).
        if !name.is_empty() {
            entry.name = name;
        }
    }
    if let Some(texture) = texture {
        entry.texture = texture;
    }
    if let Some(body) = body {
        entry.body = clamp_chars(&body, MAX_MACRO_BODY);
    }
    entry.local_only = local_only;
    model.macros_dirty = true;
    model.macros_generation += 1;
    Some(index)
}

/// `DeleteMacro(index)` closes the gap, as the reference's dense list does, so an action-bar
/// button holding a higher index then shows the next macro.
fn delete_macro(model: &mut Model, index: usize) -> bool {
    let Some(at) = MacroIndex::split(index) else {
        return false;
    };
    let list = model.macros.list_mut(at.per_character);
    if at.pos >= list.len() {
        return false;
    }
    list.remove(at.pos);
    model.macros_dirty = true;
    model.macros_generation += 1;
    true
}

/// `PickupMacro(index)` (`0x4f1ae0`): the macro payload (cursor mode 8, `[0xb4e2fc]`), which
/// `PlaceAction` (`0x4e62e0`) packs as `macroId | 0x40000000`. A missing macro does nothing
/// (`0x4f1b1e`); otherwise the setter (`0x494f80`) clears whatever is held with its inner
/// `ClearCursor(1,1)`, then sets the macro and shows the action bar's grid. Returns whether a
/// macro went on.
fn pickup_macro(lua: &Lua, index: usize) -> bool {
    let payload = {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        let Some(entry) = model.macros.get(index) else {
            return false;
        };
        CursorMacro {
            index: index as u32,
            texture: entry.texture.clone(),
        }
    };
    set_cursor(lua, true, |_| Some(CursorPayload::Macro(payload)))
}

fn opt_string(v: Option<&Value>) -> Option<String> {
    match v {
        Some(Value::String(s)) => Some(s.to_string_lossy()),
        _ => None,
    }
}

/// The `local` and `perCharacter` flags: `GetBoolOrDefault` (`0x6f1c10`) with default 0
/// (`CreateMacro` `0x4f167b`, `0x4f168a`; `EditMacro` `0x4f19ad`), so `"0"` and `0.5` are false and
/// `"yes"` true. The stock popup passes a Lua boolean and never `local`.
fn flag(v: Option<&Value>) -> bool {
    super::binding_abi::bool_or_default(v, false)
}

/// Register the macro globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    g.set(
        "GetNumMacros",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok((
                model.macros.account.len() as i64,
                model.macros.character.len() as i64,
            ))
        })?,
    )?;

    g.set(
        "GetMacroInfo",
        lua.create_function(|lua, Idx(index): Idx| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let Some(m) = model.macros.get(index) else {
                // `0x4f1829`: four nils.
                return Ok(super::binding_abi::tuple(&[None; 4]));
            };
            let texture = match &m.texture {
                Some(t) => Value::String(lua.create_string(t)?),
                None => Value::Nil,
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&m.name)?),
                texture,
                Value::String(lua.create_string(&m.body)?),
                if m.local_only {
                    Value::Integer(1)
                } else {
                    Value::Nil
                },
            ]))
        })?,
    )?;

    g.set(
        "GetMacroIndexByName",
        lua.create_function(|lua, name: String| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.macros.index_by_name(&name) as i64)
        })?,
    )?;

    g.set(
        "GetNumMacroIcons",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.macro_icons.len() as i64)
        })?,
    )?;

    g.set(
        "GetMacroIconInfo",
        lua.create_function(|lua, Idx(index): Idx| {
            let path = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                index
                    .checked_sub(1)
                    .and_then(|i| model.macro_icons.get(i))
                    .cloned()
            };
            match path {
                Some(p) => Ok(Value::String(lua.create_string(&p)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // CreateMacro(name, iconIndex, body, local, perCharacter) (`0x4f15b0`): the name is a string
    // or number and the icon a number or numeric string, else the usage text (`0x84cb74`) raises.
    // `iconIndex` is an index into the chooser list. An empty name raises (`0x84cbb4`, tested on
    // the first byte, so a blank one is a name), and so does a full tab (`0x84cbdc`); a made macro
    // answers exactly its new 1-based index (`0x4f16ce`). Not modelled: the script-context gate
    // `0x494a50(7)` ahead of the body, which has no taint to read here.
    g.set(
        "CreateMacro",
        lua.create_function(|lua, args: MultiValue| {
            let a: Vec<Value> = args.into_iter().collect();
            let usage = || {
                mlua::Error::RuntimeError(
                    "Usage: CreateMacro(name, iconIndex, body, local, perCharacter)".into(),
                )
            };
            let name = super::binding_abi::string_arg(
                lua,
                a.first().cloned().unwrap_or(Value::Nil),
                "Usage: CreateMacro(name, iconIndex, body, local, perCharacter)",
            )?;
            let icon = lua
                .coerce_number(a.get(1).cloned().unwrap_or(Value::Nil))?
                .ok_or_else(usage)?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            // `trunc(n) - 1` against the chooser list, unsigned (`0x4f1632`, `0x4f163c`).
            let texture = usize::try_from(super::binding_abi::ftol(icon).wrapping_sub(1))
                .ok()
                .and_then(|i| model.macro_icons.get(i).cloned());
            let body = a
                .get(2)
                .and_then(|v| super::binding_abi::optional_string(lua, v))
                .unwrap_or_default();
            let (local_only, per_character) = (flag(a.get(3)), flag(a.get(4)));
            create_macro(&mut model, &name, texture, &body, local_only, per_character)
                .map(|i| Value::Integer(i as i64))
        })?,
    )?;

    g.set(
        "EditMacro",
        lua.create_function(|lua, args: MultiValue| {
            let a: Vec<Value> = args.into_iter().collect();
            let Some(index) = a.first().and_then(|v| v.as_integer()).map(|i| i as usize) else {
                return Ok(Value::Nil);
            };
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            // A nil icon leaves the icon alone: `MacroFrame_SaveMacro` passes nil on every save.
            let texture = match a.get(2) {
                None | Some(Value::Nil) => None,
                other => Some(icon_path(&model, other)),
            };
            let name = opt_string(a.get(1));
            let body = opt_string(a.get(3));
            let local_only = flag(a.get(4));
            Ok(
                match edit_macro(&mut model, index, name, texture, body, local_only) {
                    Some(i) => Value::Integer(i as i64),
                    None => Value::Nil,
                },
            )
        })?,
    )?;

    g.set(
        "DeleteMacro",
        lua.create_function(|lua, index: usize| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            delete_macro(&mut model, index);
            Ok(())
        })?,
    )?;

    g.set(
        "PickupMacro",
        lua.create_function(|lua, index: usize| {
            pickup_macro(lua, index);
            Ok(())
        })?,
    )?;

    Ok(())
}

/// Resolve a chooser-list index to its texture path; a string is taken as a path already.
fn icon_path(model: &Model, v: Option<&Value>) -> Option<String> {
    match v {
        Some(Value::String(s)) => Some(s.to_string_lossy()),
        Some(v) => {
            let i = v.as_integer()?;
            let i = usize::try_from(i).ok()?.checked_sub(1)?;
            model.macro_icons.get(i).cloned()
        }
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    fn seeded() -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.set_macro_icons(vec![
            "Interface\\Icons\\Ability_Ambush".into(),
            "Interface\\Icons\\Ability_BackStab".into(),
            "Interface\\Icons\\Spell_Fire_FlameBolt".into(),
        ]);
        s
    }

    #[test]
    fn the_macro_api_round_trip_over_the_two_tabs() {
        let s = seeded();
        assert_eq!(
            s.eval::<(i64, i64)>("local a, c = GetNumMacros() return a, c")
                .unwrap(),
            (0, 0)
        );

        let i = s
            .eval::<i64>(r#"return CreateMacro("Ambush", 1, "/cast Ambush", nil, nil)"#)
            .unwrap();
        assert_eq!(i, 1);
        let j = s
            .eval::<i64>(r#"return CreateMacro("Bolt", 3, "/cast Fireball", nil, true)"#)
            .unwrap();
        assert_eq!(j, 19, "the character tab starts at MAX_MACROS + 1");
        assert_eq!(
            s.eval::<(i64, i64)>("local a, c = GetNumMacros() return a, c")
                .unwrap(),
            (1, 1)
        );

        let (name, tex, body, is_local) = s
            .eval::<(String, String, String, Value)>(
                "local n, t, b, l = GetMacroInfo(1) return n, t, b, l",
            )
            .unwrap();
        assert_eq!(
            (name.as_str(), tex.as_str(), body.as_str()),
            ("Ambush", "Interface\\Icons\\Ability_Ambush", "/cast Ambush")
        );
        assert_eq!(is_local, Value::Nil);

        assert_eq!(
            s.eval::<i64>(r#"return GetMacroIndexByName("ambush")"#)
                .unwrap(),
            1
        );
        assert_eq!(
            s.eval::<i64>(r#"return GetMacroIndexByName("bolt")"#)
                .unwrap(),
            19
        );
        assert_eq!(
            s.eval::<i64>(r#"return GetMacroIndexByName("nope")"#)
                .unwrap(),
            0
        );

        // The character macro keeps its own base after the delete.
        assert!(delete_macro(&mut s.model_mut(), 1));
        assert_eq!(
            s.eval::<(i64, i64)>("local a, c = GetNumMacros() return a, c")
                .unwrap(),
            (0, 1)
        );
        assert!(s.eval::<bool>("return GetMacroInfo(1) == nil").unwrap());
        assert_eq!(s.eval::<String>("return GetMacroInfo(19)").unwrap(), "Bolt");
    }

    #[test]
    fn edit_macro_leaves_omitted_fields_alone() {
        let s = seeded();
        s.run(r#"CreateMacro("Old", 1, "/cast Ambush")"#).unwrap();

        // MacroPopupOkayButton_OnClick's form: name and icon.
        s.run(r#"EditMacro(1, "New", 2)"#).unwrap();
        let (name, tex, body) = s
            .eval::<(String, String, String)>("local n, t, b = GetMacroInfo(1) return n, t, b")
            .unwrap();
        assert_eq!(name, "New");
        assert_eq!(tex, "Interface\\Icons\\Ability_BackStab");
        assert_eq!(body, "/cast Ambush", "the body survives a rename");

        // MacroFrame_SaveMacro's form: the body alone.
        s.run(r#"EditMacro(1, nil, nil, "/cast Backstab")"#)
            .unwrap();
        let (name, tex, body) = s
            .eval::<(String, String, String)>("local n, t, b = GetMacroInfo(1) return n, t, b")
            .unwrap();
        assert_eq!(
            (name.as_str(), tex.as_str(), body.as_str()),
            (
                "New",
                "Interface\\Icons\\Ability_BackStab",
                "/cast Backstab"
            ),
            "a body save touches neither the name nor the icon"
        );
    }

    #[test]
    fn create_refuses_a_blank_name_and_a_full_tab_and_clamps_the_caps() {
        let s = seeded();
        let raised = |s: &UiScript, call: &str| s.run(call).expect_err(call).to_string();
        assert!(raised(&s, r#"CreateMacro("", 1, "x")"#).contains("failed, no name specified"));
        // Tested on the first byte, not trimmed: a blank name is a name (`0x4f1610`).
        assert_eq!(
            s.eval::<i64>(r#"return CreateMacro("   ", 1, "x")"#)
                .unwrap(),
            1
        );
        s.run("DeleteMacro(1)").unwrap();
        // A non-string name, a non-numeric icon and a missing icon raise the usage text.
        for call in [
            "CreateMacro()",
            "CreateMacro({}, 1)",
            "CreateMacro('a')",
            "CreateMacro('a', 'x')",
            "CreateMacro('a', nil, 'b')",
        ] {
            assert!(
                raised(&s, call)
                    .contains("Usage: CreateMacro(name, iconIndex, body, local, perCharacter)"),
                "{call}"
            );
        }
        // A number is a name, a numeric string an icon, and exactly one value is pushed.
        assert_eq!(
            s.eval::<i64>("return (function(...) return arg.n end)(CreateMacro(12, '1', 'b'))")
                .unwrap(),
            1
        );
        s.run("DeleteMacro(1)").unwrap();

        for i in 1..=MAX_MACROS {
            let made = s
                .eval::<i64>(&format!(r#"return CreateMacro("m{i}", 1, "")"#))
                .unwrap();
            assert_eq!(made as usize, i);
        }
        assert!(
            raised(&s, r#"CreateMacro("one too many", 1, "")"#)
                .contains("CreateMacro() failed, already have 18 macros"),
            "the 19th account macro is refused"
        );
        // The character tab is a separate 18.
        assert_eq!(
            s.eval::<i64>(r#"return CreateMacro("c", 1, "", nil, 1)"#)
                .unwrap(),
            19
        );

        let s = seeded();
        s.run(&format!(
            r#"CreateMacro("{}", 1, "{}")"#,
            "N".repeat(40),
            "b".repeat(400)
        ))
        .unwrap();
        let (name, body) = s
            .eval::<(String, String)>("local n, _, b = GetMacroInfo(1) return n, b")
            .unwrap();
        assert_eq!(name.chars().count(), MAX_MACRO_NAME);
        assert_eq!(body.chars().count(), MAX_MACRO_BODY);
    }

    #[test]
    fn mutations_raise_the_dirty_flag_and_a_seed_does_not() {
        let mut s = seeded();
        assert!(!s.take_macros_dirty());

        s.run(r#"CreateMacro("a", 1, "")"#).unwrap();
        assert!(s.take_macros_dirty());
        assert!(!s.take_macros_dirty(), "the drain clears it");

        s.run(r#"EditMacro(1, nil, nil, "/say hi")"#).unwrap();
        assert!(s.take_macros_dirty());

        s.run("DeleteMacro(1)").unwrap();
        assert!(s.take_macros_dirty());

        s.run("DeleteMacro(7)").unwrap();
        assert!(!s.take_macros_dirty());

        s.set_macros(MacroState {
            account: vec![MacroView {
                name: "loaded".into(),
                ..Default::default()
            }],
            character: Vec::new(),
        });
        assert!(!s.take_macros_dirty());
        assert_eq!(s.macros().account.len(), 1);
    }

    #[test]
    fn pickup_macro_loads_the_cursor_over_whatever_is_held() {
        use crate::script::cursor::CursorPayload;

        let s = seeded();
        s.run(r#"CreateMacro("Ambush", 1, "/cast Ambush")"#)
            .unwrap();

        assert!(!pickup_macro(&s.lua, 5), "empty slot");
        assert!(s.cursor_payload().is_none());
        assert!(pickup_macro(&s.lua, 1));
        let ambush = Some(CursorPayload::Macro(CursorMacro {
            index: 1,
            texture: Some("Interface\\Icons\\Ability_Ambush".into()),
        }));
        assert_eq!(s.cursor_payload(), ambush);
        // The setter's inner clear drops what is held, then sets (`0x494fad`).
        assert!(pickup_macro(&s.lua, 1));
        assert_eq!(s.cursor_payload(), ambush);
    }

    #[test]
    fn the_icon_chooser_list_is_one_based_with_a_nil_tail() {
        let s = seeded();
        assert_eq!(s.eval::<i64>("return GetNumMacroIcons()").unwrap(), 3);
        assert_eq!(
            s.eval::<String>("return GetMacroIconInfo(2)").unwrap(),
            "Interface\\Icons\\Ability_BackStab"
        );
        assert!(s.eval::<bool>("return GetMacroIconInfo(4) == nil").unwrap());
        assert!(s.eval::<bool>("return GetMacroIconInfo(0) == nil").unwrap());
    }

    /// The macro setter (`0x494f80`) signals only through its inner clear (`0x494fad`): that
    /// `CURSOR_UPDATE` reads the empty cursor, and the set ends on the grid (`0x494fd7`).
    #[test]
    fn a_macro_pickup_signals_its_clear_then_shows_the_grid() {
        let s = seeded();
        s.run(r#"CreateMacro("Ambush", 1, "/cast Ambush")"#)
            .unwrap();
        crate::script::cursor::install_kind_probe(&s);
        crate::script::tick::heard::listen(
            &s,
            &["CURSOR_UPDATE", "ACTIONBAR_SHOWGRID", "ACTIONBAR_HIDEGRID"],
            Some("CursorKind()"),
        );
        s.run("PickupMacro(1)").unwrap();
        assert_eq!(
            crate::script::tick::heard::take(&s),
            ["CURSOR_UPDATE none", "ACTIONBAR_SHOWGRID macro"]
        );
        s.run("PickupMacro(1)").unwrap();
        assert_eq!(
            crate::script::tick::heard::take(&s),
            [
                "ACTIONBAR_HIDEGRID macro",
                "CURSOR_UPDATE none",
                "ACTIONBAR_SHOWGRID macro"
            ]
        );
    }
}
