//! What a unit's aura list answers, which `UnitBuff`, `UnitDebuff` and the tooltip's
//! `SetUnitBuff`/`SetUnitDebuff` walk off the unit as the call runs. The reference's walk reads
//! the slots of the object its token resolves (`UnitBuff` `0x519500`: the lookup at `0x51955e`,
//! the slots from `[unit+0x110]+0xa4` at `0x519617`; `UnitDebuff` `0x5198f0` from `+0x124`;
//! `SetUnitBuff` `0x534ac0`), else the roster record of a member with none (`0x519735`), and gates
//! the buffs on the active player as it stands (`0x5195be`-`0x5195f7`), so a getter the lent host
//! can answer reads the slots now, not the frame's snapshot ([`read_auras_of`]).

use mlua::Lua;

use super::{AuraState, UnitAuraList};
use crate::script::Model;

/// One aura of a unit's list, each fact the walks test read on its own, and the aura whole for
/// the hit: off the object or record through the lent host, or off a snapshot's [`AuraState`],
/// which holds them as its build read them. The app's one derivation of each from a slot is its
/// implementation of this, which its snapshots' builds read too.
pub trait AuraRead {
    /// A buff rather than a debuff: one of the first 32 slots, the half `UnitBuff` walks
    /// (`0x519617`-`0x51969d`) and `UnitDebuff` the 16 after (`0x519979`).
    fn helpful(&self) -> bool;
    /// [`AuraState::vision_hidden`].
    fn vision_hidden(&self) -> bool;
    /// [`AuraState::raid_listed`].
    fn raid_listed(&self) -> bool;
    /// The aura as the getters return it.
    fn state(&self) -> AuraState;
    /// Its spell, its caster's level and its stack count, the tooltip's key
    /// ([`crate::script::AuraKey`]), without building the whole aura.
    fn key(&self) -> (u32, u8, u8);
}

impl AuraRead for AuraState {
    fn helpful(&self) -> bool {
        self.helpful
    }
    fn vision_hidden(&self) -> bool {
        self.vision_hidden
    }
    fn raid_listed(&self) -> bool {
        self.raid_listed
    }
    fn state(&self) -> AuraState {
        self.clone()
    }
    fn key(&self) -> (u32, u8, u8) {
        (self.spell_id, self.level, self.count)
    }
}

/// One unit's aura list as the walks read it: its auras in ascending slot, and `UnitBuff`'s unit
/// gate ([`UnitAuraList::buffs_visible`]), each read on its own.
pub trait AuraListRead {
    /// [`UnitAuraList::buffs_visible`].
    fn buffs_visible(&self) -> bool;
    /// Hand `visit` each aura in ascending slot, until it answers `true`.
    fn find(&self, visit: &mut dyn FnMut(&dyn AuraRead) -> bool);
}

impl AuraListRead for UnitAuraList {
    fn buffs_visible(&self) -> bool {
        self.buffs_visible
    }
    fn find(&self, visit: &mut dyn FnMut(&dyn AuraRead) -> bool) {
        for a in &self.auras {
            if visit(a) {
                return;
            }
        }
    }
}

/// The `n`-th (0-based) aura of `list` that `keep` passes, whole.
pub(crate) fn nth_kept(
    list: &dyn AuraListRead,
    n: usize,
    keep: impl Fn(&dyn AuraRead) -> bool,
) -> Option<AuraState> {
    let mut left = n;
    let mut hit = None;
    list.find(&mut |a| {
        if !keep(a) {
            return false;
        }
        if left == 0 {
            hit = Some(a.state());
            return true;
        }
        left -= 1;
        false
    });
    hit
}

/// The guid `token` names through the resolver (`0x515970`, which `UnitBuff` calls at
/// `0x519542`), `None` for nobody; a token it does not recognise raises `Unknown unit name`.
pub(crate) fn aura_unit(lua: &Lua, token: &str) -> mlua::Result<Option<u64>> {
    let model = lua.app_data_ref::<Model>().expect("model app_data");
    crate::script::unit::guid_of(lua, &model, token)
}

/// `f` over the aura list of the unit held under `guid`, `None` for nobody and for a unit with
/// none: off its object or roster record through the lent host, as of the place the getter runs;
/// with no host free to ask (a glue VM, or a call's apply under way), off the snapshot.
pub(crate) fn read_auras_of<T>(
    lua: &Lua,
    guid: Option<u64>,
    f: impl FnOnce(Option<&dyn AuraListRead>) -> T,
) -> T {
    let Some(guid) = guid else {
        return f(None);
    };
    let mut f = Some(f);
    let mut out = None;
    crate::script::host::read_host(lua, |h| {
        h.read_unit_auras(guid, &mut |list| {
            out = f.take().map(|f| f(list));
        })
    });
    match out {
        Some(out) => out,
        None => {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            f.take().expect("an unread `f`")(
                model.unit_auras.get(&guid).map(|l| l as &dyn AuraListRead),
            )
        }
    }
}
