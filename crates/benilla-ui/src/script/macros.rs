//! Player **macros** (decision 0983) — the one seam in this crate that owns its game state
//! outright, because the 1.12 macro system has no server side at all: there is no macro opcode on
//! the wire (vmangos has none, and the client's own `UIMacros.cpp` persists to
//! `WTF/…/macros-cache.txt`), so the macro table *is* client state. Contrast [`super::action`],
//! whose 120-slot table the app owns because the server hands it back at login.
//!
//! That ownership is forced by the reference's own API shape, not chosen for convenience:
//! `MacroPopupOkayButton_OnClick` does `index = CreateMacro(…)` and immediately selects `index`,
//! so the mutation must be synchronous and must return the new slot — a queue-an-intent-and-wait
//! seam cannot answer it. The app therefore **seeds** the table once ([`UiScript::set_macros`],
//! from `benilla-config/macros/…`), **reads** it back to persist ([`UiScript::macros`]) whenever
//! [`UiScript::take_macros_dirty`] says something moved, and pushes the icon-chooser list
//! ([`UiScript::set_macro_icons`]) it builds off `SpellIcon.dbc`.
//!
//! ## The index space (VERIFIED, the shipped `Blizzard_MacroUI`)
//!
//! `MAX_MACROS = 18` per tab; `MacroFrame.macroBase` is `0` for the account tab and `18` for the
//! character tab, and every binding takes `macroBase + i`. So **1..=18 are the account macros and
//! 19..=36 the character macros**, each list dense from its base — `MacroFrame_Update` draws
//! `i <= numMacros` and disables the rest, so a gap is not representable. [`MacroIndex`] is that
//! split, made once at the boundary.
//!
//! ## What the engine does NOT know
//!
//! A macro's **bound spell** — the spell whose cooldown/usability/range a macro action-bar slot
//! reports (`0x4e5a50`'s macro arm returns `[rec+0x564]`) — is the app's derivation, because
//! resolving a name to a spell id needs the catalog and the player's book. The engine stores the
//! body; `benilla::ui_macro` parses it. Same split as everywhere else in this crate: data and
//! layout here, game knowledge
//! there.

use mlua::{Lua, MultiValue, Value};

use super::cursor::{queue_cursor_update, CursorMacro, CursorPayload};
use super::Model;

/// Macros per tab — the shipped `Blizzard_MacroUI.lua`'s own `MAX_MACROS = 18`, and the account
/// tab's size *and* the character tab's base (`MacroFrame_SetCharacterMacros` sets
/// `macroBase = MAX_MACROS`).
pub const MAX_MACROS: usize = 18;

/// The macro **name** length cap — `MacroPopupEditBox`'s `letters="16"` and its label
/// `MACRO_POPUP_TEXT = "Enter Macro Name (Max 16 Characters):"`. Enforced here as well as in the
/// box, because `CreateMacro`/`EditMacro` are callable from any script.
pub const MAX_MACRO_NAME: usize = 16;

/// The macro **body** length cap — `MacroFrameText`'s `letters="255"` and its counter
/// `MACROFRAME_CHAR_LIMIT = "%d/255 Characters Used"`.
pub const MAX_MACRO_BODY: usize = 255;

/// One macro, as `GetMacroInfo` reports it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MacroView {
    /// The player's name for it (≤ [`MAX_MACRO_NAME`] characters).
    pub name: String,
    /// The chosen icon's **texture path** (`Interface\Icons\Ability_Ambush`). Stored resolved
    /// rather than as an index into the chooser list: the reference's own save format writes the
    /// icon by NAME (`MACRO %d "%s" %s`, byte-verified at `0x44cb60`), so a name is what survives
    /// a restart — an index would silently re-point if the list ever changed.
    pub texture: Option<String>,
    /// The macro body: the lines run when the macro is used, `\n`-separated, ≤ [`MAX_MACRO_BODY`].
    pub body: String,
    /// `GetMacroInfo`'s fourth return (`isLocal`) and `CreateMacro`/`EditMacro`'s `local`
    /// argument. The 1.12 client keeps a second file for these (`macros-local.txt` beside
    /// `macros-cache.txt`, both byte-verified strings at `0x45de74`/`0x45de88`); the shipped macro
    /// UI never passes the flag, so it is carried faithfully and is otherwise inert.
    pub local_only: bool,
}

