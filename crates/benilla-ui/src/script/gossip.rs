//! The gossip bindings (decision 0081 phase 3) — the Era-shaped NPC-dialog surface, the same
//! two-way seam as [`super::container`]: the app pushes a **gossip menu snapshot**
//! ([`UiScript::set_gossip`] — the greeting + option rows already resolved from the wire), and the
//! Lua `SelectGossipOption`/`CloseGossip` calls queue outbound **intents** the app drains
//! ([`UiScript::take_gossip_selects`] / [`UiScript::take_gossip_close`]). The engine holds no NPC
//! knowledge — an option is "a label, an icon type, and whether it's coded".
//!
//! ## The Era API shape
//!
//! 1.12's `GetGossipOptions()` returns a flat vararg of `(text, type)` pairs — the FrameXML's
//! `GossipFrameUpdate` walks it two-at-a-time, `SetText`ing each `GossipTitleButton` and mapping the
//! `type` string to an `Interface\GossipFrame\<Type>GossipIcon` texture. Benilla keeps that exact
//! shape: `GetGossipOptions()` returns `label1, type1, label2, type2, …`, where `type` is the
//! lowercase Era icon name (`"gossip"`/`"vendor"`/`"taxi"`/`"trainer"`/…) the app derived from the
//! wire `GOSSIP_ICON` byte. The one addition is `IsGossipOptionCoded(i)` — a benilla-local
//! predicate the XML uses to grey password-gated options (decision 0081 v1: coded options are
//! parsed and greyed, never selected; the real client pops a password box, out of scope here).
//! `GetGossipText()` returns the greeting body (`SMSG_NPC_TEXT_UPDATE`), `nil` when no menu is
//! open. There is no "menu open, text pending" state: while a greeting is in flight the app
//! pushes nothing and fires nothing — the VM keeps its last menu (or none), exactly as the
//! reference's frame keeps its last paint (its handler returns on a cache miss without an event,
//! and its greeting write and `GOSSIP_SHOW` are adjacent and unconditional on one success path —
//! `0x4e2010`, `0x4e22b0`; B292, benilla decisions 1508/1994), which is why
//! [`GossipMenu::greeting`] is a plain `String`.

use mlua::{Lua, MultiValue, Value};

use super::Model;

/// One gossip menu option, resolved by the app from the wire `GossipOption` (decision 0081). Plain
/// data — 1-based order in the menu is its position in [`GossipMenu::options`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GossipOptionView {
    /// The option label (`SetText` on its row button).
    pub label: String,
    /// The Era `GetGossipOptions()` second value — the lowercase icon *type* the app mapped from
    /// the wire `GOSSIP_ICON` byte (`"gossip"`/`"vendor"`/`"taxi"`/`"trainer"`/…). The XML resolves
    /// it to a `Interface\GossipFrame\<Type>GossipIcon` texture.
    pub icon_type: String,
    /// A password-gated (`coded`) option — greyed and unselectable in v1 (decision 0081).
    pub coded: bool,
}

/// One quest row riding a gossip menu (`SMSG_GOSSIP_MESSAGE`'s quest-option block). A gossip NPC
/// that also gives quests lists them above the gossip options; a click sends
/// `CMSG_QUESTGIVER_QUERY_QUEST` (decision 0088). `active` splits the row into the "current quests"
/// vs "available quests" headers (the app derives it from the wire dialog-status icon); the app maps
/// the clicked 1-based row back to its quest id.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GossipQuestRow {
    pub title: String,
    /// The quest's level — `SMSG_GOSSIP_MESSAGE`'s own third field per quest row.
    ///
    /// The app parsed it and dropped it until 1751's twenty-first window, because the invented
    /// `GetGossipQuestInfo` had nowhere to put it. The reference's two verbs return the rows as
    /// `(title, level)` PAIRS, and stock `GossipFrame.lua` strides its walk by 2 over them — so
    /// the level is load-bearing for the stride even though 1.12's own FrameXML never reads it.
    pub level: u32,
    pub active: bool,
}

/// One open gossip menu: the greeting body, the quest rows, and the option rows. Pushed whole by
/// the app; `None` means no menu is open.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GossipMenu {
    /// The NPC greeting, always resolved: a menu is only pushed once its `SMSG_NPC_TEXT_UPDATE`
    /// answered — an open gossip frame with a blank page is not a reachable state (module doc).
    pub greeting: String,
    /// Quest rows the NPC offers/has active, riding the same packet (decision 0088).
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

    /// Whether `CloseGossip` was called since the last drain (and clear the flag). vanilla's
    /// client-side close sends no packet — the app just clears its local menu state.
    pub fn take_gossip_close(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().gossip_close)
    }

    /// Drain the 1-based quest-row positions queued by `SelectGossipQuest` since the last call. The
    /// app maps each to the row's quest id + the open NPC and sends `CMSG_QUESTGIVER_QUERY_QUEST`.
    pub fn take_gossip_quest_selects(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().gossip_quest_selects)
    }
}

