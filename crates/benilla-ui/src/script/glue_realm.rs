//! The realm list's glue verbs (`RealmList.cpp`, registrar `0x46f480`, table `0x837ea0`) and the
//! list they read: the realm holder the `CMD_REALM_LIST` parser fills (`0x5b2230`), the client
//! Region's categories (`0x46e300`), the sort (`0xb41f40`) and the two selection cells
//! (`0x837e98`, `0x837e9c`). The host's glue manager seeds the holder and runs the rebuild
//! (`0x46e510`) where `CGlueMgr::Update` does; the verbs that only read or re-sort the list answer
//! here, firing their events as they return, and the ones that reach the network call the glue
//! manager in place ([`super::GlueCall`]).
//!
//! Every category argument that crosses the Lua boundary is a 1-based ordinal over the categories
//! holding a realm, mapped to the stored index by `0x46edf0`, which has no sentinel: an ordinal
//! past the last one, or below 1, reads stored category 0.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::{bool_or_default, flag, ftol, string_arg};
use super::glue_core::{call, GlueCall};
use super::{Model, ScriptValue};

/// One realm of the holder, a `CMD_REALM_LIST` record (`0x5b2230`, stride `0x134`) as the parser
/// leaves it, the population sentinels already remapped into flag bits `0x20`/`0x40`/`0x80`.
#[derive(Clone, Debug, PartialEq)]
pub struct GlueRealm {
    /// `+0x09`.
    pub name: String,
    /// `+0x04`, the `Cfg_Configs.dbc` key.
    pub realm_type: u32,
    /// `+0x08`: bit `0x01` invalid, `0x02` down, `0x20`/`0x40`/`0x80` recommended, new, full.
    pub flags: u8,
    /// `+0x12c`.
    pub population: f32,
    /// `+0x130`, the account's characters there.
    pub characters: u8,
    /// `+0x131`, matched by equality against a category's id; 0 takes the first category's.
    pub category: u8,
}

/// One `Cfg_Categories.dbc` row of the client's Region, the category set `0x46e300` builds once
/// per process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlueRealmCategory {
    /// Column 0, the id a realm's category byte is matched against (`0x46e60b`).
    pub id: u32,
    /// The locale's `Name_Lang` slot (`[row + 8 + 4·locale]`, `0x46f238`).
    pub name: String,
}

/// One `Cfg_Configs.dbc` row (the store `0xc0dfa0`): a realm type and its PvP and RP flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GlueRealmType {
    pub realm_type: u32,
    pub pvp: bool,
    pub rp: bool,
}

/// A sort column, `SortRealms`' string argument (`0x46f330`) and the comparator's case
/// (`0x46e790`, jump table `0x46e998`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RealmSortKey {
    /// `"characters"` (`0x83807c`), case 0: more characters first.
    Characters,
    /// `"load"` (`0x838088`), case 1: the load band.
    Load,
    /// `"name"` (`0x838090`), case 2, and any string that names no column.
    Name,
    /// `"mode"` (`0x838074`), case 3: the `Cfg_Configs` `(rp, pvp)` pair.
    Mode,
}

/// The cells that outlive a glue VM: the reference keeps them in C globals, so a glue rebuild
/// (`0x46a7b0`, back from the world) finds them as they were. The host carries them from one glue
/// VM to the next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GlueRealmCells {
    /// The four `{key, descending}` records at `0xb41f40`, written once per process by
    /// `0x46e300`'s tail to characters, load, name, mode, all ascending.
    pub sort: [(RealmSortKey, bool); 4],
    /// `[0x837e98]`, the stored category `GetSelectedCategory` reads; -1 at load.
    pub category: i32,
    /// `[0x837e9c]`, "a realm has been chosen this session": -1 (`None`) at load, then 0 from
    /// `ChangeRealm` (`0x46f1d1`) or a roster with characters (`0x472556`, the host's), or a
    /// category id from `SetPreferredInfo` (`0x46ea52`).
    pub chosen: Option<u32>,
    /// The type `SetPreferredInfo` last asked for (`[0xb41fa8]`, `[0xb41fac]`).
    pub preferred: (bool, bool),
}

impl Default for GlueRealmCells {
    fn default() -> Self {
        GlueRealmCells {
            sort: [
                (RealmSortKey::Characters, false),
                (RealmSortKey::Load, false),
                (RealmSortKey::Name, false),
                (RealmSortKey::Mode, false),
            ],
            category: -1,
            chosen: None,
            preferred: (false, false),
        }
    }
}

/// A category as the list holds it: its id, `None` for the one benilla makes when the Region has
/// no rows, which every realm joins.
#[derive(Clone, Debug)]
struct Category {
    id: Option<u32>,
    name: Option<String>,
}

impl Category {
    fn holds(&self, id: Option<u32>) -> bool {
        self.id.is_none() || self.id == id
    }
}

/// The list the realm verbs read.
#[derive(Default)]
pub(crate) struct GlueRealmList {
    holder: Vec<GlueRealm>,
    /// The Region's rows; empty until seeded.
    rows: Vec<GlueRealmCategory>,
    /// Each category's realms as the last rebuild left them, indices into the holder (`[cat+0x10]`,
    /// counted by `[cat+0x18]`), each sorted by [`GlueRealmCells::sort`].
    buckets: Vec<Vec<usize>>,
    types: Vec<GlueRealmType>,
    /// `[0xb41fb0]` and `[0xb41fb4]`, the load band's mean and scaled deviation.
    mean: f32,
    stddev: f32,
    pub(super) cells: GlueRealmCells,
    /// `[svc+0x1ae4]`, the last `SMSG_CHAR_ENUM`'s character count.
    characters: u16,
}

