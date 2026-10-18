//! The character select's glue verbs (`SelectCharacter.cpp`, registrar `0x4736c0`, table
//! `0x838578`) and the roster they read: the record array (`[0xb42144]`, stride `0x120`) and its
//! count (`[0xb42140]`), the selection (`[0x83856c]`), the model frame (`[0xb42174]`), the facing
//! (`[0xb4217c]`) and the build counter (`[0xb42178]`).
//!
//! The host seeds the roster where the reference's list reply is walked (`0x4724d0` through
//! `0x5aabe0` and the appender `0x472300`) and tears it down where `0x472580` does; the verbs that
//! only read it answer here, and `SelectCharacter` fires `UPDATE_SELECTED_CHARACTER` as it returns,
//! as `0x472740` does. Indices are 1-based in Lua and 0-based inside, the boundary at `0x473470`.
//!
//! `DeleteCharacter` and `RenameCharacter` check here and call the glue manager in place with what
//! the reference sends (`0x46b2e0`, `0x46b340`), whose screen states are the host's; `EnterWorld`
//! calls it with the selected record (`0x46b500`); the rename reply's applier
//! (`0x4725f0`) changes the record here once the host has moved the character's files.
//!
//! The 3-D side of the reference (`0x472950`'s model build into the scene) is the host's scene
//! drive; what it reads is kept here: the shown `ModelFFX` pane's scene ([`GlueScenePane`]), the
//! character standing on the scene's attachment 0 ([`super::UiScript::glue_standing_guid`]), and
//! the facing. What a build leaves behind for Lua to see is kept here too: the facing reset and
//! the build counter that gates `SetCharacterSelectFacing`.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::{flag, number_arg, string_arg};
use super::glue_core::{call, glue_status_key, GlueCall};
use super::{check_char_name, Model, ScriptValue, CHAR_NAME_SUCCESS};

/// The record flag a server sets on a character it wants renamed (`[rec+0xfc]` bit `0x4000`,
/// tested at `0x4735ab`, cleared at `0x472722`).
const FLAG_RENAME: u32 = 0x4000;

/// One character of the roster as `GetCharacterInfo` (`0x4732a0`) reads the record. The host
/// resolves the DBC-backed strings; an absent row is the empty string where the reference pushes
/// one (`0x882748`) and `None` where it pushes nil.
#[derive(Clone, Debug, PartialEq)]
pub struct GlueCharacter {
    /// `+0x00`.
    pub guid: u64,
    /// `+0x08`.
    pub name: String,
    /// The `ChrRaces` row's `Name_Lang` for the race byte at `+0x100`; empty without a row.
    pub race: String,
    /// The `ChrClasses` row's `Name_Lang` for the class byte at `+0x101`; empty without a row.
    pub class: String,
    /// `+0x108`.
    pub level: u8,
    /// The `AreaTable` row's name for the id at `+0x3c`; `None` without a row.
    pub zone: Option<String>,
    /// The race row's file string (`ChrRaces+0x3c`); empty without a row.
    pub file_string: String,
    /// `+0x102`.
    pub gender: u8,
    /// `+0xfc`: bit `0x2000` is a ghost.
    pub flags: u32,
    /// Whether the race and gender have a character model (`0x5ec480` finds a row with a model
    /// path): the build `0x472950` makes for a record only goes ahead with one.
    pub model: bool,
}

/// The selection cells that outlive a glue VM: the reference keeps them in C globals, so a glue
/// rebuild finds them as they were. The host carries them from one glue VM to the next.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlueSelectCells {
    /// `[0x83856c]`, 0-based; -1 at load (the image's `.data`), written only by `0x472740`.
    pub selected: i32,
    /// `[0xb4217c]`, radians.
    pub facing: f32,
    /// `[0xb42178]` is nonzero: a select model has been built this process (`0x472fb2` is its one
    /// writer), which opens `SetCharacterSelectFacing`'s gate and nothing closes.
    pub built: bool,
}

impl Default for GlueSelectCells {
    fn default() -> Self {
        GlueSelectCells {
            selected: -1,
            facing: 0.0,
            built: false,
        }
    }
}

/// The scene a shown `ModelFFX` pane draws, as the host's scene drive reads it.
#[derive(Clone, Debug, PartialEq)]
pub struct GlueScenePane {
    /// The pane, whose `OnShow` (`0x46fa60`) resets the glue glow when it appears.
    pub handle: crate::widget::FrameHandle,
    /// The pane's rect as fractions of the screen, `[left, right, top, bottom]` from the top-left:
    /// the scene's viewport (`0x76d5d7`), whose `w/h` is the projection's aspect. `None` until the
    /// layout has placed it.
    pub rect: Option<[f32; 4]>,
    /// The instance the pane's last `SetModel` made ([`crate::widget::ModelState::instance`]).
    pub instance: u64,
    /// The file the pane's last `SetModel` gave it: the XML `file`, `Model:SetModel`, or
    /// `SetCharSelectBackground`'s (`0x472128`, the pane's own `SetModel`).
    pub path: String,
    /// The fog the scene draws with ([`crate::widget::ModelState::scene_fog`]).
    pub fog: Option<crate::widget::ModelFog>,
    /// The pane is the select screen's model frame (`[0xb42174]`), whose scene the selected
    /// character stands in.
    pub select: bool,
    /// The pane is the create screen's customize frame (`[0xb420f0]`), whose scene the preview
    /// character stands in.
    pub create: bool,
    /// The root inputs the paint places the scene by (`0x76d1a0`): `SetModelScale`, the frame's
    /// effective scale, `SetPosition` in layout units and `SetFacing` in radians.
    pub model_scale: f32,
    pub layout_scale: f32,
    pub position: [f32; 3],
    pub facing: f32,
}

/// A record of the array: the character and whether `+0x110`, the built composite, is set.
#[derive(Clone, Debug)]
struct Record {
    character: GlueCharacter,
    built: bool,
}

/// The roster and the select screen's cells the verbs read.
#[derive(Clone, Default)]
pub(crate) struct GlueRoster {
    records: Vec<Record>,
    cells: GlueSelectCells,
    /// `[0xb42174]`: the name of the model frame `SetCharSelectModelFrame` found.
    model_frame: Option<String>,
}

/// `rad = deg · 0.017453292` (`0x7ffaac`, a float).
pub(super) const DEG_TO_RAD: f32 = f32::from_bits(0x3c8e_fa35);
/// `deg = rad · 57.29578` (`0x803a78`, a float).
pub(super) const RAD_TO_DEG: f32 = f32::from_bits(0x4265_2ee1);

impl GlueRoster {
    /// `0x473700` on every record and the count to 0 (`0x4723ff`, `0x4725af`).
    fn clear(&mut self) {
        self.records.clear();
    }

    /// `0x472740`: a negative index reads 0, so does one past the end, then the cell is stored and
    /// `0x472950` runs. Returns the event's argument, the stored index plus one, re-read from the
    /// cell (`0x47275e`), not the caller's.
    ///
    /// The build, for what Lua sees (`0x472950`): an index off the roster exits at `0x472971`
    /// before touching anything; otherwise the facing is zeroed (`0x472a61`, ahead of the built
    /// test and not gated), and a record with no composite yet that has a model gets one, which is
    /// the one place the build counter rises (`0x472fb2`).
    fn select(&mut self, index: i32) -> i32 {
        let clamped = if index < 0 { 0 } else { index };
        let stored = if (clamped as usize) < self.records.len() {
            clamped
        } else {
            0
        };
        self.cells.selected = stored;
        if let Some(record) = self.records.get_mut(stored as usize) {
            self.cells.facing = 0.0;
            if !record.built && record.character.model {
                record.built = true;
                self.cells.built = true;
            }
        }
        stored + 1
    }

