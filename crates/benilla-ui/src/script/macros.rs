//! Player macros. 1.12 keeps them client-side with no opcode (`macros-cache.txt`), and
//! `CreateMacro` must return the new index at once, so the table lives here: the app seeds it
//! ([`UiScript::set_macros`]), saves it when [`UiScript::take_macros_dirty`] says, and installs
//! what lists the icon chooser ([`UiScript::set_macro_icon_source`]), run on first use.
//!
//! A macro is a record with an id, kept in one list in the order it was made or loaded
//! (`0xbdcc44`). Lua addresses it by position in a table of 36 ids (`0xbdcc60`): positions 1..=18
//! the account macros and 19..=36 the character macros, each scope sorted by name
//! ([`order`]) and rebuilt after every create, edit and delete (`0x4f0d30`), so the first character
//! macro is index 19 however few account macros there are. An action slot holds the id
//! (`macroId | 0x40000000`, `0x4e62e0`), never the position. A macro's bound spell
//! (`0x4e5a50`'s macro arm returns `[rec+0x564]`) is the app's to derive: it needs the spell catalog
//! and the player's book.

mod order;

#[cfg(test)]
mod lua_tests;

use mlua::{Lua, MultiValue, Value};

use self::order::{crt_qsort, name_cmp};
use super::binding_abi;
use super::cursor::{set_cursor, CursorMacro, CursorPayload};
use super::Model;

/// Macros per scope (`0x4f1010` refuses the 19th, `cmp edx,0x12`), and so the character tab's base.
pub const MAX_MACROS: usize = 18;

/// The bytes a name keeps: `SStrCopy` into the record's 0x40-byte name (`0x4f11fc`, `0x4f12e1`)
/// leaves one for the terminator. The stock edit box stops at 16 characters on its own
/// (`MacroPopupEditBox`, `letters="16"`); the verbs do not.
pub const MAX_MACRO_NAME: usize = 0x3f;

/// The characters a body keeps: `0x4f1550` cuts after 256 (`cmp edi,0x100`, `0x4f1580`). The stock
/// edit box stops at 255 on its own (`MacroFrameText`, `letters="255"`).
pub const MAX_MACRO_BODY: usize = 0x100;

/// The bytes a body passes through first: `0x4f1550` copies it with `SStrCopy` size 0x400
/// (`0x4f1556`) before it counts characters.
const BODY_COPY_BYTES: usize = 0x3ff;

/// Where the character scope's ids start, its counter's initial value (`0x807b90`, `0x4eff90`):
/// the first id it hands out is one more. The account scope's start at 0 (`0x807b8c`).
pub const CHARACTER_ID_BASE: u32 = 0x100_0000;

/// Where each scope's ids start.
const SCOPE_BASE: [u32; 2] = [0, CHARACTER_ID_BASE];

/// One macro, as `GetMacroInfo` reports it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MacroView {
    /// The record's id (`rec+0`), which an action slot holds and the file stores. 0 on a view the
    /// state has not taken: [`MacroState::from_scopes`] gives it the scope's next.
    pub id: u32,
    pub name: String,
    /// The icon's texture path, kept by name because the reference saves the icon by name
    /// (`MACRO %d "%s" %s`, `0x84cb60`).
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

/// A record and the scope it belongs to (`rec+0x1c`).
#[derive(Clone, Debug, PartialEq)]
struct Record {
    per_character: bool,
    view: MacroView,
}

/// The macro table: every record in list order, and each scope's id counter.
#[derive(Clone, Debug, PartialEq)]
pub struct MacroState {
    /// The reference's list (`0xbdcc44`), oldest first. A new record is linked at its tail
    /// (`0x4f11f3`), and `GetMacroIndexByName` and the sort walk it from the head.
    list: Vec<Record>,
    /// The last id each scope handed out (`0xbdcbec`), the largest seen when macros load.
    last_id: [u32; 2],
}

impl Default for MacroState {
    fn default() -> Self {
        Self {
            list: Vec::new(),
            last_id: SCOPE_BASE,
        }
    }
}