/// The std-dev scale, 0.6 (`[0x8038d8]`).
const STDDEV_MULT: f32 = f32::from_bits(0x3f19_999a);

/// `realm_load_stats` (`0x46e510`): `(mean, sqrt(Σ(pop − mean)² / (n − 1)) · 0.6)` over every
/// realm's population. Both accumulators round to `f32` each iteration; the divide, sqrt and scale
/// run at 53 bits. `n <= 1` gives `(1.0, 0.0)`, so a lone realm is banded against the literal 1.0.
pub fn realm_load_stats(populations: &[f32]) -> (f32, f32) {
    let n = populations.len();
    if n <= 1 {
        return (1.0, 0.0);
    }
    // `fld sum; fadd pop; fstp sum`: stored as f32 each step.
    let mut sum: f32 = 0.0;
    for &pop in populations {
        sum = (f64::from(sum) + f64::from(pop)) as f32;
    }
    // `fild n; fdivr sum; fstp mean`.
    let mean = (f64::from(sum) / n as f64) as f32;
    let mut acc: f32 = 0.0;
    for &pop in populations {
        let d = f64::from(pop) - f64::from(mean);
        acc = (d * d + f64::from(acc)) as f32;
    }
    // `fild n-1; fdivr acc; fsqrt; fmul 0.6; fstp stddev`.
    let stddev = ((f64::from(acc) / (n - 1) as f64).sqrt() * f64::from(STDDEV_MULT)) as f32;
    (mean, stddev)
}

/// `realm_load_classify` (`0x46ec60`): `GetRealmInfo`'s load. The three sentinel bits, which the
/// parser synthesizes from magic populations, win in this order; then the band, whose 53-bit
/// comparisons are strict, so a population on an edge reads 0.
pub fn realm_load_classify(flags: u8, population: f32, mean: f32, stddev: f32) -> f32 {
    if flags & 0x20 != 0 {
        return -3.0;
    }
    if flags & 0x40 != 0 {
        return -2.0;
    }
    if flags & 0x80 != 0 {
        return 2.0;
    }
    if (f64::from(mean) - f64::from(stddev)) > f64::from(population) {
        return -1.0;
    }
    if (f64::from(mean) + f64::from(stddev)) < f64::from(population) {
        return 1.0;
    }
    0.0
}

/// `SStrCmpI` (`0x64a4c0` → the CRT's `_strnicmp`): bytes folded to lower case.
fn cmp_folded(a: &str, b: &str) -> std::cmp::Ordering {
    a.bytes()
        .map(|c| c.to_ascii_lowercase())
        .cmp(b.bytes().map(|c| c.to_ascii_lowercase()))
}

impl GlueRealmList {
    /// The categories: the Region's rows, or one that holds every realm when there are none.
    ///
    /// Deviation: with no Region rows the reference reads a NULL category (`0x46e5b8`,
    /// `0x46f26a`) and faults; benilla lists every realm under one category, so a client whose
    /// `Wow.ini` names another Region still reaches a realm.
    fn categories(&self) -> Vec<Category> {
        if self.rows.is_empty() {
            return vec![Category {
                id: None,
                name: None,
            }];
        }
        self.rows
            .iter()
            .map(|r| Category {
                id: Some(r.id),
                name: Some(r.name.clone()),
            })
            .collect()
    }

    /// `GetRealm` (`0x5ab930`): a holder index, or NULL past its end.
    fn realm(&self, index: usize) -> Option<&GlueRealm> {
        self.holder.get(index)
    }

    /// A category's realm count, `[cat+0x18]`: 0 before the first rebuild.
    fn count(&self, at: usize) -> usize {
        self.buckets.get(at).map_or(0, Vec::len)
    }

    /// `0x46edf0`: the stored index of the `visible`-th (0-based) category holding a realm, else 0.
    fn stored_index(&self, visible: i32) -> usize {
        let n = self.categories().len();
        (0..n)
            .filter(|&i| self.count(i) != 0)
            .nth(usize::try_from(visible).unwrap_or(usize::MAX))
            .unwrap_or(0)
    }

    /// The first `Cfg_Configs` row of a realm type, `(pvp, rp)`; `(false, false)` with none, as
    /// `GetRealmInfo` (`0x46efda`) and `realm_select` (`0x46ea98`) scan it.
    pub(super) fn type_of(&self, realm_type: u32) -> (bool, bool) {
        self.types
            .iter()
            .find(|t| t.realm_type == realm_type)
            .map_or((false, false), |t| (t.pvp, t.rp))
    }

