//! The chat-type colour registry — `GetChatTypeIndex`, `ChangeChatColor`, `ResetChatColors`.
//!
//! The reference keeps one runtime table of chat types (`0xb4e518`, stride 0x43: a 0x40-byte
//! name and three RGB bytes), seeded at boot from the 94-entry static table at `0x804710` and
//! then extended with ten extras `CHANNEL1`…`CHANNEL10`, each coloured from the live `CHANNEL`
//! entry (`0x4982c0`). The `chat-cache.txt`
//! `COLORS` block overwrites matched entries in place at load — absent names keep the compiled
//! defaults — and the app owns that file; here the table is state the app feeds and drains
//! ([`super::UiScript::set_chat_colors`], [`super::UiScript::take_chat_color_changes`]).
//!
//! The Lua surface, from the same node:
//!
//! - `GetChatTypeIndex(name)` — a case-insensitive linear scan of the 94 fixed entries, **1-based**
//!   (`4a0adf: inc ebx` before the match test); no fixed match → the extras, numbered **95, 96, …**
//!   (`count_before_match + 0x5f`); no match at all → **0**.
//! - `ChangeChatColor(name, r, g, b)` — `r,g,b` are 0.0–1.0 floats, each `fmul 255.0` then
//!   `__ftol` (**truncate**, no rounding, bytes `4a085f–4a08a3`), written into the matched entry.
//!   On success fires `UPDATE_CHAT_COLOR` with `"%s%f%f%f"`: the name and the three
//!   **just-written bytes re-normalised** by `f32 1/255` — so an addon that passed `0.5` hears
//!   `127/255`, not `0.5`. (The event's symbolic name is the node's one INFERRED item — it is
//!   `0xe2` in a runtime-populated listener table; `ChatFrame.lua:1349` registers and reads it
//!   as `UPDATE_CHAT_COLOR` with exactly those four args.)
//! - `ResetChatColors()` — the boot seed again: the 94 defaults copied back, then every extra
//!   recoloured from the live `CHANNEL` entry.
//!
//! What the line colours *mean* to a message frame is `ScrollingMessageFrame:UpdateColorByID`,
//! which lives with the frame (`messageframe/scrolling.rs`); this file is only the registry.

use mlua::{Lua, MultiValue, Value};

use super::Model;

/// The fixed segment: 94 entries, in the reference's order (index = position + 1).
pub const FIXED_CHAT_TYPES: usize = 94;
/// The extras seeded at boot: `CHANNEL1`…`CHANNEL10`, indices 95–104.
pub const EXTRA_CHAT_TYPES: usize = 10;