/// `SStrCopy`'s cut of `s` to at most `max` bytes, backed up to a character boundary.
fn cut_bytes(s: &str, max: usize) -> &str {
    let mut end = max.min(s.len());
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// A name as `SStrCopy` keeps it (`0x4f11fc`): its first [`MAX_MACRO_NAME`] bytes, not trimmed.
/// Deviation: the reference's byte copy can end inside a character and keep the stray bytes; a
/// `String` must stay UTF-8, so the cut stops at the character boundary.
fn cut_name(name: &str) -> String {
    cut_bytes(name, MAX_MACRO_NAME).to_string()
}

/// A body as `0x4f1550` keeps it: the first 0x3ff bytes, cut again after [`MAX_MACRO_BODY`]
/// characters. A character the byte cut splits is not kept: its decoder (`0x41aab0`) returns -1 at
/// the NUL inside the sequence (`0x41ab69` into `0x41aada`), which ends the count (`0x4f1577`) and
/// puts the terminator where the character began, as the boundary cut does.
fn cut_body(body: &str) -> String {
    cut_bytes(body, BODY_COPY_BYTES)
        .chars()
        .take(MAX_MACRO_BODY)
        .collect()
}

impl MacroState {
    /// The table the file reader builds (`0x4f0600`), the account scope read first: per scope, in
    /// order, a record past the 18th is skipped (`0x4f06ee`); an id below the scope's base gets the
    /// base added (`0x4f0737`); the largest id seen becomes the scope's last (`0x4f0744`); an id
    /// that exists is skipped (`0x4f0789`); the local flag is cleared (`0x4f095c`). A view with no
    /// id, the seed form, takes the scope's next. The name and body keep their verbs' caps.
    pub fn from_scopes(account: Vec<MacroView>, character: Vec<MacroView>) -> Self {
        let mut state = Self::default();
        for (per_character, views) in [(false, account), (true, character)] {
            let scope = usize::from(per_character);
            for mut view in views {
                if state.count(per_character) >= MAX_MACROS {
                    break;
                }
                if view.id == 0 {
                    view.id = state.last_id[scope] + 1;
                } else if view.id < SCOPE_BASE[scope] {
                    view.id += SCOPE_BASE[scope];
                }
                state.last_id[scope] = state.last_id[scope].max(view.id);
                if state.by_id(view.id).is_some() {
                    continue;
                }
                view.name = cut_name(&view.name);
                view.body = cut_body(&view.body);
                view.local_only = false;
                state.list.push(Record {
                    per_character,
                    view,
                });
            }
        }
        state
    }

    /// A scope's macros in list order.
    fn scope(&self, per_character: bool) -> impl Iterator<Item = &MacroView> {
        self.list
            .iter()
            .filter(move |r| r.per_character == per_character)
            .map(|r| &r.view)
    }

    /// A scope's macros in the order the reference saves them: by position in the sorted table
    /// (`0x4f0ad0`-`0x4f0aeb` walk `0xbdcc60 + scope * 0x48`), not in list order. The file then
    /// loads as the list, so a tie the sort left reversed is sorted again on the next load.
    pub fn saved(&self, per_character: bool) -> Vec<&MacroView> {
        self.table(per_character)
            .into_iter()
            .filter_map(|id| self.by_id(id))
            .collect()
    }

    /// How many macros a scope holds (`0xbdcc00`).
    pub fn count(&self, per_character: bool) -> usize {
        self.scope(per_character).count()
    }

    /// Every macro in list order.
    pub fn views(&self) -> impl Iterator<Item = &MacroView> {
        self.list.iter().map(|r| &r.view)
    }

    /// The macro with `id` (`0x4f0f40`); id 0 is none.
    pub fn by_id(&self, id: u32) -> Option<&MacroView> {
        if id == 0 {
            return None;
        }
        self.views().find(|v| v.id == id)
    }

    /// A scope's ids by name: the table's half (`0x4f0d30`), `qsort` over the scope's records in
    /// list order.
    fn table(&self, per_character: bool) -> Vec<u32> {
        let mut records: Vec<&MacroView> = self.scope(per_character).collect();
        crt_qsort(&mut records, |a, b| name_cmp(&a.name, &b.name));
        records.into_iter().map(|v| v.id).collect()
    }

    /// The id at a 1-based Lua index (`0x4f0eb0`): 0 for an empty entry, an index outside 1..=36
    /// (the unsigned compare `0x4f0eb0` makes) or 0 itself.
    fn id_at(&self, index: usize) -> u32 {
        let pos = (index as u32).wrapping_sub(1) as usize;
        if pos >= MAX_MACROS * 2 {
            return 0;
        }
        self.table(pos >= MAX_MACROS)
            .get(pos % MAX_MACROS)
            .copied()
            .unwrap_or(0)
    }

    /// The macro at a 1-based Lua index (`0x4f0e40`).
    pub fn get(&self, index: usize) -> Option<&MacroView> {
        self.by_id(self.id_at(index))
    }

    /// The 1-based Lua index of the macro with `id` (`0x4f0ec0` plus one); id 0 matches the first
    /// empty entry, so it answers where the next macro of the account scope would sit, else of the
    /// character scope, and `None` when both are full.
    pub fn index_of_id(&self, id: u32) -> Option<usize> {
        let (account, character) = (self.table(false), self.table(true));
        if id == 0 {
            return if account.len() < MAX_MACROS {
                Some(account.len() + 1)
            } else if character.len() < MAX_MACROS {
                Some(MAX_MACROS + character.len() + 1)
            } else {
                None
            };
        }
        account
            .iter()
            .position(|&i| i == id)
            .map(|p| p + 1)
            .or_else(|| {
                character
                    .iter()
                    .position(|&i| i == id)
                    .map(|p| MAX_MACROS + p + 1)
            })
    }

    /// `GetMacroIndexByName` (`0x4f0fa0`): the first macro in list order whose name matches without
    /// case (`0x4f0ee0`), by its index; 0 on a miss, and for an empty name (`0x4f0ee8`).
    pub fn index_by_name(&self, name: &str) -> usize {
        if name.is_empty() {
            return 0;
        }
        self.views()
            .find(|v| name_cmp(name, &v.name).is_eq())
            .and_then(|v| self.index_of_id(v.id))
            .unwrap_or(0)
    }
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

    /// Push every macro's cached cast by id, replacing the last push.
    pub fn set_macro_bindings(&mut self, bindings: std::collections::HashMap<u32, MacroBinding>) {
        self.model_mut().macro_bindings = bindings;
    }

    /// Whether a script changed the table since the last call; the app then saves it and fires
    /// `UPDATE_MACROS` (`0x452460`).
    pub fn take_macros_dirty(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().macros_dirty)
    }

    /// Push the icon chooser's texture paths in grid order: the files under `Interface\Icons\`
    /// that `0x4f0090` lists (`benilla_formats::load_macro_icons`), not `SpellIcon.dbc`.
    pub fn set_macro_icons(&mut self, icons: Vec<String>) {
        self.model_mut().macro_icons = icons;
    }

    /// Install what builds the icon list. `GetNumMacroIcons`, `CreateMacro` and `EditMacro` run it
    /// while the list is empty, tested on the count `[0xbdcc1c]` (`0x4f19f6`, `0x4f162d`,
    /// `0x4f1961`), so a VM lists nothing until a script asks; the list lives as long as the VM,
    /// as `CGGameUI::Shutdown` frees it (`0x4effb0`, at `0x490c5f`) on logout and `ReloadUI`.
    pub fn set_macro_icon_source(&mut self, source: Box<dyn Fn() -> Vec<String>>) {
        self.model_mut().macro_icon_source = Some(source.into());
    }

    /// Whether this VM has a source to build the icon list from.
    pub fn has_macro_icon_source(&self) -> bool {
        self.model_ref().macro_icon_source.is_some()
    }

    /// Bumped by every seed and every change, so a per-frame reader such as the action bar compares
    /// a `u64` instead of diffing the table. Not [`Self::take_macros_dirty`]: that drain has one
    /// owner, the save, and a second reader would steal its edge.
    pub fn macros_generation(&self) -> u64 {
        self.model_mut().macros_generation
    }

    /// How many `EditMacro` calls have edited a macro: each one's worker `EditMacroRecord 0x4f1280`
    /// ends in the action bar's usable refresh `0x4e5c00` (`0x4f132e`), which the app runs.
    pub fn macro_edits(&self) -> u64 {
        self.model_mut().macro_edits
    }
}

