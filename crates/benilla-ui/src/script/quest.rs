//! The questgiver bindings: each getter reads the panel's cells through the lent host at the call
//! ([`QuestGiverQuery`]), as the reference's read its globals (`0x501a20`-`0x502090`), and each
//! panel verb hands the host its call ([`QuestCall`]), which applies it before the verb returns.
//! One [`QuestState`] holds every cell `QuestFrame.lua` reads; the app fires `QUEST_GREETING`,
//! `QUEST_DETAIL`, `QUEST_PROGRESS` or `QUEST_COMPLETE` at each panel's packet. With no host the
//! getters read the VM's copy ([`UiScript::set_quest`]), which only a harness sets.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::{self, flag};
use super::live::{ask, LiveQuery};
use super::Model;

/// Which questgiver panel a [`QuestState`] is for, set from the wire packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuestPanel {
    /// `SMSG_QUESTGIVER_QUEST_LIST`: the greeting, with the active and available lists.
    Greeting,
    /// `SMSG_QUESTGIVER_QUEST_DETAILS`: the accept panel.
    Detail,
    /// `SMSG_QUESTGIVER_REQUEST_ITEMS`: the turn-in progress panel.
    Progress,
    /// `SMSG_QUESTGIVER_OFFER_REWARD`: the reward panel.
    Reward,
}

/// One choice, reward or required item row; its 1-based index is its place in its list.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QuestItemView {
    /// `None` while the item template is in flight: `GetQuestItemInfo` answers an empty name, the
    /// log getters their miss.
    pub name: Option<String>,
    pub texture: Option<String>,
    pub count: u32,
    /// 0 poor to 5 legendary, -1 for an item with no inventory type; 0 until the template lands.
    pub quality: i32,
    /// The item id the quest tooltips render by; 0 until the wire row resolves.
    pub item_id: u32,
    /// The escaped item link `GetQuestItemLink` and `GetQuestLogItemLink` serve; `None` until the
    /// template lands, since it embeds the name and the quality.
    pub link: Option<String>,
}

/// The reward spell, `rewSpell` on the giver packets and `SMSG_QUEST_QUERY_RESPONSE`, as
/// `GetRewardSpell` and `GetQuestLogRewardSpell` answer it; stock counts it as one more reward slot
/// (`QuestFrame.lua:332`). `tradeskill` is the third return, `isTradeskillSpell`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QuestRewardSpell {
    pub spell_id: u32,
    pub name: Option<String>,
    pub texture: Option<String>,
    pub tradeskill: bool,
}

/// The questgiver panel's cells as the getters read them: each written by its own panel's packet
/// and kept until another writes it, past a close (`0x500bd0`, `0x500ef0`, `0x501070`).
#[derive(Clone, Debug, PartialEq)]
pub struct QuestState {
    pub panel: QuestPanel,
    /// `0xbde068`, the greeting's text.
    pub greeting: String,
    /// The greeting's two pools, each row's title at `+0x08` and level at `+0x04` of one 0x4c
    /// record (`0xbde690` active, `0xbdfe60` available), refilled by the next greeting alone.
    pub active_titles: Vec<String>,
    pub available_titles: Vec<String>,
    /// A pool built without levels reads 0.
    pub active_levels: Vec<u32>,
    pub available_levels: Vec<u32>,
    /// `0xbdf900`.
    pub title: String,
    /// The three panel texts, each its own buffer (`0xbde288`, `0xbdf0f8`, `0xbdf958`).
    pub quest_text: String,
    pub progress_text: String,
    pub reward_text: String,
    /// `0xbdf500`.
    pub objectives: String,
    /// Rewards the player picks one of.
    pub choices: Vec<QuestItemView>,
    /// Rewards all granted.
    pub rewards: Vec<QuestItemView>,
    /// Items the progress panel asks for.
    pub required: Vec<QuestItemView>,
    /// In copper.
    pub reward_money: u32,
    /// In copper.
    pub required_money: u32,
    pub completable: bool,
    pub reward_spell: Option<QuestRewardSpell>,
    /// `GetQuestBackgroundMaterial`, which the app fills from an item or GameObject source, never
    /// from the wire; nil reads as "Parchment" (`QuestFrame.lua:598-604`).
    pub background_material: Option<String>,
}

impl Default for QuestState {
    fn default() -> Self {
        QuestState {
            panel: QuestPanel::Detail,
            greeting: String::new(),
            active_titles: Vec::new(),
            available_titles: Vec::new(),
            active_levels: Vec::new(),
            available_levels: Vec::new(),
            title: String::new(),
            quest_text: String::new(),
            progress_text: String::new(),
            reward_text: String::new(),
            objectives: String::new(),
            choices: Vec::new(),
            rewards: Vec::new(),
            required: Vec::new(),
            reward_money: 0,
            required_money: 0,
            completable: false,
            reward_spell: None,
            background_material: None,
        }
    }
}

/// The panel's cells as the host holds them at the call: the item rows resolved off the item cache
/// then (`0x5017de`, `0x502114`) and `completable` the packet's flag with the player's held counts
/// (`0x501230`).
#[derive(Clone, Copy, Debug)]
pub struct QuestGiverQuery;

impl LiveQuery for QuestGiverQuery {
    type Answer = QuestState;
}

