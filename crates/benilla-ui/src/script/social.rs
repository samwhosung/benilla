//! The social **Era API surface** — friends, ignores, and `/who`.
//!
//! The [`super::party`] shape exactly: the app pushes a [`SocialState`] snapshot built from its
//! own wire mirror ([`UiScript::set_social`]) and the getters here read that plain data; every
//! verb queues a [`SocialRequest`] the app drains ([`UiScript::take_social_requests`]) into the
//! matching `CMSG_*` send. No ECS or net reach from the engine.
//!
//! **The snapshot is already display-ready** — names, class name, zone name, the `<AFK>` tag —
//! because every one of those is a *lookup the engine owns* in the real client too: the friend
//! slot on the wire holds a bare guid + class/area **ids**, and `FriendList`'s formatter
//! (`0x5ae160`) resolves them through the ObjectMgr name cache and the race/class/area GameTables
//! before Lua ever sees them. Pushing ids here and resolving in Lua would invent a client the
//! reference isn't.
//!
//! **Selection lives in the engine, not in Lua** — the reference keeps the selected friend and
//! ignore as *guids* on the FriendList object (`+0x648` / `+0x720`, read back by
//! `GetSelectedFriend` `0x5ad260` / `0x5ae510`), which is why `SetSelectedFriend(i)` here mutates
//! the snapshot in place *and* queues the intent: the same Lua tick reads the new value back
//! (`FriendsList_Update` does exactly that), and the app's own state follows so the next push
//! agrees.

use mlua::{Lua, Value};

use super::binding_abi::string_arg;
use super::who_sort::WhoSortChain;
use super::Model;

/// One friend row, already resolved for display — see the module doc on why the app resolves
/// rather than the VM. Mirrors `GetFriendInfo`'s six returns in field order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FriendInfo {
    /// The character's name, from the name cache. Empty while the `CMSG_NAME_QUERY` is still in
    /// flight — the FrameXML's own `if ( not name ) then name = UNKNOWN` covers that frame, so
    /// the row shows "Unknown" rather than vanishing.
    pub name: String,
    /// Level, `0` when offline (the wire sends no level for an offline friend).
    pub level: u32,
    /// Localized class name ("Warrior"), empty when offline.
    pub class: String,
    /// Zone name ("Elwynn Forest"), empty when offline or when the id has no `AreaTable` row.
    pub area: String,
    /// Online at all — `GetFriendInfo`'s fifth return, and what enables the Send Message /
    /// Group Invite buttons.
    pub connected: bool,
    /// The away tag as the friends-list template wants it: `""`, `"<AFK>"`, or `"<DND>"`.
    pub status: String,
}

/// One `/who` row — `GetWhoInfo`'s six returns in order (note the wire's class/race order is
/// swapped relative to this Lua-facing one: the API returns race *before* class).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WhoInfo {
    pub name: String,
    pub guild: String,
    pub level: u32,
    /// Localized race name ("Night Elf").
    pub race: String,
    /// Localized class name ("Druid").
    pub class: String,
    /// Zone name.
    pub zone: String,
}

/// The social snapshot, pushed whole by the app whenever it changes ([`UiScript::set_social`]).
/// `default()` is the fresh-login shape: no friends, no ignores, no `/who` run yet.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SocialState {
    /// The friend list, in the order the list frame shows it (the app sorts — the reference's
    /// own `FriendList` comparator `0x5ada00` does the same engine-side).
    pub friends: Vec<FriendInfo>,
    /// The selected friend as a **1-based** index, `0` = none — `GetSelectedFriend`'s scale
    /// (`0x5ad260` returns the stored slot + 1).
    pub selected_friend: u32,
    /// The ignore list: names only, which is all `GetIgnoreName` returns.
    pub ignores: Vec<String>,
    /// The selected ignore, same 1-based scale.
    pub selected_ignore: u32,
    /// The last `/who` answer's rows (≤ 49).
    pub who: Vec<WhoInfo>,
    /// The last `/who` answer's *total* match count — `GetNumWhoResults`'s second return, which
    /// can exceed `who.len()` and is what drives the "(50 displayed)" suffix.
    pub who_total: u32,
    /// The `/who` sort chain ([`WhoSortChain`]) as the app holds it. Pushed with the rows because
    /// `SortWho` has to promote it and re-sort **inside the binding** — the reference's
    /// `WHO_LIST_UPDATE` is synchronous, so the redraw it triggers reads the new order before the
    /// click script returns, a tick before the app's own copy could have pushed it back.
    /// [`super::SocialRequest::SortWho`] carries the same click to the app, whose next push then
    /// agrees. Same shape as `SetSelectedFriend`'s (module doc).
    pub who_sort: WhoSortChain,
}

