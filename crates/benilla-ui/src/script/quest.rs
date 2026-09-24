//! The questgiver bindings (decision 0088) — the Era-shaped quest-dialog surface, the same two-way
//! seam as [`super::gossip`]/[`super::merchant`]: the app pushes a **quest panel snapshot**
//! ([`UiScript::set_quest`] — the greeting/detail/progress/reward text + item rows already resolved
//! from the wire), and the Lua `AcceptQuest`/`CompleteQuest`/`GetQuestReward`/`SelectActiveQuest`/…
//! calls queue outbound **intents** the app drains ([`UiScript::take_quest_selects`] /
//! [`UiScript::take_quest_actions`]). The engine holds no quest knowledge — a row is "a name, an
//! icon, a count, a quality, and whether it's usable"; a button press is a bare intent the app maps
//! to `(npc, questId)` from its own live state.
//!
//! ## The Era API shape
//!
//! 1.12's `QuestFrame.lua` reads flat getters positionally, one panel at a time: the greeting panel
//! walks `GetNumActiveQuests()`/`GetActiveTitle(i)` (+ the available twins), the detail panel reads
//! `GetTitleText()`/`GetQuestText()`/`GetObjectiveText()`, the progress panel reads
//! `GetProgressText()`/`IsQuestCompletable()`/`GetNumQuestItems()`/`GetQuestItemInfo("required",i)`,
//! the reward panel reads `GetRewardText()`/`GetNumQuestChoices()`/`GetNumQuestRewards()`/
//! `GetRewardMoney()`/`GetQuestItemInfo("choice"|"reward",i)`. Benilla keeps those exact names on a
//! single pushed [`QuestState`]; which panel is live is [`QuestState::panel`], surfaced to the XML
//! through the `QUEST_GREETING`/`QUEST_DETAIL`/`QUEST_PROGRESS`/`QUEST_COMPLETE` events the app
//! fires. `GetRewardSpell()` returns `nil` in v1 (spell-reward rows are out of scope, decision 0088).

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::flag;
use super::Model;

/// Which of the four questgiver sub-panels a [`QuestState`] is for (the app sets it from the wire
/// packet; the XML's event routing mirrors it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuestPanel {
    /// `SMSG_QUESTGIVER_QUEST_LIST` — the multi-quest greeting (active + available lists).
    Greeting,
    /// `SMSG_QUESTGIVER_QUEST_DETAILS` — the accept panel.
    Detail,
    /// `SMSG_QUESTGIVER_REQUEST_ITEMS` — the turn-in progress panel.
    Progress,
    /// `SMSG_QUESTGIVER_OFFER_REWARD` — the reward panel.
    Reward,
}

/// One quest item row (choice reward / fixed reward / required item), resolved by the app from the
/// wire triple + its item stores. Plain data — 1-based order is its position in the owning vector.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QuestItemView {
    /// Item name (`GetQuestItemInfo`'s first return); `None` while the ask-once template query is in
    /// flight (the API reports `nil`, the XML shows a placeholder).
    pub name: Option<String>,
    /// Icon texture path (`Interface\Icons\…`); `None` while the template answer is in flight (the
    /// icon comes from the wire display id, so it's usually present immediately).
    pub texture: Option<String>,
    /// Stack count for the row (`numItems`).
    pub count: u32,
    /// Item quality (0 poor .. 5 legendary); colours the reward name. `1` (common/white) when the
    /// template hasn't landed.
    pub quality: u32,
    /// The item id — the shared item-tooltip store's key (`BenillaGetItemStats`); `0` while the
    /// wire row hasn't resolved. A benilla extension: the era 5-tuple never carried it (tooltip
    /// content was C++'s alone), so it rides as a TRAILING 6th return, invisible to era callers.
    pub item_id: u32,
    /// Whether the reward is usable by the player's class/race — v1 always `true` (soft gray only,
    /// the server stays authoritative).
    pub usable: bool,
    /// The full escaped `|cff…|Hitem:…|h[Name]|h|r` link (`GetQuestItemLink` /
    /// `GetQuestLogItemLink` serve it) — the ctrl/shift click arms' payload (decisions 1059/1060).
    /// `None` until the ask-once item template lands: the link is built from the name **and** the
    /// quality, and neither is known before then, exactly like [`super::InvSlotView::link`].
    pub link: Option<String>,
}