/// `0x804710` — name and default RGB, verbatim. The order is load-bearing: it *is* the index
/// `GetChatTypeIndex` answers.
const DEFAULTS: [(&str, [u8; 3]); FIXED_CHAT_TYPES] = [
    ("SAY", [255, 255, 255]),
    ("PARTY", [170, 170, 255]),
    ("RAID", [255, 127, 0]),
    ("GUILD", [64, 255, 64]),
    ("OFFICER", [64, 192, 64]),
    ("YELL", [255, 64, 64]),
    ("WHISPER", [255, 128, 255]),
    ("WHISPER_INFORM", [255, 128, 255]),
    ("EMOTE", [255, 128, 64]),
    ("TEXT_EMOTE", [255, 128, 64]),
    ("SYSTEM", [255, 255, 0]),
    ("MONSTER_SAY", [255, 255, 159]),
    ("MONSTER_YELL", [255, 64, 64]),
    ("MONSTER_EMOTE", [255, 128, 64]),
    ("CHANNEL", [255, 192, 192]),
    ("CHANNEL_JOIN", [192, 128, 128]),
    ("CHANNEL_LEAVE", [192, 128, 128]),
    ("CHANNEL_LIST", [192, 128, 128]),
    ("CHANNEL_NOTICE", [192, 192, 192]),
    ("CHANNEL_NOTICE_USER", [192, 192, 192]),
    ("AFK", [255, 128, 255]),
    ("DND", [255, 128, 255]),
    ("IGNORED", [255, 0, 0]),
    ("SKILL", [85, 85, 255]),
    ("LOOT", [0, 170, 0]),
    ("COMBAT_MISC_INFO", [128, 128, 255]),
    ("MONSTER_WHISPER", [179, 179, 179]),
    ("COMBAT_SELF_HITS", [255, 255, 255]),
    ("COMBAT_SELF_MISSES", [255, 255, 255]),
    ("COMBAT_PET_HITS", [255, 255, 255]),
    ("COMBAT_PET_MISSES", [255, 255, 255]),
    ("COMBAT_PARTY_HITS", [255, 255, 255]),
    ("COMBAT_PARTY_MISSES", [255, 255, 255]),
    ("COMBAT_FRIENDLYPLAYER_HITS", [255, 255, 255]),
    ("COMBAT_FRIENDLYPLAYER_MISSES", [255, 255, 255]),
    ("COMBAT_HOSTILEPLAYER_HITS", [255, 255, 255]),
    ("COMBAT_HOSTILEPLAYER_MISSES", [255, 255, 255]),
    ("COMBAT_CREATURE_VS_SELF_HITS", [255, 47, 47]),
    ("COMBAT_CREATURE_VS_SELF_MISSES", [255, 47, 47]),
    ("COMBAT_CREATURE_VS_PARTY_HITS", [255, 255, 255]),
    ("COMBAT_CREATURE_VS_PARTY_MISSES", [255, 255, 255]),
    ("COMBAT_CREATURE_VS_CREATURE_HITS", [255, 255, 255]),
    ("COMBAT_CREATURE_VS_CREATURE_MISSES", [255, 255, 255]),
    ("COMBAT_FRIENDLY_DEATH", [255, 255, 255]),
    ("COMBAT_HOSTILE_DEATH", [255, 255, 255]),
    ("COMBAT_XP_GAIN", [111, 111, 255]),
    ("SPELL_SELF_DAMAGE", [255, 255, 0]),
    ("SPELL_SELF_BUFF", [255, 255, 0]),
    ("SPELL_PET_DAMAGE", [255, 255, 255]),
    ("SPELL_PET_BUFF", [255, 255, 255]),
    ("SPELL_PARTY_DAMAGE", [255, 255, 255]),
    ("SPELL_PARTY_BUFF", [255, 255, 255]),
    ("SPELL_FRIENDLYPLAYER_DAMAGE", [255, 255, 255]),
    ("SPELL_FRIENDLYPLAYER_BUFF", [255, 255, 255]),
    ("SPELL_HOSTILEPLAYER_DAMAGE", [255, 255, 255]),
    ("SPELL_HOSTILEPLAYER_BUFF", [255, 255, 255]),
    ("SPELL_CREATURE_VS_SELF_DAMAGE", [202, 76, 217]),
    ("SPELL_CREATURE_VS_SELF_BUFF", [255, 255, 255]),
    ("SPELL_CREATURE_VS_PARTY_DAMAGE", [255, 255, 255]),
    ("SPELL_CREATURE_VS_PARTY_BUFF", [255, 255, 255]),
    ("SPELL_CREATURE_VS_CREATURE_DAMAGE", [255, 255, 255]),
    ("SPELL_CREATURE_VS_CREATURE_BUFF", [255, 255, 255]),
    ("SPELL_TRADESKILLS", [255, 255, 255]),
    ("SPELL_DAMAGESHIELDS_ON_SELF", [255, 255, 255]),
    ("SPELL_DAMAGESHIELDS_ON_OTHERS", [255, 255, 255]),
    ("SPELL_AURA_GONE_SELF", [255, 255, 255]),
    ("SPELL_AURA_GONE_PARTY", [255, 255, 255]),
    ("SPELL_AURA_GONE_OTHER", [255, 255, 255]),
    ("SPELL_ITEM_ENCHANTMENTS", [255, 255, 255]),
    ("SPELL_BREAK_AURA", [255, 255, 255]),
    ("SPELL_PERIODIC_SELF_DAMAGE", [255, 255, 255]),
    ("SPELL_PERIODIC_SELF_BUFFS", [255, 255, 255]),
    ("SPELL_PERIODIC_PARTY_DAMAGE", [255, 255, 255]),
    ("SPELL_PERIODIC_PARTY_BUFFS", [255, 255, 255]),
    ("SPELL_PERIODIC_FRIENDLYPLAYER_DAMAGE", [255, 255, 255]),
    ("SPELL_PERIODIC_FRIENDLYPLAYER_BUFFS", [255, 255, 255]),
    ("SPELL_PERIODIC_HOSTILEPLAYER_DAMAGE", [255, 255, 255]),
    ("SPELL_PERIODIC_HOSTILEPLAYER_BUFFS", [255, 255, 255]),
    ("SPELL_PERIODIC_CREATURE_DAMAGE", [255, 255, 255]),
    ("SPELL_PERIODIC_CREATURE_BUFFS", [255, 255, 255]),
    ("SPELL_FAILED_LOCALPLAYER", [255, 255, 255]),
    ("COMBAT_HONOR_GAIN", [224, 202, 10]),
    ("BG_SYSTEM_NEUTRAL", [255, 120, 10]),
    ("BG_SYSTEM_ALLIANCE", [0, 174, 239]),
    ("BG_SYSTEM_HORDE", [255, 0, 0]),
    ("COMBAT_FACTION_CHANGE", [128, 128, 255]),
    ("MONEY", [255, 255, 0]),
    ("RAID_LEADER", [255, 219, 183]),
    ("RAID_WARNING", [255, 219, 183]),
    ("FOREIGN_TELL", [255, 128, 255]),
    ("RAID_BOSS_EMOTE", [255, 219, 183]),
    ("FILTERED", [255, 0, 0]),
    ("BATTLEGROUND", [255, 127, 0]),
    ("BATTLEGROUND_LEADER", [255, 219, 183]),
];