/// Outbound social intents queued by the Era API, drained by the app
/// ([`UiScript::take_social_requests`]). Plain data — [`super::party::PartyRequest`]'s twin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SocialRequest {
    /// `ShowFriends()` — refresh the list (`CMSG_FRIEND_LIST`).
    RefreshFriends,
    /// `AddFriend(name)`.
    AddFriend(String),
    /// `RemoveFriend(index)` — the friends frame's button, which addresses the *row*.
    RemoveFriendIndex(u32),
    /// `RemoveFriend(name)` — `/removefriend`, which addresses a name. One Era global takes
    /// either, so the queue carries both shapes and the app resolves each to the guid the wire
    /// wants.
    RemoveFriendName(String),
    /// `AddIgnore(name)`.
    AddIgnore(String),
    /// `DelIgnore(name)`.
    DelIgnore(String),
    /// `AddOrDelIgnore(name)` — `/ignore`'s toggle: ignore if not ignored, un-ignore if it is.
    /// The app decides which, because only it holds the list.
    ToggleIgnore(String),
    /// `SetLookingForGroup(...)` committed a change: the slots as stored and the comment, for
    /// `CMSG_SET_LOOKING_FOR_GROUP` (1961).
    SetLookingForGroup { slots: [u32; 3], comment: String },
    /// `SetSelectedFriend(index)` — mirrored into the app so the next push agrees.
    SelectFriend(u32),
    /// `SetSelectedIgnore(index)`.
    SelectIgnore(u32),
    /// `SendWho(filter)` — the raw filter string as typed; parsing it into wire fields needs the
    /// DBCs, so it happens app-side.
    Who(String),
    /// `SortWho(sortType)` — `"name"`/`"level"`/`"class"`/`"zone"`/`"guild"`/`"race"`, the raw
    /// argument as the click passed it. Sorting is client-side and the binding has **already**
    /// done it to the snapshot ([`SocialState::who_sort`]); this carries the same click to the
    /// app so its authoritative chain promotes identically and the next push agrees.
    SortWho(String),
    /// `SetWhoToUI(flag)` — where the *next* `/who` answer goes: the Who frame (true) or the chat
    /// frame (false). The WhoFrame's own OnShow/OnHide drive it.
    SetWhoToUi(bool),
}

impl super::UiScript {
    /// Push the social snapshot, replacing whatever was there. A bare setter — firing
    /// `FRIENDLIST_UPDATE`/`IGNORELIST_UPDATE`/`WHO_LIST_UPDATE` on the edges is the app's job.
    pub fn set_social(&mut self, state: SocialState) {
        self.model_mut().social = state;
    }

    /// Drain the social intents queued since the last call.
    pub fn take_social_requests(&mut self) -> Vec<SocialRequest> {
        std::mem::take(&mut self.model_mut().social_requests)
    }

    /// Queue an intent from the app side — the slash commands. In the reference these ARE Lua
    /// (`SlashCmdList["FRIENDS"]` calls `AddFriend`, …); benilla parses slash lines in Rust, so
    /// the same intents enter the same queue here. [`super::duel`]'s `queue_duel_request` twin.
    pub fn queue_social_request(&mut self, request: SocialRequest) {
        self.model_mut().social_requests.push(request);
    }
}