/// One open questgiver panel: the active sub-panel plus every field the four panels might read.
/// Pushed whole by the app; `None` means no quest window is open.
/// The quest's reward spell — `rewSpell` on `SMSG_QUESTGIVER_QUEST_DETAILS` /
/// `SMSG_QUESTGIVER_OFFER_REWARD` and on `SMSG_QUEST_QUERY_RESPONSE` — resolved by the app to
/// the name and icon `GetRewardSpell` / `GetQuestLogRewardSpell` answer (stock
/// `QuestFrameItems_Update` counts it as one more reward slot, `rewardType = "spell"`, and the
/// slot's hover is `GameTooltip:SetQuestRewardSpell()`). `tradeskill` is the third return the
/// reference derives from the spell itself (1944 — the derivation is not yet pinned).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QuestRewardSpell {
    pub spell_id: u32,
    pub name: Option<String>,
    pub texture: Option<String>,
    pub tradeskill: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct QuestState {
    pub panel: QuestPanel,
    // Greeting panel.
    pub greeting: String,
    pub active_titles: Vec<String>,
    pub available_titles: Vec<String>,
    // Detail / progress / reward.
    /// The quest title (`GetTitleText`).
    pub title: String,
    /// The panel body text: quest description (detail), request text (progress), or reward text
    /// (reward). One field — only one of the three panels is live at a time.
    pub body: String,
    /// The quest objectives line (`GetObjectiveText`, detail panel only).
    pub objectives: String,
    /// Choice rewards (the player picks one) — reward/detail panel.
    pub choices: Vec<QuestItemView>,
    /// Fixed rewards (all granted) — reward/detail panel.
    pub rewards: Vec<QuestItemView>,
    /// Required items — progress panel.
    pub required: Vec<QuestItemView>,
    /// Reward money in copper (`GetRewardMoney`, detail/reward panels).
    pub reward_money: u32,
    /// Required money in copper (`GetQuestMoneyToGet`, progress panel).
    pub required_money: u32,
    /// Whether the turn-in is completable (`IsQuestCompletable`, progress panel).
    pub completable: bool,
    /// The reward spell, if the quest teaches one (`GetRewardSpell`; detail/reward panels).
    pub reward_spell: Option<QuestRewardSpell>,
    /// The panel's background material (`GetQuestBackgroundMaterial` — nil is the reference's own
    /// "Parchment" fallback in `QuestFrame_GetMaterial`). What fills it on the wire is 1944's
    /// open question; until the binary says, nothing does, and the verb answers nil honestly.
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

/// A greeting-panel row click queued by `SelectActiveQuest`/`SelectAvailableQuest`: the 1-based row
/// in its list. `active` picks between the active and available lists (the app maps the index to the
/// quest id).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuestSelect {
    pub active: bool,
    pub index: u32,
}

