//! The guild tabard designer: the `TabardModel` method table (`0x84ee40`) and the window's two
//! globals. Its state is five ints on the frame, which the constructor leaves unwritten until
//! `InitializeTabardColors()` seeds them; the app pushes the guild record and save latch and runs
//! the save's fourteen checks (`0x5e03f0`).

use mlua::{Lua, Value};

use super::binding_abi::number_arg;
use super::object::{decode_id, frame_handle_of};
use super::region::region_handle_of;
use super::Model;
use crate::widget::{FrameHandle, FrameKind, RegionKind};

/// Registry key of the TabardModel method table, probed before PlayerModel's and Model's.
pub(super) const REG_TABARDMODEL_METHODS: &str = "__benilla_tabardmodel_methods";

/// The designer's price in copper, a constant in the reference (`0x6d6de0`).
pub const TABARD_CREATION_COST: u32 = 100_000;

/// The five slot counts in `CycleVariation` order (emblem style and colour, border style and
/// colour, background colour): the reference's constant table at `0x808220`, not a DBC.
pub const TABARD_COUNTS: [i32; 5] = [170, 17, 6, 17, 51];

/// The prefix a Texture carries before the emblem path after `Get{Upper,Lower}EmblemTexture`; the
/// app draws the reference's generated mask, white carrying the emblem's alpha (`0x503431`).
pub const EMBLEM_MASK_TOKEN: &str = "benilla:emblem-mask:";

/// The BLP path inside an [`EMBLEM_MASK_TOKEN`] texture, or `None` for an ordinary path.
pub fn emblem_mask_path(texture: &str) -> Option<&str> {
    texture.strip_prefix(EMBLEM_MASK_TOKEN)
}

/// The designer's two host-side facts, pushed by the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct TabardHost {
    /// The cached guild record's emblem fields, `-1` each when undesigned; `None` until cached.
    pub guild_record: Option<[i32; 5]>,
    /// The save-in-flight latch (`0xc4d780`): set when the save is sent, cleared by the reply.
    pub save_pending: bool,
}

/// What the designer asks the app to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabardIntent {
    /// `Save()`: the five values, for the checks and the send.
    Save([i32; 5]),
    /// `CloseTabardCreation()`.
    Close,
}

impl super::UiScript {
    /// Push the guild record and the save latch.
    pub fn set_tabard_host(&mut self, host: TabardHost) {
        self.model_mut().tabard_host = host;
    }

    /// The design the app previews on the player's body, `None` before the seed.
    pub fn tabard_design(&self) -> Option<[i32; 5]> {
        self.model_ref().tabard_preview
    }

    /// The queued intents since the last drain.
    pub fn take_tabard_intents(&mut self) -> Vec<TabardIntent> {
        std::mem::take(&mut self.model_mut().tabard_intents)
    }
}

/// The three `this` raises every method shares (`0x847ef8`, `0x847ec0`, `0x847e98`).
fn this_tabard(lua: &Lua, this: &Value) -> mlua::Result<FrameHandle> {
    let Value::Table(t) = this else {
        return Err(mlua::Error::runtime(
            "Attempt to find 'this' in non-table object (used '.' instead of ':' ?)",
        ));
    };
    if decode_id(t).is_err() {
        return Err(mlua::Error::runtime(
            "Attempt to find 'this' in non-framescript object",
        ));
    }
    let wrong = || mlua::Error::runtime("Wrong object type for member function");
    let h = frame_handle_of(lua, t).map_err(|_| wrong())?;
    let model = lua.app_data_ref::<Model>().expect("model app_data");
    let is_tabard = model
        .arena
        .frame(h)
        .is_some_and(|f| f.kind == FrameKind::TabardModel);
    drop(model);
    if !is_tabard {
        return Err(wrong());
    }
    Ok(h)
}

/// The seeded design, or all zeros. Deviation: the reference reads uninitialised fields before
/// the seed; we answer the first legal design because that value is undefined.
fn design(model: &Model, h: FrameHandle) -> [i32; 5] {
    model.tabard_designs.get(&h).copied().unwrap_or([0; 5])
}