    /// `SetCharacterSelectFacing`'s body (`0x4730e0`): the store is inside the build counter's
    /// gate (`0x4730e6`).
    fn set_facing(&mut self, radians: f32) {
        if self.cells.built {
            self.cells.facing = radians;
        }
    }
}

impl super::UiScript {
    /// The selection cells as the VM holds them.
    pub fn glue_select_cells(&self) -> GlueSelectCells {
        self.model_ref().glue.roster.cells
    }

    /// Carry the selection cells over from the last glue VM.
    pub fn set_glue_select_cells(&mut self, cells: GlueSelectCells) {
        self.model_mut().glue.roster.cells = cells;
    }

    /// The file `SetCharSelectBackground` gave the model frame, while its scene still holds it: a
    /// later `SetModel` on the frame replaces the scene and its fill callback with it.
    pub fn glue_select_background(&self) -> Option<String> {
        let model = self.model_ref();
        let h = model_frame_handle(&model)?;
        match &model.arena.frame(h)?.kind_state {
            crate::widget::KindState::Model(m) if m.select_fill => m.path.clone(),
            _ => None,
        }
    }

    /// The character standing on the select scene's attachment 0: the selected record once
    /// `0x472950` has built its composite (`+0x110`) and attached it (`0x473039`). The list ask
    /// (`0x4723d0`) and the teardown (`0x472580`) free the records and then detach
    /// (`0x4722c0`/`0x4722e0` at `0x472409` and `0x4725b9`); the load (`0x4724d0`) frees them at
    /// `0x4724ee` and detaches later in the same call, through the select's build
    /// (`0x472740` → `0x472a44`) or, for an empty roster, at `0x472533`. World entry's free
    /// (`0x472090`) detaches nothing, and the glue screens go with it.
    pub fn glue_standing_guid(&self) -> Option<u64> {
        let model = self.model_ref();
        let roster = &model.glue.roster;
        usize::try_from(roster.cells.selected)
            .ok()
            .and_then(|i| roster.records.get(i))
            .filter(|r| r.built)
            .map(|r| r.character.guid)
    }

    /// The scene of the shown `ModelFFX` pane, the topmost by (strata, level) if more than one is
    /// shown; `None` with none shown, or one holding no file.
    pub fn glue_scene(&self) -> Option<GlueScenePane> {
        let model = self.model_ref();
        model
            .arena
            .model_kinds()
            .iter()
            .filter(|&&h| model.arena.frame(h).is_some_and(|f| f.effective_visible))
            .filter_map(|&h| {
                let f = model.arena.frame(h)?;
                Some(((f.strata, f.level), scene_pane(&model, h)?))
            })
            .max_by_key(|(order, _)| *order)
            .map(|(_, pane)| pane)
    }

    /// The scene of the `ModelFFX` pane `h`, shown or hidden: a hidden pane keeps its model and
    /// scene, and only its clock stops (its `OnUpdate`, `0x76d7f0`, runs from the visible-frame
    /// pump `0x765650`). `None` for a frame that is no `ModelFFX` or holds no file.
    pub fn glue_pane(&self, h: crate::widget::FrameHandle) -> Option<GlueScenePane> {
        scene_pane(&self.model_ref(), h)
    }

    /// The selected character's guid, none off the roster.
    pub fn glue_selected_guid(&self) -> Option<u64> {
        let model = self.model_ref();
        let roster = &model.glue.roster;
        usize::try_from(roster.cells.selected)
            .ok()
            .and_then(|i| roster.records.get(i))
            .map(|r| r.character.guid)
    }

    /// The name of the record with `guid`, none off the roster: the rename reply's applier
    /// (`0x4725f0`) finds the record by guid (`0x472613`-`0x472620`).
    pub fn glue_character_name(&self, guid: u64) -> Option<String> {
        self.model_ref()
            .glue
            .roster
            .records
            .iter()
            .find(|r| r.character.guid == guid)
            .map(|r| r.character.name.clone())
    }

    /// The rename reply's applier (`0x4725f0`) past the file moves, which are the host's: the
    /// AddOn state carried from the old name to the new (`0x4726ff`, `0x472706`), the new name
    /// copied into the record (`0x472718`), the rename flag cleared (`0x472722`) and
    /// `CHARACTER_LIST_UPDATE` fired (`0x472732`). No record with `guid` is no change at all.
    pub fn apply_glue_rename(&mut self, guid: u64, name: &str) {
        {
            let mut model = self.model_mut();
            let glue = &mut model.glue;
            let Some(record) = glue
                .roster
                .records
                .iter_mut()
                .find(|r| r.character.guid == guid)
            else {
                return;
            };
            glue.addons.rename(&record.character.name, name);
            record.character.name = name.to_string();
            record.character.flags &= !FLAG_RENAME;
        }
        self.fire_event("CHARACTER_LIST_UPDATE", Vec::new());
    }

    /// The roster's guids in order.
    pub fn glue_roster_guids(&self) -> Vec<u64> {
        self.model_ref()
            .glue
            .roster
            .records
            .iter()
            .map(|r| r.character.guid)
            .collect()
    }

    /// The roster teardown ([`teardown_roster`]).
    pub fn teardown_glue_roster(&mut self) {
        teardown_roster(&self.lua);
    }

    /// The roster load (`0x4724d0`), run when the character list reply completes: the old records
    /// freed, `roster` appended in order (`0x472300`), then an empty roster selects -1 and, with
    /// no realm chosen this session (`[0x837e9c]` = -1), skips `CHARACTER_LIST_UPDATE` and asks
    /// the host to raise the realm list (the return value, `RequestRealmList(1)` at `0x47254b`);
    /// otherwise the cell is set to 0 (`0x472556`), the `lastCharacterIndex` CVar's integer is
    /// selected (`0x472550`-`0x472563`) and `CHARACTER_LIST_UPDATE` fires.
    pub fn load_glue_roster(&mut self, roster: Vec<GlueCharacter>) -> bool {
        let last = self.cvar_int("lastCharacterIndex").unwrap_or(0);
        let count = roster.len();
        {
            let mut model = self.model_mut();
            // `0x51f0b0(NULL)` drops every AddOn node, then the appender loads one per record
            // (`0x4724d0` through `0x5aabe0` and `0x472300`), the file named by the realm the
            // session dialled (`0x5ab7d0`).
            let names: Vec<String> = roster.iter().map(|c| c.name.clone()).collect();
            let realm = super::glue_realm::realm_name(&model);
            model.glue.addons.load(&realm, &names);
            model.glue.roster.records = roster
                .into_iter()
                .map(|character| Record {
                    character,
                    built: false,
                })
                .collect();
        }
        if count == 0 {
            self.select_glue_character(-1);
            if self.model_ref().glue.realms.cells.chosen.is_none() {
                return true;
            }
        } else {
            self.model_mut().glue.realms.cells.chosen = Some(0);
            self.select_glue_character(last);
        }
        self.fire_event("CHARACTER_LIST_UPDATE", Vec::new());
        false
    }

    /// `0x472740` from the host: the selection stored and `UPDATE_SELECTED_CHARACTER` fired.
    fn select_glue_character(&mut self, index: i32) {
        let arg = self.model_mut().glue.roster.select(index);
        self.fire_event(
            "UPDATE_SELECTED_CHARACTER",
            vec![ScriptValue::Number(f64::from(arg))],
        );
    }
}