/// A questgiver button intent queued by the Lua panel verbs; the app maps each to `(npc, questId)`
/// from its own live state and the matching `CMSG_QUESTGIVER_*`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuestAction {
    /// Detail panel Accept → `CMSG_QUESTGIVER_ACCEPT_QUEST`.
    Accept,
    /// Progress panel Continue → `CMSG_QUESTGIVER_REQUEST_REWARD` (advance to the reward panel).
    Continue,
    /// Reward panel Complete → `CMSG_QUESTGIVER_CHOOSE_REWARD` with the chosen choice index (0 when
    /// the quest has no choice rewards).
    Reward(u32),
    /// `DeclineQuest()` — the detail panel's Decline and the progress/reward panels' Cancel
    /// (stock `QuestFrame.lua`: `QuestDetailDeclineButton_OnClick`, `QuestGoodbyeButton_OnClick`,
    /// `QuestRewardCancelButton_OnClick`). Its binding `0x501d30` calls `0x5013f0`, the ONLY
    /// routine that re-opens the giver: a four-way fork on the source's object type — a
    /// gossip-flagged unit → `CMSG_GOSSIP_HELLO`, a plain unit → `CMSG_QUESTGIVER_HELLO`, an
    /// item/player (or the `0xbe0824` latch set) → the teardown `0x501130(0,1)`, a GameObject →
    /// its interact virtual. The app owns the fork, because only it knows the giver.
    Decline,
    /// `CloseQuest()` — the window's own `QuestFrame_OnHide`: ESC, the close button, the
    /// greeting's Goodbye (`HideUIPanel(QuestFrame)`), a UIPanel eviction. Its binding `0x501a10`
    /// calls the teardown `0x501130(0,1)` and **nothing else**; that routine's one send is
    /// `MSG_QUEST_PUSH_RESULT{DECLINE}` for a PLAYER source, so closing an NPC's window is
    /// network-silent.
    ///
    /// **Two actions, because the reference has two routines.** 1738 merged this into
    /// `DeclineQuest`'s action on the reading that both verbs are one routine; the reference's
    /// callers show they share only the teardown, and the HELLO re-open lives in `DeclineQuest`'s
    /// fork alone. The merge made ESC on an NPC re-open its list — a multi-quest greeting could
    /// not be closed.
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