/// The cells now: the host's, else the VM's copy.
pub(crate) fn state(lua: &Lua) -> Option<QuestState> {
    ask(lua, &QuestGiverQuery).or_else(|| {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        model.quest.clone()
    })
}

/// A greeting row picked by `SelectActiveQuest` or `SelectAvailableQuest`, 1-based in its list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuestSelect {
    pub active: bool,
    pub index: u32,
}

/// A questgiver, quest-log or share verb, which the host applies inside the call: its gates and
/// its packet, before the verb returns, as the reference's bindings do in their own bodies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuestCall {
    /// `SelectActiveQuest(i)` (`0x501ce0` → `0x501320`) or `SelectAvailableQuest(i)`
    /// (`0x501ca0` → `0x5012a0`): `CMSG_QUESTGIVER_COMPLETE_QUEST`, or `_QUERY_QUEST` for an
    /// available row that is not one-click, and the already-acted latch.
    Select(QuestSelect),
    /// `AcceptQuest()` (`0x501d20` → `0x501380`): on the detail panel, `CMSG_QUESTGIVER_ACCEPT_QUEST`,
    /// the latch, the close `0x501130(0,0)` and the Quest Log tutorial.
    Accept,
    /// `DeclineQuest()` (`0x501d30` → `0x5013f0`): `CMSG_GOSSIP_HELLO` for a gossip unit,
    /// `CMSG_QUESTGIVER_HELLO` for another unit, the teardown `0x501130(0,1)` for an item or a
    /// player (or with `0xbe0824` set), a GameObject's interact call.
    Decline,
    /// `CompleteQuest()` (`0x501d70` → `0x501560`): on the progress panel,
    /// `CMSG_QUESTGIVER_REQUEST_REWARD` and the latch.
    Complete,
    /// `GetQuestReward([choice])` (`0x501d80` → `0x5015b0`), the choice less one, -1 for none
    /// (`0x501d89`-`0x501da8`): on the reward panel, the latch, the choice's range, which raises
    /// from the verb, and `CMSG_QUESTGIVER_CHOOSE_REWARD`.
    Reward(i32),
    /// `CloseQuest()` (`0x501a10` → `0x501130(0,1)`): a sharer's decline, the zeroed cell
    /// (`0x501201`), the latch's release (`0x50120d`) and `QUEST_FINISHED` (`0x501217`), in the
    /// call.
    Close,
    /// `ConfirmAcceptQuest()` (`0x5021b0`): `CMSG_QUEST_CONFIRM_ACCEPT` with the quest the last
    /// confirm held (`[0xbe0840]`, `0x5021e0`), an engine cell the host keeps.
    ConfirmAccept,
    /// `AbandonQuest()` on the marked quest's row (`0x4df070`): `CMSG_QUESTLOG_REMOVE_QUEST` with
    /// its slot (`0x4df0ca`).
    Abandon(u8),
    /// `QuestLogPushQuest()` past its gates (`0x4e1340`): `CMSG_PUSHQUESTTOPARTY` (`0x4e13b7`).
    Push(u32),
}

/// Hand `call` to the host, which applies it before this returns ([`super::calls::submit`]).
pub(super) fn submit(lua: &Lua, call: QuestCall) -> mlua::Result<()> {
    super::calls::submit(lua, super::ScriptCall::Quest(call))
}

impl super::UiScript {
    /// Set (or clear, with `None`) the VM's copy of the cells, which the getters read with no host
    /// lent: a harness's alone.
    pub fn set_quest(&mut self, state: Option<QuestState>) {
        self.model_mut().quest = state;
    }

    /// The quest calls a harness's VM kept ([`Self::keep_calls`]), in call order.
    pub fn take_quest_calls(&mut self) -> Vec<QuestCall> {
        self.take_calls_where(|call| match call {
            super::ScriptCall::Quest(call) => Some(*call),
            _ => None,
        })
    }
}

/// `GetRewardSpell`'s three returns: `texture, name, isTradeskillSpell`, the third 1 or nil.
pub(super) fn reward_spell_returns(
    lua: &Lua,
    spell: Option<QuestRewardSpell>,
) -> mlua::Result<MultiValue> {
    let Some(sp) = spell else {
        return Ok(MultiValue::from_vec(vec![
            Value::Nil,
            Value::Nil,
            Value::Nil,
        ]));
    };
    let texture = match &sp.texture {
        Some(t) => Value::String(lua.create_string(t)?),
        None => Value::Nil,
    };
    let name = match &sp.name {
        Some(n) => Value::String(lua.create_string(n)?),
        None => Value::Nil,
    };
    let tradeskill = if sp.tradeskill {
        Value::Integer(1)
    } else {
        Value::Nil
    };
    Ok(MultiValue::from_vec(vec![texture, name, tradeskill]))
}

/// The icon an empty offer row answers: display 0 has no `ItemDisplayInfo` row, so the icon
/// lookup falls back to the question mark (`0x5d88b0`, `0x847fe4`) under the icon folder.
const EMPTY_ROW_ICON: &str = "Interface\\Icons\\INV_Misc_QuestionMark";

/// The list `GetQuestItemInfo(type, index)` reads for `type`, matched in any case (`SStrCmpI`,
/// `0x501936`, `0x501957`, `0x501978`).
pub(super) fn item_vec<'a>(state: &'a QuestState, kind: &str) -> Option<&'a Vec<QuestItemView>> {
    [
        ("reward", &state.rewards),
        ("choice", &state.choices),
        ("required", &state.required),
    ]
    .into_iter()
    .find(|(name, _)| kind.eq_ignore_ascii_case(name))
    .map(|(_, list)| list)
}