/// The roster teardown (`0x472580`): the records freed, the count 0, the selection reset through
/// `0x472740(-1)`, which clamps to 0 and fires `UPDATE_SELECTED_CHARACTER` with 1 on the empty
/// roster whatever it held, the model frame's `SetCamera(0)` (`0x4725d7` → `0x76cec0`), then
/// `CHARACTER_LIST_UPDATE`.
pub(super) fn teardown_roster(lua: &Lua) {
    let selected = {
        let mut m = model(lua);
        m.glue.roster.clear();
        m.glue.roster.select(-1)
    };
    super::tick::fire_event_into(
        lua,
        "UPDATE_SELECTED_CHARACTER",
        vec![ScriptValue::Number(f64::from(selected))],
    );
    {
        let mut m = model(lua);
        if let Some(h) = model_frame_handle(&m) {
            m.set_pane_camera(h, 0);
        }
    }
    super::tick::fire_event_into(lua, "CHARACTER_LIST_UPDATE", Vec::new());
}

/// The scene a `ModelFFX` pane `h` holds, visible or not ([`UiScript::glue_scene`]).
fn scene_pane(model: &Model, h: crate::widget::FrameHandle) -> Option<GlueScenePane> {
    let f = model.arena.frame(h)?;
    let crate::widget::KindState::Model(m) = &f.kind_state else {
        return None;
    };
    if !m.ffx {
        return None;
    }
    let (sw, sh) = (model.screen.width(), model.screen.height());
    Some(GlueScenePane {
        handle: h,
        rect: model
            .frame_rect(h)
            .filter(|_| sw > 0.0 && sh > 0.0)
            .map(|r| {
                [
                    r.left / sw,
                    r.right / sw,
                    1.0 - r.top / sh,
                    1.0 - r.bottom / sh,
                ]
            }),
        instance: m.instance,
        path: m.path.clone()?,
        fog: m.scene_fog(),
        select: model_frame_handle(model) == Some(h),
        create: super::glue_create::create_frame_handle(model) == Some(h),
        model_scale: m.scale,
        layout_scale: f.effective_scale,
        position: m.position.into(),
        facing: m.facing,
    })
}

/// `[0xb42174]`'s frame, while the name `SetCharSelectModelFrame` took still names a model pane.
fn model_frame_handle(model: &Model) -> Option<crate::widget::FrameHandle> {
    let h = model
        .arena
        .lookup(model.glue.roster.model_frame.as_deref()?)?;
    matches!(
        model.arena.frame(h).map(|f| &f.kind_state),
        Some(crate::widget::KindState::Model(_))
    )
    .then_some(h)
}

fn model(lua: &Lua) -> mlua::AppDataRefMut<'_, Model> {
    lua.app_data_mut::<Model>().expect("model app_data")
}