    /// The comparator (`0x46e790`) over two holder indices: the sort records in order, the first
    /// that does not tie decides, negated by its own `descending`.
    fn compare(&self, a: usize, b: usize) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        let (Some(ra), Some(rb)) = (self.realm(a), self.realm(b)) else {
            return Ordering::Equal;
        };
        for (key, descending) in self.cells.sort {
            let ord = match key {
                // Case 0: two empty counts tie (`0x46e826`); otherwise more characters first.
                // Deviation: equal nonzero counts tie here, where the reference answers +1 both
                // ways (its name tie-break at `0x46e841` is clobbered by `0x46e846`), because a
                // comparator that is not a total order has no defined sort.
                RealmSortKey::Characters => rb.characters.cmp(&ra.characters),
                // Case 1: the bands, ascending (`0x46e871`-`0x46e88b`).
                RealmSortKey::Load => {
                    let la = realm_load_classify(ra.flags, ra.population, self.mean, self.stddev);
                    let lb = realm_load_classify(rb.flags, rb.population, self.mean, self.stddev);
                    la.partial_cmp(&lb).unwrap_or(Ordering::Equal)
                }
                RealmSortKey::Name => cmp_folded(&ra.name, &rb.name),
                // Case 3: the last `Cfg_Configs` row of each type (the scan at `0x46e8d3` has no
                // break), RP first, then PvP (`0x46e92c`-`0x46e978`).
                RealmSortKey::Mode => {
                    let pair = |t: u32| {
                        self.types
                            .iter()
                            .rev()
                            .find(|r| r.realm_type == t)
                            .map_or((false, false), |r| (r.rp, r.pvp))
                    };
                    pair(ra.realm_type).cmp(&pair(rb.realm_type))
                }
            };
            if ord != Ordering::Equal {
                return if descending { ord.reverse() } else { ord };
            }
        }
        Ordering::Equal
    }

    /// `0x46e750`: every category's realms re-sorted.
    fn sort(&mut self) {
        let mut buckets = std::mem::take(&mut self.buckets);
        for b in &mut buckets {
            b.sort_by(|&x, &y| self.compare(x, y));
        }
        self.buckets = buckets;
    }

    /// The realm list rebuild (`0x46e510`): every category emptied and refilled from the holder by
    /// category byte, the load statistics over every realm, the sort, and the stored category moved
    /// to the one holding the realm `realm_name` names. Returns the event its tail fires
    /// (`0x46e718`-`0x46e73d`): `GET_PREFERRED_REALM_INFO` for an account with no characters that
    /// has chosen no realm this session, else `OPEN_REALM_LIST`.
    fn rebuild(&mut self, realm_name: &str) -> &'static str {
        let cats = self.categories();
        let mut buckets = vec![Vec::new(); cats.len()];
        // `[ebp-0x14]`, -1 until a realm of that name is met; the last one met decides.
        let mut named: Option<Option<u32>> = None;
        for (i, realm) in self.holder.iter().enumerate() {
            // A 0 byte takes the first category's id (`0x46e5b8`).
            let id = match realm.category {
                0 => cats[0].id,
                b => Some(u32::from(b)),
            };
            if cmp_folded(&realm.name, realm_name).is_eq() {
                named = Some(id);
            }
            // No break: a realm joins every category of its id (`0x46e600`-`0x46e65c`).
            for (bucket, cat) in buckets.iter_mut().zip(&cats) {
                if cat.holds(id) {
                    bucket.push(i);
                }
            }
        }
        let pops: Vec<f32> = self.holder.iter().map(|r| r.population).collect();
        (self.mean, self.stddev) = realm_load_stats(&pops);
        self.buckets = buckets;
        self.sort();
        if let Some(id) = named {
            if let Some(at) = cats.iter().position(|c| c.holds(id)) {
                self.cells.category = at as i32;
            }
        }
        if self.characters == 0 && self.cells.chosen.is_none() {
            "GET_PREFERRED_REALM_INFO"
        } else {
            "OPEN_REALM_LIST"
        }
    }

    /// `0x46e9b0`: a sort click. The front record's key toggles its direction; any other moves to
    /// the front keeping its own, the records it passes shifting down; a key on no record only
    /// re-sorts.
    fn set_primary(&mut self, key: RealmSortKey) {
        let sort = &mut self.cells.sort;
        if let Some(at) = sort.iter().position(|(k, _)| *k == key) {
            let descending = if at == 0 { !sort[0].1 } else { sort[at].1 };
            sort.copy_within(0..at, 1);
            sort[0] = (key, descending);
        }
        self.sort();
    }

    /// The realm a `(category, index)` pair names (`0x46eee0`-`0x46ef35`): the 1-based ordinal
    /// through `0x46edf0`, then the 1-based index into that category, compared unsigned.
    fn by_pair(&self, category: i32, index: i32) -> (usize, Option<&GlueRealm>) {
        let at = self.stored_index(category.wrapping_sub(1));
        let index = index.wrapping_sub(1) as u32 as usize;
        let realm = self
            .buckets
            .get(at)
            .and_then(|b| b.get(index))
            .and_then(|&h| self.realm(h));
        (at, realm)
    }

    /// The realm a flat 1-based index names over every category in turn (`0x46ee8c`-`0x46eede`).
    /// The walk has no break, so a later category whose count still exceeds the residual answers
    /// again, and the last answer stands.
    fn by_flat(&self, index: i32) -> Option<&GlueRealm> {
        let mut residual = index.wrapping_sub(1) as u32;
        let mut found = None;
        for bucket in &self.buckets {
            let count = bucket.len() as u32;
            if count > residual {
                // Stored unconditionally, a NULL from `GetRealm` included (`0x46eed3`, `0x46f167`).
                found = bucket.get(residual as usize).and_then(|&h| self.realm(h));
            } else {
                residual = residual.wrapping_sub(count);
            }
        }
        found
    }

    /// `realm_select` (`0x46ea20`), `SetPreferredInfo`'s body: the first valid realm of the asked
    /// category with the asked type, else the category's first valid realm, else the first valid
    /// realm of the type in any category; `None` when there is none. "Valid" skips flag `0x01`
    /// only (`0x46ea7c`), so a realm that is down can be suggested. Returns the pair
    /// `SUGGEST_REALM` carries: the stored category index plus 1, and the index plus 1.
    fn select(&mut self, stored: usize, pvp: bool, rp: bool) -> Option<(usize, usize)> {
        let cats = self.categories();
        // `cmp edi,eax ; jbe`: an index past the count reads 0; the count itself is never reached
        // through `0x46edf0`.
        let stored = if stored < cats.len() { stored } else { 0 };
        self.cells.category = stored as i32;
        self.cells.chosen = Some(cats[stored].id.unwrap_or(0));
        self.cells.preferred = (pvp, rp);
        let valid = |list: &Self, h: usize| list.realm(h).is_some_and(|r| r.flags & 0x01 == 0);
        let fits = |list: &Self, h: usize| {
            list.realm(h)
                .is_some_and(|r| r.flags & 0x01 == 0 && list.type_of(r.realm_type) == (pvp, rp))
        };
        let own = self.buckets.get(stored).cloned().unwrap_or_default();
        if let Some(i) = own.iter().position(|&h| fits(self, h)) {
            return Some((stored + 1, i + 1));
        }
        if let Some(i) = own.iter().position(|&h| valid(self, h)) {
            return Some((stored + 1, i + 1));
        }
        for (c, bucket) in self.buckets.iter().enumerate() {
            if let Some(j) = bucket.iter().position(|&h| fits(self, h)) {
                return Some((c + 1, j + 1));
            }
        }
        None
    }
}

