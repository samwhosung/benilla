//! The social API: friends, ignores and `/who`. The app pushes a display-ready [`SocialState`]
//! (the reference's formatter `0x5ae160` also resolves names engine-side) and drains the queued
//! [`SocialRequest`]s. Selection is engine state in the reference (guids at FriendList `+0x648` and
//! `+0x720`, read back by `0x5ad260` and `0x5ae510`), so a set changes the snapshot at once and
//! queues the change for the app.

use mlua::{Lua, Value};

use super::binding_abi::string_arg;
use super::who_sort::WhoSortChain;
use super::Model;

/// One friend row in `GetFriendInfo`'s order; offline, the level is 0 and class and area empty.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FriendInfo {
    /// From the name cache; empty while the name query is in flight.
    pub name: String,
    pub level: u32,
    pub class: String,
    /// The zone name, also empty when the id has no `AreaTable` row.
    pub area: String,
    pub connected: bool,
    /// The away tag the friends-list template takes: `""`, `"<AFK>"` or `"<DND>"`.
    pub status: String,
}

/// One `/who` row in `GetWhoInfo`'s order, names localized; race comes before class, where the
/// wire has class first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WhoInfo {
    pub name: String,
    pub guild: String,
    pub level: u32,
    pub race: String,
    pub class: String,
    pub zone: String,
}

/// The social snapshot the app pushes whole; the default is a fresh login's.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SocialState {
    /// In display order: the app sorts, as the reference's comparator `0x5ada00` does.
    pub friends: Vec<FriendInfo>,
    /// The selected friend, 1-based, 0 for none (`0x5ad260` returns the stored slot + 1).
    pub selected_friend: u32,
    pub ignores: Vec<String>,
    /// The selected ignore, on the same scale.
    pub selected_ignore: u32,
    /// The last `/who` answer's rows, at most 49.
    pub who: Vec<WhoInfo>,
    /// The total match count, `GetNumWhoResults`'s second return; it can exceed `who.len()`.
    pub who_total: u32,
    /// The `/who` sort chain, pushed so `SortWho` can re-sort before its synchronous redraw.
    pub who_sort: WhoSortChain,
}

/// Outbound social intents, drained by the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SocialRequest {
    /// `ShowFriends()`: `CMSG_FRIEND_LIST`.
    RefreshFriends,
    /// `AddFriend(name)`.
    AddFriend(String),
    /// `RemoveFriend(index)`, from the friends frame's button.
    RemoveFriendIndex(u32),
    /// `RemoveFriend(name)`, from `/removefriend`; the app resolves either shape to the guid.
    RemoveFriendName(String),
    /// `AddIgnore(name)`.
    AddIgnore(String),
    /// `DelIgnore(name)`.
    DelIgnore(String),
    /// `AddOrDelIgnore(name)`, `/ignore`'s toggle; the app decides which, as it holds the list.
    ToggleIgnore(String),
    /// A changed `SetLookingForGroup`: slots and comment for `CMSG_SET_LOOKING_FOR_GROUP`.
    SetLookingForGroup { slots: [u32; 3], comment: String },
    /// `SetSelectedFriend(index)`, mirrored so the next push agrees.
    SelectFriend(u32),
    /// `SetSelectedIgnore(index)`.
    SelectIgnore(u32),
    /// `SendWho(filter)`, the raw filter; parsing it needs the DBCs, so the app does it.
    Who(String),
    /// `SortWho(sortType)`, the raw argument; the binding has already sorted the snapshot.
    SortWho(String),
    /// `SetWhoToUI(flag)`: the next `/who` answer goes to the Who frame (true) or chat (false).
    SetWhoToUi(bool),
}

impl super::UiScript {
    /// Replace the social snapshot; firing `FRIENDLIST_UPDATE` and kin is the app's job.
    pub fn set_social(&mut self, state: SocialState) {
        self.model_mut().social = state;
    }

    /// Drain the social intents queued since the last call.
    pub fn take_social_requests(&mut self) -> Vec<SocialRequest> {
        std::mem::take(&mut self.model_mut().social_requests)
    }