fn arg(args: &MultiValue, i: usize) -> Value {
    args.get(i).cloned().unwrap_or(Value::Nil)
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // `0x4731c0`: a string or number (`0x6f3510`) or the usage error (`0x838720`); the global of
    // that name castable to the model type (`0x76c760`) becomes the frame (`0x472100`), else the
    // call changes nothing.
    g.set(
        "SetCharSelectModelFrame",
        lua.create_function(|lua, args: MultiValue| {
            let name = string_arg(
                lua,
                arg(&args, 0),
                "Usage: SetCharSelectModelFrame(\"frameName\")",
            )?;
            let mut m = model(lua);
            let is_model = m
                .arena
                .lookup(&name)
                .and_then(|h| m.arena.frame(h))
                .is_some_and(|f| f.kind == crate::widget::FrameKind::Model);
            if is_model {
                m.glue.roster.model_frame = Some(name);
            }
            Ok(())
        })?,
    )?;

    // `0x473230`: the file (`0x6f3510`, else `0x83874c`) into `0x472110`, which needs the frame
    // set and a non-empty name, hands it to the frame's own `SetModel` (vtable `+0x94`,
    // `0x472128`), and on the new scene puts the select screen's fill callback `0x472150` where
    // `SetModel` put the fog one (`0x472144`): the select scene never fogs once a background is set.
    g.set(
        "SetCharSelectBackground",
        lua.create_function(|lua, args: MultiValue| {
            let file = string_arg(
                lua,
                arg(&args, 0),
                "Usage: SetCharSelectBackground(\"filename\")",
            )?;
            let mut m = model(lua);
            let Some(h) = model_frame_handle(&m).filter(|_| !file.is_empty()) else {
                return Ok(());
            };
            m.set_pane_model(h, file);
            if let Some(crate::widget::KindState::Model(pane)) =
                m.arena.frame_mut(h).map(|f| &mut f.kind_state)
            {
                pane.select_fill = true;
            }
            Ok(())
        })?,
    )?;

    // `0x473270` → `0x4723d0`: every record freed and the count 0 (`0x4723ff`) in the call, so a
    // `GetNumCharacters` straight after reads 0; then `0x46b880`, the manager's, in the call:
    // state 3 and the dialog, the list request its frame's to send.
    g.set(
        "GetCharacterListUpdate",
        lua.create_function(|lua, _: MultiValue| {
            model(lua).glue.roster.clear();
            call(lua, GlueCall::GetCharacterList);
            Ok(())
        })?,
    )?;

    // `0x473280`: the count, a number.
    g.set(
        "GetNumCharacters",
        lua.create_function(|lua, _: MultiValue| Ok(model(lua).glue.roster.records.len() as f64))?,
    )?;

    // `0x4732a0`: one number (`0x6f34d0`) or `Usage: GetCharacterInfo(index)` (`0x838778`), then
    // eight values on every returning path; off the roster the tuple is nil, nil, nil, 0, nil,
    // nil, 0, nil (`0x47341e`-`0x473459`).
    g.set(
        "GetCharacterInfo",
        lua.create_function(|lua, args: MultiValue| {
            let index =
                number_arg(lua, arg(&args, 0), "Usage: GetCharacterInfo(index)")?.wrapping_sub(1);
            let m = model(lua);
            let Some(record) = usize::try_from(index)
                .ok()
                .and_then(|i| m.glue.roster.records.get(i))
            else {
                return Ok(MultiValue::from_vec(vec![
                    Value::Nil,
                    Value::Nil,
                    Value::Nil,
                    Value::Number(0.0),
                    Value::Nil,
                    Value::Nil,
                    Value::Number(0.0),
                    Value::Nil,
                ]));
            };
            let c = &record.character;
            let text = |s: &str| lua.create_string(s).map(Value::String);
            Ok(MultiValue::from_vec(vec![
                text(&c.name)?,
                text(&c.race)?,
                text(&c.class)?,
                Value::Number(f64::from(c.level)),
                match &c.zone {
                    Some(zone) => text(zone)?,
                    None => Value::Nil,
                },
                text(&c.file_string)?,
                Value::Number(f64::from(c.gender)),
                flag(c.flags & 0x2000 != 0),
            ]))
        })?,
    )?;

    // `0x473470`: one number or `Usage: SelectCharacter(index)` (`0x838798`), decremented and
    // clamped to -1 outside `[0, count)` (`0x4734a4`-`0x4734b2`), into `0x472740`, which stores
    // and fires `UPDATE_SELECTED_CHARACTER` whatever was selected before: the gate against a
    // repeat is the Lua caller's. The binding's clamp and `0x472740`'s own land on the same 0, so
    // only the latter is kept.
    g.set(
        "SelectCharacter",
        lua.create_function(|lua, args: MultiValue| {
            let index =
                number_arg(lua, arg(&args, 0), "Usage: SelectCharacter(index)")?.wrapping_sub(1);
            let selected = model(lua).glue.roster.select(index);
            super::tick::fire_event_into(
                lua,
                "UPDATE_SELECTED_CHARACTER",
                vec![ScriptValue::Number(f64::from(selected))],
            );
            Ok(())
        })?,
    )?;

    // `0x4734c0`: one number or `Usage: DeleteCharacter(index)` (`0x8387b8`); an index in
    // `[1, count]` hands its record's guid to `0x46b2e0`, which with a nonzero guid is the
    // manager's, in the call (state 6, the dialog and the `CMSG_CHAR_DELETE`). No values on any
    // path (`0x47351b`).
    g.set(
        "DeleteCharacter",
        lua.create_function(|lua, args: MultiValue| {
            let index =
                number_arg(lua, arg(&args, 0), "Usage: DeleteCharacter(index)")?.wrapping_sub(1);
            let guid = {
                let m = model(lua);
                usize::try_from(index)
                    .ok()
                    .and_then(|i| m.glue.roster.records.get(i))
                    .map(|r| r.character.guid)
            };
            if let Some(guid) = guid.filter(|&g| g != 0) {
                call(lua, GlueCall::DeleteCharacter(guid));
            }
            Ok(MultiValue::new())
        })?,
    )?;

    // `0x473520`: one number or `Usage: RenameCharacter(index, name)` (`0x8387f0`). No values
    // when the name is not a string or number (`0x6f3510`) or is empty as a C string
    // (`0x6f3690`), the index is off the roster, or the record is not flagged for a rename
    // (`0x4735ab`). A name equal to the old one, ASCII case folded (`0x4735bd`, `0x64a4c0`), fires
    // `FORCE_RENAME_CHARACTER` with `CHAR_CREATE_NAME_IN_USE` (`0x4735d2`) and returns none.
    // Otherwise `0x46b340` decides one value: 1, or nil when the name check (`0x5abd50`) fails,
    // which fires `FORCE_RENAME_CHARACTER` with the check's key (`0x46b388`); a passing name is
    // the manager's, in the call (state 7, the dialog and the `CMSG_CHAR_RENAME`), before the 1.
    g.set(
        "RenameCharacter",
        lua.create_function(|lua, args: MultiValue| {
            let index = number_arg(lua, arg(&args, 0), "Usage: RenameCharacter(index, name)")?
                .wrapping_sub(1);
            let name = match arg(&args, 1) {
                v @ (Value::String(_) | Value::Integer(_) | Value::Number(_)) => lua
                    .coerce_string(v)?
                    .map(|s| s.to_string_lossy())
                    .unwrap_or_default(),
                _ => String::new(),
            };
            // A C string ends at its NUL.
            let name = name.split('\0').next().unwrap_or_default().to_string();
            if name.is_empty() {
                return Ok(MultiValue::new());
            }
            let (guid, old, lists, charsets) = {
                let m = model(lua);
                let Some(record) = usize::try_from(index)
                    .ok()
                    .and_then(|i| m.glue.roster.records.get(i))
                    .filter(|r| r.character.flags & FLAG_RENAME != 0)
                else {
                    return Ok(MultiValue::new());
                };
                (
                    record.character.guid,
                    record.character.name.clone(),
                    m.glue.name_lists(),
                    m.name_charsets(),
                )
            };
            let force_rename = |key: &str| {
                super::tick::fire_event_into(
                    lua,
                    "FORCE_RENAME_CHARACTER",
                    vec![ScriptValue::Str(key.to_string())],
                );
            };
            if name.eq_ignore_ascii_case(&old) {
                force_rename("CHAR_CREATE_NAME_IN_USE");
                return Ok(MultiValue::new());
            }
            // `0x46b340`: a zero guid sends nothing and answers 1 (`0x46b355`).
            if guid != 0 {
                let status = check_char_name(Some(&name), &*lists, charsets);
                if status != CHAR_NAME_SUCCESS {
                    force_rename(glue_status_key(i32::from(status)));
                    return Ok(MultiValue::from_vec(vec![Value::Nil]));
                }
                call(lua, GlueCall::RenameCharacter { guid, name });
            }
            Ok(MultiValue::from_vec(vec![Value::Number(1.0)]))
        })?,
    )?;

    // `0x46d3c0`, a glue core binding (table `0x8373b8`) that reads this roster: `CGlueMgr::
    // EnterWorld` (`0x46b500`), then no values (`0x46d3c5 xor eax,eax`), arguments ignored. The
    // selection cell indexes the record (`0x46b513`-`0x46b533`); off the roster the call returns
    // having done nothing (`0x46b53e`). The record goes to the manager, whose guards come next,
    // in the call.
    g.set(
        "EnterWorld",
        lua.create_function(|lua, _: MultiValue| {
            let entry = {
                let m = model(lua);
                let roster = &m.glue.roster;
                usize::try_from(roster.cells.selected)
                    .ok()
                    .and_then(|i| roster.records.get(i))
                    .map(|r| GlueCall::EnterWorld {
                        index: roster.cells.selected,
                        guid: r.character.guid,
                        flags: r.character.flags,
                    })
            };
            if let Some(entry) = entry {
                call(lua, entry);
            }
            Ok(MultiValue::new())
        })?,
    )?;

    // `0x473640` → `0x473190`: the selected record's composite (`+0x110`) is handed to the
    // character bake (`0x477860`), once per `OnUpdateModel` (`CharacterSelect.lua:208`). The
    // composite is the scene drive's, which bakes when the selection or the look changes, so
    // there is nothing for the call to do on the Lua side.
    g.set(
        "UpdateSelectionCustomizationScene",
        lua.create_function(|_, _: MultiValue| Ok(()))?,
    )?;

    // `0x473650`: the cell (a float) times 57.29578, pushed as a double.
    g.set(
        "GetCharacterSelectFacing",
        lua.create_function(|lua, _: MultiValue| {
            let radians = model(lua).glue.roster.cells.facing;
            Ok(f64::from(radians) * f64::from(RAD_TO_DEG))
        })?,
    )?;

    // `0x473670`: one number or `Usage: SetCharacterSelectFacing(degrees)` (`0x838814`), times
    // 0.017453292 and stored as a float, with no wrap or clamp, once a model has been built. It
    // returns one value (`0x4736ae`, `mov eax,1`) with nothing pushed since the arguments, so the
    // value is the top of the stack: the last argument, the degrees as the caller wrote them.
    g.set(
        "SetCharacterSelectFacing",
        lua.create_function(|lua, args: MultiValue| {
            let degrees = match lua.coerce_number(arg(&args, 0))? {
                Some(n) => n,
                None => {
                    return Err(mlua::Error::RuntimeError(
                        "Usage: SetCharacterSelectFacing(degrees)".into(),
                    ))
                }
            };
            let radians = (degrees * f64::from(DEG_TO_RAD)) as f32;
            model(lua).glue.roster.set_facing(radians);
            Ok(args.back().cloned().unwrap_or(Value::Nil))
        })?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    fn character(guid: u64, name: &str) -> GlueCharacter {
        GlueCharacter {
            guid,
            name: name.into(),
            race: "Night Elf".into(),
            class: "Druid".into(),
            level: 12,
            zone: Some("Teldrassil".into()),
            file_string: "NightElf".into(),
            gender: 1,
            flags: 0,
            model: true,
        }
    }

    /// A VM with the three characters loaded and `FIRED` recording the select events.
    fn vm(roster: Vec<GlueCharacter>) -> UiScript {
        let mut s = UiScript::new_glue().unwrap();
        s.register_cvars([("lastCharacterIndex", "0")]);
        s.run(
            "FIRED = {} local f = CreateFrame('Frame', 'Listener') \
             f:SetScript('OnEvent', function() \
               table.insert(FIRED, event .. ' ' .. tostring(arg1)) end) \
             f:RegisterEvent('UPDATE_SELECTED_CHARACTER') \
             f:RegisterEvent('CHARACTER_LIST_UPDATE')",
        )
        .unwrap();
        let _ = s.load_glue_roster(roster);
        s.run("FIRED = {}").unwrap();
        s
    }

    fn three() -> Vec<GlueCharacter> {
        vec![
            character(1, "Alpha"),
            character(2, "Bravo"),
            character(3, "Charlie"),
        ]
    }

    /// A chunk returning how many values `call` returns.
    fn count(call: &str) -> String {
        format!("local function n(...) return arg.n end return n({call})")
    }

    fn fired(s: &UiScript) -> Vec<String> {
        s.eval::<Vec<String>>("return FIRED").unwrap()
    }

    #[test]
    fn the_count_and_the_eight_values_of_a_character() {
        let mut roster = three();
        roster[1].zone = None;
        roster[1].flags = 0x2000;
        roster[1].gender = 0;
        let s = vm(roster);
        assert_eq!(s.eval::<f64>("return GetNumCharacters()").unwrap(), 3.0);
        assert_eq!(
            s.eval::<(
                String,
                String,
                String,
                f64,
                String,
                String,
                f64,
                Option<i64>
            )>("return GetCharacterInfo(1)")
                .unwrap(),
            (
                "Alpha".into(),
                "Night Elf".into(),
                "Druid".into(),
                12.0,
                "Teldrassil".into(),
                "NightElf".into(),
                1.0,
                None
            )
        );
        // No zone row is nil in the fifth place; a ghost is 1 in the last.
        assert_eq!(
            s.eval::<(String, Option<String>, f64, Option<i64>)>(
                "local n, _, _, _, zone, _, gender, ghost = GetCharacterInfo(2) \
                 return n, zone, gender, ghost"
            )
            .unwrap(),
            ("Bravo".into(), None, 0.0, Some(1))
        );
        assert_eq!(s.eval::<f64>(&count("GetCharacterInfo(3)")).unwrap(), 8.0);
    }

    /// Off the roster the tuple is nil, nil, nil, 0, nil, nil, 0, nil, and still eight values
    /// (`0x47341e`).
    #[test]
    fn an_index_off_the_roster_answers_the_eight_tuple() {
        let s = vm(three());
        for call in [
            "GetCharacterInfo(0)",
            "GetCharacterInfo(4)",
            "GetCharacterInfo(-7)",
        ] {
            assert_eq!(
                s.eval::<(
                    Option<String>,
                    Option<String>,
                    Option<String>,
                    f64,
                    Option<String>,
                    Option<String>,
                    f64,
                    Option<i64>
                )>(&format!("return {call}"))
                    .unwrap(),
                (None, None, None, 0.0, None, None, 0.0, None),
                "{call}"
            );
            assert_eq!(s.eval::<f64>(&count(call)).unwrap(), 8.0);
        }
        // A numeric string passes `is-number`; 2.9 truncates to 2.
        assert_eq!(
            s.eval::<String>("return (GetCharacterInfo('2'))").unwrap(),
            "Bravo"
        );
        assert_eq!(
            s.eval::<String>("return (GetCharacterInfo(2.9))").unwrap(),
            "Bravo"
        );
        for call in ["GetCharacterInfo()", "GetCharacterInfo('x')"] {
            let err = s.run(call).unwrap_err().to_string();
            assert!(err.contains("Usage: GetCharacterInfo(index)"), "{err}");
        }
    }

    /// `0x472740` clamps a negative or a past-the-end index to 0 and fires with the STORED index
    /// plus one, whatever the caller passed or the cell held: a bad index fires 1 (`0x47275e`).
    #[test]
    fn select_character_clamps_and_fires_the_stored_index() {
        let s = vm(three());
        for (call, want) in [
            ("SelectCharacter(2)", "UPDATE_SELECTED_CHARACTER 2"),
            ("SelectCharacter(3)", "UPDATE_SELECTED_CHARACTER 3"),
            ("SelectCharacter(0)", "UPDATE_SELECTED_CHARACTER 1"),
            ("SelectCharacter(-5)", "UPDATE_SELECTED_CHARACTER 1"),
            ("SelectCharacter(4)", "UPDATE_SELECTED_CHARACTER 1"),
            ("SelectCharacter(1.9)", "UPDATE_SELECTED_CHARACTER 1"),
        ] {
            s.run("FIRED = {}").unwrap();
            s.run(call).unwrap();
            assert_eq!(fired(&s), [want], "{call}");
        }
        // No "already selected" test: the same index fires again.
        s.run("FIRED = {} SelectCharacter(2) SelectCharacter(2)")
            .unwrap();
        assert_eq!(fired(&s), ["UPDATE_SELECTED_CHARACTER 2"; 2]);
        assert_eq!(s.glue_selected_guid(), Some(2));
        let err = s.run("SelectCharacter()").unwrap_err().to_string();
        assert!(err.contains("Usage: SelectCharacter(index)"), "{err}");
        assert!(fired(&s).len() == 2, "a usage error fires nothing");
    }

    /// An empty roster still fires, with 1, and stores 0.
    #[test]
    fn select_character_on_an_empty_roster_fires_one() {
        let s = vm(Vec::new());
        s.run("SelectCharacter(1)").unwrap();
        assert_eq!(fired(&s), ["UPDATE_SELECTED_CHARACTER 1"]);
        assert_eq!(s.glue_select_cells().selected, 0);
        assert_eq!(s.glue_selected_guid(), None);
    }

    /// The facing store is inside the build counter's gate (`0x4730e6`): nothing before a model
    /// has been built, and then degrees in, degrees out through the float cell, unwrapped.
    #[test]
    fn the_facing_stores_only_once_a_model_has_been_built() {
        let facing = |s: &UiScript| s.eval::<f64>("return GetCharacterSelectFacing()").unwrap();
        // A fresh process: nothing built, the gate shut.
        let s = UiScript::new_glue().unwrap();
        s.run("SetCharacterSelectFacing(90)").unwrap();
        assert_eq!(facing(&s), 0.0);

        // No record builds without a model, so selecting one leaves the gate shut.
        let mut roster = three();
        for c in &mut roster {
            c.model = false;
        }
        let s = vm(roster);
        s.run("SelectCharacter(1) SetCharacterSelectFacing(90)")
            .unwrap();
        assert_eq!(facing(&s), 0.0);
        assert!(!s.glue_select_cells().built);

        // The roster load selects the last character, which builds its model.
        let s = vm(three());
        assert!(s.glue_select_cells().built);
        s.run("SetCharacterSelectFacing(90)").unwrap();
        let got = facing(&s);
        assert!((got - 90.0).abs() < 1e-4, "{got}");
        // One value back, the argument itself, as the caller wrote it (`0x4736ae`).
        assert_eq!(
            s.eval::<(f64, String, f64)>(
                "local n = function(...) return arg.n end \
                 return n(SetCharacterSelectFacing(5)), SetCharacterSelectFacing('45'), \
                 SetCharacterSelectFacing(1, 7)"
            )
            .unwrap(),
            (1.0, "45".to_string(), 7.0)
        );
        // No wrap or clamp.
        s.run("SetCharacterSelectFacing(-725.5)").unwrap();
        let got = facing(&s);
        assert!((got + 725.5).abs() < 1e-3, "{got}");
        // Every select zeroes it first (`0x472a61`), already-built records included.
        s.run("SelectCharacter(1)").unwrap();
        assert_eq!(facing(&s), 0.0);
        // The gate stays open: a teardown does not close it.
        let mut s = s;
        s.teardown_glue_roster();
        s.run("SetCharacterSelectFacing(10)").unwrap();
        assert!((facing(&s) - 10.0).abs() < 1e-4);
        let err = s
            .run("SetCharacterSelectFacing('x')")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("Usage: SetCharacterSelectFacing(degrees)"),
            "{err}"
        );
    }

    /// An index off the roster exits before the facing reset (`0x472971`), so an empty roster
    /// keeps the facing the player left.
    #[test]
    fn an_empty_select_keeps_the_facing() {
        let mut s = UiScript::new_glue().unwrap();
        s.set_glue_select_cells(GlueSelectCells {
            selected: 0,
            facing: 1.5,
            built: true,
        });
        s.run("SelectCharacter(1)").unwrap();
        assert_eq!(s.glue_select_cells().facing, 1.5);
    }

    #[test]
    fn the_model_frame_and_the_background() {
        let s = vm(three());
        s.run("CreateFrame('Frame', 'Plain') CreateFrame('ModelFFX', 'Scene')")
            .unwrap();
        // No frame set yet: the call records nothing.
        s.run("SetCharSelectBackground('a.mdx')").unwrap();
        assert_eq!(s.glue_select_background(), None);
        // A frame that is not a model is not taken, a missing name is not either.
        s.run("SetCharSelectModelFrame('Plain') SetCharSelectModelFrame('Nope')")
            .unwrap();
        s.run("SetCharSelectBackground('a.mdx')").unwrap();
        assert_eq!(s.glue_select_background(), None);
        s.run("SetCharSelectModelFrame('Scene') SetCharSelectBackground('a.mdx')")
            .unwrap();
        assert_eq!(s.glue_select_background().as_deref(), Some("a.mdx"));
        // It is the frame's own `SetModel` (`0x472128`).
        assert_eq!(
            s.eval::<String>("return Scene:GetModel()").unwrap(),
            "a.mdx"
        );
        // An empty name is ignored (`0x472123`).
        s.run("SetCharSelectBackground('')").unwrap();
        assert_eq!(s.glue_select_background().as_deref(), Some("a.mdx"));
        // The frame's own `SetModel` replaces the scene, and the select fill with it.
        s.run("Scene:SetModel('b.mdx')").unwrap();
        assert_eq!(s.glue_select_background(), None);
        s.run("SetCharSelectBackground('a.mdx')").unwrap();
        for (call, usage) in [
            (
                "SetCharSelectModelFrame()",
                "Usage: SetCharSelectModelFrame(\"frameName\")",
            ),
            (
                "SetCharSelectBackground({})",
                "Usage: SetCharSelectBackground(\"filename\")",
            ),
        ] {
            let err = s.run(call).unwrap_err().to_string();
            assert!(err.contains(usage), "{err}");
        }
        s.run("UpdateSelectionCustomizationScene()").unwrap();
    }

    /// The scene's fog follows its fill callback: `SetModel` installs the fog one (`0x76cd96`), so
    /// the select screen's OnLoad scene fogs with the fog its fields hold; `SetCharSelectBackground`
    /// installs `0x472150` over it (`0x472144`), and the fog fields written after it change nothing;
    /// the next `SetModel` brings the fog back.
    #[test]
    fn the_select_scene_fogs_until_a_background_is_set() {
        let s = UiScript::new_glue().unwrap();
        s.run(
            "local f = CreateFrame('ModelFFX', 'Scene') \
             f:SetModel('UI_Orc.mdx') f:SetFogColor(0.5, 0.5, 0.5) f:SetFogNear(0) \
             f:SetFogFar(270) SetCharSelectModelFrame('Scene')",
        )
        .unwrap();
        let pane = s.glue_scene().expect("the shown ModelFFX");
        assert_eq!(pane.path, "UI_Orc.mdx");
        assert!(pane.select);
        let fog = pane.fog.expect("the OnLoad scene fogs");
        assert_eq!((fog.near, fog.far), (0.0, 270.0));
        assert_eq!(fog.rgb().map(|c| (c * 255.0).round()), [128.0; 3]);

        s.run(
            "SetCharSelectBackground('UI_Human.mdx') \
             Scene:SetFogColor(0.8, 0.65, 0.73) Scene:SetFogFar(222)",
        )
        .unwrap();
        let pane = s.glue_scene().unwrap();
        assert_eq!((pane.path.as_str(), pane.fog), ("UI_Human.mdx", None));
        // Every call makes a new instance, the same file included (`0x76ccf5`-`0x76cd0d`).
        let first = pane.instance;
        s.run("SetCharSelectBackground('UI_Human.mdx')").unwrap();
        assert!(s.glue_scene().unwrap().instance > first);

        s.run("Scene:SetModel('UI_Dwarf.mdx')").unwrap();
        let pane = s.glue_scene().unwrap();
        assert_eq!(pane.path, "UI_Dwarf.mdx");
        assert_eq!(pane.fog.map(|f| f.far), Some(222.0));
    }

    /// The scene is the shown `ModelFFX`'s, the topmost when two are shown; a hidden one, a plain
    /// `Model` and a pane with no file are none.
    #[test]
    fn the_scene_is_the_topmost_shown_model_ffx() {
        let mut s = UiScript::new_glue().unwrap();
        s.set_screen_size(1024.0, 768.0);
        assert_eq!(s.glue_scene(), None);
        s.run(
            "local low = CreateFrame('ModelFFX', 'Low') low:SetModel('low.mdx') \
             low:SetFrameLevel(2) \
             local high = CreateFrame('ModelFFX', 'High') high:SetModel('high.mdx') \
             high:SetFrameLevel(5) \
             local plain = CreateFrame('Model', 'Plain') plain:SetModel('plain.mdx') \
             plain:SetFrameLevel(9) \
             CreateFrame('ModelFFX', 'Empty'):SetFrameLevel(9)",
        )
        .unwrap();
        let pane = s.glue_scene().unwrap();
        assert_eq!(pane.path, "high.mdx");
        assert!(!pane.select && pane.fog.is_none());
        assert_eq!(pane.rect, None, "not laid out yet");
        s.run("High:SetPoint('TOPLEFT', 256, -192) High:SetWidth(512) High:SetHeight(384)")
            .unwrap();
        s.resolve();
        crate::assert_near!(
            s.glue_scene().unwrap().rect,
            Some([0.25, 0.75, 0.25, 0.75]),
            "the rect as screen fractions from the top-left"
        );
        s.run("High:Hide()").unwrap();
        assert_eq!(s.glue_scene().unwrap().path, "low.mdx");
        s.run("Low:Hide()").unwrap();
        assert_eq!(s.glue_scene(), None);
    }

    /// The character on the scene's attachment 0 is the selected record once its composite is
    /// built: none for a record with no model, none once the records are freed.
    #[test]
    fn the_standing_character_is_the_built_selection() {
        let mut roster = three();
        roster[1].model = false;
        let mut s = vm(roster);
        assert_eq!(s.glue_standing_guid(), Some(1));
        s.run("SelectCharacter(2)").unwrap();
        assert_eq!(s.glue_selected_guid(), Some(2));
        assert_eq!(s.glue_standing_guid(), None, "no model, nothing built");
        s.run("SelectCharacter(3)").unwrap();
        assert_eq!(s.glue_standing_guid(), Some(3));
        s.run("GetCharacterListUpdate()").unwrap();
        assert_eq!(s.glue_standing_guid(), None);
        let _ = s.load_glue_roster(three());
        assert_eq!(s.glue_standing_guid(), Some(1));
        s.teardown_glue_roster();
        assert_eq!(s.glue_standing_guid(), None);
    }

    /// `0x472580` ends with the model frame's `SetCamera(0)` (`0x4725d7`).
    #[test]
    fn the_teardown_puts_the_model_frame_back_on_camera_zero() {
        let mut s = vm(three());
        s.set_model_facts(
            "a.mdx",
            crate::widget::ModelFileFacts {
                sequences: Vec::new(),
                bbox: ([0.0; 3], [1.0; 3]),
                cameras: 2,
            },
        );
        s.run(
            "CreateFrame('ModelFFX', 'Scene'):SetModel('a.mdx') Scene:SetCamera(1) \
             SetCharSelectModelFrame('Scene')",
        )
        .unwrap();
        let camera = |s: &UiScript| {
            let model = s.model_ref();
            let h = model.arena.lookup("Scene").unwrap();
            match &model.arena.frame(h).unwrap().kind_state {
                crate::widget::KindState::Model(m) => m.camera,
                _ => unreachable!(),
            }
        };
        assert_eq!(camera(&s), Some(1));
        s.teardown_glue_roster();
        assert_eq!(camera(&s), Some(0));
    }

    /// `0x4723d0` clears at the call: the count reads 0 straight after, and the host is asked for
    /// the list.
    #[test]
    fn get_character_list_update_clears_and_asks_the_host() {
        let mut s = vm(three());
        s.take_glue_calls();
        s.run("GetCharacterListUpdate()").unwrap();
        assert_eq!(s.eval::<f64>("return GetNumCharacters()").unwrap(), 0.0);
        assert!(matches!(
            s.take_glue_calls().as_slice(),
            [GlueCall::GetCharacterList]
        ));
        assert!(fired(&s).is_empty(), "the clear fires nothing");
    }

    /// `0x472580`: whatever the roster held, an empty one is selected through the clamp, which
    /// fires with 1, then `CHARACTER_LIST_UPDATE`.
    #[test]
    fn the_teardown_fires_select_one_then_the_list_update() {
        let mut s = vm(three());
        s.run("SelectCharacter(3)").unwrap();
        s.run("FIRED = {}").unwrap();
        s.teardown_glue_roster();
        assert_eq!(
            fired(&s),
            ["UPDATE_SELECTED_CHARACTER 1", "CHARACTER_LIST_UPDATE nil"]
        );
        assert_eq!(s.eval::<f64>("return GetNumCharacters()").unwrap(), 0.0);
        assert_eq!(s.glue_select_cells().selected, 0);
    }

    /// `0x4724d0` with characters: the realm marked chosen, `lastCharacterIndex` selected (0-based
    /// into the roster, one more in the event), then the list update; out of range reads 0.
    #[test]
    fn the_load_selects_the_last_character_then_updates_the_list() {
        let mut s = UiScript::new_glue().unwrap();
        s.register_cvars([("lastCharacterIndex", "0")]);
        s.run(
            "FIRED = {} local f = CreateFrame('Frame', 'Listener') \
             f:SetScript('OnEvent', function() table.insert(FIRED, event .. ' ' .. tostring(arg1)) end) \
             f:RegisterEvent('UPDATE_SELECTED_CHARACTER') f:RegisterEvent('CHARACTER_LIST_UPDATE')",
        )
        .unwrap();
        s.set_cvar_engine("lastCharacterIndex", "2");
        assert!(!s.load_glue_roster(three()));
        assert_eq!(
            fired(&s),
            ["UPDATE_SELECTED_CHARACTER 3", "CHARACTER_LIST_UPDATE nil"]
        );
        assert_eq!(s.glue_realm_cells().chosen, Some(0));
        assert_eq!(s.glue_selected_guid(), Some(3));

        s.run("FIRED = {}").unwrap();
        s.set_cvar_engine("lastCharacterIndex", "9");
        assert!(!s.load_glue_roster(three()));
        assert_eq!(fired(&s)[0], "UPDATE_SELECTED_CHARACTER 1");
        s.run("FIRED = {}").unwrap();
        s.set_cvar_engine("lastCharacterIndex", "-3");
        assert!(!s.load_glue_roster(three()));
        assert_eq!(fired(&s)[0], "UPDATE_SELECTED_CHARACTER 1");
        // The record's integer is `SStrToInt` (`0x472560` reads `+0x28`): no whitespace skip and
        // no `+`, so these read 0, and the 32-bit wrap makes `4294967298` 2.
        for (text, picked) in [(" 2", "1"), ("+2", "1"), ("2x", "3"), ("4294967298", "3")] {
            s.run("FIRED = {}").unwrap();
            s.set_cvar_engine("lastCharacterIndex", text);
            assert!(!s.load_glue_roster(three()));
            assert_eq!(
                fired(&s)[0],
                format!("UPDATE_SELECTED_CHARACTER {picked}"),
                "{text:?}"
            );
        }
    }

    /// A VM over `three()` with Bravo flagged for a rename, `FIRED` also logging
    /// `FORCE_RENAME_CHARACTER`.
    fn rename_vm() -> UiScript {
        let mut roster = three();
        roster[1].flags = FLAG_RENAME | 0x2000;
        let mut s = vm(roster);
        s.run("Listener:RegisterEvent('FORCE_RENAME_CHARACTER')")
            .unwrap();
        s.take_glue_calls();
        s
    }

    /// `0x4734c0`: an index on the roster hands its guid to the host, every path returns nothing,
    /// and only a non-number raises.
    #[test]
    fn delete_character_hands_the_records_guid_to_the_host() {
        let mut roster = three();
        roster[2].guid = 0;
        let mut s = vm(roster);
        s.take_glue_calls();
        for call in [
            "DeleteCharacter(2)",
            "DeleteCharacter('2')",
            "DeleteCharacter(0)",
            "DeleteCharacter(4)",
            "DeleteCharacter(-1)",
            "DeleteCharacter(3)",
        ] {
            assert_eq!(s.eval::<f64>(&count(call)).unwrap(), 0.0, "{call}");
        }
        let queued = s.take_glue_calls();
        assert!(
            matches!(
                queued.as_slice(),
                [GlueCall::DeleteCharacter(2), GlueCall::DeleteCharacter(2)]
            ),
            "off the roster, or a zero guid (`0x46b2ed`), sends nothing: {queued:?}"
        );
        for call in ["DeleteCharacter()", "DeleteCharacter('x')"] {
            let err = s.run(call).unwrap_err().to_string();
            assert!(err.contains("Usage: DeleteCharacter(index)"), "{err}");
        }
        assert!(fired(&s).is_empty(), "the verb fires nothing itself");
    }

    /// `0x473520`'s silent arms: no values, nothing fired, nothing sent.
    #[test]
    fn rename_character_returns_nothing_off_its_checks() {
        let mut s = rename_vm();
        for call in [
            "RenameCharacter(2)",
            "RenameCharacter(2, nil)",
            "RenameCharacter(2, {})",
            "RenameCharacter(2, '')",
            "RenameCharacter(2, '\\0Newname')",
            "RenameCharacter(0, 'Newname')",
            "RenameCharacter(4, 'Newname')",
            // Alpha is not flagged (`0x4735ab`).
            "RenameCharacter(1, 'Newname')",
        ] {
            assert_eq!(s.eval::<f64>(&count(call)).unwrap(), 0.0, "{call}");
        }
        assert!(fired(&s).is_empty());
        assert!(s.take_glue_calls().is_empty());
        for call in ["RenameCharacter()", "RenameCharacter('x', 'Newname')"] {
            let err = s.run(call).unwrap_err().to_string();
            assert!(err.contains("Usage: RenameCharacter(index, name)"), "{err}");
        }
    }

    /// The old name again, case folded, is `CHAR_CREATE_NAME_IN_USE` and no values (`0x4735bd`).
    #[test]
    fn rename_character_to_the_same_name_is_name_in_use() {
        let mut s = rename_vm();
        assert_eq!(
            s.eval::<f64>(&count("RenameCharacter(2, 'bRAVO')"))
                .unwrap(),
            0.0
        );
        assert_eq!(
            fired(&s),
            ["FORCE_RENAME_CHARACTER CHAR_CREATE_NAME_IN_USE"]
        );
        assert!(s.take_glue_calls().is_empty());
    }

    /// A name the check fails is `FORCE_RENAME_CHARACTER` with the check's key, one nil and
    /// nothing sent (`0x46b379`-`0x46b392`); a number is a name too, as its string.
    #[test]
    fn rename_character_with_a_failing_name_fires_the_checks_key() {
        struct Reserved;
        impl super::super::NameLists for Reserved {
            fn matches(&self, name: &str) -> bool {
                name.eq_ignore_ascii_case("thrall")
            }
        }
        let mut s = rename_vm();
        s.set_glue_name_lists(std::rc::Rc::new(Reserved));
        for (call, key) in [
            ("RenameCharacter(2, 'X')", "CHAR_NAME_TOO_SHORT"),
            ("RenameCharacter(2, 'Abc1')", "CHAR_NAME_INVALID_CHARACTER"),
            ("RenameCharacter(2, 12)", "CHAR_NAME_INVALID_CHARACTER"),
            ("RenameCharacter(2, 'Abbbc')", "CHAR_NAME_THREE_CONSECUTIVE"),
            ("RenameCharacter(2, 'Abcdefghijklm')", "CHAR_NAME_TOO_LONG"),
            ("RenameCharacter(2, 'Thrall')", "CHAR_NAME_RESERVED"),
        ] {
            s.run("FIRED = {}").unwrap();
            assert_eq!(
                s.eval::<(f64, Option<f64>)>(&format!(
                    "local function n(...) return arg.n, arg[1] end return n({call})"
                ))
                .unwrap(),
                (1.0, None),
                "{call}"
            );
            assert_eq!(
                fired(&s),
                [format!("FORCE_RENAME_CHARACTER {key}")],
                "{call}"
            );
        }
        assert!(s.take_glue_calls().is_empty());
    }

    /// A passing name is the host's, and the verb answers 1 with nothing fired; a C string ends at
    /// its NUL.
    #[test]
    fn rename_character_with_a_passing_name_asks_the_host() {
        let mut s = rename_vm();
        assert_eq!(
            s.eval::<(f64, f64)>(
                "local function n(...) return arg.n, arg[1] end \
                 return n(RenameCharacter(2, 'Newname'))"
            )
            .unwrap(),
            (1.0, 1.0)
        );
        assert_eq!(
            s.eval::<f64>("return RenameCharacter('2', 'Zoë\\0Bravo')")
                .unwrap(),
            1.0
        );
        assert!(fired(&s).is_empty());
        let queued = s.take_glue_calls();
        assert!(
            matches!(
                queued.as_slice(),
                [
                    GlueCall::RenameCharacter { guid: 2, name: a },
                    GlueCall::RenameCharacter { guid: 2, name: b },
                ] if a == "Newname" && b == "Zoë"
            ),
            "{queued:?}"
        );
    }

    /// The rename's check holds the name to the client's charsets (`0x46b36f` → `0x5abd50`): a
    /// Hangul name is the host's on a koKR client and invalid on an enUS one, and a `Region` of
    /// 2 gives enUS the same Hangul pair.
    #[test]
    fn rename_character_checks_the_clients_charsets() {
        let hangul = "RenameCharacter(2, '한글')";
        let key = |s: &mut super::super::UiScript| {
            s.run("FIRED = {}").unwrap();
            s.eval::<f64>(&format!(
                "local function n(...) return arg.n end return n({hangul})"
            ))
            .unwrap();
            fired(s)
        };
        let mut s = rename_vm();
        assert_eq!(
            key(&mut s),
            ["FORCE_RENAME_CHARACTER CHAR_NAME_INVALID_CHARACTER"]
        );
        assert!(s.take_glue_calls().is_empty());

        s.set_locale(1);
        assert!(key(&mut s).is_empty(), "koKR takes Hangul");
        assert!(matches!(
            s.take_glue_calls().as_slice(),
            [GlueCall::RenameCharacter { guid: 2, name }] if name == "한글"
        ));

        s.set_locale(0);
        s.set_client_region(2);
        assert!(
            key(&mut s).is_empty(),
            "Region 2 takes Hangul on any locale"
        );
        assert_eq!(s.take_glue_calls().len(), 1);

        s.set_client_region(1);
        s.set_locale(1);
        assert_eq!(
            key(&mut s),
            ["FORCE_RENAME_CHARACTER CHAR_NAME_INVALID_CHARACTER"],
            "Region 1 is Latin even on koKR"
        );
    }

    /// `0x46d3c0`: the selected record's index, guid and flags go to the host and the call returns
    /// no values; with the selection off the roster nothing goes (`0x46b53e`), and the verb fires
    /// nothing itself.
    #[test]
    fn enter_world_hands_the_selected_record_to_the_host() {
        let mut roster = three();
        roster[1].flags = 0x4004;
        let mut s = vm(roster);
        s.take_glue_calls();
        s.run("SelectCharacter(2) FIRED = {}").unwrap();
        assert_eq!(s.eval::<f64>(&count("EnterWorld('x', 9)")).unwrap(), 0.0);
        let queued = s.take_glue_calls();
        assert!(
            matches!(
                queued.as_slice(),
                [GlueCall::EnterWorld {
                    index: 1,
                    guid: 2,
                    flags: 0x4004
                }]
            ),
            "{queued:?}"
        );
        assert!(fired(&s).is_empty());

        for mut s in [vm(Vec::new()), UiScript::new_glue().unwrap()] {
            s.take_glue_calls();
            assert_eq!(s.eval::<f64>(&count("EnterWorld()")).unwrap(), 0.0);
            assert!(s.take_glue_calls().is_empty());
        }
    }

    /// `0x4725f0` on the VM's side: the record found by guid takes the name, loses the rename flag
    /// and keeps the rest, and `CHARACTER_LIST_UPDATE` fires; another guid changes nothing.
    #[test]
    fn the_rename_applier_renames_the_record_and_clears_its_flag() {
        let mut s = rename_vm();
        assert_eq!(s.glue_character_name(2).as_deref(), Some("Bravo"));
        s.apply_glue_rename(9, "Nobody");
        assert!(fired(&s).is_empty(), "no record, no event");
        s.apply_glue_rename(2, "Newname");
        assert_eq!(fired(&s), ["CHARACTER_LIST_UPDATE nil"]);
        assert_eq!(
            s.eval::<(String, Option<i64>)>(
                "local n, _, _, _, _, _, _, ghost = GetCharacterInfo(2) return n, ghost"
            )
            .unwrap(),
            ("Newname".to_string(), Some(1)),
            "the ghost bit stays"
        );
        assert_eq!(s.glue_character_name(2).as_deref(), Some("Newname"));
        // The flag is clear: a second rename is refused at `0x4735ab`.
        assert_eq!(
            s.eval::<f64>(&count("RenameCharacter(2, 'Othername')"))
                .unwrap(),
            0.0
        );
        assert!(s.take_glue_calls().is_empty());
    }

    /// `0x4724d0` with none: -1 is selected (the event carries 1), then with no realm chosen the
    /// load asks for the realm list and fires no list update; with one it fires the update.
    #[test]
    fn an_empty_load_asks_for_the_realm_list_unless_one_is_chosen() {
        let mut s = vm(Vec::new());
        assert!(s.load_glue_roster(Vec::new()));
        assert_eq!(fired(&s), ["UPDATE_SELECTED_CHARACTER 1"]);

        s.run("FIRED = {}").unwrap();
        let mut cells = s.glue_realm_cells();
        cells.chosen = Some(4);
        s.set_glue_realm_cells(cells);
        assert!(!s.load_glue_roster(Vec::new()));
        assert_eq!(
            fired(&s),
            ["UPDATE_SELECTED_CHARACTER 1", "CHARACTER_LIST_UPDATE nil"]
        );
        assert_eq!(s.glue_realm_cells().chosen, Some(4), "a choice stands");
    }
}