impl super::UiScript {
    /// Seed the client Region's categories and the realm types, read once per process
    /// (`0x46e300`, `0x5413f0`).
    pub fn set_glue_realm_setup(
        &mut self,
        categories: Vec<GlueRealmCategory>,
        types: Vec<GlueRealmType>,
    ) {
        let mut model = self.model_mut();
        model.glue.realms.rows = categories;
        model.glue.realms.types = types;
    }

    /// Fill the holder from a realm list reply, as the parser does (`0x5b2230`): the categories
    /// keep what the last rebuild put in them.
    pub fn set_glue_realms(&mut self, holder: Vec<GlueRealm>) {
        self.model_mut().glue.realms.holder = holder;
    }

    /// The character count of the last `SMSG_CHAR_ENUM` (`[svc+0x1ae4]`).
    pub fn set_glue_realm_characters(&mut self, characters: u16) {
        self.model_mut().glue.realms.characters = characters;
    }

    /// The cells the next glue VM must start from.
    pub fn glue_realm_cells(&self) -> GlueRealmCells {
        self.model_ref().glue.realms.cells
    }

    /// Carry the cells over from the last glue VM.
    pub fn set_glue_realm_cells(&mut self, cells: GlueRealmCells) {
        self.model_mut().glue.realms.cells = cells;
    }

    /// The realm list rebuild (`0x46e510`) and the event its tail fires.
    pub fn rebuild_glue_realms(&mut self) {
        let event = {
            let mut model = self.model_mut();
            let name = realm_name(&model);
            model.glue.realms.rebuild(&name)
        };
        self.fire_event(event, Vec::new());
    }
}

/// The `realmName` CVar's value (`0x5ab7d0`), empty when unregistered.
pub(super) fn realm_name(model: &Model) -> String {
    model
        .cvars
        .get("realmname")
        .map(|s| s.value.clone())
        .unwrap_or_default()
}

fn model(lua: &Lua) -> mlua::AppDataRefMut<'_, Model> {
    lua.app_data_mut::<Model>().expect("model app_data")
}

/// `is-number` (`0x6f34d0`) on an argument, then `tonumber` and `0x40a2b0`.
fn number_at(lua: &Lua, args: &MultiValue, i: usize) -> Option<i32> {
    let v = args.get(i)?.clone();
    lua.coerce_number(v).ok().flatten().map(ftol)
}