/// The 1-based index of the `CHANNEL` entry — the extras' colour source at seed and reset.
const CHANNEL_INDEX: usize = 15;

/// One registry entry: the type's name and its live colour bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatTypeColor {
    pub name: String,
    pub rgb: [u8; 3],
}

/// The boot seed: the 94 defaults, then `CHANNEL1`…`CHANNEL10` coloured from the live `CHANNEL`.
pub(super) fn seed() -> Vec<ChatTypeColor> {
    let mut table: Vec<ChatTypeColor> = DEFAULTS
        .iter()
        .map(|(name, rgb)| ChatTypeColor {
            name: (*name).to_string(),
            rgb: *rgb,
        })
        .collect();
    let channel = table[CHANNEL_INDEX - 1].rgb;
    for n in 1..=EXTRA_CHAT_TYPES {
        table.push(ChatTypeColor {
            name: format!("CHANNEL{n}"),
            rgb: channel,
        });
    }
    table
}

/// `ResetChatColors`' copy loop: the fixed segment back to `0x804710`, the extras from `CHANNEL`.
fn reset(table: &mut [ChatTypeColor]) {
    for (entry, (_, rgb)) in table.iter_mut().zip(DEFAULTS.iter()) {
        entry.rgb = *rgb;
    }
    let channel = table[CHANNEL_INDEX - 1].rgb;
    for entry in table.iter_mut().skip(FIXED_CHAT_TYPES) {
        entry.rgb = channel;
    }
}

/// The reference's compare (`0x64a4c0` → `0x414310`): an `'A'–'Z' + 0x20` fold, i.e. ASCII
/// case-insensitive and nothing wider.
fn position(table: &[ChatTypeColor], name: &str) -> Option<usize> {
    table.iter().position(|e| e.name.eq_ignore_ascii_case(name))
}

/// `fmul 255.0; __ftol` — truncation toward zero, then the low byte is what the `mov` stores.
fn to_byte(x: f64) -> u8 {
    if !x.is_finite() {
        return 0;
    }
    ((x * 255.0).trunc() as i64) as u8
}

/// `fmul dword [0x8026c8]` — the f32 reciprocal, so the event carries `byte / 255` in single
/// precision, widened.
fn from_byte(b: u8) -> f64 {
    f64::from(b as f32 * (1.0f32 / 255.0f32))
}

impl super::UiScript {
    /// The `COLORS` block of `chat-cache.txt`, applied the way the loader applies it: each named
    /// entry overwritten in place, unknown names ignored, absent names left at their defaults.
    pub fn set_chat_colors(&mut self, colors: impl IntoIterator<Item = (String, [u8; 3])>) {
        let mut model = self.model_mut();
        for (name, rgb) in colors {
            if let Some(i) = position(&model.chat_colors, &name) {
                model.chat_colors[i].rgb = rgb;
            }
        }
    }

    /// The live registry, fixed segment first, then the extras.
    pub fn chat_colors(&self) -> Vec<ChatTypeColor> {
        self.model_mut().chat_colors.clone()
    }

    /// Whether a Lua-side write (`ChangeChatColor`, `ResetChatColors`) has landed since the last
    /// drain — the app's persist cue.
    pub fn take_chat_color_changes(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().chat_colors_changed)
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    g.set(
        "GetChatTypeIndex",
        lua.create_function(|lua, name: Option<String>| {
            let Some(name) = name else {
                return Ok(0);
            };
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(position(&model.chat_colors, &name).map_or(0, |i| i as i64 + 1))
        })?,
    )?;

    g.set(
        "ChangeChatColor",
        lua.create_function(
            |lua, (name, r, g, b): (String, Option<f64>, Option<f64>, Option<f64>)| {
                let written = {
                    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                    let Some(i) = position(&model.chat_colors, &name) else {
                        return Ok(());
                    };
                    let rgb = [
                        to_byte(r.unwrap_or(0.0)),
                        to_byte(g.unwrap_or(0.0)),
                        to_byte(b.unwrap_or(0.0)),
                    ];
                    model.chat_colors[i].rgb = rgb;
                    model.chat_colors_changed = true;
                    (model.chat_colors[i].name.clone(), rgb)
                };
                let (name, rgb) = written;
                super::tick::fire_event_into(
                    lua,
                    "UPDATE_CHAT_COLOR",
                    vec![
                        super::ScriptValue::Str(name),
                        super::ScriptValue::Number(from_byte(rgb[0])),
                        super::ScriptValue::Number(from_byte(rgb[1])),
                        super::ScriptValue::Number(from_byte(rgb[2])),
                    ],
                );
                Ok(())
            },
        )?,
    )?;