/// The whole macro table: the two dense lists behind the frame's two tabs (module docs).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MacroState {
    /// Indices 1..=18.
    pub account: Vec<MacroView>,
    /// Indices 19..=36.
    pub character: Vec<MacroView>,
}

/// A 1-based Lua macro index resolved into `(which list, position in it)` — the one place the
/// 1..36 space is split, so no binding re-derives the base.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MacroIndex {
    /// True for 19..=36 (the character tab).
    per_character: bool,
    /// 0-based position within that tab's list.
    pos: usize,
}

impl MacroIndex {
    /// Split a 1-based Lua index; `None` outside 1..=36. Deliberately NOT range-checked against
    /// the live lists — callers that need an occupied slot go through [`MacroState::get`].
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

    /// The 1-based Lua index this position occupies.
    fn lua_index(self) -> usize {
        self.pos + 1 + if self.per_character { MAX_MACROS } else { 0 }
    }
}

impl MacroState {
    /// The macro at a 1-based Lua index, or `None` for an empty/out-of-range slot.
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

    /// `GetMacroIndexByName`'s search — case-insensitive, account list first (the reference walks
    /// the one flat 1..36 space, and the account half is its low end). `0` when nothing matches,
    /// which is the reference's own miss value (a number, never nil — the shipped
    /// `Usage: GetMacroIndexByName(name)` binding pushes a number).
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

/// Clamp a player-supplied string to a character (not byte) cap — the EditBox's own
/// `max_letters` rule, applied again at the API so a script cannot store what the box refuses.
fn clamp_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

impl super::UiScript {
    /// Seed the whole macro table, replacing whatever was there — the app's load path
    /// (`benilla-config/macros/…`). Does **not** mark the table dirty: the app already has what it just
    /// handed over, and a save triggered by its own load would be a write-back loop.
    pub fn set_macros(&mut self, state: MacroState) {
        let mut model = self.model_mut();
        model.macros = state;
        model.macros_dirty = false;
        model.macros_generation += 1;
    }

    /// Read the live macro table — the app's save path, taken when [`Self::take_macros_dirty`]
    /// reports a change.
    pub fn macros(&self) -> MacroState {
        self.model_mut().macros.clone()
    }

    /// Did a script mutate the table since the last call (`CreateMacro`/`EditMacro`/`DeleteMacro`)?
    /// The app persists on a `true` and fires `UPDATE_MACROS` — the reference's own event for
    /// exactly this transition (byte-verified string at `0x452460`).
    pub fn take_macros_dirty(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().macros_dirty)
    }

    /// Push the icon-chooser list — the full texture paths `GetMacroIconInfo` serves, in the order
    /// the popup's grid shows them. Built by the app off `SpellIcon.dbc` (`benilla::ui_macro`).
    pub fn set_macro_icons(&mut self, icons: Vec<String>) {
        self.model_mut().macro_icons = icons;
    }

    /// The macro table's **generation** — bumped by every seed and every mutation, so a per-frame
    /// consumer can gate an expensive re-resolve on a `u64` compare instead of cloning the table
    /// to diff it. The action bar's identity feed reads it as a third input beside its `dirty`
    /// flag and the item-template epoch (the same shape decision 0660 gave that one): a macro's
    /// icon changes when the macro is edited, which touches neither of the other two.
    ///
    /// Distinct from [`Self::take_macros_dirty`] on purpose — that one is a *drained* edge with
    /// exactly one owner (the save), and a second consumer draining it would silently eat the
    /// other's save.
    pub fn macros_generation(&self) -> u64 {
        self.model_mut().macros_generation
    }
}