/// `GetQuestItemInfo(type, index)` reads from this vector by `type`; unknown types → `None`.
/// The three returns of `GetRewardSpell` / `GetQuestLogRewardSpell`: `texture, name,
/// isTradeskillSpell` — the third a 1/nil boolean in the reference's convention.
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

    // ── Text getters (one per panel field; nil/"" when no window is open) ─────────────────────────
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

    // ── Count + money getters ─────────────────────────────────────────────────────────────────────
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

    // IsQuestCompletable() → 1/nil (progress panel gate for the Continue button).
    g.set(
        "IsQuestCompletable",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(flag(model.quest.as_ref().is_some_and(|q| q.completable)))
        })?,
    )?;

    // GetActiveTitle(i) / GetAvailableTitle(i) — 1-based; "" when out of range.
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

    // GetQuestItemInfo(type, index) → name, texture, numItems, quality, isUsable (the Era tuple).
    // `type` is "choice" / "reward" / "required"; `index` 1-based; out of range → nil.
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
                // Benilla extension (6th): the item id, the shared tooltip store's key.
                Value::Integer(i64::from(it.item_id)),
            ]))
        })?,
    )?;

    // GetQuestItemLink(type, index) → the full escaped `|cff…|Hitem:…|h[Name]|h|r` | nil. The
    // questgiver panels' ctrl/shift click arms read it — `DressUpItemLink(GetQuestItemLink(
    // this.type, this:GetID()))` (ref QuestFrame.lua:118/130) and the shift-click
    // `ChatFrameEditBox:Insert(...)` beside it (l.122/134). Same `type`/`index` addressing as
    // `GetQuestItemInfo` above; nil for an out-of-range row, an unknown type, or a row whose
    // template answer is still in flight (see [`QuestItemView::link`]). Decisions 1059/1060.
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

    // GetRewardSpell() → texture, name, isTradeskillSpell (three returns, 0x501df0; three nils on
    // every empty path, the reference's own `(nil,nil,nil)` kind — 1842). Real since 1944: the
    // app resolves `rewSpell` off the giver packets; stock's `if ( GetRewardSpell() ) then` counts
    // the slot.
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

    // ── Intents ───────────────────────────────────────────────────────────────────────────────────
    // SelectActiveQuest(i) / SelectAvailableQuest(i) — queue a greeting-row select (1-based).
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
    // AcceptQuest → Accept; DeclineQuest → Decline (the re-opening fork `0x5013f0`); CloseQuest →
    // Close (the teardown `0x501130` alone — see `QuestAction::Close`); CompleteQuest (the
    // progress panel's Continue) → Continue.
    install_action(lua, "AcceptQuest", super::QuestAction::Accept)?;
    install_action(lua, "DeclineQuest", super::QuestAction::Decline)?;
    install_action(lua, "CloseQuest", super::QuestAction::Close)?;
    install_action(lua, "CompleteQuest", super::QuestAction::Continue)?;

    // GetQuestBackgroundMaterial() → nil | string (0x502230; 0 args, 1 return). The reference's
    // `QuestFrame_GetMaterial` (QuestFrame.lua:597-603) treats nil as "Parchment" and any string
    // as the name of a material whose four `$parentMaterial*` quadrants and text colours
    // (`GetMaterialTextColors`) the panel switches to. Answered from the pushed panel state.
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

    // QuestChooseRewardError() — 0x5021a0, 0 args, 0 returns: the Complete button pressed with
    // choices on offer and none picked (stock `QuestRewardCompleteButton_OnClick`,
    // QuestFrame.lua:96-100). Sixteen bytes — `push 0x98; call 0x496720` — the game-error row
    // `ERR_QUEST_MUST_CHOOSE`: the row's sound
    // cue `igQuestFailed` plays first, then kind 2 routes the GlobalStrings text through
    // `UI_ERROR_MESSAGE` — synchronously, as SignalEvent is — and UIErrorsFrame prints it. The
    // text is read from the same global the FrameXML shows, so a localised chain shows its own
    // string; the shipped English is the fallback when no GlobalStrings is loaded.
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

    // ConfirmAcceptQuest() — the escort confirm's Yes (ref StaticPopup.lua:731-733, raised by
    // QUEST_ACCEPT_CONFIRM). No argument and no quest id: the client answers whichever confirm it
    // is holding, so the engine counts the calls and the app supplies the id (decision 1733).
    g.set(
        "ConfirmAcceptQuest",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.quest_confirms += 1;
            Ok(())
        })?,
    )?;

    // GetQuestReward(choice) — the reward panel's Complete button. FrameXML passes the 1-based row
    // (`QuestFrameRewardPanel.itemChoice = this:GetID()`, stock QuestFrame.lua:97-101) and the
    // wire wants the 0-based choice (`CMSG_QUESTGIVER_CHOOSE_REWARD`'s reward index — vmangos
    // indexes `RewChoiceItemId[reward]` with it), so the conversion is the binding's, as it is the
    // client's; our own Lua used to do the -1 itself (1944). A quest with no choice rewards
    // passes 0, which stays 0.
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

        // Row 1 (resolved): the Era tuple.
        let (name, texture, count, quality, usable) = s
            .eval::<(String, String, i64, i64, bool)>("return GetQuestItemInfo(\"reward\", 1)")
            .unwrap();
        assert_eq!(name, "Brdle Leather Boots");
        assert_eq!(texture, "Interface\\Icons\\INV_Boots_01");
        assert_eq!((count, quality, usable), (1, 2, true));

        // Choice row 2 (in flight): name + texture nil, the rest present.
        assert!(s
            .eval::<bool>(
                "local n, t, c = GetQuestItemInfo(\"choice\", 2)\n\
                 return n == nil and t == nil and c == 1"
            )
            .unwrap());
        // Out of range / unknown type → nil.
        assert!(s
            .eval::<bool>("return GetQuestItemInfo(\"reward\", 9) == nil")
            .unwrap());
        assert!(s
            .eval::<bool>("return GetQuestItemInfo(\"bogus\", 1) == nil")
            .unwrap());

        // GetQuestItemLink: the resolved row's full escaped link; nil for a row still waiting on
        // its template (choice 2 has no name yet, so no link either), and nil out of range /
        // unknown type — the three nils the ref's ctrl/shift arms hand to DressUpItemLink.
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

    /// `DeclineQuest` and `CloseQuest` are TWO intents — two reference routines (`0x5013f0` and
    /// `0x501a10`) that share only the teardown `0x501130`; the giver re-open is `DeclineQuest`'s
    /// alone.
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

    /// `ConfirmAcceptQuest` counts rather than carrying a quest id — the reference's verb takes no
    /// argument and answers whichever confirm the client holds (decision 1733).
    #[test]
    fn confirm_accept_quest_counts_and_drains() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.take_quest_confirms(), 0);
        s.run("ConfirmAcceptQuest() ConfirmAcceptQuest()").unwrap();
        assert_eq!(s.take_quest_confirms(), 2);
        assert_eq!(s.take_quest_confirms(), 0, "drained");
    }
}