/// `GetRealmInfo`'s eight returns for a realm, or its not-found leg (`0x46f090`).
fn realm_info(
    lua: &Lua,
    list: &GlueRealmList,
    realm: Option<&GlueRealm>,
    current: &str,
) -> mlua::Result<MultiValue> {
    let Some(r) = realm else {
        return Ok(MultiValue::from_vec(vec![
            Value::Nil,
            Value::Number(0.0),
            Value::Nil,
            Value::Nil,
            Value::Nil,
            Value::Nil,
            Value::Nil,
            Value::Number(0.0),
        ]));
    };
    let (pvp, rp) = list.type_of(r.realm_type);
    Ok(MultiValue::from_vec(vec![
        Value::String(lua.create_string(&r.name)?),
        Value::Number(f64::from(r.characters)),
        flag(r.flags & 0x01 != 0),
        flag(r.flags & 0x02 != 0),
        flag(cmp_folded(&r.name, current).is_eq()),
        flag(pvp),
        flag(rp),
        Value::Number(f64::from(realm_load_classify(
            r.flags,
            r.population,
            list.mean,
            list.stddev,
        ))),
    ]))
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // An optional flag (`0x6f1c10`, default 0) into `0x46b8d0`, which raises the status dialog
    // only for a nonzero one; the stock five-second refresh passes none.
    g.set(
        "RequestRealmList",
        lua.create_function(|lua, args: MultiValue| {
            let dialog = bool_or_default(args.front(), false);
            call(lua, GlueCall::RequestRealmList { dialog });
            Ok(())
        })?,
    )?;
    g.set(
        "CancelRealmListQuery",
        lua.create_function(|lua, _: MultiValue| {
            call(lua, GlueCall::CancelRealmListQuery);
            Ok(())
        })?,
    )?;
    // `0x46b810`: the current glue screen (`[0xb41478]`, `SetCurrentScreen`'s buffer) against
    // "login" (`0x82e3e0`, `SStrCmpI`); anywhere else a no-op, on login `CLoginMgr::Cancel`
    // (`0x5b3320`).
    g.set(
        "RealmListDialogCancelled",
        lua.create_function(|lua, _: MultiValue| {
            let on_login = cmp_folded(&model(lua).glue.screen, "login").is_eq();
            if on_login {
                call(lua, GlueCall::CancelLogon);
            }
            Ok(())
        })?,
    )?;

    // One numeric argument counts one category, 0 off the end (`0x46ed94`); none, or a
    // non-number, sums every category (`0x46ed47`).
    g.set(
        "GetNumRealms",
        lua.create_function(|lua, args: MultiValue| {
            let at = number_at(lua, &args, 0);
            let m = model(lua);
            let list = &m.glue.realms;
            let n = match at {
                Some(ordinal) => list.count(list.stored_index(ordinal.wrapping_sub(1))),
                None => list.buckets.iter().map(Vec::len).sum(),
            };
            Ok(n as f64)
        })?,
    )?;

    // Eight values on every returning path (`0x46ee40`).
    g.set(
        "GetRealmInfo",
        lua.create_function(|lua, args: MultiValue| {
            const USAGE: &str = "Usage: GetRealmInfo(category, index)";
            let Some(first) = number_at(lua, &args, 0) else {
                return Err(mlua::Error::RuntimeError(USAGE.into()));
            };
            let second = number_at(lua, &args, 1);
            let m = model(lua);
            let list = &m.glue.realms;
            let current = realm_name(&m);
            let realm = match second {
                Some(index) => list.by_pair(first, index).1,
                None => list.by_flat(first),
            };
            realm_info(lua, list, realm, &current)
        })?,
    )?;

    // `0x46f0e0`: the pair leg stores the category (`0x46f188`) before its range check; a realm
    // found marks the session's choice (`0x46f1d1`) and reaches `ConnectToRealm` (`0x46b210`),
    // whose first act sets `realmName` (`0x5ab7b0`).
    g.set(
        "ChangeRealm",
        lua.create_function(|lua, args: MultiValue| {
            const USAGE: &str = "Usage: ChangeRealm(category, index)";
            let Some(first) = number_at(lua, &args, 0) else {
                return Err(mlua::Error::RuntimeError(USAGE.into()));
            };
            let second = number_at(lua, &args, 1);
            let name = {
                let mut m = model(lua);
                let list = &mut m.glue.realms;
                let name = match second {
                    Some(index) => {
                        let (at, realm) = list.by_pair(first, index);
                        let name = realm.map(|r| r.name.clone());
                        list.cells.category = at as i32;
                        name
                    }
                    None => list.by_flat(first).map(|r| r.name.clone()),
                };
                if name.is_some() && list.cells.chosen.is_none() {
                    list.cells.chosen = Some(0);
                }
                if let Some(name) = &name {
                    super::cvars::set_from_engine(&mut m, "realmName", name.clone());
                }
                name
            };
            if let Some(name) = name {
                call(lua, GlueCall::ChangeRealm(name));
            }
            Ok(())
        })?,
    )?;

    // The names of the categories holding a realm (`0x46f1f0`); with none, the first category's
    // name alone (`0x46f255`). A category with no row reads "UNKNOWN" (`0x838044`).
    g.set(
        "GetRealmCategories",
        lua.create_function(|lua, _: MultiValue| {
            let m = model(lua);
            let list = &m.glue.realms;
            let cats = list.categories();
            let name = |c: &Category| c.name.clone().unwrap_or_else(|| "UNKNOWN".into());
            let mut names: Vec<String> = cats
                .iter()
                .enumerate()
                .filter(|(i, _)| list.count(*i) != 0)
                .map(|(_, c)| name(c))
                .collect();
            if names.is_empty() {
                names.push(name(&cats[0]));
            }
            let values = names
                .into_iter()
                .map(|n| lua.create_string(n).map(Value::String))
                .collect::<mlua::Result<Vec<_>>>()?;
            Ok(MultiValue::from_vec(values))
        })?,
    )?;

    // `0x46f2c0`: the category through `0x46edf0`, the two flags through `0x6f1c10` with default
    // 0, then `realm_select` (`0x46ea20`), which fires `SUGGEST_REALM` with `"%d%d"` or, with no
    // realm to suggest, `OPEN_REALM_LIST` (`0x46ec21`).
    g.set(
        "SetPreferredInfo",
        lua.create_function(|lua, args: MultiValue| {
            const USAGE: &str = "Usage: SetPreferredInfo(index, pvp, rp)";
            let Some(first) = number_at(lua, &args, 0) else {
                return Err(mlua::Error::RuntimeError(USAGE.into()));
            };
            let pvp = bool_or_default(args.get(1), false);
            let rp = bool_or_default(args.get(2), false);
            let suggested = {
                let mut m = model(lua);
                let list = &mut m.glue.realms;
                let at = list.stored_index(first.wrapping_sub(1));
                list.select(at, pvp, rp)
            };
            match suggested {
                Some((category, index)) => super::tick::fire_event_into(
                    lua,
                    "SUGGEST_REALM",
                    vec![
                        ScriptValue::Number(category as f64),
                        ScriptValue::Number(index as f64),
                    ],
                ),
                None => super::tick::fire_event_into(lua, "OPEN_REALM_LIST", Vec::new()),
            }
            Ok(())
        })?,
    )?;

    // `0x46f330`: a string (or number) argument, matched case-insensitively; anything else sorts by
    // name. The usage string is the binary's own spelling (`0x838098`).
    g.set(
        "SortRealms",
        lua.create_function(|lua, args: MultiValue| {
            const USAGE: &str = "Usgae: SortRealms(\"type\")";
            let arg = args.front().cloned().unwrap_or(Value::Nil);
            if !matches!(arg, Value::String(_) | Value::Integer(_) | Value::Number(_)) {
                return Err(mlua::Error::RuntimeError(USAGE.into()));
            }
            let s = string_arg(lua, arg, USAGE)?;
            let key = [
                ("name", RealmSortKey::Name),
                ("load", RealmSortKey::Load),
                ("characters", RealmSortKey::Characters),
                ("mode", RealmSortKey::Mode),
            ]
            .into_iter()
            .find(|(k, _)| cmp_folded(&s, k).is_eq())
            .map_or(RealmSortKey::Name, |(_, key)| key);
            model(lua).glue.realms.set_primary(key);
            // Both exits of `0x46e9b0` tail-jump the event (`0x46e9cb`, `0x46ea1a`).
            super::tick::fire_event_into(lua, "OPEN_REALM_LIST", Vec::new());
            Ok(())
        })?,
    )?;

    // `0x46f400`: the ordinal of the first category holding a realm at or past the stored index
    // (a signed compare, so -1 reads the first), else 1.
    g.set(
        "GetSelectedCategory",
        lua.create_function(|lua, _: MultiValue| {
            let m = model(lua);
            let list = &m.glue.realms;
            let from = list.cells.category;
            let n = list.categories().len();
            let ordinal = (0..n)
                .filter(|&i| list.count(i) != 0)
                .enumerate()
                .find(|(_, i)| *i as i64 >= i64::from(from))
                .map_or(1, |(o, _)| o + 1);
            Ok(ordinal as f64)
        })?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    fn realm(
        name: &str,
        category: u8,
        realm_type: u32,
        flags: u8,
        chars: u8,
        pop: f32,
    ) -> GlueRealm {
        GlueRealm {
            name: name.into(),
            realm_type,
            flags,
            population: pop,
            characters: chars,
            category,
        }
    }

    /// Region 1's rows and the shipped `Cfg_Configs.dbc` types 0, 1, 6, 8.
    fn vm(holder: Vec<GlueRealm>) -> UiScript {
        let mut s = UiScript::new_glue().unwrap();
        s.set_glue_realm_setup(
            vec![
                GlueRealmCategory {
                    id: 1,
                    name: "United States".into(),
                },
                GlueRealmCategory {
                    id: 5,
                    name: "Oceanic".into(),
                },
            ],
            vec![
                GlueRealmType {
                    realm_type: 0,
                    pvp: false,
                    rp: false,
                },
                GlueRealmType {
                    realm_type: 1,
                    pvp: true,
                    rp: false,
                },
                GlueRealmType {
                    realm_type: 6,
                    pvp: false,
                    rp: true,
                },
                GlueRealmType {
                    realm_type: 8,
                    pvp: true,
                    rp: true,
                },
            ],
        );
        s.register_cvars([("realmName", "")]);
        s.set_glue_realms(holder);
        s
    }

    /// Record every firing of `events` as `"EVENT a b"` in `FIRED`.
    fn listen(s: &UiScript, events: &[&str]) {
        let mut chunk = String::from(
            "FIRED = {} local f = CreateFrame('Frame', 'Listener') \
             f:SetScript('OnEvent', function() \
               table.insert(FIRED, event .. ' ' .. tostring(arg1) .. ' ' .. tostring(arg2)) end) ",
        );
        for e in events {
            chunk.push_str(&format!("f:RegisterEvent('{e}') "));
        }
        s.run(&chunk).unwrap();
    }

    fn fired(s: &UiScript) -> Vec<String> {
        let v = s.eval::<Vec<String>>("return FIRED").unwrap();
        s.run("FIRED = {}").unwrap();
        v
    }

    /// `GetRealmInfo`'s eight returns.
    type Info = (
        Option<String>,
        f64,
        Option<i64>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
        f64,
    );

    fn info(s: &UiScript, call: &str) -> Info {
        s.eval(&format!("return {call}")).unwrap()
    }

    /// The rebuild fires `GET_PREFERRED_REALM_INFO` for a fresh account with no choice made, and
    /// `OPEN_REALM_LIST` once it has characters or has chosen (`0x46e718`-`0x46e73d`).
    #[test]
    fn the_rebuild_asks_for_a_preference_only_with_no_characters_and_no_choice() {
        let mut s = vm(vec![realm("Alpha", 1, 0, 0, 0, 1.0)]);
        listen(&s, &["OPEN_REALM_LIST", "GET_PREFERRED_REALM_INFO"]);
        s.rebuild_glue_realms();
        assert_eq!(fired(&s), ["GET_PREFERRED_REALM_INFO nil nil"]);
        s.set_glue_realm_characters(2);
        s.rebuild_glue_realms();
        assert_eq!(fired(&s), ["OPEN_REALM_LIST nil nil"]);
        s.set_glue_realm_characters(0);
        s.run("ChangeRealm(1, 1)").unwrap();
        let _ = s.take_glue_calls();
        s.rebuild_glue_realms();
        assert_eq!(fired(&s), ["OPEN_REALM_LIST nil nil"], "a choice was made");
    }

    /// The categories come from the rebuild: none before it, and a byte no Region row carries is
    /// on no tab.
    #[test]
    fn the_categories_are_the_ones_holding_a_realm_after_the_rebuild() {
        let mut s = vm(vec![
            realm("Alpha", 1, 0, 0, 0, 1.0),
            realm("Down Under", 5, 0, 0, 0, 1.0),
            realm("Nowhere", 9, 0, 0, 0, 1.0),
        ]);
        assert_eq!(
            s.eval::<Vec<String>>("return {GetRealmCategories()}")
                .unwrap(),
            ["United States"],
            "before the rebuild no category holds a realm: the first one's name alone"
        );
        assert_eq!(s.eval::<f64>("return GetNumRealms()").unwrap(), 0.0);
        s.rebuild_glue_realms();
        assert_eq!(
            s.eval::<Vec<String>>("return {GetRealmCategories()}")
                .unwrap(),
            ["United States", "Oceanic"]
        );
        assert_eq!(s.eval::<f64>("return GetNumRealms()").unwrap(), 2.0);
        assert_eq!(s.eval::<f64>("return GetNumRealms(2)").unwrap(), 1.0);
        assert_eq!(
            s.eval::<f64>("return GetNumRealms(9)").unwrap(),
            1.0,
            "an ordinal past the last reads stored category 0"
        );
        assert_eq!(
            info(&s, "GetRealmInfo(2, 1)").0.as_deref(),
            Some("Down Under")
        );
    }

    /// Eight values on every path; the not-found leg is `nil, 0, nil ×5, 0`.
    #[test]
    fn get_realm_info_returns_eight_values_in_the_references_order() {
        let mut s = vm(vec![
            realm("Alpha", 1, 1, 0x01, 3, 1.0),
            realm("Beta", 1, 8, 0x02 | 0x20, 0, 1.0),
        ]);
        s.set_cvar_host("realmName", "ALPHA");
        s.rebuild_glue_realms();
        assert_eq!(
            s.eval::<usize>("return (function(...) return arg.n end)(GetRealmInfo(1, 1))")
                .unwrap(),
            8
        );
        assert_eq!(
            info(&s, "GetRealmInfo(1, 1)"),
            (
                Some("Alpha".into()),
                3.0,
                Some(1),
                None,
                Some(1),
                Some(1),
                None,
                0.0
            )
        );
        assert_eq!(
            info(&s, "GetRealmInfo(1, 2)"),
            (
                Some("Beta".into()),
                0.0,
                None,
                Some(1),
                None,
                Some(1),
                Some(1),
                -3.0
            )
        );
        assert_eq!(
            info(&s, "GetRealmInfo(1, 3)"),
            (None, 0.0, None, None, None, None, None, 0.0)
        );
        assert_eq!(
            s.eval::<usize>("return (function(...) return arg.n end)(GetRealmInfo(1, 3))")
                .unwrap(),
            8
        );
        let err = s.run("GetRealmInfo()").unwrap_err().to_string();
        assert!(
            err.contains("Usage: GetRealmInfo(category, index)"),
            "{err}"
        );
        assert_eq!(
            info(&s, "GetRealmInfo(2)").0.as_deref(),
            Some("Beta"),
            "the flat leg"
        );
    }

    /// The default order is characters, load, name, mode; a click moves its column to the front
    /// and fires `OPEN_REALM_LIST`; mode sorts RP ahead of PvP (`0x46e92c`).
    #[test]
    fn sort_realms_rotates_the_keys_and_fires_the_list() {
        let mut s = vm(vec![
            realm("Zeta", 1, 6, 0, 0, 1.0),
            realm("Alpha", 1, 1, 0, 0, 1.0),
            realm("Mu", 1, 0, 0, 3, 1.0),
            realm("Kappa", 1, 8, 0, 0, 1.0),
        ]);
        s.rebuild_glue_realms();
        let names = |s: &UiScript| {
            s.eval::<Vec<String>>(
                "local t = {} for i = 1, GetNumRealms(1) do t[i] = GetRealmInfo(1, i) end return t",
            )
            .unwrap()
        };
        assert_eq!(names(&s), ["Mu", "Alpha", "Kappa", "Zeta"]);
        listen(&s, &["OPEN_REALM_LIST"]);
        s.run("SortRealms('NAME')").unwrap();
        assert_eq!(fired(&s), ["OPEN_REALM_LIST nil nil"]);
        assert_eq!(names(&s), ["Alpha", "Kappa", "Mu", "Zeta"]);
        s.run("SortRealms('name')").unwrap();
        assert_eq!(
            names(&s),
            ["Zeta", "Mu", "Kappa", "Alpha"],
            "a re-click flips it"
        );
        s.run("SortRealms('mode')").unwrap();
        assert_eq!(
            names(&s),
            ["Mu", "Alpha", "Zeta", "Kappa"],
            "Normal, PvP, RP, RP-PvP: RP decides before PvP"
        );
        assert_eq!(
            s.glue_realm_cells().sort,
            [
                (RealmSortKey::Mode, false),
                (RealmSortKey::Name, true),
                (RealmSortKey::Characters, false),
                (RealmSortKey::Load, false),
            ]
        );
        s.run("SortRealms('nonsense')").unwrap();
        assert_eq!(s.glue_realm_cells().sort[0], (RealmSortKey::Name, true));
        let err = s.run("SortRealms()").unwrap_err().to_string();
        assert!(err.contains("Usgae: SortRealms(\"type\")"), "{err}");
    }

    /// `SetPreferredInfo` suggests the first valid realm of the asked type in the asked category,
    /// else the category's first valid realm, else the type in any category, else opens the list;
    /// it marks the choice, so the next rebuild opens the list.
    #[test]
    fn set_preferred_info_suggests_a_realm_or_opens_the_list() {
        let mut s = vm(vec![
            realm("Invalid", 1, 1, 0x01, 0, 1.0),
            realm("Normal", 1, 0, 0, 0, 1.0),
            realm("Pvp", 1, 1, 0, 0, 1.0),
            realm("Rp", 1, 6, 0x02, 0, 1.0),
            realm("Oz", 5, 6, 0x01, 0, 1.0),
        ]);
        s.rebuild_glue_realms();
        listen(
            &s,
            &[
                "SUGGEST_REALM",
                "OPEN_REALM_LIST",
                "GET_PREFERRED_REALM_INFO",
            ],
        );
        s.run("SetPreferredInfo(1, 1, nil)").unwrap();
        assert_eq!(fired(&s), ["SUGGEST_REALM 1 3"], "the type in the category");
        assert_eq!(info(&s, "GetRealmInfo(1, 3)").0.as_deref(), Some("Pvp"));
        s.run("SetPreferredInfo(1, 1, 1)").unwrap();
        assert_eq!(
            fired(&s),
            ["SUGGEST_REALM 1 2"],
            "no RP-PvP realm: the category's first valid one"
        );
        s.run("SetPreferredInfo(2, nil, 1)").unwrap();
        assert_eq!(
            fired(&s),
            ["SUGGEST_REALM 1 4"],
            "Oceanic holds nothing valid: the type in any category, a realm that is down included"
        );
        let cells = s.glue_realm_cells();
        assert_eq!(
            (cells.category, cells.chosen, cells.preferred),
            (1, Some(5), (false, true))
        );
        s.set_glue_realms(vec![realm("Invalid", 1, 1, 0x01, 0, 1.0)]);
        s.rebuild_glue_realms();
        assert_eq!(fired(&s), ["OPEN_REALM_LIST nil nil"], "a choice was made");
        s.run("SetPreferredInfo(1, 1, nil)").unwrap();
        assert_eq!(
            fired(&s),
            ["OPEN_REALM_LIST nil nil"],
            "nothing valid to suggest"
        );
        let err = s.run("SetPreferredInfo()").unwrap_err().to_string();
        assert!(
            err.contains("Usage: SetPreferredInfo(index, pvp, rp)"),
            "{err}"
        );
    }

    /// `ChangeRealm` stores the category before its range check, sets `realmName`, and asks the
    /// host to connect by name.
    #[test]
    fn change_realm_names_the_realm_and_stores_the_category() {
        let mut s = vm(vec![
            realm("Alpha", 1, 0, 0, 0, 1.0),
            realm("Oz", 5, 0, 0, 0, 1.0),
        ]);
        s.rebuild_glue_realms();
        s.run("ChangeRealm(2, 7)").unwrap();
        assert!(s.take_glue_calls().is_empty(), "no realm, no connect");
        assert_eq!(
            s.glue_realm_cells().category,
            1,
            "stored before the range check"
        );
        assert_eq!(s.glue_realm_cells().chosen, None);
        assert_eq!(s.eval::<f64>("return GetSelectedCategory()").unwrap(), 2.0);
        s.run("ChangeRealm(2, 1)").unwrap();
        assert!(matches!(
            s.take_glue_calls().as_slice(),
            [GlueCall::ChangeRealm(n)] if n == "Oz"
        ));
        assert_eq!(s.cvar("realmName").as_deref(), Some("Oz"));
        assert_eq!(s.glue_realm_cells().chosen, Some(0));
        assert_eq!(
            info(&s, "GetRealmInfo(2, 1)").4,
            Some(1),
            "now the current realm"
        );
        let err = s.run("ChangeRealm()").unwrap_err().to_string();
        assert!(err.contains("Usage: ChangeRealm(category, index)"), "{err}");
    }

    /// `GetSelectedCategory` is the stored category's ordinal, 1 at load; the rebuild moves it to
    /// the category of the realm `realmName` names.
    #[test]
    fn the_selected_category_follows_the_named_realm() {
        let mut s = vm(vec![
            realm("Alpha", 1, 0, 0, 0, 1.0),
            realm("Oz", 5, 0, 0, 0, 1.0),
        ]);
        assert_eq!(s.eval::<f64>("return GetSelectedCategory()").unwrap(), 1.0);
        s.set_cvar_host("realmName", "oz");
        s.rebuild_glue_realms();
        assert_eq!(s.eval::<f64>("return GetSelectedCategory()").unwrap(), 2.0);
    }

    /// The network verbs call the manager (here, with none seated, they wait for one);
    /// `RealmListDialogCancelled` only on the login screen.
    #[test]
    fn the_request_verbs_call_the_manager_and_the_cancel_is_screen_conditional() {
        let mut s = vm(Vec::new());
        s.run("RequestRealmList() RequestRealmList(1) CancelRealmListQuery()")
            .unwrap();
        assert!(matches!(
            s.take_glue_calls().as_slice(),
            [
                GlueCall::RequestRealmList { dialog: false },
                GlueCall::RequestRealmList { dialog: true },
                GlueCall::CancelRealmListQuery
            ]
        ));
        s.run("SetCurrentScreen('charselect') RealmListDialogCancelled()")
            .unwrap();
        assert!(
            s.take_glue_calls().is_empty(),
            "a no-op off the login screen"
        );
        s.run("SetCurrentScreen('LOGIN') RealmListDialogCancelled()")
            .unwrap();
        assert!(matches!(
            s.take_glue_calls().as_slice(),
            [GlueCall::CancelLogon]
        ));
    }

    /// The flat leg keeps walking after a hit and stores each later hit unconditionally, a NULL
    /// from `GetRealm` included (`0x46eed3`): a holder that lost the realm since the rebuild
    /// answers nil, not the earlier hit.
    #[test]
    fn the_flat_leg_keeps_the_last_hit_even_when_it_is_gone() {
        let mut s = vm(vec![
            realm("Alpha", 1, 0, 0, 0, 1.0),
            realm("Oz", 5, 0, 0, 0, 1.0),
            realm("Perth", 5, 0, 0, 0, 1.0),
        ]);
        s.rebuild_glue_realms();
        assert_eq!(
            info(&s, "GetRealmInfo(1)").0.as_deref(),
            Some("Oz"),
            "both categories hold index 0, and the later one answers"
        );
        // A reply fills the holder; the categories keep the last rebuild's indices.
        s.set_glue_realms(vec![realm("Alpha", 1, 0, 0, 0, 1.0)]);
        assert_eq!(info(&s, "GetRealmInfo(1)").0, None);
    }

    /// With no Region rows every realm lists under one category named "UNKNOWN".
    #[test]
    fn no_region_rows_list_every_realm_under_one_category() {
        let mut s = UiScript::new_glue().unwrap();
        s.set_glue_realms(vec![
            realm("Alpha", 3, 0, 0, 0, 1.0),
            realm("Beta", 0, 0, 0, 0, 1.0),
        ]);
        s.rebuild_glue_realms();
        assert_eq!(
            s.eval::<Vec<String>>("return {GetRealmCategories()}")
                .unwrap(),
            ["UNKNOWN"]
        );
        assert_eq!(s.eval::<f64>("return GetNumRealms(1)").unwrap(), 2.0);
    }
}
