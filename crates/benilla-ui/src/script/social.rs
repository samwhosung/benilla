//! The social API: friends, ignores, `/who` and the looking-for-group pair. The getters read the
//! friend list and the `/who` array through the lent host at the call, display-ready (the
//! reference's formatter `0x5ae160` also resolves names engine-side), as the reference's read
//! them at the call ([`super::social_read`]); with no host free to ask they read the snapshot
//! [`SocialState`]. The verbs act inside their calls through the host ([`SocialCall`]), as the
//! reference's send their packets before they return. Selection is engine state in the reference
//! (guids at FriendList `+0x648` and `+0x720`, read back by `0x5ae510` and `0x5ae630`), which the
//! getters read back through the host.

use mlua::{Lua, Value};

use super::binding_abi::{self, bool_or_default, string_arg};
use super::social_read::{read_live, SocialAnswer, SocialRead};
use super::Model;

/// One friend slot as `GetFriendInfo` (`0x5ad060`) reads it, its ids resolved; the default is an
/// empty slot, as the list's constructor leaves one (`0x5aca10`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FriendInfo {
    /// The slot's own copy of the name, `None` while it is null: the list fills it only from the
    /// name cache (`0x5ae438`), and `lua_pushstring` pushes a null as nil (`0x5ad0c5`).
    pub name: Option<String>,
    /// `+0x10`, pushed signed (`fild` at `0x5ad0ca`).
    pub level: i32,
    /// The `ChrClasses` name of `+0x14`, `None` for an id with no row (`UNKNOWN`, `0x5ad102`).
    pub class: Option<String>,
    /// The `AreaTable` name of `+0x18` or of its parent, `None` for no row (`UNKNOWN`, `0x5ad151`).
    pub area: Option<String>,
    /// The online byte `+0`.
    pub online: bool,
    /// The status flags `+1`: 4 busy, 2 away (`0x5ad17b`).
    pub flags: u8,
    /// Whether the slot holds a guid: `GetNumFriends` counts the slots before the first that does
    /// not (`0x5ae490`).
    pub listed: bool,
}

/// One ignore slot as `GetIgnoreName` (`0x5ad460`) reads it; the default is an empty slot.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IgnoreInfo {
    /// The guid's name as the cache holds it (`0x5ad4e5`, no callback), `None` for none.
    pub name: Option<String>,
    /// Whether the slot holds a guid: `GetNumIgnores` counts the slots before the first that does
    /// not (`0x5ae550`).
    pub listed: bool,
}

/// One `/who` row in `GetWhoInfo`'s order, names localized; race comes before class, where the
/// wire has class first. A race, class or zone is `None` for an id with no row, which
/// `GetWhoInfo` answers `UNKNOWN` (`0x5ad782`, `0x5ad7c5`, `0x5ad816`) and the sort ties
/// (`0x5adbb2`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WhoInfo {
    pub name: String,
    pub guild: String,
    pub level: u32,
    pub race: Option<String>,
    pub class: Option<String>,
    pub zone: Option<String>,
}

/// The social snapshot the getters read with no host free to ask; the default is a fresh
/// login's.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SocialState {
    /// The 50 friend slots in the engine's order, then the 32 bytes after them, which
    /// `GetFriendInfo(51)` reads as a slot (`0x5ae4b6` bounds at 50 inclusive). A slot past the end
    /// of a shorter list reads empty.
    pub friends: Vec<FriendInfo>,
    /// The selected friend, 1-based, 0 for none (`0x5ad260` returns the stored slot + 1).
    pub selected_friend: u32,
    /// The 25 ignore slots in the engine's order, then the eight bytes after them, which
    /// `GetIgnoreName(26)` reads as a guid (`0x5ae576` bounds at 25 inclusive). A slot past the
    /// end of a shorter list reads empty.
    pub ignores: Vec<IgnoreInfo>,
    /// The selected ignore, on the same scale.
    pub selected_ignore: u32,
    /// The kept `/who` rows (`0xc2a120`, at most 50), in the array's order.
    pub who: Vec<WhoInfo>,
    /// The array's slot after the kept rows, which `GetWhoInfo(count + 1)` reads (`0x5ad71f`): what
    /// an earlier, longer answer left there, or an empty record. `None` after a full 50.
    pub who_next: Option<WhoInfo>,
    /// The total match count, `GetNumWhoResults`'s second return; it can exceed `who.len()`.
    pub who_total: u32,
}