    /// Queue an intent from the app side.
    pub fn queue_social_request(&mut self, request: SocialRequest) {
        self.model_mut().social_requests.push(request);
    }
}

/// Register the social globals against the snapshot store.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // ── The friend list ──
    // GetNumFriends(): `0x5ad000` over CountFriends `0x5ae490`.
    g.set(
        "GetNumFriends",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.social.friends.len() as i64)
        })?,
    )?;

    // GetFriendInfo(index) (`0x5ad060`): an index past the end answers nils, not an error;
    // FrameXML tests `if ( not name )`.
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
                if friend.connected {
                    Value::Integer(1)
                } else {
                    Value::Nil
                },
                Value::String(lua.create_string(&friend.status)?),
            ))
        })?,
    )?;

    g.set(
        "GetSelectedFriend",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(i64::from(model.social.selected_friend))
        })?,
    )?;

    // SetSelectedFriend(index): set now, since `FriendsList_Update` reads it back in the same
    // call, and queue it for the app.
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
                // `&`, not `|`: the reference's own pack.
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

    g.set(
        "ShowFriends",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.social_requests.push(SocialRequest::RefreshFriends);
            Ok(())
        })?,
    )?;

    // AddFriend(name): a blank name is not sent, as vmangos drops it without a reply
    // (`MiscHandler.cpp:468`).
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

    // RemoveFriend(indexOrName): the friends frame passes a row, `/removefriend` a name.
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

    // ── The ignore list ──
    // GetNumIgnores(): CountIgnores `0x5ae550`, over the 25 slots at `+0x650`.
    g.set(
        "GetNumIgnores",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.social.ignores.len() as i64)
        })?,
    )?;

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

    // GetSelectedIgnore / SetSelectedIgnore (`0x5ae630` / `0x5ae5f0`), as the friend pair.
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

    // ── /who ──
    // GetNumWhoResults() → displayed, total, the server's full match count.
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

    // SortWho(sortType) (`0x5ad890`): promotes the key in the seven-slot chain, reversing it if
    // it is already in front, sorts the list, and fires `WHO_LIST_UPDATE` synchronously
    // (`0x5ad9ed`, `SignalEvent` `0x703e50`), so the list redraws before the click's sound. The
    // app gets the same click for its own chain. Returns nothing; an argument neither string nor
    // number raises with the client's own typo (`0x85db88`, `0x5ad898`'s `0x6f3510` guard).
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

    // SetWhoToUI(flag): the Who frame passes 1 on show and 0 on hide; nil, false and 0 are off.
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

/// A selection index in `1..=len`, else 0, nothing selected.
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

    /// Two `/who` hits and a `WHO_LIST_UPDATE` handler that records the order it sees.
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

    /// A queued event would leave `order` one click stale.
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
        // Returns nothing (`0x5ad9f8`, `xor eax,eax; ret`).
        assert_eq!(s.arity(r#"SortWho("name")"#).unwrap(), 0);
        assert!(s.errors().is_empty(), "{:?}", s.errors());
    }

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

        // Back to name, promoted from slot 1: it keeps its descending direction.
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

    /// A number is stringified and falls to the name key; anything else raises with the client's
    /// misspelt usage string (`0x85db88`).
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

    #[test]
    fn the_lfg_pair_stores_the_comment_and_sends_only_on_a_change() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.arity("GetLookingForGroup()").unwrap(), 4);
        assert!(s
            .eval::<bool>(
                "local a, b, c, d = GetLookingForGroup() return a == nil and b == nil and c == nil and d == \"\""
            )
            .unwrap());
        // Three admissible pairs: every word packs to 0, so nothing changes or sends.
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
        s.run(r#"SetLookingForGroup(1, 3, 3, 12, 5, 0, "LF2M UBRS")"#)
            .unwrap();
        assert!(s.take_social_requests().is_empty());
        // Argument 4 nil: the comment at 7 is not read.
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