fn store(lua: &Lua, h: FrameHandle, five: [i32; 5]) {
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    model.tabard_designs.insert(h, five);
    model.tabard_preview = Some(five);
}

/// `(count * rand) >> 32`, a draw in `[0, count)` (`0x455c70`).
fn draw(rand: u32, count: i32) -> i32 {
    ((u64::from(count as u32) * u64::from(rand)) >> 32) as i32
}

/// A clock-seeded generator in place of the reference's table-driven one (`0x4531e0`); only the
/// range law above is the reference's.
fn random_words() -> impl Iterator<Item = u32> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E37_79B9_7F4A_7C15);
    let mut x = nanos | 1;
    std::iter::from_fn(move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        Some((x >> 32) as u32)
    })
}

/// `InitializeTabardColors()`'s worker (`0x5028e0`): the cached guild record unless a field is
/// `-1`, else five random draws.
fn seed(host: TabardHost) -> [i32; 5] {
    match host.guild_record {
        Some(rec) if rec.iter().all(|v| *v != -1) => rec,
        _ => {
            let mut words = random_words();
            std::array::from_fn(|i| draw(words.next().unwrap_or(0), TABARD_COUNTS[i]))
        }
    }
}

/// `(v + count + delta) mod count`, a signed `idiv` (`0x502ac0`); `|delta| >= count` is a silent
/// no-op, with no repaint.
fn cycle(five: &mut [i32; 5], slot: usize, delta: i32) -> bool {
    let count = TABARD_COUNTS[slot];
    if delta.wrapping_abs() >= count {
        return false;
    }
    five[slot] = (five[slot] + count + delta) % count;
    true
}

/// The emblem path for a half (`TU` upper, `TL` lower) in `0x47a520`'s format, left without the
/// `.BLP` the reference appends (`0x503396`), as every region texture path is.
fn emblem_path(five: [i32; 5], half: &str) -> String {
    format!(
        "Textures\\GuildEmblems\\Emblem_{:02}_{:02}_{half}_U",
        five[0], five[1]
    )
}