/// Register the social globals against the snapshot store.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // ── The friend list ──────────────────────────────────────────────────────────────────────
    // GetNumFriends() → how many friends are listed (`0x5ad000` over CountFriends `0x5ae490`).
    g.set(
        "GetNumFriends",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.social.friends.len() as i64)
        })?,
    )?;

    // GetFriendInfo(index) → name, level, class, area, connected, status (`0x5ad060`). An index
    // past the end returns nothing at all — the FrameXML tests `if ( not name )`, so nil-per-
    // return is the shape it expects, not an error.
    g.set(
        "GetFriendInfo",
        lua.create_function(|lua, index: i64| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let Some(friend) = friend_at(&model.social.friends, index) else {
                return Ok((
                    Value::Nil,
                    Value::Nil,
                    Value::Nil,
                    Value::Nil,
                    Value::Nil,
                    Value::Nil,
                ));
            };
            Ok((
                Value::String(lua.create_string(&friend.name)?),
                Value::Integer(i64::from(friend.level)),
                Value::String(lua.create_string(&friend.class)?),
                Value::String(lua.create_string(&friend.area)?),
                // `connected` is the era 1/nil boolean the list frame branches on.
                if friend.connected {
                    Value::Integer(1)
                } else {
                    Value::Nil
                },
                Value::String(lua.create_string(&friend.status)?),
            ))
        })?,
    )?;

    // GetSelectedFriend() → the 1-based selected row, 0 when nothing is selected.
    g.set(
        "GetSelectedFriend",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(i64::from(model.social.selected_friend))
        })?,
    )?;

    // SetSelectedFriend(index) — mutate now (the caller reads it back this tick) and mirror the
    // intent to the app (module doc: selection is engine state in the reference too).
    g.set(
        "SetSelectedFriend",
        lua.create_function(|lua, index: i64| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let index = clamp_index(index, model.social.friends.len());
            model.social.selected_friend = index;
            model
                .social_requests
                .push(SocialRequest::SelectFriend(index));
            Ok(())
        })?,
    )?;

    // The LFG pair, as the bytes define it (`GetLookingForGroup 0x4e95d0` / `SetLookingForGroup
    // 0x4e96b0`; decision 1961 — which corrects 1959's flag): the stock 1.12.1 FrameXML never
    // calls either (FriendsFrame.xml's two call sites sit inside its l.1212-1301 XML comment), so
    // this is an addon surface.
    //
    // `GetLookingForGroup()` → FOUR values: the three slot NAMES — each nil for a slot word whose
    // id maps to nothing, and the reference's own pack (below) leaves every slot word 0, so nil
    // is the only value a name can take here — then the comment, always a string. Never a
    // number, never `1|nil`.
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
    // `SetLookingForGroup(type1, entry1, type2, entry2, type3, entry3, comment)` → nothing. Up to
    // three pairs, read at arguments 1/3/5 and 2/4/6; the loop ENDS (it does not skip a pair) on
    // a non-number type, a type >= 6, or an entry at or past the per-type eligible count, and the
    // word it stores is `(type << 24) & entry` — an AND where every consumer decodes an OR
    // (`0x4e9713`), so the stored word is 0 for every admissible input. Which is why the
    // eligible-count table is not modelled: with the pack as it is, no admissible pair can store
    // anything but 0, and an inadmissible one ends the loop leaving 0 — the slots never change.
    // The comment is gated on argument 4 being a number or a string (`lua_isstring(L, 4)`) and
    // read from argument 7 (`lua_tostring(L, 7)`, nil for an absent one); both immediates raw.
    // The commit stores what changed and sends `CMSG_SET_LOOKING_FOR_GROUP` only then — so only a
    // changed comment ever sends. `SStrCopy(…, 0x80)`: the comment keeps 127 bytes.
    g.set(
        "SetLookingForGroup",
        lua.create_function(|lua, args: mlua::MultiValue| {
            let args: Vec<Value> = args.into_iter().collect();
            let arg = |i: usize| args.get(i - 1).cloned().unwrap_or(Value::Nil);
            let number = |v: &Value| match v {
                Value::Integer(i) => Some(*i as f64),
                Value::Number(n) => Some(*n),
                Value::String(s) => s.to_str().ok().and_then(|s| s.trim().parse::<f64>().ok()),
                _ => None,
            };
            let mut slots = [0u32; 3];
            for (slot, i) in slots.iter_mut().zip([1usize, 3, 5]) {
                let Some(ty) = number(&arg(i)) else { break };
                let ty = ty.trunc();
                if !(0.0..6.0).contains(&ty) {
                    break;
                }
                let entry = number(&arg(i + 1)).unwrap_or(0.0).trunc();
                // `(type << 24) & entry`, the reference's own pack.
                *slot = ((ty as u32) << 24) & (entry as i64 as u32);
            }
            let comment = match arg(4) {
                Value::Integer(_) | Value::Number(_) | Value::String(_) => match arg(7) {
                    Value::String(s) => Some(s.to_string_lossy()),
                    Value::Integer(i) => Some(i.to_string()),
                    Value::Number(n) => Some(n.to_string()),
                    _ => None,
                },
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
            if changed {
                let slots = model.lfg_slots;
                let comment = model.lfg_comment.clone();
                model
                    .social_requests
                    .push(SocialRequest::SetLookingForGroup { slots, comment });
            }
            Ok(())
        })?,
    )?;

    // ShowFriends() — ask the server for the list again.
    g.set(
        "ShowFriends",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.social_requests.push(SocialRequest::RefreshFriends);
            Ok(())
        })?,
    )?;

    // AddFriend(name) — an empty name is dropped here rather than sent: the server answers a
    // blank lookup with nothing at all, so it would look like a hang.
    g.set(
        "AddFriend",
        lua.create_function(|lua, name: String| {
            if !name.trim().is_empty() {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                model.social_requests.push(SocialRequest::AddFriend(name));
            }
            Ok(())
        })?,
    )?;

    // RemoveFriend(indexOrName) — the one global with two callers of different shapes (the
    // frame's button passes a row index, /removefriend a name).
    g.set(
        "RemoveFriend",
        lua.create_function(|lua, who: Value| {
            let request = match who {
                Value::Integer(i) if i >= 1 => Some(SocialRequest::RemoveFriendIndex(i as u32)),
                Value::Number(n) if n >= 1.0 => Some(SocialRequest::RemoveFriendIndex(n as u32)),
                Value::String(s) => {
                    let name = s.to_str()?.to_string();
                    (!name.trim().is_empty()).then_some(SocialRequest::RemoveFriendName(name))
                }
                _ => None,
            };
            if let Some(request) = request {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                model.social_requests.push(request);
            }
            Ok(())
        })?,
    )?;

    // ── The ignore list ──────────────────────────────────────────────────────────────────────
    // GetNumIgnores() → CountIgnores `0x5ae550` (the 25 slots at +0x650).
    g.set(
        "GetNumIgnores",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.social.ignores.len() as i64)
        })?,
    )?;

    // GetIgnoreName(index) → the name, or nil past the end. The ignore list frame calls this for
    // all 20 of its rows every update, most of them empty — nil is the ordinary case, not an
    // error.
    g.set(
        "GetIgnoreName",
        lua.create_function(|lua, index: i64| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let name = usize::try_from(index - 1)
                .ok()
                .and_then(|i| model.social.ignores.get(i));
            Ok(match name {
                Some(name) => Value::String(lua.create_string(name)?),
                None => Value::Nil,
            })
        })?,
    )?;

    // GetSelectedIgnore() / SetSelectedIgnore(index) — the friend pair's twin (`0x5ae630` /
    // `0x5ae5f0`).
    g.set(
        "GetSelectedIgnore",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(i64::from(model.social.selected_ignore))
        })?,
    )?;
    g.set(
        "SetSelectedIgnore",
        lua.create_function(|lua, index: i64| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let index = clamp_index(index, model.social.ignores.len());
            model.social.selected_ignore = index;
            model
                .social_requests
                .push(SocialRequest::SelectIgnore(index));
            Ok(())
        })?,
    )?;

    // AddIgnore / DelIgnore / AddOrDelIgnore(name).
    for (global, make) in [
        ("AddIgnore", SocialRequest::AddIgnore as fn(String) -> _),
        ("DelIgnore", SocialRequest::DelIgnore as fn(String) -> _),
        (
            "AddOrDelIgnore",
            SocialRequest::ToggleIgnore as fn(String) -> _,
        ),
    ] {
        g.set(
            global,
            lua.create_function(move |lua, name: String| {
                if !name.trim().is_empty() {
                    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                    model.social_requests.push(make(name));
                }
                Ok(())
            })?,
        )?;
    }

    // ── /who ─────────────────────────────────────────────────────────────────────────────────
    // GetNumWhoResults() → displayed, total. The second return is the server's true match count,
    // which is why the frame can say "132 players total (50 displayed)".
    g.set(
        "GetNumWhoResults",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok((
                model.social.who.len() as i64,
                i64::from(model.social.who_total),
            ))
        })?,
    )?;

    // GetWhoInfo(index) → name, guild, level, race, class, zone.
    g.set(
        "GetWhoInfo",
        lua.create_function(|lua, index: i64| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let row = usize::try_from(index - 1)
                .ok()
                .and_then(|i| model.social.who.get(i));
            let Some(row) = row else {
                return Ok((
                    Value::Nil,
                    Value::Nil,
                    Value::Nil,
                    Value::Nil,
                    Value::Nil,
                    Value::Nil,
                ));
            };
            Ok((
                Value::String(lua.create_string(&row.name)?),
                Value::String(lua.create_string(&row.guild)?),
                Value::Integer(i64::from(row.level)),
                Value::String(lua.create_string(&row.race)?),
                Value::String(lua.create_string(&row.class)?),
                Value::String(lua.create_string(&row.zone)?),
            ))
        })?,
    )?;

    // SendWho(filter) — the filter string as typed, parsed app-side.
    g.set(
        "SendWho",
        lua.create_function(|lua, filter: Option<String>| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model
                .social_requests
                .push(SocialRequest::Who(filter.unwrap_or_default()));
            Ok(())
        })?,
    )?;

    // `SortWho(sortType) 0x5ad890` — the column-header and dropdown sorts, and three things at
    // once:
    //
    //  1. **promote** the key into the seven-slot chain, flipping its direction only if it was
    //     already at the front — so a repeated click on the same header REVERSES;
    //  2. **sort right here**, through the chain-walking comparator; and
    //  3. fire `WHO_LIST_UPDATE` **synchronously**, inside the binding (`0x5ad9ed
    //     mov ecx,0x184; call SignalEvent 0x703e50`), so `FriendsFrame_OnEvent` has already
    //     re-read the list through `GetWhoInfo` by the time the header's OnClick plays its sound.
    //     Queueing it would redraw a tick late — the visible half of B365.
    //
    // The intent still goes to the app, which owns the same chain and re-sorts the answers it
    // receives; sorting the snapshot here is what makes the synchronous redraw show the new
    // order, exactly as `SetSelectedFriend` mutates the snapshot it also queues (module doc).
    //
    // Zero return values, and a non-string/number argument RAISES with the client's own typo
    // (`.rdata 0x85db88`) rather than answering nil — `0x5ad898`'s `0x6f3510` guard into
    // `luaL_error`, [`super::binding_abi`]'s shape A.
    g.set(
        "SortWho",
        lua.create_function(|lua, sort_type: Value| {
            let sort_type = string_arg(lua, sort_type, "Usgae: SortWho(\"type\")")?;
            {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                let model = &mut *model;
                model.social.who_sort.promote(&sort_type);
                model.social.who_sort.sort(&mut model.social.who);
                model
                    .social_requests
                    .push(SocialRequest::SortWho(sort_type));
            }
            super::tick::fire_event_into(lua, "WHO_LIST_UPDATE", Vec::new());
            Ok(())
        })?,
    )?;

    // SetWhoToUI(flag) — the WhoFrame's OnShow/OnHide. Era passes 0/1, so anything but a falsey
    // 0/nil is "the frame wants them".
    g.set(
        "SetWhoToUI",
        lua.create_function(|lua, flag: Value| {
            let on = match flag {
                Value::Nil | Value::Boolean(false) => false,
                Value::Integer(0) => false,
                Value::Number(n) => n != 0.0,
                _ => true,
            };
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.social_requests.push(SocialRequest::SetWhoToUi(on));
            Ok(())
        })?,
    )?;

    Ok(())
}