/// Register the questgiver globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // ── Text getters ──
    fn install_text(lua: &Lua, name: &str, pick: fn(&QuestState) -> String) -> mlua::Result<()> {
        lua.globals().set(
            name,
            lua.create_function(move |lua, ()| {
                let text = state(lua).as_ref().map(pick).unwrap_or_default();
                Ok(Value::String(lua.create_string(&text)?))
            })?,
        )
    }
    install_text(lua, "GetGreetingText", |q| q.greeting.clone())?;
    install_text(lua, "GetTitleText", |q| q.title.clone())?;
    // Each reads its own buffer (`0x501a40`, `0x501a60`, `0x501a70`).
    install_text(lua, "GetQuestText", |q| q.quest_text.clone())?;
    install_text(lua, "GetProgressText", |q| q.progress_text.clone())?;
    install_text(lua, "GetRewardText", |q| q.reward_text.clone())?;
    install_text(lua, "GetObjectiveText", |q| q.objectives.clone())?;

    // ── Count and money getters ──
    fn install_count(lua: &Lua, name: &str, pick: fn(&QuestState) -> i64) -> mlua::Result<()> {
        lua.globals().set(
            name,
            lua.create_function(move |lua, ()| Ok(state(lua).as_ref().map(pick).unwrap_or(0)))?,
        )
    }
    install_count(lua, "GetNumActiveQuests", |q| q.active_titles.len() as i64)?;
    install_count(lua, "GetNumAvailableQuests", |q| {
        q.available_titles.len() as i64
    })?;
    install_count(lua, "GetNumQuestChoices", |q| q.choices.len() as i64)?;
    install_count(lua, "GetNumQuestRewards", |q| q.rewards.len() as i64)?;
    install_count(lua, "GetNumQuestItems", |q| q.required.len() as i64)?;
    install_count(lua, "GetRewardMoney", |q| i64::from(q.reward_money))?;
    install_count(lua, "GetQuestMoneyToGet", |q| i64::from(q.required_money))?;

    // 1 or nil (`0x501d40`): the host's answer counts the required items at the call (`0x501230`).
    g.set(
        "IsQuestCompletable",
        lua.create_function(|lua, ()| Ok(flag(state(lua).is_some_and(|q| q.completable))))?,
    )?;

    fn install_title(lua: &Lua, name: &str, usage: &'static str, active: bool) -> mlua::Result<()> {
        lua.globals().set(
            name,
            lua.create_function(move |lua, i: Value| {
                let i = binding_abi::index_arg(lua, i, usage)?;
                let title = state(lua).and_then(|q| {
                    let v = if active {
                        q.active_titles
                    } else {
                        q.available_titles
                    };
                    i.checked_sub(1).and_then(|n| v.into_iter().nth(n))
                });
                // Past the pool a null string, which is nil (`jae` at `0x501ae9`, `0x501b05`).
                match title {
                    Some(t) => Ok(Value::String(lua.create_string(&t)?)),
                    None => Ok(Value::Nil),
                }
            })?,
        )
    }
    // Each `is-number` else its own usage line (`0x84ecfc` at `0x501b87`, `0x84ecdc` at
    // `0x501b17`).
    install_title(lua, "GetActiveTitle", "Usage: GetActiveTitle(index)", true)?;
    install_title(
        lua,
        "GetAvailableTitle",
        "Usage: GetAvailableTitle(index)",
        false,
    )?;

    // GetActiveLevel(index) (`0x501c20`) and GetAvailableLevel(index) (`0x501ba0`): the level
    // stored
    // beside the greeting row, 0 for an index outside the pool (`dec`, then an unsigned compare
    // against the count, so 0 and negatives miss too). The pools hold the rows of the last
    // greeting, whether or not its panel is still open, and 0 before any. The usage texts are the
    // exe's own,
    // `Get` twice (`0x84ed40`, `0x84ed1c`).
    fn install_level(lua: &Lua, name: &str, usage: &'static str, active: bool) -> mlua::Result<()> {
        lua.globals().set(
            name,
            lua.create_function(move |lua, index: Value| {
                let row = binding_abi::number_arg(lua, index, usage)?.wrapping_sub(1) as u32;
                let level = state(lua).map_or(0, |q| {
                    let (titles, levels) = if active {
                        (q.active_titles, q.active_levels)
                    } else {
                        (q.available_titles, q.available_levels)
                    };
                    let row = row as usize;
                    if row < titles.len() {
                        levels.get(row).copied().unwrap_or(0)
                    } else {
                        0
                    }
                });
                // The level is pushed through `fild` of a signed dword.
                Ok(i64::from(level as i32))
            })?,
        )
    }
    install_level(
        lua,
        "GetActiveLevel",
        "Usage: GetGetActiveLevel(index)",
        true,
    )?;
    install_level(
        lua,
        "GetAvailableLevel",
        "Usage: GetGetAvailableLevel(index)",
        false,
    )?;

    // GetQuestItemInfo(type, index) → name, texture, numItems, quality, isUsable (`0x501f80`): five
    // values, `isUsable` 1 or nil. A type other than `"reward"`, `"choice"` or `"required"` (any
    // case, `0x64a4c0`) or an index outside 1..7 (`0x501719`) raises `0x84edd8`, as does a
    // non-string type or non-number index. An index inside that range with no item answers the
    // zeroed row the offer publisher leaves (`0x500ef0`): no name, the question-mark icon of
    // display 0, count 0, quality 0 and usable. A template in flight answers an empty name.
    g.set(
        "GetQuestItemInfo",
        lua.create_function(|lua, (kind, index): (Value, Value)| {
            let invalid =
                || mlua::Error::runtime(r#"Invalid quest item in GetQuestItemInfo("type", index)"#);
            let kind = match &kind {
                Value::String(_) | Value::Integer(_) | Value::Number(_) => lua
                    .coerce_string(kind.clone())?
                    .map(|s| s.to_string_lossy().to_ascii_lowercase()),
                _ => None,
            }
            .ok_or_else(invalid)?;
            let index = lua.coerce_number(index)?.ok_or_else(invalid)?;
            // `_ftol` then `dec`, compared unsigned against 6.
            let row = index.trunc() as i64 - 1;
            if !(0..=6).contains(&row) {
                return Err(invalid());
            }
            let item = {
                let empty = QuestState::default();
                let state = state(lua);
                let Some(list) = item_vec(state.as_ref().unwrap_or(&empty), &kind) else {
                    return Err(invalid());
                };
                list.get(row as usize).cloned()
            };
            // The lookup (`0x5017de`) of a template in flight asks it and keeps the callback
            // `0x5018c0` on it inside the call, one per call; item id 0 is never looked up.
            if let Some(it) = item
                .as_ref()
                .filter(|it| it.name.is_none() && it.item_id != 0)
            {
                super::calls::ask(
                    lua,
                    super::RecordAsk::Item {
                        entry: it.item_id,
                        // The panel's source (`[0xbe0810]`), which the host holds.
                        owner: 0,
                        callback: super::ItemCallback::Quest,
                    },
                )?;
            }
            let (name, texture, count, quality, usable) = match item {
                Some(it) => {
                    // `isUsable` seeds 1 (`0x50172e`) and the item-usable predicate clears it for
                    // a cached template it refuses (`0x501882`-`0x501899`); the stock frame tints
                    // the row red by it (`QuestFrame.lua:393`).
                    let usable = it.name.is_none() || {
                        let model = lua.app_data_ref::<Model>().expect("model app_data");
                        super::item_stats::item_usable_by_id(lua, &model, it.item_id)
                    };
                    (
                        it.name.unwrap_or_default(),
                        it.texture,
                        it.count,
                        it.quality,
                        usable,
                    )
                }
                None => (String::new(), Some(EMPTY_ROW_ICON.to_string()), 0, 0, true),
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&name)?),
                match texture {
                    Some(t) => Value::String(lua.create_string(&t)?),
                    None => Value::Nil,
                },
                Value::Integer(i64::from(count)),
                Value::Integer(i64::from(quality)),
                super::binding_abi::flag(usable),
            ]))
        })?,
    )?;

    // GetQuestItemLink(type, index), for a row's ctrl and shift clicks (`QuestFrame.lua:118`):
    // `lua_isstring(1)` and `lua_isnumber(2)`, else its usage (`0x5020b5`, `0x5020c9`, `0x50217f`).
    g.set(
        "GetQuestItemLink",
        lua.create_function(|lua, (kind, index): (Value, Value)| {
            const USAGE: &str = "Usage: GetQuestItemLink(\"type\", index)";
            let kind = binding_abi::string_arg(lua, kind, USAGE)?;
            // `index - 1` is compared unsigned against 6 (`0x501924`), so a zero or negative index
            // is no item; every miss pushes one nil (`0x50216a`).
            let index = binding_abi::index_arg(lua, index, USAGE)?;
            let link = state(lua).and_then(|q| {
                item_vec(&q, &kind)
                    .and_then(|v| index.checked_sub(1).and_then(|n| v.get(n)))
                    .and_then(|it| it.link.clone())
            });
            match link {
                Some(l) => Ok(Value::String(lua.create_string(&l)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // GetRewardSpell() (`0x501df0`): three nils without a spell.
    g.set(
        "GetRewardSpell",
        lua.create_function(|lua, ()| {
            reward_spell_returns(lua, state(lua).and_then(|q| q.reward_spell))
        })?,
    )?;

    // ── Verbs ──
    // The row is a number or numeric string (`0x501ca8`, `0x501ce8`) truncated (`_ftol`,
    // `0x501cbd`), else the binding's usage raises; the host takes it less one (`0x501cc4`,
    // `0x501d04`).
    fn install_select(
        lua: &Lua,
        name: &str,
        active: bool,
        usage: &'static str,
    ) -> mlua::Result<()> {
        lua.globals().set(
            name,
            lua.create_function(move |lua, i: Value| {
                let index = binding_abi::index_arg32(lua, i, usage)?;
                submit(lua, QuestCall::Select(QuestSelect { active, index }))
            })?,
        )
    }
    install_select(
        lua,
        "SelectActiveQuest",
        true,
        "Usage: SelectActiveQuest(index)",
    )?;
    install_select(
        lua,
        "SelectAvailableQuest",
        false,
        "Usage: SelectAvailableQuest(index)",
    )?;

    fn install_verb(lua: &Lua, name: &str, call: QuestCall) -> mlua::Result<()> {
        lua.globals()
            .set(name, lua.create_function(move |lua, ()| submit(lua, call))?)
    }
    install_verb(lua, "AcceptQuest", QuestCall::Accept)?;
    install_verb(lua, "DeclineQuest", QuestCall::Decline)?;
    install_verb(lua, "CompleteQuest", QuestCall::Complete)?;

    // CloseQuest(), from `QuestFrame_OnHide`: ESC, the close button, the greeting's Goodbye, a
    // panel eviction. Its binding (`0x501a10`) calls only the teardown `0x501130(0,1)`, whose one
    // send is `MSG_QUEST_PUSH_RESULT` DECLINE for a player source.
    g.set(
        "CloseQuest",
        lua.create_function(|lua, ()| submit(lua, QuestCall::Close))?,
    )?;

    // GetQuestBackgroundMaterial() → nil or a material name (`0x502230`).
    g.set(
        "GetQuestBackgroundMaterial",
        lua.create_function(
            |lua, ()| match state(lua).and_then(|q| q.background_material) {
                Some(m) => Ok(Value::String(lua.create_string(&m)?)),
                None => Ok(Value::Nil),
            },
        )?,
    )?;

    // QuestChooseRewardError() (`0x5021a0`), no returns: Complete pressed with choices and none
    // picked (`QuestFrame.lua:98`). It shows game error `0x98` through `0x496720`: the sound
    // `igQuestFailed`, then `ERR_QUEST_MUST_CHOOSE` through `UI_ERROR_MESSAGE`, synchronously.
    g.set(
        "QuestChooseRewardError",
        lua.create_function(|lua, ()| {
            {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                model
                    .sound_queue
                    .push(super::SoundRequest::KitName("igQuestFailed".to_string()));
            }
            let text: String = lua
                .globals()
                .get::<Option<String>>("ERR_QUEST_MUST_CHOOSE")?
                .unwrap_or_else(|| "You must choose a reward.".to_string());
            super::tick::fire_event_into(
                lua,
                "UI_ERROR_MESSAGE",
                vec![super::ScriptValue::Str(text)],
            );
            Ok(())
        })?,
    )?;

    // ConfirmAcceptQuest(): the escort confirm's Yes (`StaticPopup.lua:731-733`), which answers the
    // confirm the client holds (`[0xbe0840]`, `0x5021e0`), with no test.
    install_verb(lua, "ConfirmAcceptQuest", QuestCall::ConfirmAccept)?;

    // GetQuestReward([choice]): stock passes the 1-based row (`QuestFrame.lua:100`). A number is
    // `trunc(n) - 1` (`0x501da8`), read as signed; an absent or non-number one leaves -1
    // (`0x501d89`). The host runs `0x5015b0` on it, whose range failure raises here.
    g.set(
        "GetQuestReward",
        lua.create_function(|lua, choice: Option<Value>| {
            let choice = match choice {
                Some(v) => lua
                    .coerce_number(v)?
                    .map_or(-1, |n| super::binding_abi::ftol(n).wrapping_sub(1)),
                None => -1,
            };
            submit(lua, QuestCall::Reward(choice))
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{QuestCall, QuestItemView, QuestPanel, QuestSelect, QuestState};
    use crate::script::UiScript;

    fn detail() -> QuestState {
        QuestState {
            panel: QuestPanel::Detail,
            title: "A Threat Within".into(),
            quest_text: "Kill the kobolds infesting the mine.".into(),
            objectives: "Slay 10 Kobold Vermin.".into(),
            rewards: vec![QuestItemView {
                item_id: 7278,
                name: Some("Brdle Leather Boots".into()),
                texture: Some("Interface\\Icons\\INV_Boots_01".into()),
                count: 1,
                quality: 2,
                link: Some("|cff1eff00|Hitem:7278:0:0:0|h[Brdle Leather Boots]|h|r".into()),
            }],
            choices: vec![
                QuestItemView {
                    item_id: 0,
                    name: Some("Cudgel".into()),
                    texture: None,
                    count: 1,
                    quality: 1,
                    ..Default::default()
                },
                QuestItemView {
                    item_id: 0,
                    name: None, // template still in flight
                    texture: None,
                    count: 1,
                    quality: 1,
                    ..Default::default()
                },
            ],
            reward_money: 1234,
            ..Default::default()
        }
    }

    #[test]
    fn background_material_answers_the_pushed_material_or_nil() {
        let mut s = UiScript::new().unwrap();
        s.set_quest(Some(QuestState::default()));
        assert!(s
            .eval::<bool>("return GetQuestBackgroundMaterial() == nil")
            .unwrap());
        s.set_quest(Some(QuestState {
            background_material: Some("Stone".into()),
            ..QuestState::default()
        }));
        assert_eq!(
            s.eval::<String>("return GetQuestBackgroundMaterial()")
                .unwrap(),
            "Stone"
        );
    }

    #[test]
    fn choose_reward_error_fires_the_error_event_with_the_global_string() {
        let mut s = UiScript::new().unwrap();
        s.run(
            "ERR_QUEST_MUST_CHOOSE = 'Pick one.' \
             local f = CreateFrame('Frame') f:RegisterEvent('UI_ERROR_MESSAGE') \
             f:SetScript('OnEvent', function() SEEN = arg1 end) \
             N = table.getn({QuestChooseRewardError()})",
        )
        .unwrap();
        assert_eq!(s.eval::<String>("return SEEN").unwrap(), "Pick one.");
        assert_eq!(s.eval::<i64>("return N").unwrap(), 0, "zero return values");
        assert_eq!(
            s.take_sounds(),
            vec![crate::script::SoundRequest::KitName("igQuestFailed".into())],
            "row 0x98's cue plays with the message"
        );
    }

    /// `GetQuestItemInfo` answers `isUsable` through the item-usable predicate for a cached
    /// template (`0x50188d` into `0x5ea930`): nil for one the gate refuses. A row whose template
    /// is in flight stays usable, as `0x501700` seeds it (`0x50172e`).
    #[test]
    fn a_giver_row_answers_the_usable_gate_and_one_in_flight_reads_usable() {
        let mut s = UiScript::new().unwrap();
        s.set_player_req_state(crate::script::PlayerReqState {
            level: 10,
            class_id: 1,
            race_id: 1,
            ..Default::default()
        });
        s.set_item_template(
            7278,
            crate::script::ItemTemplateView {
                name: "Brdle Leather Boots".into(),
                quality: 2,
                inventory_type: 8,
                required_level: 20,
                allowable_class: -1,
                allowable_race: -1,
                ..Default::default()
            },
        );
        s.set_quest(Some(detail()));
        assert!(s
            .eval::<bool>(r#"local _, _, _, _, u = GetQuestItemInfo("reward", 1) return u == nil"#)
            .unwrap());
        assert!(s
            .eval::<bool>(r#"local _, _, _, _, u = GetQuestItemInfo("choice", 2) return u == 1"#)
            .unwrap());
    }

    #[test]
    fn detail_panel_getters_read() {
        let mut s = UiScript::new().unwrap();
        // No window: text empty, counts zero.
        assert_eq!(s.eval::<String>("return GetTitleText()").unwrap(), "");
        assert_eq!(s.eval::<i64>("return GetNumQuestChoices()").unwrap(), 0);

        s.set_quest(Some(detail()));
        assert_eq!(
            s.eval::<String>("return GetTitleText()").unwrap(),
            "A Threat Within"
        );
        assert_eq!(
            s.eval::<String>("return GetObjectiveText()").unwrap(),
            "Slay 10 Kobold Vermin."
        );
        assert_eq!(s.eval::<i64>("return GetNumQuestChoices()").unwrap(), 2);
        assert_eq!(s.eval::<i64>("return GetNumQuestRewards()").unwrap(), 1);
        assert_eq!(s.eval::<i64>("return GetRewardMoney()").unwrap(), 1234);

        // Five values, `isUsable` 1 or nil (`0x501f80`), the type matched in any case.
        assert_eq!(s.arity(r#"GetQuestItemInfo("reward", 1)"#).unwrap(), 5);
        let (name, texture, count, quality, usable) = s
            .eval::<(String, String, i64, i64, i64)>(r#"return GetQuestItemInfo("REWARD", 1)"#)
            .unwrap();
        assert_eq!(name, "Brdle Leather Boots");
        assert_eq!(texture, "Interface\\Icons\\INV_Boots_01");
        assert_eq!((count, quality, usable), (1, 2, 1));

        // Choice row 2 is in flight: an empty name, the rest present.
        assert!(s
            .eval::<bool>(
                "local n, t, c = GetQuestItemInfo(\"choice\", 2)\n\
                 return n == '' and t == nil and c == 1"
            )
            .unwrap());
        // Inside 1..7 with no item: the zeroed row, display 0's question mark.
        assert_eq!(
            s.eval::<(String, String, i64, i64, i64)>(r#"return GetQuestItemInfo("reward", 7)"#)
                .unwrap(),
            (
                String::new(),
                "Interface\\Icons\\INV_Misc_QuestionMark".to_string(),
                0,
                0,
                1
            )
        );
        // Outside 1..7, an unknown type or a missing argument raises `0x84edd8`.
        for call in [
            r#"GetQuestItemInfo("reward", 8)"#,
            r#"GetQuestItemInfo("reward", 0)"#,
            r#"GetQuestItemInfo("bogus", 1)"#,
            r#"GetQuestItemInfo("reward")"#,
            "GetQuestItemInfo()",
        ] {
            let e = s.run(call).expect_err(call).to_string();
            assert!(
                e.contains(r#"Invalid quest item in GetQuestItemInfo("type", index)"#),
                "{call}: {e}"
            );
        }

        // The link is nil for the in-flight row, out of range, and for an unknown type.
        assert_eq!(
            s.eval::<String>("return GetQuestItemLink(\"reward\", 1)")
                .unwrap(),
            "|cff1eff00|Hitem:7278:0:0:0|h[Brdle Leather Boots]|h|r"
        );
        assert!(s
            .eval::<bool>("return GetQuestItemLink(\"choice\", 2) == nil")
            .unwrap());
        assert!(s
            .eval::<bool>("return GetQuestItemLink(\"reward\", 9) == nil")
            .unwrap());
        assert!(s
            .eval::<bool>("return GetQuestItemLink(\"bogus\", 1) == nil")
            .unwrap());
    }

    /// `GetQuestItemLink` matches its type in any case (`SStrCmpI`, `0x501936`-`0x501978`), and a
    /// non-string type or non-number index raises its own usage (`0x5020b5`, `0x50217f`).
    #[test]
    fn the_item_link_type_is_any_case_and_its_arguments_raise_the_usage() {
        let mut s = UiScript::new().unwrap();
        s.set_quest(Some(detail()));
        assert_eq!(
            s.eval::<String>("return GetQuestItemLink(\"Reward\", 1)")
                .unwrap(),
            "|cff1eff00|Hitem:7278:0:0:0|h[Brdle Leather Boots]|h|r"
        );
        for call in [
            "GetQuestItemLink(nil, 1)",
            "GetQuestItemLink({}, 1)",
            "GetQuestItemLink(\"reward\")",
            "GetQuestItemLink(\"reward\", \"x\")",
        ] {
            let e = s.run(call).expect_err(call).to_string();
            assert!(
                e.contains(r#"Usage: GetQuestItemLink("type", index)"#),
                "{call}: {e}"
            );
        }
    }

    /// The three texts are three buffers (`0xbde288`, `0xbdf0f8`, `0xbdf958`): each getter reads
    /// its own.
    #[test]
    fn each_panel_text_getter_reads_its_own_buffer() {
        let mut s = UiScript::new().unwrap();
        s.set_quest(Some(QuestState {
            panel: QuestPanel::Progress,
            quest_text: "Q".into(),
            progress_text: "P".into(),
            reward_text: "R".into(),
            ..Default::default()
        }));
        assert_eq!(
            s.eval::<(String, String, String)>(
                "return GetQuestText(), GetProgressText(), GetRewardText()"
            )
            .unwrap(),
            ("Q".into(), "P".into(), "R".into())
        );
    }

    #[test]
    fn greeting_panel_lists_and_selects() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.set_quest(Some(QuestState {
            panel: QuestPanel::Greeting,
            greeting: "What can I do for you?".into(),
            active_titles: vec!["Report to Goldshire".into()],
            available_titles: vec!["A Threat Within".into(), "Kobold Camp Cleanup".into()],
            ..Default::default()
        }));
        assert_eq!(
            s.eval::<String>("return GetGreetingText()").unwrap(),
            "What can I do for you?"
        );
        assert_eq!(s.eval::<i64>("return GetNumActiveQuests()").unwrap(), 1);
        assert_eq!(s.eval::<i64>("return GetNumAvailableQuests()").unwrap(), 2);
        assert_eq!(
            s.eval::<String>("return GetActiveTitle(1)").unwrap(),
            "Report to Goldshire"
        );
        assert_eq!(
            s.eval::<String>("return GetAvailableTitle(2)").unwrap(),
            "Kobold Camp Cleanup"
        );
        // Past the pool a null string, nil (`0x501ae9`, `0x501b05`).
        for call in [
            "GetActiveTitle(2)",
            "GetAvailableTitle(0)",
            "GetAvailableTitle(3)",
        ] {
            assert!(
                s.eval::<bool>(&format!("return {call} == nil")).unwrap(),
                "{call}"
            );
        }

        s.run("SelectActiveQuest(1)").unwrap();
        s.run("SelectAvailableQuest(2)").unwrap();
        assert_eq!(
            s.take_quest_calls(),
            vec![
                QuestCall::Select(QuestSelect {
                    active: true,
                    index: 1
                }),
                QuestCall::Select(QuestSelect {
                    active: false,
                    index: 2
                }),
            ]
        );
        assert!(s.take_quest_calls().is_empty(), "taken");
    }

    /// `GetActiveLevel` / `GetAvailableLevel` (`0x501c20`, `0x501ba0`): the level stored beside
    /// the greeting row, 0 outside the pool or with no greeting, and the exe's own usage texts.
    #[test]
    fn greeting_levels_answer_per_pool_and_zero_outside_it() {
        let mut s = UiScript::new().unwrap();
        for (call, usage) in [
            ("GetActiveLevel()", "Usage: GetGetActiveLevel(index)"),
            ("GetActiveLevel(nil)", "Usage: GetGetActiveLevel(index)"),
            ("GetActiveLevel('x')", "Usage: GetGetActiveLevel(index)"),
            ("GetAvailableLevel()", "Usage: GetGetAvailableLevel(index)"),
            (
                "GetAvailableLevel({})",
                "Usage: GetGetAvailableLevel(index)",
            ),
        ] {
            let err = s.run(call).expect_err(call).to_string();
            assert!(err.contains(usage), "{call}: {err}");
        }
        // No greeting open: every index reads 0.
        assert_eq!(s.eval::<i64>("return GetActiveLevel(1)").unwrap(), 0);
        s.set_quest(Some(QuestState {
            panel: QuestPanel::Greeting,
            active_titles: vec!["Report to Goldshire".into(), "Wolves".into()],
            active_levels: vec![1, 7],
            available_titles: vec!["Kobold Camp Cleanup".into()],
            available_levels: vec![3],
            ..QuestState::default()
        }));
        for (call, want) in [
            ("GetActiveLevel(1)", 1),
            ("GetActiveLevel(2)", 7),
            ("GetActiveLevel('2')", 7),
            ("GetActiveLevel(2.9)", 7),
            ("GetActiveLevel(3)", 0),
            ("GetActiveLevel(0)", 0),
            ("GetActiveLevel(-1)", 0),
            ("GetAvailableLevel(1)", 3),
            ("GetAvailableLevel(2)", 0),
        ] {
            assert_eq!(
                s.eval::<i64>(&format!("return {call}")).unwrap(),
                want,
                "{call}"
            );
        }
        s.set_quest(Some(QuestState {
            panel: QuestPanel::Greeting,
            active_titles: vec!["Another".into()],
            active_levels: vec![9],
            ..QuestState::default()
        }));
        assert_eq!(s.eval::<i64>("return GetActiveLevel(1)").unwrap(), 9);
        assert_eq!(s.eval::<i64>("return GetActiveLevel(2)").unwrap(), 0);
        // A level of the wire's top bit reads as the signed dword the reference pushes.
        s.set_quest(Some(QuestState {
            panel: QuestPanel::Greeting,
            active_titles: vec!["Odd".into()],
            active_levels: vec![u32::MAX],
            ..QuestState::default()
        }));
        assert_eq!(s.eval::<i64>("return GetActiveLevel(1)").unwrap(), -1);
    }

    #[test]
    fn progress_panel_completability() {
        let mut s = UiScript::new().unwrap();
        s.set_quest(Some(QuestState {
            panel: QuestPanel::Progress,
            progress_text: "Do you have the tusks?".into(),
            required: vec![QuestItemView {
                item_id: 0,
                name: Some("Chipped Boar Tusk".into()),
                texture: None,
                count: 8,
                quality: 0,
                ..Default::default()
            }],
            required_money: 500,
            completable: true,
            ..Default::default()
        }));
        assert_eq!(
            s.eval::<String>("return GetProgressText()").unwrap(),
            "Do you have the tusks?"
        );
        assert_eq!(s.eval::<i64>("return GetNumQuestItems()").unwrap(), 1);
        assert_eq!(s.eval::<i64>("return GetQuestMoneyToGet()").unwrap(), 500);
        assert!(s.eval::<bool>("return IsQuestCompletable()").unwrap());
    }

    /// Each panel verb hands the host its call; the gates are the host's ([`QuestCall`]).
    #[test]
    fn the_panel_verbs_hand_the_host_their_calls() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.set_quest(Some(detail()));
        s.run("AcceptQuest() CompleteQuest() GetQuestReward(2) DeclineQuest()")
            .unwrap();
        assert_eq!(
            s.take_quest_calls(),
            vec![
                QuestCall::Accept,
                QuestCall::Complete,
                QuestCall::Reward(1),
                QuestCall::Decline,
            ]
        );
    }

    /// The row selects take a number or numeric string (`0x501ca8`, `0x501ce8`), truncated
    /// (`0x501cbd`), and raise their own usage for anything else (`0x84ed60`, `0x84ed84`); a row
    /// of 0 or below reaches the host, whose unsigned bound refuses it.
    #[test]
    fn the_row_selects_read_their_number_as_the_reference_does() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.run("SelectAvailableQuest(0) SelectActiveQuest(-1) SelectAvailableQuest('2.7')")
            .unwrap();
        let select = |active, index| QuestCall::Select(QuestSelect { active, index });
        assert_eq!(
            s.take_quest_calls(),
            [select(false, 0), select(true, u32::MAX), select(false, 2)]
        );
        for verb in ["SelectAvailableQuest", "SelectActiveQuest"] {
            let err = s.run(&format!("{verb}('x')")).unwrap_err().to_string();
            assert!(err.contains(&format!("Usage: {verb}(index)")), "{err}");
        }
    }

    /// `GetQuestReward` (`0x501d80`) passes `trunc(n) - 1`, signed (`0x501da8`), and -1 for an
    /// absent or non-number argument (`0x501d89`); `0x5015b0` judges it in the host.
    #[test]
    fn get_quest_reward_passes_the_choice_less_one() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.run(
            "GetQuestReward() GetQuestReward(0) GetQuestReward(-3) GetQuestReward(2) \
             GetQuestReward(1.9) GetQuestReward('x') GetQuestReward('3')",
        )
        .unwrap();
        assert_eq!(
            s.take_quest_calls(),
            [-1, -1, -4, 1, 0, -1, 2].map(QuestCall::Reward)
        );
    }

    #[test]
    fn clearing_the_quest_empties_it() {
        let mut s = UiScript::new().unwrap();
        s.set_quest(Some(detail()));
        s.set_quest(None);
        assert_eq!(s.eval::<String>("return GetTitleText()").unwrap(), "");
        assert_eq!(s.eval::<i64>("return GetNumQuestRewards()").unwrap(), 0);
        assert!(s.eval::<bool>("return GetRewardSpell() == nil").unwrap());
    }

    /// `DeclineQuest` is a call; `CloseQuest` is the app's teardown, taken apart from it.
    #[test]
    fn decline_is_a_call_and_close_the_teardown() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.set_quest(Some(detail()));
        s.run("DeclineQuest() CloseQuest()").unwrap();
        assert_eq!(
            s.take_quest_calls(),
            vec![QuestCall::Decline, QuestCall::Close]
        );
    }

    /// `ConfirmAcceptQuest` is a call each time, held confirm or none (`0x5021b0`).
    #[test]
    fn confirm_accept_quest_is_a_call_each_time() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.run("ConfirmAcceptQuest() ConfirmAcceptQuest()").unwrap();
        assert_eq!(
            s.take_quest_calls(),
            vec![QuestCall::ConfirmAccept, QuestCall::ConfirmAccept]
        );
    }
}
