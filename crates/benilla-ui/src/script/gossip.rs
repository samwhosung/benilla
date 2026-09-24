//! The gossip bindings: the app pushes the open menu ([`UiScript::set_gossip`]), and
//! `SelectGossipOption` and `CloseGossip` queue intents it drains.
//!
//! `GetGossipOptions()` returns flat `(text, type)` pairs, `type` the lowercase icon name the app
//! maps from the wire `GOSSIP_ICON`. A coded option is not built: the app drops its select, where
//! the reference opens a password box (`UIParent.lua:563`). `IsGossipOptionCoded(i)` is benilla's
//! own, not a 1.12 global, and no stock file calls it.
//!
//! A menu is pushed only once its greeting (`SMSG_NPC_TEXT_UPDATE`) has arrived; until then the
//! VM keeps its last menu, as the reference's frame keeps its last paint: its handler returns on a
//! cache miss without an event, and the greeting write and `GOSSIP_SHOW` share one success path
//! (`0x4e2010`, `0x4e22b0`).

use mlua::{Lua, MultiValue, Value};

use super::Model;

/// One gossip option; its 1-based index is its place in [`GossipMenu::options`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GossipOptionView {
    /// The option's label.
    pub label: String,
    /// The lowercase icon type (`"gossip"`, `"vendor"`, `"taxi"`, …); `GossipFrame.lua:123` draws
    /// `Interface\GossipFrame\<Type>GossipIcon`.
    pub icon_type: String,
    /// A password-gated option, which the app never selects.
    pub coded: bool,
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

/// One open gossip menu, pushed whole by the app.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GossipMenu {
    /// The NPC greeting, always resolved.
    pub greeting: String,
    /// Quest rows from the same packet.
    pub quests: Vec<GossipQuestRow>,
    pub options: Vec<GossipOptionView>,
}

impl super::UiScript {
    /// Push (or clear, with `None`) the open gossip menu.
    pub fn set_gossip(&mut self, menu: Option<GossipMenu>) {
        self.model_mut().gossip = menu;
    }

    /// Drain the 1-based option positions queued by `SelectGossipOption` since the last call.
    pub fn take_gossip_selects(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().gossip_selects)
    }

    /// Whether `CloseGossip` was called since the last drain; the 1.12 close sends no packet.
    pub fn take_gossip_close(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().gossip_close)
    }

    /// Drain the queued 1-based whole-menu quest-row positions.
    pub fn take_gossip_quest_selects(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().gossip_quest_selects)
    }
}