/// The 1-based friend lookup `GetFriendInfo` does, `None` past either end.
fn friend_at(friends: &[FriendInfo], index: i64) -> Option<&FriendInfo> {
    usize::try_from(index - 1).ok().and_then(|i| friends.get(i))
}

/// Clamp a selection index to `0..=len` — `0` means "nothing selected", and a row past the end
/// selects nothing rather than a phantom.
fn clamp_index(index: i64, len: usize) -> u32 {
    if index >= 1 && index <= len as i64 {
        index as u32
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::{SocialRequest, SocialState, WhoInfo};
    use crate::script::UiScript;

    fn who(name: &str, level: u32, zone: &str) -> WhoInfo {
        WhoInfo {
            name: name.to_string(),
            guild: String::new(),
            level,
            race: "Human".to_string(),
            class: "Warrior".to_string(),
            zone: zone.to_string(),
        }
    }

    /// A VM seeded with two hits and a watcher that records the order it can SEE from inside the
    /// `WHO_LIST_UPDATE` handler — which is the only vantage point that can tell a synchronous
    /// fire from a queued one.
    fn seated() -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.run(
            r#"
            fired = 0
            order = ""
            local f = CreateFrame("Frame", "WhoWatcher")
            f:RegisterEvent("WHO_LIST_UPDATE")
            f:SetScript("OnEvent", function()
                fired = fired + 1
                order = ""
                for i = 1, GetNumWhoResults() do
                    order = order .. GetWhoInfo(i) .. ","
                end
            end)
            "#,
        )
        .unwrap();
        s.set_social(SocialState {
            who: vec![
                who("Galas", 60, "Elwynn Forest"),
                who("Erdrin", 12, "Elwynn Forest"),
            ],
            who_total: 2,
            ..Default::default()
        });
        s
    }

    /// **B365.** `SortWho` sorts the list it already holds, fires `WHO_LIST_UPDATE`
    /// **synchronously** so the handler reads the NEW order (`0x5ad9ed`, `SignalEvent 0x703e50`
    /// running every listener inline), and queues the same click for the app. A queued event
    /// would leave `order` one click stale here — which is exactly what the bug looked like.
    #[test]
    fn sort_who_sorts_in_place_and_fires_the_event_synchronously() {
        let mut s = seated();
        assert_eq!(s.eval::<i64>("return fired").unwrap(), 0);

        s.run(r#"SortWho("name")"#).unwrap();
        assert_eq!(s.eval::<i64>("return fired").unwrap(), 1);
        assert_eq!(
            s.eval::<String>("return order").unwrap(),
            "Erdrin,Galas,",
            "the handler must already see the sorted list"
        );
        assert_eq!(
            s.take_social_requests(),
            vec![SocialRequest::SortWho("name".into())],
            "and the app hears the same click, so its copy of the chain follows"
        );
        // Zero return values (`0x5ad9f8 xor eax,eax; ret`).
        assert_eq!(s.arity(r#"SortWho("name")"#).unwrap(), 0);
        assert!(s.errors().is_empty(), "{:?}", s.errors());
    }

    /// The visible half of the report: **clicking the same header twice reverses**, and clicking
    /// a different one in between does not undo that — the direction is remembered per key and
    /// flipped only when the key was already at the front of the chain.
    #[test]
    fn a_repeated_header_click_reverses_and_the_direction_is_remembered() {
        let s = seated();
        s.run(r#"SortWho("name")"#).unwrap();
        assert_eq!(s.eval::<String>("return order").unwrap(), "Erdrin,Galas,");

        s.run(r#"SortWho("name")"#).unwrap();
        assert_eq!(
            s.eval::<String>("return order").unwrap(),
            "Galas,Erdrin,",
            "the second click on the same key reverses it"
        );

        // Level ascending puts Erdrin (12) first; name is still descending behind it.
        s.run(r#"SortWho("level")"#).unwrap();
        assert_eq!(s.eval::<String>("return order").unwrap(), "Erdrin,Galas,");

        // Back to name: promoted from slot 1, so it CARRIES its descending direction rather than
        // flipping to ascending.
        s.run(r#"SortWho("name")"#).unwrap();
        assert_eq!(
            s.eval::<String>("return order").unwrap(),
            "Galas,Erdrin,",
            "a key promoted from behind keeps the direction it was left in"
        );
        assert_eq!(
            s.eval::<i64>("return fired").unwrap(),
            4,
            "one fire per click"
        );
    }

    /// The argument ABI: a number is stringified and falls through to the name key; anything that
    /// is neither number nor string RAISES with the client's own misspelt usage string
    /// (`.rdata 0x85db88`), abandoning the caller's statement rather than answering nil.
    #[test]
    fn sort_who_takes_the_reference_argument_abi() {
        let mut s = seated();
        s.run("SortWho(5)").unwrap();
        assert_eq!(
            s.eval::<String>("return order").unwrap(),
            "Erdrin,Galas,",
            "an unrecognised key is the name key"
        );
        let _ = s.take_social_requests();

        let err = s.run("SortWho({})").unwrap_err().to_string();
        assert!(
            err.contains(r#"Usgae: SortWho("type")"#),
            "the reference's own typo, verbatim: {err}"
        );
        assert_eq!(
            s.eval::<i64>("return fired").unwrap(),
            1,
            "a raised call sorts nothing and fires nothing"
        );
    }

    /// The LFG pair as the bytes define it (1961, correcting 1959): four string-or-nil returns,
    /// the slots zeroed by the reference's own pack, the comment from argument 7 behind the gate
    /// on argument 4, and the wire only on a change.
    #[test]
    fn the_lfg_pair_stores_the_comment_and_sends_only_on_a_change() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.arity("GetLookingForGroup()").unwrap(), 4);
        assert!(s
            .eval::<bool>(
                "local a, b, c, d = GetLookingForGroup() return a == nil and b == nil and c == nil and d == \"\""
            )
            .unwrap());
        // Three admissible pairs: every word packs to 0, nothing changed, nothing sent.
        s.run("SetLookingForGroup(1, 3, 3, 12, 5, 0)").unwrap();
        assert!(
            s.take_social_requests().is_empty(),
            "the AND pack stores zero"
        );
        // A comment behind the gate: argument 4 is a number, argument 7 the text.
        s.run(r#"SetLookingForGroup(1, 3, 3, 12, 5, 0, "LF2M UBRS")"#)
            .unwrap();
        assert_eq!(
            s.take_social_requests(),
            vec![SocialRequest::SetLookingForGroup {
                slots: [0; 3],
                comment: "LF2M UBRS".into()
            }]
        );
        assert_eq!(
            s.eval::<String>("local _, _, _, comment = GetLookingForGroup() return comment")
                .unwrap(),
            "LF2M UBRS"
        );
        // The same comment again: no change, no send.
        s.run(r#"SetLookingForGroup(1, 3, 3, 12, 5, 0, "LF2M UBRS")"#)
            .unwrap();
        assert!(s.take_social_requests().is_empty());
        // Fewer than four arguments: the comment at 7 is never read.
        s.run(r#"SetLookingForGroup(1, 3, nil, nil, nil, nil, "ignored")"#)
            .unwrap();
        assert!(s.take_social_requests().is_empty());
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