/// `CreateMacro`'s worker (`0x4f1010`): the new macro's 1-based index in the rebuilt table
/// (`0x4f16ba`), or the full-scope error (`0x84cbdc`). The verb has refused an empty name already.
fn create_macro(
    model: &mut Model,
    name: &str,
    texture: Option<String>,
    body: &str,
    local_only: bool,
    per_character: bool,
) -> mlua::Result<usize> {
    let state = &mut model.macros;
    if state.count(per_character) >= MAX_MACROS {
        return Err(mlua::Error::RuntimeError(format!(
            "CreateMacro() failed, already have {MAX_MACROS} macros"
        )));
    }
    let scope = usize::from(per_character);
    state.last_id[scope] += 1;
    let id = state.last_id[scope];
    state.list.push(Record {
        per_character,
        view: MacroView {
            id,
            name: cut_name(name),
            texture,
            body: cut_body(body),
            local_only,
        },
    });
    model.macros_dirty = true;
    model.macros_generation += 1;
    Ok(model.macros.index_of_id(id).unwrap_or(0))
}

/// `EditMacro(index, name, icon, body, local)` (`0x4f18b0`): an omitted name, icon or body leaves
/// its field alone, since the stock UI calls it with disjoint halves, `(sel, name, icon)` from the
/// rename popup and `(sel, nil, nil, text)` from `MacroFrame_SaveMacro`; the worker copies the name
/// and the body when they are given, the icon when it has one (`0x4f12e1`-`0x4f12fe`, `0x4f130c`).
/// The `local` flag is always written (`0x4f12dc`, `[macro+0x20]`), so an omitted one clears it.
/// The answer is the macro's index in the rebuilt table; an index naming no macro, whose id is 0,
/// answers where the first empty entry sits (`0x4f19c4` into `0x4f0ec0`), or 0 with none.
fn edit_macro(
    model: &mut Model,
    index: usize,
    name: Option<String>,
    texture: Option<String>,
    body: Option<String>,
    local_only: bool,
) -> usize {
    let id = model.macros.id_at(index);
    if let Some(record) = model
        .macros
        .list
        .iter_mut()
        .find(|r| id != 0 && r.view.id == id)
    {
        let view = &mut record.view;
        if let Some(name) = name {
            view.name = cut_name(&name);
        }
        if let Some(texture) = texture {
            view.texture = Some(texture);
        }
        if let Some(body) = body {
            view.body = cut_body(&body);
        }
        view.local_only = local_only;
        model.macros_dirty = true;
        model.macros_generation += 1;
        model.macro_edits += 1;
    }
    model.macros.index_of_id(id).unwrap_or(0)
}