/// Register the gossip globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    g.set(
        "GetGossipText",
        lua.create_function(|lua, ()| {
            let text = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model.gossip.as_ref().map(|m| m.greeting.clone())
            };
            match text {
                Some(t) => Ok(Value::String(lua.create_string(&t)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    g.set(
        "GetGossipOptions",
        lua.create_function(|lua, ()| {
            // Copy out under one short borrow; build the Lua values with none held.
            let pairs: Vec<(String, String)> = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model.gossip.as_ref().map_or_else(Vec::new, |m| {
                    m.options
                        .iter()
                        .map(|o| (o.label.clone(), o.icon_type.clone()))
                        .collect()
                })
            };
            let mut out: Vec<Value> = Vec::with_capacity(pairs.len() * 2);
            for (label, icon) in pairs {
                out.push(Value::String(lua.create_string(&label)?));
                out.push(Value::String(lua.create_string(&icon)?));
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    g.set(
        "IsGossipOptionCoded",
        lua.create_function(|lua, i: usize| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model
                .gossip
                .as_ref()
                .and_then(|m| i.checked_sub(1).and_then(|n| m.options.get(n)))
                .is_some_and(|o| o.coded))
        })?,
    )?;

    // SelectGossipOption(i [, ...]): queue the 1-based position. Further arguments, a code among
    // them, are ignored: coded options are not built, so no code is ever sent.
    g.set(
        "SelectGossipOption",
        lua.create_function(|lua, (i, _rest): (u32, mlua::MultiValue)| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.gossip_selects.push(i);
            Ok(())
        })?,
    )?;

    g.set(
        "CloseGossip",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.gossip_close = true;
            Ok(())
        })?,
    )?;

    // ══ The two quest lists ══
    // The reference splits the quest rows by wire icon, 3 and 4 active and the rest available,
    // behind two vararg verbs (`0x4e2430`, `0x4e2580`); the app splits them at parse time.
    fn quest_rows(lua: &Lua, active: bool) -> mlua::Result<MultiValue> {
        let rows: Vec<(String, u32)> = {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            model.gossip.as_ref().map_or_else(Vec::new, |m| {
                m.quests
                    .iter()
                    .filter(|q| q.active == active)
                    .map(|q| (q.title.clone(), q.level))
                    .collect()
            })
        };
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
    // buttons; it maps back to the whole-menu position the app's queue holds.
    fn select_quest(lua: &Lua, active: bool, index: usize) {
        let whole = {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            model.gossip.as_ref().and_then(|m| {
                m.quests
                    .iter()
                    .enumerate()
                    .filter(|(_, q)| q.active == active)
                    .nth(index.wrapping_sub(1))
                    .map(|(n, _)| n as u32 + 1)
            })
        };
        if let Some(n) = whole {
            lua.app_data_mut::<Model>()
                .expect("model app_data")
                .gossip_quest_selects
                .push(n);
        }
    }

    g.set(
        "SelectGossipAvailableQuest",
        lua.create_function(|lua, i: usize| {
            select_quest(lua, false, i);
            Ok(())
        })?,
    )?;
    g.set(
        "SelectGossipActiveQuest",
        lua.create_function(|lua, i: usize| {
            select_quest(lua, true, i);
            Ok(())
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
                    coded: false,
                },
                GossipOptionView {
                    label: "I would like to sign the petition.".into(),
                    icon_type: "gossip".into(),
                    coded: true,
                },
            ],
        }
    }

    #[test]
    fn gossip_snapshot_reads_and_selects_queue() {
        let mut s = UiScript::new().unwrap();
        assert!(s.eval::<bool>("return GetGossipText() == nil").unwrap());
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
        assert!(!s.eval::<bool>("return IsGossipOptionCoded(1)").unwrap());
        assert!(s.eval::<bool>("return IsGossipOptionCoded(2)").unwrap());
        assert!(!s.eval::<bool>("return IsGossipOptionCoded(9)").unwrap()); // out of range

        s.run("SelectGossipOption(1)").unwrap();
        s.run("SelectGossipOption(2, 'unused-code')").unwrap(); // extra arg ignored
        assert_eq!(s.take_gossip_selects(), vec![1, 2]);
        assert!(s.take_gossip_selects().is_empty(), "drained");

        assert!(!s.take_gossip_close());
        s.run("CloseGossip()").unwrap();
        assert!(s.take_gossip_close());
        assert!(!s.take_gossip_close(), "drained");
    }

    #[test]
    fn the_two_gossip_quest_lists_split_by_active_and_select_within_themselves() {
        use super::GossipQuestRow;
        let mut s = UiScript::new().unwrap();
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

        // Available #2 is the menu's third row.
        s.run("SelectGossipAvailableQuest(2)").unwrap();
        assert_eq!(s.take_gossip_quest_selects(), vec![3]);
        s.run("SelectGossipActiveQuest(1)").unwrap();
        assert_eq!(s.take_gossip_quest_selects(), vec![1]);
        s.run("SelectGossipAvailableQuest(9) SelectGossipActiveQuest(0)")
            .unwrap();
        assert!(s.take_gossip_quest_selects().is_empty(), "drained");
    }

    #[test]
    fn clearing_the_menu_empties_it() {
        let mut s = UiScript::new().unwrap();
        s.set_gossip(Some(menu()));
        s.set_gossip(None);
        assert!(s.eval::<bool>("return GetGossipText() == nil").unwrap());
        assert_eq!(s.arity("GetGossipOptions()").unwrap(), 0);
    }
}
