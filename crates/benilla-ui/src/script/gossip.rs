//! The gossip bindings: the getters read the message's slots and the drawn text through the host
//! at the call ([`GossipQuery`]), each select hands the host its call ([`GossipCall`]), which
//! reads the slots and sends before the verb returns, and `CloseGossip` leaves its teardown to the
//! app.
//!
//! `GetGossipOptions()` returns flat `(text, type)` pairs, `type` the lowercase icon name the app
//! maps from the wire `GOSSIP_ICON`. A coded option selected with no code fires
//! `GOSSIP_ENTER_CODE`, whose stock popup (`StaticPopup.lua:1383`) calls
//! `SelectGossipOption(index, code)`.
//!
//! The message writes the slots at once (`0x4e26e0`), and the text only once its greeting is
//! drawn (`0x4e229a`), right before `GOSSIP_SHOW`; while the text query is out the getters read
//! the new slots beside the last text, and nothing signals, so the frame keeps its last paint.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi;
use super::live::{ask, LiveQuery};
use super::Model;

/// One gossip option; its 1-based index is its place in [`GossipMenu::options`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GossipOptionView {
    /// The option's label.
    pub label: String,
    /// The lowercase icon type (`"gossip"`, `"vendor"`, `"taxi"`, …); `GossipFrame.lua:123` draws
    /// `Interface\GossipFrame\<Type>GossipIcon`.
    pub icon_type: String,
}

/// One quest row of `SMSG_GOSSIP_MESSAGE`; a click sends `CMSG_QUESTGIVER_QUERY_QUEST`. `active`,
/// from the wire icon, puts it under current quests rather than available ones.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GossipQuestRow {
    pub title: String,
    /// The row's third wire field. 1.12 never shows it, but the verbs return `(title, level)` pairs
    /// and `GossipFrame.lua:66` strides by 2 over them.
    pub level: u32,
    pub active: bool,
}

/// The gossip store as the getters read it: the host's answer at the call, else the VM's copy,
/// which only a test sets.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GossipMenu {
    /// The drawn text `0xbbb678`, expanded at its draw, which `GetGossipText` returns (`0x4e28c0`).
    pub greeting: String,
    /// Quest rows from the same packet.
    pub quests: Vec<GossipQuestRow>,
    pub options: Vec<GossipOptionView>,
}

/// A gossip select, which the host applies inside the call: its packet before the verb returns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GossipCall {
    /// `SelectGossipOption(i [, code])` (`0x4e2a30` → `0x4e2320`), the 1-based slot and the code,
    /// a string or a number (`0x4e2a60`): `CMSG_GOSSIP_SELECT_OPTION` with the slot's wire index
    /// (`0x4e23ef`), or `GOSSIP_ENTER_CODE` for a coded slot and no code.
    Option { index: u32, code: Option<Vec<u8>> },
    /// `SelectGossipAvailableQuest(i)` (`0x4e24b0`) or `SelectGossipActiveQuest(i)`
    /// (`0x4e2600`), the 1-based row in its own list: the questgiver query or turn-in for it.
    Quest { active: bool, index: u32 },
    /// `CloseGossip()` (`0x4e2b20` → `0x4e1fa0` with a zero guid): the interaction's release
    /// (`0x4e1fc5`), the cell zeroed (`0x4e1fcc`) and `GOSSIP_CLOSED` (`0x4e1fdc`), in the call.
    Close,
}

/// The question the gossip getters ask at the call: the text buffer (`0xbbb678`), the used option
/// slots in slot order (`0xbbbe90`, `0x4e28d0`) and the quest rows (`0xbb74c0`).
pub struct GossipQuery;

impl LiveQuery for GossipQuery {
    type Answer = GossipMenu;
}

/// `f` over the store now: the host's, else the VM's copy; `None` with neither.
fn read<T>(lua: &Lua, f: impl FnOnce(&GossipMenu) -> T) -> Option<T> {
    match ask(lua, &GossipQuery) {
        Some(menu) => Some(f(&menu)),
        None => {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            model.gossip.as_ref().map(f)
        }
    }
}

/// Hand `call` to the host, which applies it before this returns ([`super::calls::submit`]).
fn submit(lua: &Lua, call: GossipCall) -> mlua::Result<()> {
    super::calls::submit(lua, super::ScriptCall::Gossip(call))
}

impl super::UiScript {
    /// Set (or clear, with `None`) the VM's copy of the store, for a test.
    pub fn set_gossip(&mut self, menu: Option<GossipMenu>) {
        self.model_mut().gossip = menu;
    }

    /// `ResetGossipOptions` (`0x4e26a0`) as the getters see it: no option and no quest row; the
    /// text stays.
    pub fn reset_gossip_slots(&mut self) {
        if let Some(menu) = self.model_mut().gossip.as_mut() {
            menu.options.clear();
            menu.quests.clear();
        }
    }

