//! A refused cast's line, its world half: the arms that need our bags, the item cache or a DBC name
//! table, and the combat-log line beside it. `ReportCastFailure 0x6e1a00` builds the text, shows it
//! (`DisplayError`, `0x6e21dd`) and logs it (`0x62c360`, `0x6e21fc`) inside the cast, so the line
//! is resolved where the report is signalled: inside the script call that raised it
//! ([`crate::script_calls`]), or by the frame's report feed ([`crate::ui_cast`]). The text layers
//! are [`super::cast_fail`]'s.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::UiScript;

use crate::items::Items;
use crate::net::{NetCommands, ObjectStore, SelfPlayer};
use crate::query_cache::Ask;
use crate::ui_chat::combat::PendingCombat;

use super::cast_fail::{self, CastFailLine};
use super::errors::{first_missing_totem, first_short_reagent};
use super::{CastFail, Caster, Shown, Spells};

/// The refusals whose item had no cached record when their line was due: the reference's first
/// pass exits past the line (`0x6e1eab`), and the cache callback `0x6e29b0` shows it once the
/// record answers, with no log line.
#[derive(Resource, Default)]
pub(crate) struct CastFailRedisplays(pub(crate) Vec<CastFail>);

/// The DBC name tables the cast-fail argument arms read; each is absent without game data. The
/// shapeshift forms ride [`Spells`].
#[derive(SystemParam)]
pub(crate) struct FailNameTables<'w> {
    /// `SpellFocusObject.dbc`, for `0x5e`.
    focus: Option<Res<'w, crate::ui_tradeskill::SpellFocus>>,
    /// `AreaTable.dbc`, for `0x5d`.
    areas: Option<Res<'w, crate::area::AreaTableRes>>,
    /// `SpellMechanic.dbc`, for `0x8d`.
    mechanics: Option<Res<'w, super::SpellMechanics>>,
}

impl FailNameTables<'_> {
    fn args<'a>(&'a self, spells: Option<&'a Spells>) -> cast_fail::FailArgs<'a> {
        cast_fail::FailArgs {
            arg: None,
            focus: self.focus.as_deref().map(|f| &f.catalog),
            areas: self.areas.as_deref().map(|a| &a.0),
            mechanics: self.mechanics.as_deref().map(|m| &m.catalog),
            forms: spells.map(|s| &s.forms),
        }
    }
}

/// One resolved refusal: the line, and the combat-log line the reporter writes after it.
pub(crate) struct ResolvedFail {
    pub(crate) shown: Shown,
    pub(crate) log: Option<PendingCombat>,
}

/// What a refused cast's line reads.
#[derive(SystemParam)]
pub(crate) struct CastFailLines<'w, 's> {
    spells: Option<Res<'w, Spells>>,
    sub_classes: Option<Res<'w, crate::ui_items::ItemSubClasses>>,
    names: FailNameTables<'w>,
    self_q: Query<'w, 's, &'static ObjectStore, With<SelfPlayer>>,
    objects: crate::net::Objects<'w, 's>,
    items: Res<'w, Items>,
    commands: Res<'w, NetCommands>,
    waiting: ResMut<'w, CastFailRedisplays>,
}

impl CastFailLines<'_, '_> {
    /// The spell catalog, which the cast bar's labels read too.
    pub(crate) fn spells(&self) -> Option<&Spells> {
        self.spells.as_deref()
    }

    /// `fail`'s line and its log line, `None` where the reference shows nothing now; a line whose
    /// item record is not cached waits in [`CastFailRedisplays`].
    pub(crate) fn resolve(&mut self, script: &UiScript, fail: CastFail) -> Option<ResolvedFail> {
        let line = self.line(script, fail)?;
        // The reason, its wire word and both resolved buffers, so a declined arm is readable
        // from a probe run.
        debug!(
            "ui_action: cast fail — {:?} spell {} reason {:#04x} arg {:?} → {line:?}",
            fail.caster, fail.spell_id, fail.reason, fail.arg
        );
        let log = self.log_line(&line, fail);
        Some(ResolvedFail {
            shown: Shown::keyed(line.key, line.text),
            log,
        })
    }

    /// The waiting lines whose item record has answered, each shown with no log line; the rest
    /// keep waiting.
    pub(crate) fn redisplays(&mut self, script: &UiScript) -> Vec<Shown> {
        std::mem::take(&mut self.waiting.0)
            .into_iter()
            .filter_map(|fail| self.resolve(script, fail).map(|r| r.shown))
            .collect()
    }

