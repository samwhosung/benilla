//! The questgiver bindings: the app pushes the open panel ([`UiScript::set_quest`]), and the panel
//! verbs queue selects and button intents it drains. One [`QuestState`] serves the flat getters
//! `QuestFrame.lua` reads on each panel; the app fires `QUEST_GREETING`, `QUEST_DETAIL`,
//! `QUEST_PROGRESS` or `QUEST_COMPLETE` for the live one.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::flag;
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
    /// `None`, answered as nil, while the item template is in flight.
    pub name: Option<String>,
    pub texture: Option<String>,
    pub count: u32,
    /// 0 poor to 5 legendary; 1 until the template lands.
    pub quality: u32,
    /// The item id the quest tooltips render by; 0 until the wire row resolves.
    pub item_id: u32,
    /// Whether the player can use the item; stock tints the row red when not
    /// (`QuestFrame.lua:393`). The app always sends `true`.
    pub usable: bool,
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

/// One open questgiver panel, with every field the four panels read; pushed whole by the app.
#[derive(Clone, Debug, PartialEq)]
pub struct QuestState {
    pub panel: QuestPanel,
    // Greeting panel.
    pub greeting: String,
    pub active_titles: Vec<String>,
    pub available_titles: Vec<String>,
    // Detail, progress and reward panels.
    pub title: String,
    /// The text of whichever panel is live: quest, progress or reward.
    pub body: String,
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
            title: String::new(),
            body: String::new(),
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

/// A greeting row picked by `SelectActiveQuest` or `SelectAvailableQuest`, 1-based in its list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuestSelect {
    pub active: bool,
    pub index: u32,
}

/// A questgiver button intent; the app sends the matching message for its NPC and quest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuestAction {
    /// Detail panel Accept → `CMSG_QUESTGIVER_ACCEPT_QUEST`.
    Accept,
    /// Progress panel Continue → `CMSG_QUESTGIVER_REQUEST_REWARD`.
    Continue,
    /// Reward panel Complete → `CMSG_QUESTGIVER_CHOOSE_REWARD` with the 0-based choice.
    Reward(u32),
    /// `DeclineQuest()`: the detail panel's Decline and the progress and reward panels' Cancel.
    /// Its binding (`0x501d30`) re-opens the giver through `0x5013f0`: `CMSG_GOSSIP_HELLO` for a
    /// gossip unit, `CMSG_QUESTGIVER_HELLO` for another unit, the teardown `0x501130(0,1)` for an
    /// item or a player (or with `0xbe0824` set), a GameObject's interact call. The app owns this.
    Decline,
    /// `CloseQuest()`, from `QuestFrame_OnHide`: ESC, the close button, the greeting's Goodbye, a
    /// panel eviction. Its binding (`0x501a10`) calls only the teardown `0x501130(0,1)`, whose one
    /// send is `MSG_QUEST_PUSH_RESULT` DECLINE for a player source, so closing an NPC's window
    /// sends nothing. It stays apart from `Decline`: the re-open is `DeclineQuest`'s alone.
    Close,
}

impl super::UiScript {
    /// Push (or clear, with `None`) the open questgiver panel snapshot.
    pub fn set_quest(&mut self, state: Option<QuestState>) {
        self.model_mut().quest = state;
    }

    /// Drain the greeting-panel row selects queued since the last call.
    pub fn take_quest_selects(&mut self) -> Vec<QuestSelect> {
        std::mem::take(&mut self.model_mut().quest_selects)
    }

    /// Drain the button intents queued since the last call.
    pub fn take_quest_actions(&mut self) -> Vec<QuestAction> {
        std::mem::take(&mut self.model_mut().quest_actions)
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

/// The list `GetQuestItemInfo(type, index)` reads for `type`.
pub(super) fn item_vec<'a>(state: &'a QuestState, kind: &str) -> Option<&'a Vec<QuestItemView>> {
    match kind {
        "choice" => Some(&state.choices),
        "reward" => Some(&state.rewards),
        "required" => Some(&state.required),
        _ => None,
    }
}