    /// The 1-based slots of the selects a harness's VM kept ([`Self::keep_calls`]).
    pub fn take_gossip_selects(&mut self) -> Vec<u32> {
        self.take_calls_where(|call| match call {
            super::ScriptCall::Gossip(GossipCall::Option { index, .. }) => Some(*index),
            _ => None,
        })
    }

    /// The quest selects a harness's VM kept ([`Self::keep_calls`]): whether active, and the
    /// 1-based row in its list.
    pub fn take_gossip_quest_selects(&mut self) -> Vec<(bool, u32)> {
        self.take_calls_where(|call| match call {
            super::ScriptCall::Gossip(GossipCall::Quest { active, index }) => {
                Some((*active, *index))
            }
            _ => None,
        })
    }
}

/// Register the gossip globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // The static buffer `0xbbb678` (`0x4e28c0`): "" before any gossip (.bss), never nil, and what
    // the last window drew after it closes.
    g.set(
        "GetGossipText",
        lua.create_function(|lua, ()| {
            let text = read(lua, |m| m.greeting.clone()).unwrap_or_default();
            lua.create_string(&text)
        })?,
    )?;

    g.set(
        "GetGossipOptions",
        lua.create_function(|lua, ()| {
            // Copy out under one short borrow; build the Lua values with none held.
            let pairs: Vec<(String, String)> = read(lua, |m| {
                m.options
                    .iter()
                    .map(|o| (o.label.clone(), o.icon_type.clone()))
                    .collect()
            })
            .unwrap_or_default();
            let mut out: Vec<Value> = Vec::with_capacity(pairs.len() * 2);
            for (label, icon) in pairs {
                out.push(Value::String(lua.create_string(&label)?));
                out.push(Value::String(lua.create_string(&icon)?));
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // SelectGossipOption(i [, code]): the 1-based slot, and the code when the second argument is
    // a string or a number (`0x4e2a60`, `0x4e2a70`).
    g.set(
        "SelectGossipOption",
        lua.create_function(|lua, (i, code): (Value, Value)| {
            // `is-number` else `Usage: SelectGossipOption(index)` (`0x84b984`, `0x4e2a86`).
            let index = binding_abi::index_arg32(lua, i, "Usage: SelectGossipOption(index)")?;
            let code = match code {
                // `lua_tostring`'s C string: its bytes up to the first NUL.
                Value::String(_) | Value::Integer(_) | Value::Number(_) => {
                    lua.coerce_string(code)?.map(|s| {
                        let bytes = s.as_bytes();
                        let len = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
                        bytes[..len].to_vec()
                    })
                }
                _ => None,
            };
            submit(lua, GossipCall::Option { index, code })
        })?,
    )?;

    // CloseGossip() (`0x4e2b20`): the close core `0x4e1fa0` with a zero guid, inside the call.
    g.set(
        "CloseGossip",
        lua.create_function(|lua, ()| submit(lua, GossipCall::Close))?,
    )?;

    // ══ The two quest lists ══
    // The reference splits the quest rows by wire icon, 3 and 4 active and the rest available,
    // behind two vararg verbs (`0x4e2430`, `0x4e2580`); the app splits them at parse time.
    fn quest_rows(lua: &Lua, active: bool) -> mlua::Result<MultiValue> {
        let rows: Vec<(String, u32)> = read(lua, |m| {
            m.quests
                .iter()
                .filter(|q| q.active == active)
                .map(|q| (q.title.clone(), q.level))
                .collect()
        })
        .unwrap_or_default();
        let mut out = Vec::with_capacity(rows.len() * 2);
        for (title, level) in rows {
            out.push(Value::String(lua.create_string(&title)?));
            out.push(Value::Integer(i64::from(level)));
        }
        Ok(MultiValue::from_vec(out))
    }

    g.set(
        "GetGossipAvailableQuests",
        lua.create_function(|lua, ()| quest_rows(lua, false))?,
    )?;
    g.set(
        "GetGossipActiveQuests",
        lua.create_function(|lua, ()| quest_rows(lua, true))?,
    )?;

    // A select's index is 1-based within its own list, as `GossipFrame.lua:73` numbers the
    // buttons; the host scans the message's rows for it (`0x4e2ad7`, `0x4e2b17`).
    g.set(
        "SelectGossipAvailableQuest",
        lua.create_function(|lua, i: Value| {
            // `is-number` else `Usage: SelectGossipAvailableQuest(index)` (`0x84b9a8`, `0x4e2ab1`).
            let index =
                binding_abi::index_arg32(lua, i, "Usage: SelectGossipAvailableQuest(index)")?;
            submit(
                lua,
                GossipCall::Quest {
                    active: false,
                    index,
                },
            )
        })?,
    )?;
    g.set(
        "SelectGossipActiveQuest",
        lua.create_function(|lua, i: Value| {
            // `is-number` else `Usage: SelectGossipActiveQuest(index)` (`0x84b9d4`, `0x4e2af1`).
            let index = binding_abi::index_arg32(lua, i, "Usage: SelectGossipActiveQuest(index)")?;
            submit(
                lua,
                GossipCall::Quest {
                    active: true,
                    index,
                },
            )
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{GossipMenu, GossipOptionView};
    use crate::script::UiScript;

    fn menu() -> GossipMenu {
        GossipMenu {
            greeting: "Greetings, traveler.".into(),
            quests: Vec::new(),
            options: vec![
                GossipOptionView {
                    label: "Let me browse your goods.".into(),
                    icon_type: "vendor".into(),
                },
                GossipOptionView {
                    label: "I would like to sign the petition.".into(),
                    icon_type: "gossip".into(),
                },
            ],
        }
    }

    #[test]
    fn gossip_snapshot_reads_and_selects_queue() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        assert_eq!(
            s.eval::<String>("return GetGossipText()").unwrap(),
            "",
            "the buffer is empty before any gossip, never nil (`0x4e28c0`)"
        );
        assert_eq!(s.arity("GetGossipOptions()").unwrap(), 0);

        s.set_gossip(Some(menu()));
        assert_eq!(
            s.eval::<String>("return GetGossipText()").unwrap(),
            "Greetings, traveler."
        );
        let (l1, t1, l2, t2) = s
            .eval::<(String, String, String, String)>(
                "local a,b,c,d = GetGossipOptions()\n\
                 return a, b, c, d",
            )
            .unwrap();
        assert_eq!(s.arity("GetGossipOptions()").unwrap(), 4);
        assert_eq!(
            (l1.as_str(), t1.as_str()),
            ("Let me browse your goods.", "vendor")
        );
        assert_eq!(l2, "I would like to sign the petition.");
        assert_eq!(t2, "gossip");

        s.run("SelectGossipOption(1)").unwrap();
        s.run(
            "SelectGossipOption(2, 'a code') SelectGossipOption(3, 12) SelectGossipOption(4, {})",
        )
        .unwrap();
        assert_eq!(
            s.take_calls_where(|c| match c {
                crate::script::ScriptCall::Gossip(c) => Some(c.clone()),
                _ => None,
            }),
            [
                super::GossipCall::Option {
                    index: 1,
                    code: None
                },
                super::GossipCall::Option {
                    index: 2,
                    code: Some(b"a code".to_vec())
                },
                super::GossipCall::Option {
                    index: 3,
                    code: Some(b"12".to_vec())
                },
                super::GossipCall::Option {
                    index: 4,
                    code: None
                },
            ]
        );
        assert!(s.take_gossip_selects().is_empty(), "taken");

        s.run("CloseGossip()").unwrap();
        assert!(
            s.take_calls_where(|call| match call {
                super::super::ScriptCall::Gossip(super::GossipCall::Close) => Some(()),
                _ => None,
            }) == [()]
        );
    }

    #[test]
    fn the_two_gossip_quest_lists_split_by_active_and_select_within_themselves() {
        use super::GossipQuestRow;
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        // No menu: `arg.n == 0`, not a nil.
        assert_eq!(s.arity("GetGossipAvailableQuests()").unwrap(), 0);
        assert_eq!(s.arity("GetGossipActiveQuests()").unwrap(), 0);

        let mut m = menu();
        m.quests = vec![
            GossipQuestRow {
                title: "Report to Goldshire".into(),
                level: 5,
                active: true,
            },
            GossipQuestRow {
                title: "A Threat Within".into(),
                level: 7,
                active: false,
            },
            GossipQuestRow {
                title: "Kobold Camp Cleanup".into(),
                level: 9,
                active: false,
            },
        ];
        s.set_gossip(Some(m));

        assert_eq!(
            s.eval::<(String, i64, String, i64)>("return GetGossipAvailableQuests()")
                .unwrap(),
            (
                "A Threat Within".to_string(),
                7,
                "Kobold Camp Cleanup".to_string(),
                9
            )
        );
        // The active row comes first in the menu but splits by icon, not position.
        assert_eq!(
            s.eval::<(String, i64)>("return GetGossipActiveQuests()")
                .unwrap(),
            ("Report to Goldshire".to_string(), 5)
        );

        // Each select is a call with its own list's row; the host reads the message's rows.
        s.run(
            "SelectGossipAvailableQuest(2) SelectGossipActiveQuest(1) SelectGossipActiveQuest(0)",
        )
        .unwrap();
        assert_eq!(
            s.take_gossip_quest_selects(),
            vec![(false, 2), (true, 1), (true, 0)]
        );
        assert!(s.take_gossip_quest_selects().is_empty(), "taken");
    }

    #[test]
    fn clearing_the_menu_empties_it() {
        let mut s = UiScript::new().unwrap();
        s.set_gossip(Some(menu()));
        s.set_gossip(None);
        assert_eq!(s.eval::<String>("return GetGossipText()").unwrap(), "");
        assert_eq!(s.arity("GetGossipOptions()").unwrap(), 0);
    }
}