/// `Get{Upper,Lower}EmblemTexture(texture)`: the argument's three raises (`0x84efc8`,
/// `0x84f050`, `0x84f010` for the upper), then the install.
fn emblem_texture(lua: &Lua, this: Value, texture: Value, upper: bool) -> mlua::Result<()> {
    let h = this_tabard(lua, &this)?;
    let (name, half) = if upper {
        ("GetUpperEmblemTexture", "TU")
    } else {
        ("GetLowerEmblemTexture", "TL")
    };
    let Value::Table(t) = texture else {
        return Err(mlua::Error::runtime(format!(
            "Usage: TabardModel:{name}(texture)"
        )));
    };
    if decode_id(&t).is_err() {
        return Err(mlua::Error::runtime(format!(
            "TabardModel:{name}(): Couldn't find 'this' in texture object"
        )));
    }
    let wrong = || {
        mlua::Error::runtime(format!(
            "TabardModel:{name}(): Wrong object type, expected texture"
        ))
    };
    let rh = region_handle_of(lua, &t).map_err(|_| wrong())?;
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    if !model
        .arena
        .region(rh)
        .is_some_and(|r| r.kind == RegionKind::Texture)
    {
        return Err(wrong());
    }
    let path = emblem_path(design(&model, h), half);
    let data = model.region_data.entry(rh).or_default();
    data.texture = Some(format!("{EMBLEM_MASK_TOKEN}{path}"));
    data.fill = None;
    data.portrait_unit = None;
    data.circular = false;
    Ok(())
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let m = lua.create_table()?;

    // `0x502b10`: the seed needs an active player.
    m.set(
        "InitializeTabardColors",
        lua.create_function(|lua, this: Value| {
            let h = this_tabard(lua, &this)?;
            let host = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                if !model.unit("player").is_some_and(|u| u.exists) {
                    return Ok(());
                }
                model.tabard_host
            };
            store(lua, h, seed(host));
            Ok(())
        })?,
    )?;

    // `0x502bd0`: the app runs the fourteen checks on the queued five.
    m.set(
        "Save",
        lua.create_function(|lua, this: Value| {
            let h = this_tabard(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            if !model.unit("player").is_some_and(|u| u.exists) {
                return Ok(());
            }
            let five = design(&model, h);
            model.tabard_intents.push(TabardIntent::Save(five));
            Ok(())
        })?,
    )?;

    // `0x502ca0`: a non-number raises the usage, an index off 1..=5 its own error; both truncate.
    m.set(
        "CycleVariation",
        lua.create_function(|lua, (this, index, delta): (Value, Value, Value)| {
            let h = this_tabard(lua, &this)?;
            const USAGE: &str = "Usage: CycleVariation(variationIndex, delta)";
            let index = number_arg(lua, index, USAGE)?;
            let delta = number_arg(lua, delta, USAGE)?;
            let slot = usize::try_from(index)
                .ok()
                .and_then(|i| i.checked_sub(1))
                .filter(|i| *i < 5)
                .ok_or_else(|| mlua::Error::runtime("Invalid variationIndex in CycleVariation"))?;
            let mut five = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                design(&model, h)
            };
            if cycle(&mut five, slot, delta as i32) {
                store(lua, h, five);
            }
            Ok(())
        })?,
    )?;

    // `0x502dc0`, `0x502ea0`, `0x502f80`, `0x503070`: one string each, without `.blp`.
    for (name, upper, background) in [
        ("GetUpperBackgroundFileName", true, true),
        ("GetLowerBackgroundFileName", false, true),
        ("GetUpperEmblemFileName", true, false),
        ("GetLowerEmblemFileName", false, false),
    ] {
        m.set(
            name,
            lua.create_function(move |lua, this: Value| {
                let h = this_tabard(lua, &this)?;
                let five = {
                    let model = lua.app_data_ref::<Model>().expect("model app_data");
                    design(&model, h)
                };
                let half = if upper { "TU" } else { "TL" };
                Ok(if background {
                    format!("Textures\\GuildEmblems\\Background_{:02}_{half}_U", five[4])
                } else {
                    format!(
                        "Textures\\GuildEmblems\\Emblem_{:02}_{:02}_{half}_U",
                        five[0], five[1]
                    )
                })
            })?,
        )?;
    }

    // Setters despite their names (`0x503160`, `0x503540`): the reference installs a 128x64 or
    // 128x32 mask into the passed texture.
    m.set(
        "GetUpperEmblemTexture",
        lua.create_function(|lua, (this, texture): (Value, Value)| {
            emblem_texture(lua, this, texture, true)
        })?,
    )?;
    m.set(
        "GetLowerEmblemTexture",
        lua.create_function(|lua, (this, texture): (Value, Value)| {
            emblem_texture(lua, this, texture, false)
        })?,
    )?;

    // `0x503910` never resolves `this`: the number 1 or nil, from a player, a cached record and
    // no save pending (`0x5f0800`), not a would-it-succeed test.
    m.set(
        "CanSaveTabardNow",
        lua.create_function(|lua, _: mlua::MultiValue| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let can = model.unit("player").is_some_and(|u| u.exists)
                && model.tabard_host.guild_record.is_some()
                && !model.tabard_host.save_pending;
            Ok(if can { Value::Integer(1) } else { Value::Nil })
        })?,
    )?;

    lua.set_named_registry_value(REG_TABARDMODEL_METHODS, m)?;

    let g = lua.globals();
    g.set(
        "GetTabardCreationCost",
        lua.create_function(|_, ()| Ok(i64::from(TABARD_CREATION_COST)))?,
    )?;
    // `0x4f5900`, no packet: the app runs the close (`0x4f58a0`) and its `CLOSE_TABARD_FRAME`,
    // gated on a stored vendor.
    g.set(
        "CloseTabardCreation",
        lua.create_function(|lua, _: mlua::MultiValue| {
            lua.app_data_mut::<Model>()
                .expect("model app_data")
                .tabard_intents
                .push(TabardIntent::Close);
            Ok(())
        })?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::{UiScript, UnitState};

    fn vm() -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.set_unit(
            "player",
            Some(UnitState {
                exists: true,
                name: Some("Probe".into()),
                level: 60,
                ..Default::default()
            }),
        );
        s.run(r#"t = CreateFrame("TabardModel", "TM") f = CreateFrame("Frame", "Plain") tex = f:CreateTexture("Cell")"#)
            .unwrap();
        s
    }

    #[test]
    fn the_creation_cost_is_ten_gold_and_a_tabard_model_is_its_own_kind() {
        let s = UiScript::new().unwrap();
        assert_eq!(
            s.eval::<i64>("return GetTabardCreationCost()").unwrap(),
            100_000
        );
        s.run(r#"t = CreateFrame("TabardModel", "TM")"#).unwrap();
        assert_eq!(
            s.eval::<String>("return t:GetObjectType()").unwrap(),
            "TabardModel"
        );
        assert!(s
            .eval::<bool>("return t:IsObjectType('PlayerModel') and t:IsObjectType('Model')")
            .unwrap());
        assert!(
            s.eval::<bool>("return t.SetUnit ~= nil and t.SetRotation ~= nil")
                .unwrap(),
            "the inherited pane verbs"
        );
    }

    #[test]
    fn the_seed_takes_the_guild_record_or_draws_in_range() {
        let mut s = vm();
        s.set_tabard_host(TabardHost {
            guild_record: Some([7, 3, 2, 12, 5]),
            save_pending: false,
        });
        s.run("t:InitializeTabardColors()").unwrap();
        assert_eq!(s.tabard_design(), Some([7, 3, 2, 12, 5]));
        assert_eq!(
            s.eval::<String>("return t:GetUpperEmblemFileName()")
                .unwrap(),
            r"Textures\GuildEmblems\Emblem_07_03_TU_U"
        );
        assert_eq!(
            s.eval::<String>("return t:GetLowerBackgroundFileName()")
                .unwrap(),
            r"Textures\GuildEmblems\Background_05_TL_U"
        );
        // An undesigned record (a -1 anywhere) and no record both draw at random, in range.
        for rec in [Some([7, 3, 2, -1, 5]), None] {
            s.set_tabard_host(TabardHost {
                guild_record: rec,
                save_pending: false,
            });
            s.run("t:InitializeTabardColors()").unwrap();
            let five = s.tabard_design().unwrap();
            for (v, count) in five.iter().zip(TABARD_COUNTS) {
                assert!((0..count).contains(v), "{five:?} within {TABARD_COUNTS:?}");
            }
        }
        // No player: nothing is seeded (the design keeps its last value).
        s.set_unit("player", None);
        s.set_tabard_host(TabardHost {
            guild_record: Some([1, 1, 1, 1, 1]),
            save_pending: false,
        });
        s.run("t:InitializeTabardColors()").unwrap();
        assert_ne!(s.tabard_design(), Some([1, 1, 1, 1, 1]));
    }

    #[test]
    fn cycle_variation_wraps_both_ways_and_ignores_a_delta_past_the_count() {
        let mut s = vm();
        s.set_tabard_host(TabardHost {
            guild_record: Some([0, 0, 0, 0, 0]),
            save_pending: false,
        });
        s.run("t:InitializeTabardColors()").unwrap();
        s.run("t:CycleVariation(1, -1)").unwrap();
        assert_eq!(s.tabard_design().unwrap()[0], 169, "wraps down");
        s.run("t:CycleVariation(1, 1) t:CycleVariation(3, 5) t:CycleVariation(3, 1)")
            .unwrap();
        assert_eq!(s.tabard_design().unwrap()[0], 0);
        assert_eq!(
            s.tabard_design().unwrap()[2],
            0,
            "5 then 1 over a count of 6 wraps to 0"
        );
        s.run("t:CycleVariation(5, 6) t:CycleVariation('2.9', '1.9')")
            .unwrap();
        assert_eq!(
            s.tabard_design().unwrap()[4],
            6,
            "truncation: (2.9, 1.9) is (2, 1)"
        );
        assert_eq!(s.tabard_design().unwrap()[1], 1);
        s.run("t:CycleVariation(2, 17)").unwrap();
        assert_eq!(
            s.tabard_design().unwrap()[1],
            1,
            "|delta| >= count is a silent no-op"
        );
        for (call, needle) in [
            (
                "t:CycleVariation(0, 1)",
                "Invalid variationIndex in CycleVariation",
            ),
            (
                "t:CycleVariation(6, 1)",
                "Invalid variationIndex in CycleVariation",
            ),
            (
                "t:CycleVariation(1)",
                "Usage: CycleVariation(variationIndex, delta)",
            ),
            (
                "t:CycleVariation('x', 1)",
                "Usage: CycleVariation(variationIndex, delta)",
            ),
            ("t.CycleVariation(1, 1)", "used '.' instead of ':'"),
            ("t.CycleVariation({}, 1, 1)", "non-framescript object"),
            (
                "t.CycleVariation(f, 1, 1)",
                "Wrong object type for member function",
            ),
        ] {
            let err = s.run(call).unwrap_err().to_string();
            assert!(err.contains(needle), "{call}: {err}");
        }
    }

    #[test]
    fn the_texture_setters_install_the_mask_token_and_raise_on_a_non_texture() {
        let mut s = vm();
        s.set_tabard_host(TabardHost {
            guild_record: Some([12, 4, 0, 0, 0]),
            save_pending: false,
        });
        s.run("t:InitializeTabardColors() t:GetUpperEmblemTexture(tex)")
            .unwrap();
        let got = s.eval::<String>("return tex:GetTexture()").unwrap();
        assert_eq!(
            emblem_mask_path(&got),
            Some(r"Textures\GuildEmblems\Emblem_12_04_TU_U")
        );
        s.run("t:GetLowerEmblemTexture(tex)").unwrap();
        let got = s.eval::<String>("return tex:GetTexture()").unwrap();
        assert!(got.ends_with("Emblem_12_04_TL_U"), "{got}");
        for (call, needle) in [
            (
                "t:GetUpperEmblemTexture()",
                "Usage: TabardModel:GetUpperEmblemTexture(texture)",
            ),
            (
                "t:GetUpperEmblemTexture({})",
                "Couldn't find 'this' in texture object",
            ),
            (
                "t:GetLowerEmblemTexture(f)",
                "Wrong object type, expected texture",
            ),
        ] {
            let err = s.run(call).unwrap_err().to_string();
            assert!(err.contains(needle), "{call}: {err}");
        }
    }

    #[test]
    fn can_save_is_one_or_nil_over_the_three_conjuncts_and_save_and_close_queue_intents() {
        let mut s = vm();
        s.set_tabard_host(TabardHost {
            guild_record: None,
            save_pending: false,
        });
        assert!(s
            .eval::<bool>("return t:CanSaveTabardNow() == nil")
            .unwrap());
        s.set_tabard_host(TabardHost {
            guild_record: Some([-1; 5]),
            save_pending: false,
        });
        assert!(
            s.eval::<bool>("return t:CanSaveTabardNow() == 1").unwrap(),
            "a cached record counts even undesigned; the number 1, not true"
        );
        s.set_tabard_host(TabardHost {
            guild_record: Some([-1; 5]),
            save_pending: true,
        });
        assert!(s
            .eval::<bool>("return t:CanSaveTabardNow() == nil")
            .unwrap());
        assert!(
            s.eval::<bool>("return TabardModel_CanSave == nil and t.CanSaveTabardNow() == nil")
                .unwrap(),
            "never resolves this: a dot-call is fine"
        );
        s.run("t:InitializeTabardColors() t:Save() CloseTabardCreation()")
            .unwrap();
        let intents = s.take_tabard_intents();
        assert!(matches!(intents[0], TabardIntent::Save(_)));
        assert_eq!(intents[1], TabardIntent::Close);
        assert!(s.take_tabard_intents().is_empty());
    }
}