    g.set(
        "ResetChatColors",
        lua.create_function(|lua, _ignored: MultiValue| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            reset(&mut model.chat_colors);
            model.chat_colors_changed = true;
            Ok(Value::Nil)
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    #[test]
    fn the_index_is_one_based_case_folded_and_zero_on_a_miss() {
        let s = UiScript::new().unwrap();
        assert_eq!(s.eval::<i64>("return GetChatTypeIndex('say')").unwrap(), 1);
        assert_eq!(
            s.eval::<i64>("return GetChatTypeIndex('BATTLEGROUND_LEADER')")
                .unwrap(),
            94
        );
        assert_eq!(
            s.eval::<i64>("return GetChatTypeIndex('channel3')")
                .unwrap(),
            97,
            "extras are 95… — count_before_match + 0x5f"
        );
        assert_eq!(s.eval::<i64>("return GetChatTypeIndex('NOPE')").unwrap(), 0);
        assert_eq!(s.eval::<i64>("return GetChatTypeIndex()").unwrap(), 0);
    }

    #[test]
    fn change_chat_color_truncates_stores_and_fires_the_renormalised_bytes() {
        let mut s = UiScript::new().unwrap();
        s.run(
            "local f = CreateFrame('Frame') f:RegisterEvent('UPDATE_CHAT_COLOR') \
             f:SetScript('OnEvent', function() SEEN = {arg1, arg2, arg3, arg4} end) \
             ChangeChatColor('say', 0.5, 1.0, 0.999)",
        )
        .unwrap();
        let say = &s.chat_colors()[0];
        assert_eq!(say.name, "SAY");
        assert_eq!(say.rgb, [127, 255, 254], "truncate, never round");
        assert_eq!(
            s.eval::<String>("return SEEN[1]").unwrap(),
            "SAY",
            "the registry's spelling, not the caller's"
        );
        let heard = s.eval::<f64>("return SEEN[2]").unwrap();
        assert!(
            (heard - f64::from(127.0f32 * (1.0f32 / 255.0f32))).abs() < 1e-12,
            "the just-written byte re-normalised, not the 0.5 that was passed: {heard}"
        );
        assert!(s.take_chat_color_changes());
        assert!(!s.take_chat_color_changes(), "the cue is one-shot");
    }

    #[test]
    fn an_unknown_type_writes_nothing_and_fires_nothing() {
        let mut s = UiScript::new().unwrap();
        s.run(
            "local f = CreateFrame('Frame') f:RegisterEvent('UPDATE_CHAT_COLOR') \
             f:SetScript('OnEvent', function() SEEN = true end) \
             ChangeChatColor('NOPE', 0.5, 0.5, 0.5)",
        )
        .unwrap();
        assert!(s.eval::<bool>("return SEEN == nil").unwrap());
        assert!(!s.take_chat_color_changes());
    }

    #[test]
    fn the_cache_block_overwrites_in_place_and_reset_restores_the_seed() {
        let mut s = UiScript::new().unwrap();
        s.set_chat_colors([
            ("system".to_string(), [1, 2, 3]),
            ("CHANNEL".to_string(), [9, 9, 9]),
            ("BOGUS".to_string(), [7, 7, 7]),
        ]);
        let table = s.chat_colors();
        assert_eq!(table.len(), FIXED_CHAT_TYPES + EXTRA_CHAT_TYPES);
        assert_eq!(table[10].rgb, [1, 2, 3], "SYSTEM, matched case-folded");
        assert_eq!(
            table[FIXED_CHAT_TYPES].rgb,
            [255, 192, 192],
            "an extra keeps its seed colour — the cache does not re-seed it from CHANNEL"
        );
        s.run("ResetChatColors()").unwrap();
        let table = s.chat_colors();
        assert_eq!(table[10].rgb, [255, 255, 0]);
        assert_eq!(table[14].rgb, [255, 192, 192]);
        assert_eq!(table[FIXED_CHAT_TYPES + 9].name, "CHANNEL10");
        assert!(s.take_chat_color_changes());
    }

    #[test]
    fn reset_recolours_the_extras_from_the_live_channel_entry() {
        let s = UiScript::new().unwrap();
        s.run("ChangeChatColor('CHANNEL', 0.0, 0.0, 0.0) ChangeChatColor('CHANNEL2', 1, 1, 1)")
            .unwrap();
        // Reset copies the fixed defaults first, so CHANNEL is back to FFC0C0 by the time the
        // extras are recoloured from it.
        s.run("ResetChatColors()").unwrap();
        assert_eq!(s.chat_colors()[FIXED_CHAT_TYPES + 1].rgb, [255, 192, 192]);
    }
}