/// `DeleteMacro(index)` (`0x4f1850` into `0x4f1340`): the record goes, and every action slot that
/// holds its id is cleared and sent as 0 (`0x4e5d10`), so the slots of the macros after it keep
/// theirs. Returns whether a macro went.
fn delete_macro(model: &mut Model, index: usize) -> bool {
    let id = model.macros.id_at(index);
    let Some(at) = model
        .macros
        .list
        .iter()
        .position(|r| id != 0 && r.view.id == id)
    else {
        return false;
    };
    model.macros.list.remove(at);
    super::cursor::clear_macro_slots(model, id);
    model.macros_dirty = true;
    model.macros_generation += 1;
    true
}

/// `PickupMacro(index)` (`0x4f1ae0`): the macro payload (cursor mode 8, `[0xb4e2fc]`, the id),
/// which `PlaceAction` (`0x4e62e0`) packs as `macroId | 0x40000000`. A missing macro does nothing
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
            id: entry.id,
            texture: entry.texture.clone(),
        }
    };
    set_cursor(lua, true, |_| Some(CursorPayload::Macro(payload)))
}

/// Build the icon list if it is empty, as the three verbs do before they read it (`0x4f0090`
/// behind a zero count). The source runs with no borrow of the model held. Deviation: a listing
/// that finds no icon stays empty, to be listed again on the next ask, where the reference's
/// dedup then reads below its null list (`0x4f0134`-`0x4f0145`) and faults.
fn ensure_macro_icons(lua: &Lua) {
    let source = {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        if !model.macro_icons.is_empty() {
            return;
        }
        model.macro_icon_source.clone()
    };
    if let Some(source) = source {
        let icons = source();
        lua.app_data_mut::<Model>()
            .expect("model app_data")
            .macro_icons = icons;
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
                model.macros.count(false) as i64,
                model.macros.count(true) as i64,
            ))
        })?,
    )?;

    g.set(
        "GetMacroInfo",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: GetMacroInfo(index)` (`0x84cc0c`, `0x4f177b`).
            let index = binding_abi::index_arg(lua, index, "Usage: GetMacroInfo(index)")?;
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

    // `GetMacroIndexByName(name)` (`0x4f1b30`): a string or a number, else
    // `Usage: GetMacroIndexByName(name)` (`0x84ccb4`, `0x4f1b3c`).
    g.set(
        "GetMacroIndexByName",
        lua.create_function(|lua, name: Value| {
            let name = binding_abi::string_arg(lua, name, "Usage: GetMacroIndexByName(name)")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.macros.index_by_name(&name) as i64)
        })?,
    )?;

    // `GetNumMacroIcons()` (`0x4f19f0`): the list's size, built first while it is empty.
    g.set(
        "GetNumMacroIcons",
        lua.create_function(|lua, ()| {
            ensure_macro_icons(lua);
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.macro_icons.len() as i64)
        })?,
    )?;

    // `GetMacroIconInfo(index)` (`0x4f1a30`): an index that is no number raises the usage text
    // (`0x4f1a41` `lua_isnumber`, `0x84cc78`); `trunc(n) - 1` is compared unsigned with the count
    // (`0x4f1a76`), so 0, a negative and an index past the list push "" (`0x4f1ac5`, `0x882748`),
    // never nil. It never builds the list.
    g.set(
        "GetMacroIconInfo",
        lua.create_function(|lua, v: Value| {
            let index = super::binding_abi::index_arg(lua, v, "Usage: GetMacroIconInfo(index)")?;
            let path = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model
                    .macro_icons
                    .get((index as u32).wrapping_sub(1) as usize)
                    .cloned()
                    .unwrap_or_default()
            };
            Ok(Value::String(lua.create_string(&path)?))
        })?,
    )?;

    // CreateMacro(name, iconIndex, body, local, perCharacter) (`0x4f15b0`): the name is a string
    // or number and the icon a number or numeric string, else the usage text (`0x84cb74`) raises.
    // `iconIndex` is an index into the chooser list. An empty name raises (`0x84cbb4`, tested on
    // the first byte, so a blank one is a name) before the list is built (`0x4f1613`, `0x4f1637`),
    // and a full tab raises after (`0x84cbdc`); a made macro answers exactly its new 1-based index
    // (`0x4f16ce`). The gate `0x494a50(7)` ahead of the body passes every context (`0x494b54` maps
    // tag 7 to the arm that returns 1).
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
            if name.is_empty() {
                return Err(mlua::Error::RuntimeError(
                    "CreateMacro() failed, no name specified".into(),
                ));
            }
            ensure_macro_icons(lua);
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            // `0x4f1632`, `0x4f163c`.
            let texture = icon_at(&model, icon);
            let body = a
                .get(2)
                .and_then(|v| super::binding_abi::optional_string(lua, v))
                .unwrap_or_default();
            let (local_only, per_character) = (flag(a.get(3)), flag(a.get(4)));
            create_macro(&mut model, &name, texture, &body, local_only, per_character)
                .map(|i| Value::Integer(i as i64))
        })?,
    )?;

    // `EditMacro(index, name, icon, body, local)` (`0x4f18b0`): a first argument that is no number
    // or numeric string raises the usage text (`0x84cc44`, `0x4f18be`-`0x4f18c7`); the rest
    // read as the worker takes them, a name and a body when `lua_isstring` (a number counts), the
    // icon when `lua_isnumber`. It answers one number, the macro's index after the rebuild, or for
    // an index naming no macro where the first empty entry sits (`0x4f19c2`-`0x4f19d8`).
    g.set(
        "EditMacro",
        lua.create_function(|lua, args: MultiValue| {
            let a: Vec<Value> = args.into_iter().collect();
            let index = binding_abi::index_arg(
                lua,
                a.first().cloned().unwrap_or(Value::Nil),
                "Usage: EditMacro(index, name, icon, body, local)",
            )?;
            // The icon counts only as `lua_isnumber` reads it (`0x4f1945`): a numeric string is
            // its number, and any other value, nil among them, leaves the icon alone. The list is
            // built only for an index that names a macro and such an icon (`0x4f1913`,
            // `0x4f196b`).
            let icon = lua.coerce_number(a.get(2).cloned().unwrap_or(Value::Nil))?;
            let names_macro = lua
                .app_data_ref::<Model>()
                .expect("model app_data")
                .macros
                .get(index)
                .is_some();
            if names_macro && icon.is_some() {
                ensure_macro_icons(lua);
            }
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            // `0x4f1966`-`0x4f1983`: a miss leaves the pointer null, and the macro its old icon.
            let texture = icon.and_then(|n| icon_at(&model, n));
            let name = a.get(1).and_then(|v| binding_abi::optional_string(lua, v));
            let body = a.get(3).and_then(|v| binding_abi::optional_string(lua, v));
            let local_only = flag(a.get(4));
            let at = edit_macro(&mut model, index, name, texture, body, local_only);
            Ok(Value::Integer(at as i64))
        })?,
    )?;

    g.set(
        "DeleteMacro",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: DeleteMacro(index)` (`0x84cc28`, `0x4f1861`).
            let index = binding_abi::index_arg(lua, index, "Usage: DeleteMacro(index)")?;
            super::tick::verb(lua, |model| delete_macro(model, index));
            Ok(())
        })?,
    )?;

    g.set(
        "PickupMacro",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: PickupMacro(index)` (`0x84cc98`, `0x4f1af1`).
            let index = binding_abi::index_arg(lua, index, "Usage: PickupMacro(index)")?;
            pickup_macro(lua, index);
            Ok(())
        })?,
    )?;

    Ok(())
}

/// The chooser entry at a 1-based icon number: `trunc(n) - 1` (`__ftol`, `0x40a2b0`) compared
/// unsigned with the count, so 0, a negative or a number past the list is no entry.
fn icon_at(model: &Model, n: f64) -> Option<String> {
    let i = (super::binding_abi::ftol(n) as u32).wrapping_sub(1);
    model.macro_icons.get(i as usize).cloned()
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

    /// The id a slot would hold for the macro at `index`, by placing it on a bar slot.
    fn slot_word(s: &mut UiScript, index: i64, slot: i64) -> u32 {
        s.take_action_sets();
        s.run(&format!("PickupMacro({index}) PlaceAction({slot})"))
            .unwrap();
        let sets = s.take_action_sets();
        assert_eq!(sets.len(), 1, "one write: {sets:?}");
        assert_eq!(sets[0].0, slot as u32);
        sets[0].1
    }

    /// `0x4f0600`: an id below the scope's base gets the base added, the largest id seen becomes
    /// the scope's last, an id that exists is skipped, and a scope holds 18. Each new macro then
    /// takes the next id (`0x4f1072`-`0x4f1081`), and a slot holds it as `macroId | 0x40000000`
    /// (`0x4e62e0`): a character macro's id carries bit 24, so its kind byte is 0x41.
    #[test]
    fn loaded_ids_are_rebased_and_the_counters_continue_from_the_largest() {
        let view = |id: u32, name: &str| MacroView {
            id,
            name: name.into(),
            texture: Some("Interface\\Icons\\Ability_Ambush".into()),
            ..Default::default()
        };
        let state = MacroState::from_scopes(
            vec![view(5, "a"), view(5, "dup"), view(9, "b")],
            vec![view(1, "c"), view(0x100_0007, "d"), view(1, "dup")],
        );
        let ids = |per_character| state.scope(per_character).map(|v| v.id).collect::<Vec<_>>();
        assert_eq!(ids(false), [5, 9]);
        assert_eq!(ids(true), [0x100_0001, 0x100_0007]);

        let mut s = seeded();
        s.set_macros(state);
        assert_eq!(slot_word(&mut s, 1, 1), 0x4000_0005, "a, the first by name");
        let account = s.eval::<i64>(r#"return CreateMacro("zz", 1, "")"#).unwrap();
        assert_eq!(
            slot_word(&mut s, account, 2),
            0x4000_000A,
            "one past the largest, 9"
        );
        let character = s
            .eval::<i64>(r#"return CreateMacro("zz", 1, "", nil, 1)"#)
            .unwrap();
        assert_eq!(
            slot_word(&mut s, character, 3),
            0x4100_0008,
            "one past 0x1000007, in the character kind"
        );
    }

    #[test]
    fn a_scope_loads_no_more_than_eighteen() {
        let many = |n: u32| {
            (1..=n)
                .map(|id| MacroView {
                    id,
                    name: format!("m{id:02}"),
                    ..Default::default()
                })
                .collect::<Vec<_>>()
        };
        let state = MacroState::from_scopes(many(25), many(3));
        assert_eq!((state.count(false), state.count(true)), (18, 3));
        assert!(state.by_id(19).is_none());
        assert_eq!(
            state.by_id(0x100_0003).map(|v| v.name.as_str()),
            Some("m03")
        );
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
                .eval::<i64>(&format!(r#"return CreateMacro("m{i:02}", 1, "")"#))
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
            "N".repeat(100),
            "b".repeat(400)
        ))
        .unwrap();
        let (name, body) = s
            .eval::<(String, String)>("local n, _, b = GetMacroInfo(1) return n, b")
            .unwrap();
        assert_eq!(name.len(), MAX_MACRO_NAME);
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

        s.set_macros(MacroState::from_scopes(
            vec![MacroView {
                name: "loaded".into(),
                ..Default::default()
            }],
            Vec::new(),
        ));
        assert!(!s.take_macros_dirty());
        assert_eq!(s.macros().count(false), 1);
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
            id: 1,
            texture: Some("Interface\\Icons\\Ability_Ambush".into()),
        }));
        assert_eq!(s.cursor_payload(), ambush);
        // The setter's inner clear drops what is held, then sets (`0x494fad`).
        assert!(pickup_macro(&s.lua, 1));
        assert_eq!(s.cursor_payload(), ambush);
    }

    /// Off the list `0x4f1a30` pushes "" (`0x4f1ac5`), for 0 and a negative too, as the unsigned
    /// compare reads them as huge.
    #[test]
    fn the_icon_chooser_list_is_one_based_with_an_empty_string_tail() {
        let s = seeded();
        assert_eq!(s.eval::<i64>("return GetNumMacroIcons()").unwrap(), 3);
        assert_eq!(
            s.eval::<String>("return GetMacroIconInfo(2)").unwrap(),
            "Interface\\Icons\\Ability_BackStab"
        );
        assert_eq!(
            s.eval::<String>("return GetMacroIconInfo(2.9)").unwrap(),
            "Interface\\Icons\\Ability_BackStab",
            "the index truncates"
        );
        for call in [
            "GetMacroIconInfo()",
            "GetMacroIconInfo('x')",
            "GetMacroIconInfo({})",
        ] {
            let err = s.run(call).expect_err(call).to_string();
            assert!(
                err.contains("Usage: GetMacroIconInfo(index)"),
                "{call}: {err}"
            );
        }
        for off in ["4", "0", "-1", "'5'"] {
            assert_eq!(
                s.eval::<String>(&format!(
                    "return type(GetMacroIconInfo({off})) .. '|' .. GetMacroIconInfo({off})"
                ))
                .unwrap(),
                "string|",
                "GetMacroIconInfo({off})"
            );
        }
    }

    /// `EditMacro` reads its icon through `lua_isnumber` (`0x4f1945`) and truncates it
    /// (`0x4f195a`); a string that is no number, or a number off the list, keeps the old icon
    /// (`0x4f1983`, `0x4f12f0`).
    #[test]
    fn edit_macro_reads_its_icon_as_a_number_and_keeps_the_old_one_on_a_miss() {
        let s = seeded();
        s.run(r#"CreateMacro("m", 1, "")"#).unwrap();
        let icon = |s: &UiScript| {
            s.eval::<String>("local _, t = GetMacroInfo(1) return t")
                .unwrap()
        };
        for kept in [
            r#"EditMacro(1, nil, "Interface\\Icons\\Spell_Fire_FlameBolt")"#,
            "EditMacro(1, nil, 0)",
            "EditMacro(1, nil, -1)",
            "EditMacro(1, nil, 4)",
            "EditMacro(1, nil, 0.5)",
            "EditMacro(1, nil, {})",
        ] {
            s.run(kept).unwrap();
            assert_eq!(
                icon(&s),
                "Interface\\Icons\\Ability_Ambush",
                "{kept} keeps the icon"
            );
        }
        s.run(r#"EditMacro(1, nil, "3")"#).unwrap();
        assert_eq!(
            icon(&s),
            "Interface\\Icons\\Spell_Fire_FlameBolt",
            "a numeric string is its number"
        );
        s.run("EditMacro(1, nil, 2.5)").unwrap();
        assert_eq!(
            icon(&s),
            "Interface\\Icons\\Ability_BackStab",
            "the number truncates"
        );
        s.run(r#"EditMacro(1, nil, " 1 ")"#).unwrap();
        assert_eq!(
            icon(&s),
            "Interface\\Icons\\Ability_Ambush",
            "as Lua converts a string"
        );
    }

    /// A VM whose icon list is built by a source that counts its runs and answers `icons`.
    fn lazily_listed(
        icons: &'static [&'static str],
    ) -> (UiScript, std::rc::Rc<std::cell::Cell<u32>>) {
        let mut s = UiScript::new().unwrap();
        let runs = std::rc::Rc::new(std::cell::Cell::new(0));
        let counter = runs.clone();
        s.set_macro_icon_source(Box::new(move || {
            counter.set(counter.get() + 1);
            icons.iter().map(|i| i.to_string()).collect()
        }));
        (s, runs)
    }

    const TWO_ICONS: &[&str] = &[
        "Interface\\Icons\\Ability_Ambush",
        "Interface\\Icons\\Ability_BackStab",
    ];

    /// `GetNumMacroIcons` builds on first use (`0x4f19f6`-`0x4f1a02`) and `GetMacroIconInfo`
    /// never does (`0x4f1a30`), so a VM lists nothing until a script asks for the count.
    #[test]
    fn the_icon_list_is_built_on_the_first_count_and_kept() {
        let (s, runs) = lazily_listed(TWO_ICONS);
        assert_eq!(runs.get(), 0, "installing the source lists nothing");
        assert_eq!(
            s.eval::<String>("return GetMacroIconInfo(1)").unwrap(),
            "",
            "before a build the list is empty"
        );
        assert_eq!(runs.get(), 0, "GetMacroIconInfo never builds");
        assert_eq!(s.eval::<i64>("return GetNumMacroIcons()").unwrap(), 2);
        assert_eq!(runs.get(), 1);
        assert_eq!(s.eval::<i64>("return GetNumMacroIcons()").unwrap(), 2);
        assert_eq!(runs.get(), 1, "a built list is kept");
        assert_eq!(
            s.eval::<String>("return GetMacroIconInfo(2)").unwrap(),
            "Interface\\Icons\\Ability_BackStab"
        );
    }

    /// The build is gated on the count, not on a built flag (`0x4f19fb`), so a list that comes
    /// back empty is listed again on the next ask.
    #[test]
    fn an_empty_icon_list_is_listed_again() {
        let (s, runs) = lazily_listed(&[]);
        for _ in 0..2 {
            assert_eq!(s.eval::<i64>("return GetNumMacroIcons()").unwrap(), 0);
        }
        assert_eq!(runs.get(), 2);
    }

    /// `CreateMacro` builds after its usage check and its empty-name raise (`0x4f1613`,
    /// `0x4f1637`), then reads its icon from the built list.
    #[test]
    fn create_macro_builds_the_list_only_past_its_checks() {
        let (s, runs) = lazily_listed(TWO_ICONS);
        assert!(s.run("CreateMacro('a', 'x')").is_err());
        assert!(s.run("CreateMacro('', 1)").is_err());
        assert_eq!(runs.get(), 0, "a refused call lists nothing");
        s.run("CreateMacro('a', 2, '/say hi')").unwrap();
        assert_eq!(runs.get(), 1);
        assert_eq!(
            s.eval::<String>("local _, t = GetMacroInfo(1) return t")
                .unwrap(),
            "Interface\\Icons\\Ability_BackStab"
        );
    }

    /// `EditMacro` builds only for an index that names a macro (`0x4f1913`) and a numeric icon
    /// (`0x4f1945`), then reads the icon from the built list (`0x4f196b`).
    #[test]
    fn edit_macro_builds_the_list_only_for_a_macro_and_a_numeric_icon() {
        let (mut s, runs) = lazily_listed(TWO_ICONS);
        s.set_macros(MacroState::from_scopes(
            vec![MacroView {
                name: "m".into(),
                ..Default::default()
            }],
            Vec::new(),
        ));
        for call in [
            "EditMacro(2, nil, 1)",
            "EditMacro(1, 'n')",
            "EditMacro(1, nil, 'x')",
            "EditMacro(1, nil, nil, '/say x')",
        ] {
            s.run(call).unwrap();
            assert_eq!(runs.get(), 0, "{call} lists nothing");
        }
        s.run("EditMacro(1, nil, 1)").unwrap();
        assert_eq!(runs.get(), 1);
        assert_eq!(
            s.eval::<String>("local _, t = GetMacroInfo(1) return t")
                .unwrap(),
            "Interface\\Icons\\Ability_Ambush"
        );
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