/// Register the gossip globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // → the greeting body, or nil while there is no menu (an open menu always has one — the
    // app-side hold, module doc).
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

    // → flat (label, type) pairs, the Era `GetGossipOptions()` vararg shape.
    g.set(
        "GetGossipOptions",
        lua.create_function(|lua, ()| {
            // Collect owned data under one short borrow, then build the Lua values with none held.
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

    // IsGossipOptionCoded(i) — benilla-local: the XML greys a coded (password) option (decision
    // 0081). `i` is 1-based; an out-of-range index is `false`.
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

    // SelectGossipOption(i [, ...]) — queue the 1-based option position; the app maps it to the
    // wire option index + guid. Extra Era args (the code / a confirm flag) are ignored: v1 never
    // sends a code (decision 0081).
    g.set(
        "SelectGossipOption",
        lua.create_function(|lua, (i, _rest): (u32, mlua::MultiValue)| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.gossip_selects.push(i);
            Ok(())
        })?,
    )?;

    // CloseGossip() — client-side close (no packet, vanilla): flag it so the app clears its menu.
    g.set(
        "CloseGossip",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.gossip_close = true;
            Ok(())
        })?,
    )?;

    // ══ THE TWO QUEST LISTS, and why there are two ══════════════════════════════════════════
    //
    // 1.12 splits the gossip packet's quest rows by their WIRE ICON — `{3,4}` are "active", every
    // other value "available" (decision 0758) — and publishes each list through its own vararg
    // verb. The reference runs that same test lazily behind these two bindings
    // (`0x4e2430`/`0x4e2580`); we run it at parse time and keep the answer on the row.
    //
    // **Each row is a PAIR: `(title, level)`.** Stock `GossipFrameAvailableQuestsUpdate` /
    // `…ActiveQuestsUpdate` walk `for i = 1, arg.n, 2` and read only `arg[i]`, so the level is
    // never displayed in 1.12 — but it is what makes the stride land, and an addon reading the
    // second slot gets the number the server sent rather than a nil.
    //
    // These four REPLACE `GetNumGossipQuests` / `GetGossipQuestInfo` / `SelectGossipQuest`, which
    // were benilla's own single-list shape and appear nowhere in `reference/1.12-globals.tsv`.
    // 1189's rule: a superset is not free — an addon that feature-detects on a name the client
    // does not have takes a branch we cannot honour.
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

    // The two selects. Each index is 1-based **within its own list**, which is what
    // `GossipTitleButton_OnClick` passes: the reference re-numbers the rows per list as it lays
    // them out (`titleIndex`), so a menu with two available and one active quest hands the active
    // row a 1, not a 3.
    //
    // The queue the app drains is one list, so the index is mapped back to the WHOLE row order
    // here — the app's own `take_gossip_quest_selects` contract is unchanged, and the two verbs
    // stay a pure re-expression of it rather than a second drain to wire up.
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
        // No menu: text nil, no options.
        assert!(s.eval::<bool>("return GetGossipText() == nil").unwrap());
        assert_eq!(s.arity("GetGossipOptions()").unwrap(), 0);

        s.set_gossip(Some(menu()));
        assert_eq!(
            s.eval::<String>("return GetGossipText()").unwrap(),
            "Greetings, traveler."
        );
        // Flat (label, type) pairs: 2 options → 4 return values.
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
        // The coded (petition) option is flagged for greying; the vendor one isn't.
        assert!(!s.eval::<bool>("return IsGossipOptionCoded(1)").unwrap());
        assert!(s.eval::<bool>("return IsGossipOptionCoded(2)").unwrap());
        assert!(!s.eval::<bool>("return IsGossipOptionCoded(9)").unwrap()); // out of range

        // Selecting queues the 1-based position; closing flags the intent.
        s.run("SelectGossipOption(1)").unwrap();
        s.run("SelectGossipOption(2, 'unused-code')").unwrap(); // extra arg ignored
        assert_eq!(s.take_gossip_selects(), vec![1, 2]);
        assert!(s.take_gossip_selects().is_empty(), "drained");

        assert!(!s.take_gossip_close());
        s.run("CloseGossip()").unwrap();
        assert!(s.take_gossip_close());
        assert!(!s.take_gossip_close(), "drained");
    }

    /// **The two quest lists the reference publishes, and the two selects that index INTO them.**
    ///
    /// This replaces a test of `GetNumGossipQuests`/`GetGossipQuestInfo`/`SelectGossipQuest` —
    /// benilla's own single-list shape, and three names that appear nowhere in
    /// `reference/1.12-globals.tsv`. 1189's rule is why they had to go rather than sit beside the
    /// real four: an addon that feature-detects on a name this client does not have takes a branch
    /// we cannot honour.
    ///
    /// The row shape is a `(title, level)` PAIR because stock `GossipFrameAvailableQuestsUpdate`
    /// walks `for i = 1, arg.n, 2` and reads `arg[i]`. 1.12 never displays the level; it is the
    /// stride that needs it, and an addon reading the second slot should get the server's number.
    #[test]
    fn the_two_gossip_quest_lists_split_by_active_and_select_within_themselves() {
        use super::GossipQuestRow;
        let mut s = UiScript::new().unwrap();
        // No menu → both lists are empty, and that is `arg.n == 0`, not a nil.
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

        // Two available, in menu order, each a pair.
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
        // One active, and the active row does NOT appear in the available list even though it
        // comes first in the menu — the split is the wire icon, not the position.
        assert_eq!(
            s.eval::<(String, i64)>("return GetGossipActiveQuests()")
                .unwrap(),
            ("Report to Goldshire".to_string(), 5)
        );

        // **Each select is 1-based within its OWN list**, which is what
        // `GossipTitleButton_OnClick` passes (the reference re-numbers per list as it lays the
        // rows out). Available #2 is "Kobold Camp Cleanup", the THIRD row of the menu — and the
        // queue the app drains is still whole-menu positions, so it must come back as 3.
        s.run("SelectGossipAvailableQuest(2)").unwrap();
        assert_eq!(s.take_gossip_quest_selects(), vec![3]);
        // …and the single active row is #1 in its list, the FIRST of the menu.
        s.run("SelectGossipActiveQuest(1)").unwrap();
        assert_eq!(s.take_gossip_quest_selects(), vec![1]);
        // Out of range queues nothing rather than a wrong row.
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