/// Register the questgiver globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // ── Text getters ──
    fn install_text(lua: &Lua, name: &str, pick: fn(&QuestState) -> String) -> mlua::Result<()> {
        lua.globals().set(
            name,
            lua.create_function(move |lua, ()| {
                let text = {
                    let model = lua.app_data_ref::<Model>().expect("model app_data");
                    model.quest.as_ref().map(pick).unwrap_or_default()
                };
                Ok(Value::String(lua.create_string(&text)?))
            })?,
        )
    }
    install_text(lua, "GetGreetingText", |q| q.greeting.clone())?;
    install_text(lua, "GetTitleText", |q| q.title.clone())?;
    install_text(lua, "GetQuestText", |q| q.body.clone())?;
    install_text(lua, "GetProgressText", |q| q.body.clone())?;
    install_text(lua, "GetRewardText", |q| q.body.clone())?;
    install_text(lua, "GetObjectiveText", |q| q.objectives.clone())?;

    // ── Count and money getters ──
    fn install_count(lua: &Lua, name: &str, pick: fn(&QuestState) -> i64) -> mlua::Result<()> {
        lua.globals().set(
            name,
            lua.create_function(move |lua, ()| {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                Ok(model.quest.as_ref().map(pick).unwrap_or(0))
            })?,
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

    g.set(
        "IsQuestCompletable",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(flag(model.quest.as_ref().is_some_and(|q| q.completable)))
        })?,
    )?;

    fn install_title(lua: &Lua, name: &str, active: bool) -> mlua::Result<()> {
        lua.globals().set(
            name,
            lua.create_function(move |lua, i: usize| {
                let title = {
                    let model = lua.app_data_ref::<Model>().expect("model app_data");
                    model.quest.as_ref().and_then(|q| {
                        let v = if active {
                            &q.active_titles
                        } else {
                            &q.available_titles
                        };
                        i.checked_sub(1).and_then(|n| v.get(n)).cloned()
                    })
                };
                Ok(Value::String(lua.create_string(title.unwrap_or_default())?))
            })?,
        )
    }
    install_title(lua, "GetActiveTitle", true)?;
    install_title(lua, "GetAvailableTitle", false)?;

    // GetQuestItemInfo(type, index) → name, texture, numItems, quality, isUsable.
    g.set(
        "GetQuestItemInfo",
        lua.create_function(|lua, (kind, index): (String, usize)| {
            let item = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model.quest.as_ref().and_then(|q| {
                    item_vec(q, &kind)
                        .and_then(|v| index.checked_sub(1).and_then(|n| v.get(n)))
                        .cloned()
                })
            };
            let Some(it) = item else {
                return Ok(MultiValue::from_vec(vec![Value::Nil]));
            };
            let name = match &it.name {
                Some(n) => Value::String(lua.create_string(n)?),
                None => Value::Nil,
            };
            let texture = match &it.texture {
                Some(t) => Value::String(lua.create_string(t)?),
                None => Value::Nil,
            };
            Ok(MultiValue::from_vec(vec![
                name,
                texture,
                Value::Integer(i64::from(it.count)),
                Value::Integer(i64::from(it.quality)),
                Value::Boolean(it.usable),
                // Not 1.12's: a sixth return, the item id, after its five; nothing reads it.
                Value::Integer(i64::from(it.item_id)),
            ]))
        })?,
    )?;

    // GetQuestItemLink(type, index), for a row's ctrl and shift clicks (`QuestFrame.lua:118`).
    g.set(
        "GetQuestItemLink",
        lua.create_function(|lua, (kind, index): (String, usize)| {
            let link = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model.quest.as_ref().and_then(|q| {
                    item_vec(q, &kind)
                        .and_then(|v| index.checked_sub(1).and_then(|n| v.get(n)))
                        .and_then(|it| it.link.clone())
                })
            };
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
            let spell = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model.quest.as_ref().and_then(|q| q.reward_spell.clone())
            };
            reward_spell_returns(lua, spell)
        })?,
    )?;

    // ── Intents ──
    fn install_select(lua: &Lua, name: &str, active: bool) -> mlua::Result<()> {
        lua.globals().set(
            name,
            lua.create_function(move |lua, i: u32| {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                model
                    .quest_selects
                    .push(super::QuestSelect { active, index: i });
                Ok(())
            })?,
        )
    }
    install_select(lua, "SelectActiveQuest", true)?;
    install_select(lua, "SelectAvailableQuest", false)?;

    fn install_action(lua: &Lua, name: &str, action: super::QuestAction) -> mlua::Result<()> {
        lua.globals().set(
            name,
            lua.create_function(move |lua, ()| {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                model.quest_actions.push(action);
                Ok(())
            })?,
        )
    }
    install_action(lua, "AcceptQuest", super::QuestAction::Accept)?;
    install_action(lua, "DeclineQuest", super::QuestAction::Decline)?;
    install_action(lua, "CloseQuest", super::QuestAction::Close)?;
    install_action(lua, "CompleteQuest", super::QuestAction::Continue)?;

    // GetQuestBackgroundMaterial() → nil or a material name (`0x502230`).
    g.set(
        "GetQuestBackgroundMaterial",
        lua.create_function(|lua, ()| {
            let material = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model
                    .quest
                    .as_ref()
                    .and_then(|q| q.background_material.clone())
            };
            match material {
                Some(m) => Ok(Value::String(lua.create_string(&m)?)),
                None => Ok(Value::Nil),
            }
        })?,
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

    // ConfirmAcceptQuest(): the escort confirm's Yes (`StaticPopup.lua:731-733`). It takes no quest
    // id, as the client answers the confirm it holds, so calls are counted and the app adds the id.
    g.set(
        "ConfirmAcceptQuest",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.quest_confirms += 1;
            Ok(())
        })?,
    )?;

    // GetQuestReward(choice): stock passes the 1-based row (`QuestFrame.lua:100`), and the wire
    // takes it less one, floored at 0, as in the reference (`0x501d80`); vmangos indexes
    // `RewChoiceItemId` with it.
    g.set(
        "GetQuestReward",
        lua.create_function(|lua, choice: Option<u32>| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.quest_actions.push(super::QuestAction::Reward(
                choice.unwrap_or(0).saturating_sub(1),
            ));
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{QuestAction, QuestItemView, QuestPanel, QuestSelect, QuestState};
    use crate::script::UiScript;

    fn detail() -> QuestState {
        QuestState {
            panel: QuestPanel::Detail,
            title: "A Threat Within".into(),
            body: "Kill the kobolds infesting the mine.".into(),
            objectives: "Slay 10 Kobold Vermin.".into(),
            rewards: vec![QuestItemView {
                item_id: 7278,
                name: Some("Brdle Leather Boots".into()),
                texture: Some("Interface\\Icons\\INV_Boots_01".into()),
                count: 1,
                quality: 2,
                usable: true,
                link: Some("|cff1eff00|Hitem:7278:0:0:0|h[Brdle Leather Boots]|h|r".into()),
            }],
            choices: vec![
                QuestItemView {
                    item_id: 0,
                    name: Some("Cudgel".into()),
                    texture: None,
                    count: 1,
                    quality: 1,
                    usable: true,
                    ..Default::default()
                },
                QuestItemView {
                    item_id: 0,
                    name: None, // template still in flight
                    texture: None,
                    count: 1,
                    quality: 1,
                    usable: true,
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

        let (name, texture, count, quality, usable) = s
            .eval::<(String, String, i64, i64, bool)>("return GetQuestItemInfo(\"reward\", 1)")
            .unwrap();
        assert_eq!(name, "Brdle Leather Boots");
        assert_eq!(texture, "Interface\\Icons\\INV_Boots_01");
        assert_eq!((count, quality, usable), (1, 2, true));

        // Choice row 2 is in flight: name and texture nil, the rest present.
        assert!(s
            .eval::<bool>(
                "local n, t, c = GetQuestItemInfo(\"choice\", 2)\n\
                 return n == nil and t == nil and c == 1"
            )
            .unwrap());
        // Out of range, and an unknown type: nil.
        assert!(s
            .eval::<bool>("return GetQuestItemInfo(\"reward\", 9) == nil")
            .unwrap());
        assert!(s
            .eval::<bool>("return GetQuestItemInfo(\"bogus\", 1) == nil")
            .unwrap());

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

    #[test]
    fn greeting_panel_lists_and_selects() {
        let mut s = UiScript::new().unwrap();
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

        s.run("SelectActiveQuest(1)").unwrap();
        s.run("SelectAvailableQuest(2)").unwrap();
        assert_eq!(
            s.take_quest_selects(),
            vec![
                QuestSelect {
                    active: true,
                    index: 1
                },
                QuestSelect {
                    active: false,
                    index: 2
                },
            ]
        );
        assert!(s.take_quest_selects().is_empty(), "drained");
    }

    #[test]
    fn progress_panel_completability() {
        let mut s = UiScript::new().unwrap();
        s.set_quest(Some(QuestState {
            panel: QuestPanel::Progress,
            body: "Do you have the tusks?".into(),
            required: vec![QuestItemView {
                item_id: 0,
                name: Some("Chipped Boar Tusk".into()),
                texture: None,
                count: 8,
                quality: 0,
                usable: true,
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

    #[test]
    fn button_intents_queue() {
        let mut s = UiScript::new().unwrap();
        s.set_quest(Some(detail()));
        s.run("AcceptQuest()").unwrap();
        assert_eq!(s.take_quest_actions(), vec![QuestAction::Accept]);

        s.run("CompleteQuest()").unwrap(); // progress -> reward
        s.run("GetQuestReward(2)").unwrap(); // row 2 (1-based) → wire choice 1 (0-based)
        s.run("GetQuestReward()").unwrap(); // no-choice quest -> 0
        s.run("DeclineQuest()").unwrap();
        assert_eq!(
            s.take_quest_actions(),
            vec![
                QuestAction::Continue,
                QuestAction::Reward(1),
                QuestAction::Reward(0),
                QuestAction::Decline,
            ]
        );
        assert!(s.take_quest_actions().is_empty(), "drained");
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

    #[test]
    fn decline_and_close_are_two_intents() {
        let mut s = UiScript::new().unwrap();
        s.set_quest(Some(detail()));
        s.run("DeclineQuest()").unwrap();
        s.run("CloseQuest()").unwrap();
        assert_eq!(
            s.take_quest_actions(),
            vec![QuestAction::Decline, QuestAction::Close]
        );
    }

    #[test]
    fn confirm_accept_quest_counts_and_drains() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.take_quest_confirms(), 0);
        s.run("ConfirmAcceptQuest() ConfirmAcceptQuest()").unwrap();
        assert_eq!(s.take_quest_confirms(), 2);
        assert_eq!(s.take_quest_confirms(), 0, "drained");
    }
}