/// A friend, ignore, `/who` or looking-for-group verb past its argument check, which the host
/// applies inside the call ([`super::ScriptCall::Social`]). An index is the binding's `_ftol` of
/// its argument, still 1-based: the engine's own functions take it less one, compared unsigned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SocialCall {
    /// `ShowFriends()` (`0x5ad340`): `CMSG_FRIEND_LIST`.
    ShowFriends,
    /// `AddFriend(name)` (`0x5ad290` → `0x5ae670`).
    AddFriend(String),
    /// `RemoveFriend(index)` (`0x5ad2fa` → `0x5ae7e0`).
    RemoveFriendIndex(u32),
    /// `RemoveFriend(name)` (`0x5ad31f` → `0x5ae6f0`).
    RemoveFriendName(String),
    /// `SetSelectedFriend(index)` (`0x5ad24c` → `0x5ae4d0`).
    SelectFriend(u32),
    /// `AddIgnore(name)` (`0x5ad62f` → `0x5af300`).
    AddIgnore(String),
    /// `DelIgnore(name)` (`0x5ad67f` → `0x5af400`).
    DelIgnore(String),
    /// `AddOrDelIgnore(name)` (`0x5ad5df` → `0x5af1a0`), `/ignore`'s toggle.
    AddOrDelIgnore(String),
    /// `SetSelectedIgnore(index)` (`0x5ad55c` → `0x5ae5f0`).
    SelectIgnore(u32),
    /// `SendWho(filter)` (`0x5ad3b0` → `0x5aebb0`): the filter's bytes, which the host builds into
    /// `CMSG_WHO` against the tables.
    Who(Vec<u8>),
    /// `SortWho(sortType)` (`0x5ad890`): the engine's chain takes the click and sorts the array.
    SortWho(String),
    /// `LFGQuery(type, entry, class)` (`0x4e9760`) inside its counts: `MSG_LOOKING_FOR_GROUP` with
    /// the criterion word and the class (`0x4e9840`).
    LfgQuery { criterion: u32, class: u32 },
    /// A changed `SetLookingForGroup` (`0x4e96b0` → `0x4e88c0`): `CMSG_SET_LOOKING_FOR_GROUP` with
    /// the slots and the comment.
    SetLookingForGroup { slots: [u32; 3], comment: String },
}

impl super::UiScript {
    /// Replace the social snapshot, what the getters read with no host lent to ask.
    pub fn set_social(&mut self, state: SocialState) {
        self.model_mut().social = state;
    }

    /// `SetWhoToUI`'s flag (`0xc2a12c`): whether the next `/who` answer goes to the Who frame.
    pub fn who_to_ui(&self) -> bool {
        self.model_ref().who_to_ui
    }

    /// The friend and ignore calls a harness VM kept ([`Self::keep_calls`]), in call order.
    pub fn take_social_calls(&mut self) -> Vec<SocialCall> {
        self.take_calls_where(|call| match call {
            super::ScriptCall::Social(call) => Some(call.clone()),
            _ => None,
        })
    }
}

/// Hand `call` to the host, which applies it before the verb returns.
fn submit(lua: &Lua, call: SocialCall) -> mlua::Result<()> {
    super::calls::submit(lua, super::ScriptCall::Social(call))
}