    /// The displayed line first, the log reads from it: the reference's order.
    fn line(&mut self, script: &UiScript, fail: CastFail) -> Option<CastFailLine> {
        let CastFail {
            spell_id,
            reason,
            caster,
            ..
        } = fail;
        let pet = caster == Caster::Pet;
        let spells = self.spells.as_deref();
        let d = spells.and_then(|s| s.catalog.get(spell_id));
        let get = |key: &str| script.lua().globals().get::<String>(key).ok();
        // `0x19`-`0x1b` (`0x6e1db7`, through `0x6e2380`): the singular subclass name, "Must have a
        // Wand equipped"; a multi-bit mask names its `ItemSubClassMask.dbc` group, else the first
        // subclass, where the tooltip (`0x52eea7`) uses the plural and joins. The pet's table fills
        // it too (`0x6e904d`).
        if let (0x19..=0x1b, Some(d), Some(subs)) = (reason, d, self.sub_classes.as_deref()) {
            if let Some(name) = (d.equipped_item_class >= 0)
                .then(|| {
                    subs.0.requirement_display_name(
                        d.equipped_item_class as u32,
                        d.equipped_item_subclass_mask,
                    )
                })
                .flatten()
            {
                let key = cast_fail::CAST_FAIL_KEYS[reason as usize];
                return get(key)
                    .filter(|s| !s.is_empty())
                    .map(|t| CastFailLine::passthrough(t.replace("%s", &name)));
            }
        }
        // `0x31` (`0x6e1e54`): the same helper for class 6 (Projectile), mask `1 << arg`, the shift
        // masked to five bits as x86 `shl` masks it (`0x6e1e5c`). The player's arm only: the pet's
        // table (`0x6e93d0`) has no `0x31` entry. vmangos never sends the word
        // (`Spell.cpp:4419-4443`), so against it this declines to the stem.
        if let (false, 0x31, Some(arg), Some(subs)) =
            (pet, reason, fail.arg, self.sub_classes.as_deref())
        {
            const ITEM_CLASS_PROJECTILE: u32 = 6;
            if let Some(name) = subs
                .0
                .requirement_display_name(ITEM_CLASS_PROJECTILE, 1u32 << (arg & 31))
            {
                let key = cast_fail::CAST_FAIL_KEYS[reason as usize];
                return get(key)
                    .filter(|s| !s.is_empty())
                    .map(|t| CastFailLine::passthrough(t.replace("%s", &name)));
            }
        }
        // `0x78` and `0x5c` (`0x6e1e7f`), the player's arms only (the pet's table sends both to its
        // generic arm): the name of argument 1's item, the entry `0x6e4000` found; a word-less
        // refusal names the failing slot as the pre-send check finds it.
        if !pet && (reason == 0x78 || reason == 0x5c) {
            let d = d?;
            let self_store = self.self_q.iter().next();
            let failing = fail.arg.or_else(|| {
                if reason == 0x78 {
                    self_store
                        .and_then(|s| first_missing_totem(d, s, &self.objects))
                        // No self store yet: name the first tool.
                        .or_else(|| d.totems.iter().copied().find(|&t| t != 0))
                } else {
                    self_store
                        .and_then(|s| first_short_reagent(d, s, &self.objects))
                        .or_else(|| d.reagents.iter().map(|&(id, _)| id).find(|&id| id != 0))
                }
            })?;
            // The failure handler's lookup with the callback `0x6e29b0` (`0x6e1ea4`) on a first
            // pass; its redisplay only reads.
            let ask = if fail.redisplay {
                Ask::Read
            } else {
                Ask::Event(&self.commands)
            };
            let cached = self.items.template(failing, 0, ask).map(|i| i.name.clone());
            let name = match cached {
                Some(name) => name,
                // Answered unknown: the reference's callback literal (`0x838044`).
                None if self.items.template_answered_unknown(failing) => "UNKNOWN".to_string(),
                None => {
                    // Pending: shown as a redisplay once it answers, never logged.
                    self.waiting.0.push(fail.requeued());
                    return None;
                }
            };
            let key = if reason == 0x78 {
                "SPELL_FAILED_TOTEMS"
            } else {
                "SPELL_FAILED_REAGENTS"
            };
            return get(key)
                .filter(|s| !s.is_empty())
                .map(|t| CastFailLine::passthrough(t.replace("%s", &name)));
        }
        cast_fail::cast_fail_text(
            caster,
            reason,
            d,
            cast_fail::FailArgs {
                arg: fail.arg,
                ..self.names.args(spells)
            },
            &get,
        )
    }

    /// The combat-log line, after the display as in the reference (`DisplayError` at `0x6e21dd`,
    /// the log formatter `0x62c360` at `0x6e21fc`), from the argText buffer
    /// ([`CastFailLine::logged`]). Nothing is logged for a redisplay (`0x6e29b0` skips the log)
    /// or for the pet (`0x6e8eb0` never calls `0x62c360`). `SMSG_CAST_RESULT` goes to the caster
    /// alone, so the `OTHER` keys stay unused.
    fn log_line(&self, line: &CastFailLine, fail: CastFail) -> Option<PendingCombat> {
        if fail.caster == Caster::Pet || fail.redisplay {
            return None;
        }
        // `0x62c360` writes nothing for an id out of range or with no record, for `Attributes &
        // 0x180` (`0x62c4bd`) or for an empty name (`0x62c4d7`).
        let display = crate::ui_chat::combat::logged_spell(self.spells.as_deref(), fail.spell_id)?;
        // `0x62aff0`: `Attributes` bit 4 marks an ability, which "performs", not "casts".
        const ATTR_IS_ABILITY: u32 = 0x10;
        let family = if display.attributes & ATTR_IS_ABILITY != 0 {
            crate::ui_chat::combat::SPELLFAILPERFORM
        } else {
            crate::ui_chat::combat::SPELLFAILCAST
        };
        Some(PendingCombat {
            kind: crate::ui_chat::ChatEventKind::SpellFailedLocalPlayer,
            family,
            variant: crate::ui_chat::combat::Variant::SelfOther,
            subject: 0,
            object: 0,
            fills: crate::ui_chat::combat::Fills {
                spell: display.name.clone(),
                named: line.logged().to_string(),
                ..Default::default()
            },
            named: crate::ui_chat::combat::Named::Ready,
            tries: 0,
            // `0x62c360` raises no `COMBAT_TEXT_UPDATE`: it writes its line (`0x62c5b3`) and
            // returns.
            word: None,
        })
    }
}