/// `CreateMacro(name, iconIndex, body, local, perCharacter)` — the shipped binding's own argument
/// list (byte-verified usage string at `0x44cb74`). Returns the new macro's 1-based index, or
/// `None` on the two failures the client names in its own log lines (`0x44cbb4`/`0x44cbdc`):
/// an empty name, and a full tab.
fn create_macro(
    model: &mut Model,
    name: &str,
    texture: Option<String>,
    body: &str,
    local_only: bool,
    per_character: bool,
) -> Option<usize> {
    let name = clamp_chars(name.trim(), MAX_MACRO_NAME);
    if name.is_empty() {
        model.record_warning("CreateMacro() failed, no name specified");
        return None;
    }
    let list = model.macros.list_mut(per_character);
    if list.len() >= MAX_MACROS {
        model.record_warning(format!(
            "CreateMacro() failed, already have {MAX_MACROS} macros"
        ));
        return None;
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
    Some(MacroIndex { per_character, pos }.lua_index())
}

/// `EditMacro(index, name, icon, body, local)` — every argument is optional past the index, and an
/// omitted one leaves that field alone. That is load-bearing, not defensive: the shipped UI calls
/// it **twice with disjoint halves** — `EditMacro(sel, name, icon)` from the rename popup and
/// `EditMacro(sel, nil, nil, text)` from `MacroFrame_SaveMacro` — so a nil that overwrote would
/// blank the body every time the name changed.
fn edit_macro(
    model: &mut Model,
    index: usize,
    name: Option<String>,
    texture: Option<Option<String>>,
    body: Option<String>,
    local_only: Option<bool>,
) -> Option<usize> {
    let at = MacroIndex::split(index)?;
    let entry = model.macros.list_mut(at.per_character).get_mut(at.pos)?;
    if let Some(name) = name {
        let name = clamp_chars(name.trim(), MAX_MACRO_NAME);
        // An explicit blank is refused rather than stored: `MacroPopupOkayButton_Update` already
        // keeps OKAY disabled on an empty box, so reaching here means a script did it.
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
    if let Some(local_only) = local_only {
        entry.local_only = local_only;
    }
    model.macros_dirty = true;
    model.macros_generation += 1;
    Some(index)
}

/// `DeleteMacro(index)` — removes the slot and **closes the gap**, which is why every action-bar
/// slot holding a higher macro index would now point at the wrong macro. The reference has the
/// same property (its own list is dense, and `MacroFrame_Update` draws it densely); the app's
/// action feed re-resolves against the shifted table, so a bar button follows the slot, not the
/// macro. Named in decision 0983 as faithful-and-surprising rather than fixed.
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

/// `PickupMacro(index)` — the macro button's `OnDragStart` and the selected-macro button's
/// `OnClick` (both in the shipped `Blizzard_MacroUI.xml`). Loads the cursor with the macro
/// payload (the client's mode **8**, `[0xb4e2fc]`), which `PlaceAction 0x4e62e0` then packs as
/// `macroId | 0x40000000`.
///
/// Refuses while the cursor already holds something, matching `PickupSpell`'s precedent: a macro
/// button is a SOURCE, never a fit-checked drop target, so silently discarding the held payload
/// would be worse than doing nothing.
fn pickup_macro(model: &mut Model, index: usize) -> bool {
    if model.cursor.is_some() {
        return false;
    }
    let Some(entry) = model.macros.get(index) else {
        return false;
    };
    let payload = CursorMacro {
        index: index as u32,
        texture: entry.texture.clone(),
    };
    model.cursor = Some(CursorPayload::Macro(payload));
    queue_cursor_update(model);
    true
}

/// Coerce a Lua argument that the shipped UI passes as either a real value or `nil`/`false`.
fn opt_string(v: Option<&Value>) -> Option<String> {
    match v {
        Some(Value::String(s)) => Some(s.to_string_lossy()),
        _ => None,
    }
}

/// The reference's own truthiness for the `local`/`perCharacter` flags: `MacroPopupOkayButton_OnClick`
/// passes the Lua boolean `(MacroFrame.macroBase > 0)`, and the shipped `local` argument is always
/// `nil`. `0` reads falsy here (the numeric convention `UseAction`'s `checkCursor` established —
/// `super::action::truthy_nonzero`'s law, shared so the two cannot drift).
fn flag(v: Option<&Value>) -> bool {
    v.is_some_and(super::action::truthy_nonzero)
}

/// Register the macro globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetNumMacros() -> numAccountMacros, numCharacterMacros.
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

    // GetMacroInfo(index) -> name, texture, body, isLocal. An empty/out-of-range slot answers a
    // single nil (the out-of-range shape every list binding in this crate uses).
    g.set(
        "GetMacroInfo",
        lua.create_function(|lua, index: usize| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let Some(m) = model.macros.get(index) else {
                return Ok(MultiValue::from_vec(vec![Value::Nil]));
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

    // GetMacroIndexByName(name) -> index (0 = no match).
    g.set(
        "GetMacroIndexByName",
        lua.create_function(|lua, name: String| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.macros.index_by_name(&name) as i64)
        })?,
    )?;

    // GetNumMacroIcons() -> the chooser list's length.
    g.set(
        "GetNumMacroIcons",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.macro_icons.len() as i64)
        })?,
    )?;

    // GetMacroIconInfo(index) -> texture path; 1-based, out of range -> nil.
    g.set(
        "GetMacroIconInfo",
        lua.create_function(|lua, index: usize| {
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

    // CreateMacro(name, iconIndex, body, local, perCharacter) -> index | nil.
    // `iconIndex` is an index into the chooser list (the shipped popup passes
    // `MacroPopupFrame.selectedIcon`), resolved to a path HERE so the stored macro keeps a name
    // that survives a restart.
    g.set(
        "CreateMacro",
        lua.create_function(|lua, args: MultiValue| {
            let a: Vec<Value> = args.into_iter().collect();
            let name = opt_string(a.first()).unwrap_or_default();
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let texture = icon_path(&model, a.get(1));
            let body = opt_string(a.get(2)).unwrap_or_default();
            let (local_only, per_character) = (flag(a.get(3)), flag(a.get(4)));
            Ok(
                match create_macro(&mut model, &name, texture, &body, local_only, per_character) {
                    Some(i) => Value::Integer(i as i64),
                    None => Value::Nil,
                },
            )
        })?,
    )?;

    // EditMacro(index, name, icon, body, local) -> index | nil. Absent arguments leave their
    // field alone (see `edit_macro` — the shipped UI relies on it).
    g.set(
        "EditMacro",
        lua.create_function(|lua, args: MultiValue| {
            let a: Vec<Value> = args.into_iter().collect();
            let Some(index) = a.first().and_then(|v| v.as_integer()).map(|i| i as usize) else {
                return Ok(Value::Nil);
            };
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            // An icon argument that is present-and-resolvable replaces; an absent/nil one leaves
            // the icon alone. (`MacroFrame_SaveMacro` passes nil here on every body-only save.)
            let texture = match a.get(2) {
                None | Some(Value::Nil) => None,
                other => Some(icon_path(&model, other)),
            };
            let name = opt_string(a.get(1));
            let body = opt_string(a.get(3));
            let local_only = match a.get(4) {
                None | Some(Value::Nil) => None,
                other => Some(flag(other)),
            };
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
            Ok(delete_macro(&mut model, index))
        })?,
    )?;

    g.set(
        "PickupMacro",
        lua.create_function(|lua, index: usize| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            Ok(pickup_macro(&mut model, index))
        })?,
    )?;

    Ok(())
}

/// Resolve a chooser-list **index** argument to a texture path. A string is taken as a path
/// already (nothing shipped does that, but a script may, and the store holds paths anyway).
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

    /// The whole create → read → edit → delete round trip through the reference's own signatures,
    /// including the index space's account/character split.
    #[test]
    fn the_macro_api_round_trip_over_the_two_tabs() {
        let s = seeded();
        assert_eq!(
            s.eval::<(i64, i64)>("local a, c = GetNumMacros() return a, c")
                .unwrap(),
            (0, 0)
        );

        // Account tab: perCharacter falsy -> index 1.
        let i = s
            .eval::<i64>(r#"return CreateMacro("Ambush", 1, "/cast Ambush", nil, nil)"#)
            .unwrap();
        assert_eq!(i, 1);
        // Character tab: perCharacter truthy -> the 19 base.
        let j = s
            .eval::<i64>(r#"return CreateMacro("Bolt", 3, "/cast Fireball", nil, true)"#)
            .unwrap();
        assert_eq!(j, 19, "the character tab starts at MAX_MACROS + 1");
        assert_eq!(
            s.eval::<(i64, i64)>("local a, c = GetNumMacros() return a, c")
                .unwrap(),
            (1, 1)
        );

        // GetMacroInfo's four returns, with the icon index resolved to a path at create time.
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

        // By-name is case-insensitive and searches the account list first; a miss is 0.
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

        // Delete closes the gap: the character macro keeps its own base.
        assert!(s.eval::<bool>("return DeleteMacro(1)").unwrap());
        assert_eq!(
            s.eval::<(i64, i64)>("local a, c = GetNumMacros() return a, c")
                .unwrap(),
            (0, 1)
        );
        assert!(s.eval::<bool>("return GetMacroInfo(1) == nil").unwrap());
        assert_eq!(s.eval::<String>("return GetMacroInfo(19)").unwrap(), "Bolt");
    }

    /// `EditMacro`'s partial-update law — the shipped UI's two disjoint calls. A rename must not
    /// blank the body, and a body save must not blank the name or icon.
    #[test]
    fn edit_macro_leaves_omitted_fields_alone() {
        let s = seeded();
        s.run(r#"CreateMacro("Old", 1, "/cast Ambush")"#).unwrap();

        // MacroPopupOkayButton_OnClick's edit form: name + icon, no body.
        s.run(r#"EditMacro(1, "New", 2)"#).unwrap();
        let (name, tex, body) = s
            .eval::<(String, String, String)>("local n, t, b = GetMacroInfo(1) return n, t, b")
            .unwrap();
        assert_eq!(name, "New");
        assert_eq!(tex, "Interface\\Icons\\Ability_BackStab");
        assert_eq!(body, "/cast Ambush", "the body survives a rename");

        // MacroFrame_SaveMacro's form: body only, name and icon nil.
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

    /// The two refusals the client names in its own log lines, and the caps the edit boxes carry.
    #[test]
    fn create_refuses_a_blank_name_and_a_full_tab_and_clamps_the_caps() {
        let s = seeded();
        assert!(s
            .eval::<bool>(r#"return CreateMacro("", 1, "x") == nil"#)
            .unwrap());
        assert!(s
            .eval::<bool>(r#"return CreateMacro("   ", 1, "x") == nil"#)
            .unwrap());

        for i in 1..=MAX_MACROS {
            let made = s
                .eval::<i64>(&format!(r#"return CreateMacro("m{i}", 1, "")"#))
                .unwrap();
            assert_eq!(made as usize, i);
        }
        assert!(
            s.eval::<bool>(r#"return CreateMacro("one too many", 1, "") == nil"#)
                .unwrap(),
            "the 19th account macro is refused"
        );
        // …and the character tab is a separate 18.
        assert_eq!(
            s.eval::<i64>(r#"return CreateMacro("c", 1, "", nil, 1)"#)
                .unwrap(),
            19
        );

        // The caps are the shipped boxes' own letters= values, enforced at the API too.
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

    /// The dirty flag is the app's save trigger: every mutation raises it, a seed never does, and
    /// the drain clears it.
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

        // A no-op delete changes nothing and raises nothing.
        s.run("DeleteMacro(7)").unwrap();
        assert!(!s.take_macros_dirty());

        // The app's own seed must not trigger a save (a write-back loop).
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

    /// `PickupMacro` loads the cursor with the macro payload, and refuses while holding.
    #[test]
    fn pickup_macro_loads_the_cursor_and_refuses_while_holding() {
        use crate::script::cursor::CursorPayload;

        let s = seeded();
        s.run(r#"CreateMacro("Ambush", 1, "/cast Ambush")"#)
            .unwrap();

        assert!(
            !s.eval::<bool>("return PickupMacro(5)").unwrap(),
            "empty slot"
        );
        assert!(s.eval::<bool>("return PickupMacro(1)").unwrap());
        assert_eq!(
            s.cursor_payload(),
            Some(CursorPayload::Macro(CursorMacro {
                index: 1,
                texture: Some("Interface\\Icons\\Ability_Ambush".into()),
            }))
        );
        // GetCursorInfo's Era shape for a macro: the kind word + the index.
        assert_eq!(
            s.eval::<(String, i64)>("local k, i = GetCursorInfo() return k, i")
                .unwrap(),
            ("macro".to_string(), 1)
        );
        assert!(
            !s.eval::<bool>("return PickupMacro(1)").unwrap(),
            "a macro button is a source, never a drop target"
        );
    }

    /// The chooser list is the app's, served 1-based, and an out-of-range index is nil rather
    /// than an error (`MacroPopupFrame_Update` indexes past the end on the last row every time).
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
}