/// Register the social globals against the snapshot store.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // ── The friend list ──
    // GetNumFriends(): `0x5ad000` over CountFriends `0x5ae490`.
    g.set(
        "GetNumFriends",
        lua.create_function(|lua, ()| {
            if let Some(SocialAnswer::Count(n)) = read_live(lua, SocialRead::FriendCount) {
                return Ok(n as i64);
            }
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let slots = model.social.friends.iter().take(FRIEND_SLOTS);
            Ok(slots.take_while(|f| f.listed).count() as i64)
        })?,
    )?;

    // GetFriendInfo(index) (`0x5ad060`): the slot `0x5ae4b0` finds for `index − 1`, or the
    // `UNKNOWN` tuple when it finds none. The strings are global strings read at the call
    // (`0x703bf0`), the empty one when missing.
    g.set(
        "GetFriendInfo",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: GetFriendInfo(index)` (`0x85da28`, `0x5ad073`).
            let index = binding_abi::index_arg32(lua, index, "Usage: GetFriendInfo(index)")?;
            let text = |key: &str| super::glue_core::global_text(lua, key);
            let unknown = text("UNKNOWN");
            let string = |s: &str| lua.create_string(s).map(Value::String);
            let live = match read_live(lua, SocialRead::Friend(index)) {
                Some(SocialAnswer::Friend(friend)) => Some(friend.map(std::borrow::Cow::Owned)),
                _ => None,
            };
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let friend = live.unwrap_or_else(|| friend_at(&model.social.friends, index));
            let Some(friend) = friend else {
                // `0x5ad1c8`-`0x5ad1fd`.
                return Ok(mlua::MultiValue::from_vec(vec![
                    string(&unknown)?,
                    Value::Integer(1),
                    string(&unknown)?,
                    string(&unknown)?,
                    Value::Nil,
                    string("")?,
                ]));
            };
            // The away tag only for an online slot, busy before away (`0x5ad176`-`0x5ad1a5`).
            let tag = match (friend.online, friend.flags) {
                (true, f) if f & 4 != 0 => text("CHAT_FLAG_DND"),
                (true, f) if f & 2 != 0 => text("CHAT_FLAG_AFK"),
                _ => String::new(),
            };
            Ok(mlua::MultiValue::from_vec(vec![
                match &friend.name {
                    Some(name) => string(name)?,
                    None => Value::Nil,
                },
                Value::Integer(i64::from(friend.level)),
                string(friend.class.as_deref().unwrap_or(&unknown))?,
                string(friend.area.as_deref().unwrap_or(&unknown))?,
                binding_abi::flag(friend.online),
                string(&tag)?,
            ]))
        })?,
    )?;

    g.set(
        "GetSelectedFriend",
        lua.create_function(|lua, ()| {
            if let Some(SocialAnswer::Index(i)) = read_live(lua, SocialRead::SelectedFriend) {
                return Ok(i64::from(i));
            }
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(i64::from(model.social.selected_friend))
        })?,
    )?;

    // SetSelectedFriend(index): the host stores the slot's guid before the verb returns, as
    // `FriendsList_Update` reads it back on its next line.
    g.set(
        "SetSelectedFriend",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: SetSelectedFriend(index)` (`0x85da44`, `0x5ad221`).
            let index = binding_abi::index_arg32(lua, index, "Usage: SetSelectedFriend(index)")?;
            submit(lua, SocialCall::SelectFriend(index))
        })?,
    )?;

    // The LFG pair (`0x4e95d0`, `0x4e96b0`) is for addons: stock FrameXML's only calls sit inside
    // an XML comment. GetLookingForGroup() → the three slot names, each nil as the pack below
    // always stores 0, then the comment string.
    g.set(
        "GetLookingForGroup",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(mlua::MultiValue::from_vec(vec![
                Value::Nil,
                Value::Nil,
                Value::Nil,
                Value::String(lua.create_string(&model.lfg_comment)?),
            ]))
        })?,
    )?;
    // SetLookingForGroup(type1, entry1, type2, entry2, type3, entry3, comment): the loop over the
    // pairs ends at a non-number type or one >= 6. The stored word is `(type << 24) & entry`
    // (`0x4e9713`), an AND every reader decodes as an OR, so an admissible pair stores 0; the
    // per-type eligible-count check that also ends the reference's loop is not modelled. The
    // comment is argument 7, read only when argument 4 passes `lua_isstring`, and keeps 127 bytes
    // (`SStrCopy(…, 0x80)`). `CMSG_SET_LOOKING_FOR_GROUP` goes out only on a change.
    g.set(
        "SetLookingForGroup",
        lua.create_function(|lua, args: mlua::MultiValue| {
            let args: Vec<Value> = args.into_iter().collect();
            let arg = |i: usize| args.get(i - 1).cloned().unwrap_or(Value::Nil);
            use crate::script::binding_abi::{coerced_number, ftol, is_number};
            let mut slots = [0u32; 3];
            for (slot, i) in slots.iter_mut().zip([1usize, 3, 5]) {
                // `is-number` then `_ftol` for the type (`0x4e96d5`, `0x4e96e7`), compared
                // unsigned below 6; a bare `tonumber` and `_ftol` for the entry (`0x4e96fd`).
                let Some(ty) = is_number(lua, arg(i)) else {
                    break;
                };
                let ty = ftol(ty) as u32;
                if ty >= 6 {
                    break;
                }
                let entry = ftol(coerced_number(lua, Some(arg(i + 1)))) as u32;
                // `&`, not `|`: the reference's own pack.
                *slot = (ty << 24) & entry;
            }
            let comment = match arg(4) {
                // `lua_tostring(7)` (`0x4e9741`): a number is its `%.14g` text.
                Value::Integer(_) | Value::Number(_) | Value::String(_) => {
                    crate::script::binding_abi::optional_string(lua, &arg(7))
                }
                _ => None,
            };
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let mut changed = false;
            if model.lfg_slots != slots {
                model.lfg_slots = slots;
                changed = true;
            }
            if let Some(comment) = comment {
                let mut kept: String = comment
                    .chars()
                    .take_while({
                        let mut n = 0usize;
                        move |c| {
                            n += c.len_utf8();
                            n <= 127
                        }
                    })
                    .collect();
                kept.shrink_to_fit();
                if model.lfg_comment != kept {
                    model.lfg_comment = kept;
                    changed = true;
                }
            }
            if !changed {
                return Ok(());
            }
            let slots = model.lfg_slots;
            let comment = model.lfg_comment.clone();
            drop(model);
            submit(lua, SocialCall::SetLookingForGroup { slots, comment })
        })?,
    )?;

    g.set(
        "ShowFriends",
        lua.create_function(|lua, ()| submit(lua, SocialCall::ShowFriends))?,
    )?;

    // AddFriend(name): `is-number-or-string` else the usage (`0x5ad298`, `0x85da64`); any name is
    // sent, the empty one too (`0x5ae670` tests nothing).
    g.set(
        "AddFriend",
        lua.create_function(|lua, name: Value| {
            let name = string_arg(lua, name, "Usage: AddFriend(\"name\")")?;
            submit(lua, SocialCall::AddFriend(name))
        })?,
    )?;

    // RemoveFriend(indexOrName): `is-number` first, so a numeric string is an index (`0x5ad2d8`),
    // then `is-number-or-string` for a name (`0x5ad303`), else the usage (`0x85da80`).
    g.set(
        "RemoveFriend",
        lua.create_function(|lua, who: Value| {
            if let Some(n) = binding_abi::is_number(lua, who.clone()) {
                let index = binding_abi::ftol(n) as u32;
                return submit(lua, SocialCall::RemoveFriendIndex(index));
            }
            let name = string_arg(lua, who, "Usage: RemoveFriend([\"name\"] or [index])")?;
            submit(lua, SocialCall::RemoveFriendName(name))
        })?,
    )?;

    // ── The ignore list ──
    // GetNumIgnores(): CountIgnores `0x5ae550`, over the 25 slots at `+0x650`.
    g.set(
        "GetNumIgnores",
        lua.create_function(|lua, ()| {
            if let Some(SocialAnswer::Count(n)) = read_live(lua, SocialRead::IgnoreCount) {
                return Ok(n as i64);
            }
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let slots = model.social.ignores.iter().take(IGNORE_SLOTS);
            Ok(slots.take_while(|i| i.listed).count() as i64)
        })?,
    )?;

    // GetIgnoreName(index) (`0x5ad460`): the guid `0x5ae570` finds for `index − 1`, up to 25
    // inclusive, its name read from the cache with no callback (`0x5ad4e5`), else the global
    // `UNKNOWN` (`0x5ad4ee`), for no guid as for no name.
    g.set(
        "GetIgnoreName",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: GetIgnoreName(index)` (`0x85dac8`, `0x5ad48a`).
            let index = binding_abi::index_arg32(lua, index, "Usage: GetIgnoreName(index)")?;
            let slot = index.wrapping_sub(1) as usize;
            // The guid's name through the name cache's own read (`0x55f080`, no callback).
            let name = match read_live(lua, SocialRead::IgnoreGuid(index)) {
                Some(SocialAnswer::Guid(0)) => None,
                Some(SocialAnswer::Guid(guid)) => {
                    super::live::ask(lua, &super::PlayerNameQuery(guid)).flatten()
                }
                _ => {
                    let model = lua.app_data_ref::<Model>().expect("model app_data");
                    (slot <= IGNORE_SLOTS)
                        .then(|| model.social.ignores.get(slot).and_then(|i| i.name.clone()))
                        .flatten()
                }
            };
            let name = name.unwrap_or_else(|| super::glue_core::global_text(lua, "UNKNOWN"));
            lua.create_string(&name)
        })?,
    )?;

    // GetSelectedIgnore / SetSelectedIgnore (`0x5ae630` / `0x5ae5f0`), as the friend pair.
    g.set(
        "GetSelectedIgnore",
        lua.create_function(|lua, ()| {
            if let Some(SocialAnswer::Index(i)) = read_live(lua, SocialRead::SelectedIgnore) {
                return Ok(i64::from(i));
            }
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(i64::from(model.social.selected_ignore))
        })?,
    )?;
    g.set(
        "SetSelectedIgnore",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: SetSelectedIgnore(index)` (`0x85dae4`, `0x5ad531`).
            let index = binding_abi::index_arg32(lua, index, "Usage: SetSelectedIgnore(index)")?;
            submit(lua, SocialCall::SelectIgnore(index))
        })?,
    )?;

    // Each `is-number-or-string`, else its usage (`0x85db24`, `0x85db40`, `0x85db04`).
    for (global, usage, make) in [
        (
            "AddIgnore",
            "Usage: AddIgnore(\"name\")",
            SocialCall::AddIgnore as fn(String) -> _,
        ),
        (
            "DelIgnore",
            "Usage: DelIgnore(\"name\")",
            SocialCall::DelIgnore as fn(String) -> _,
        ),
        (
            "AddOrDelIgnore",
            "Usage: AddOrDelIgnore(\"name\")",
            SocialCall::AddOrDelIgnore as fn(String) -> _,
        ),
    ] {
        g.set(
            global,
            lua.create_function(move |lua, name: Value| {
                let name = string_arg(lua, name, usage)?;
                submit(lua, make(name))
            })?,
        )?;
    }

    // ── /who ──
    // GetNumWhoResults() → displayed, total, the server's full match count.
    g.set(
        "GetNumWhoResults",
        lua.create_function(|lua, ()| {
            if let Some(SocialAnswer::WhoCounts { kept, total }) =
                read_live(lua, SocialRead::WhoCounts)
            {
                return Ok((i64::from(kept), i64::from(total)));
            }
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok((
                model.social.who.len() as i64,
                i64::from(model.social.who_total),
            ))
        })?,
    )?;

    // GetWhoInfo(index) (`0x5ad6e0`): the row's six values, each name the localized one or the
    // global `UNKNOWN` for an id with no row, read at the call (`0x703bf0`).
    g.set(
        "GetWhoInfo",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: GetWhoInfo(index)` (`0x85db5c`, `0x5ad6f1`).
            let index = i64::from(binding_abi::number_arg(
                lua,
                index,
                "Usage: GetWhoInfo(index)",
            )?);
            let unknown = super::glue_core::global_text(lua, "UNKNOWN");
            let live = match read_live(lua, SocialRead::Who(index as u32)) {
                Some(SocialAnswer::Who(row)) => Some(row.map(std::borrow::Cow::Owned)),
                _ => None,
            };
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let row = live.unwrap_or_else(|| who_at(&model.social, index as u32));
            // `nil, nil, 0, nil, nil, nil` (`0x5ad836`-`0x5ad862`).
            let Some(row) = row else {
                return Ok((
                    Value::Nil,
                    Value::Nil,
                    Value::Integer(0),
                    Value::Nil,
                    Value::Nil,
                    Value::Nil,
                ));
            };
            Ok((
                Value::String(lua.create_string(&row.name)?),
                Value::String(lua.create_string(&row.guild)?),
                Value::Integer(i64::from(row.level)),
                Value::String(lua.create_string(row.race.as_deref().unwrap_or(&unknown))?),
                Value::String(lua.create_string(row.class.as_deref().unwrap_or(&unknown))?),
                Value::String(lua.create_string(row.zone.as_deref().unwrap_or(&unknown))?),
            ))
        })?,
    )?;

    // SendWho(filter) (`0x5ad3b0`): `lua_isstring`, else the usage (`0x85daac`); then the builder
    // `0x5aebb0` on `lua_tostring`'s bytes (`0x5ad3e3`), a number its text.
    g.set(
        "SendWho",
        lua.create_function(|lua, filter: Value| {
            let filter = match filter {
                Value::String(_) | Value::Integer(_) | Value::Number(_) => {
                    lua.coerce_string(filter)?
                }
                _ => None,
            };
            let Some(filter) = filter else {
                return Err(mlua::Error::runtime("Usage: SendWho(\"filter\")"));
            };
            submit(lua, SocialCall::Who(filter.as_bytes().to_vec()))
        })?,
    )?;

    // SortWho(sortType) (`0x5ad890`): promotes the key in the seven-slot chain, reversing it if
    // it is already in front, sorts the list, and fires `WHO_LIST_UPDATE` synchronously
    // (`0x5ad9ed`, `SignalEvent` `0x703e50`), so the list redraws before the click's sound: the
    // host promotes and sorts in the call, and the listeners read the array through it. Returns
    // nothing; an argument neither string nor number raises with the client's own typo
    // (`0x85db88`, `0x5ad898`'s `0x6f3510` guard).
    g.set(
        "SortWho",
        lua.create_function(|lua, sort_type: Value| {
            let sort_type = string_arg(lua, sort_type, "Usgae: SortWho(\"type\")")?;
            submit(lua, SocialCall::SortWho(sort_type))?;
            super::tick::fire_event_into(lua, "WHO_LIST_UPDATE", Vec::new());
            Ok(())
        })?,
    )?;

    // SetWhoToUI(flag) (`0x5ad870`): the Who frame passes 0 on load and hide and 1 on show; the
    // flag goes through `GetBoolOrDefault` (`0x5ad877`) with default 0 and is stored in the call
    // (`0x5ad87c`). Only `SMSG_WHO`'s routing reads it (`0x5adf9c`), which the social lane runs
    // as the answer fires.
    g.set(
        "SetWhoToUI",
        lua.create_function(|lua, flag: Option<Value>| {
            let on = bool_or_default(flag.as_ref(), false);
            lua.app_data_mut::<Model>()
                .expect("model app_data")
                .who_to_ui = on;
            Ok(())
        })?,
    )?;

    Ok(())
}

/// The friend list's slot count (`0x32`).
const FRIEND_SLOTS: usize = 50;
/// The ignore list's slot count (`0x19`).
const IGNORE_SLOTS: usize = 25;

/// The snapshot's `/who` slot `index − 1` against the kept count, unsigned, with `ja`
/// (`0x5ad71a`-`0x5ad71f`): the count's own slot passes.
fn who_at(social: &SocialState, index: u32) -> Option<std::borrow::Cow<'_, WhoInfo>> {
    let slot = index.wrapping_sub(1) as usize;
    let who = &social.who;
    match slot.cmp(&who.len()) {
        std::cmp::Ordering::Less => who.get(slot),
        // `None` after a full 50, the miss the live read answers for the bytes past the array.
        std::cmp::Ordering::Equal => social.who_next.as_ref(),
        std::cmp::Ordering::Greater => None,
    }
    .map(std::borrow::Cow::Borrowed)
}

/// `GetFriendSlot` (`0x5ae4b0`) on `index − 1`: a slot up to 50 inclusive, compared unsigned, so 0
/// and every negative index find none; one the snapshot does not hold is an empty slot.
fn friend_at(friends: &[FriendInfo], index: u32) -> Option<std::borrow::Cow<'_, FriendInfo>> {
    let slot = index.wrapping_sub(1) as usize;
    (slot <= FRIEND_SLOTS).then(|| match friends.get(slot) {
        Some(f) => std::borrow::Cow::Borrowed(f),
        None => std::borrow::Cow::Owned(FriendInfo::default()),
    })
}

#[cfg(test)]
mod tests {
    use super::SocialCall;
    use crate::script::UiScript;

    /// `SortWho` hands the click to the host and fires `WHO_LIST_UPDATE` before it returns, once
    /// per click (`0x5ad9ed`); it returns nothing (`0x5ad9f8`). A number is stringified; anything
    /// else raises with the client's misspelt usage string (`0x85db88`) and fires nothing.
    #[test]
    fn sort_who_hands_its_click_to_the_host_and_fires_in_the_call() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.run(
            r#"
            fired = 0
            local f = CreateFrame("Frame", "WhoWatcher")
            f:RegisterEvent("WHO_LIST_UPDATE")
            f:SetScript("OnEvent", function() fired = fired + 1 end)
            SortWho("name")
            "#,
        )
        .unwrap();
        assert_eq!(s.eval::<i64>("return fired").unwrap(), 1, "inside the call");
        assert_eq!(s.arity(r#"SortWho("name")"#).unwrap(), 0);
        s.run("SortWho(5)").unwrap();
        assert_eq!(
            s.take_social_calls(),
            ["name", "name", "5"].map(|k| SocialCall::SortWho(k.into()))
        );
        let err = s.run("SortWho({})").unwrap_err().to_string();
        assert!(
            err.contains(r#"Usgae: SortWho("type")"#),
            "the reference's own typo, verbatim: {err}"
        );
        assert_eq!(
            s.eval::<i64>("return fired").unwrap(),
            3,
            "one fire per click"
        );
        assert!(s.take_social_calls().is_empty());
    }

    #[test]
    fn the_lfg_pair_stores_the_comment_and_sends_only_on_a_change() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        assert_eq!(s.arity("GetLookingForGroup()").unwrap(), 4);
        assert!(s
            .eval::<bool>(
                "local a, b, c, d = GetLookingForGroup() return a == nil and b == nil and c == nil and d == \"\""
            )
            .unwrap());
        // Three admissible pairs: every word packs to 0, so nothing changes or sends.
        s.run("SetLookingForGroup(1, 3, 3, 12, 5, 0)").unwrap();
        assert!(s.take_social_calls().is_empty(), "the AND pack stores zero");
        // A comment behind the gate: argument 4 is a number, argument 7 the text.
        s.run(r#"SetLookingForGroup(1, 3, 3, 12, 5, 0, "LF2M UBRS")"#)
            .unwrap();
        assert_eq!(
            s.take_social_calls(),
            vec![SocialCall::SetLookingForGroup {
                slots: [0; 3],
                comment: "LF2M UBRS".into()
            }]
        );
        assert_eq!(
            s.eval::<String>("local _, _, _, comment = GetLookingForGroup() return comment")
                .unwrap(),
            "LF2M UBRS"
        );
        s.run(r#"SetLookingForGroup(1, 3, 3, 12, 5, 0, "LF2M UBRS")"#)
            .unwrap();
        assert!(s.take_social_calls().is_empty());
        // Argument 4 nil: the comment at 7 is not read.
        s.run(r#"SetLookingForGroup(1, 3, nil, nil, nil, nil, "ignored")"#)
            .unwrap();
        assert!(s.take_social_calls().is_empty());
        assert_eq!(
            s.eval::<String>("local _, _, _, comment = GetLookingForGroup() return comment")
                .unwrap(),
            "LF2M UBRS"
        );
        // 127 bytes kept of a longer comment (`SStrCopy` into the 0x80 buffer).
        s.run(&format!(
            r#"SetLookingForGroup(0, 0, 0, 0, 0, 0, "{}")"#,
            "x".repeat(200)
        ))
        .unwrap();
        assert_eq!(
            s.eval::<String>("local _, _, _, comment = GetLookingForGroup() return comment")
                .unwrap()
                .len(),
            127
        );
        assert!(s.errors().is_empty(), "{:?}", s.errors());
    }
}
