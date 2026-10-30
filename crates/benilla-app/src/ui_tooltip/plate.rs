//! The game object and corpse plates as their builders read them at the call (`0x52aa20`,
//! `0x52aef0`), the call host's half of their questions ([`ObjectPlateQuery`],
//! [`CorpsePlateQuery`]): the world publisher's build at a change of the mouseover and the cache
//! callbacks' rebuilds inside the answer packets (`0x52ad10`, `0x52afe0`) read the object, its
//! record and the caches as they stand there. A lookup that misses registers its callback, as the
//! builder's own does: the lock key's on the item record (`0x52aca6`), the owner's on the name
//! (`0x52af77`), each with the dedupe flag (`0x52ac8b`, `0x52af55`).

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::live::LiveAsk;
use benilla_ui::script::{
    CorpsePlateQuery, KeyedArg, ObjectPlate, ObjectPlateQuery, TooltipText, TooltipTint,
};

use crate::go_templates::MeetingStoneTemplate;
use crate::names::NameCache;
use crate::net::{NetCommands, SelfPlayer};
use crate::query_cache::Ask;
use crate::target::GO_FLAG_LOCKED;

/// The "Locked" line's colour (`0x52ab03`-`0x52ab43`): red `0xc0d3a8` when the resolver finds no
/// opener; else green `0xc0d420` for a key (`0x52ab29`) or no requirement (`0x52ab22`), and the
/// difficulty ramp `0x529fa0` of the opening spell's value against the requirement (`0x52ab31`).
/// `None`, a flag-locked object with no `Lock.dbc` row, is no requirement.
pub(super) fn locked_line_tint(
    outcome: Option<crate::target::lock::LockOutcome>,
    matched: Option<crate::target::lock::LockMatch>,
) -> TooltipTint {
    use crate::target::lock::LockOutcome;
    match (outcome, matched) {
        (Some(LockOutcome::Unmet), _) => TooltipTint::Red,
        (Some(LockOutcome::OpenBySpell(_)), Some(m)) => TooltipTint::Skill {
            required: m.required,
            value: m.points,
        },
        _ => TooltipTint::LockOpen,
    }
}

/// What the GameObject builder's lock lines read of the hovered object.
pub(super) struct GoLock<'a> {
    /// Its `Lock.dbc` row, `None` without one.
    pub(super) slots: Option<&'a [benilla_formats::LockSlot; benilla_formats::MAX_LOCK_SLOTS]>,
    /// The stored `GAMEOBJECT_STATE`, which the slot's Action gate reads.
    pub(super) state: u32,
    pub(super) flag_locked: bool,
    /// The resolver's verdict and the opener it last matched (`0x5f83d0`).
    pub(super) outcome: Option<crate::target::lock::LockOutcome>,
    pub(super) matched: Option<crate::target::lock::LockMatch>,
}

/// The type handler's lines, slot 33 of its table (`[vtbl+0x84]`, called at `0x52aad4`) after
/// the name and before the lock lines; the base slot adds none (`0x5f9f10`). A meeting stone's
/// (`0x5f6b10`) adds its area's `AreaTable` name when the row exists (`0x5f6b3d`-`0x5f6b68`),
/// then `MEETINGSTONE_LEVEL` of its minimum and maximum levels (`0x5f6b6d`-`0x5f6bdf`), both in
/// gold (`0x530380`).
pub(super) fn go_handler_lines(
    stone: Option<MeetingStoneTemplate>,
    area_name: &dyn Fn(u32) -> Option<String>,
) -> Vec<(TooltipText, TooltipTint)> {
    let Some(stone) = stone else {
        return Vec::new();
    };
    let mut lines = Vec::new();
    lines.extend(area_name(stone.area).map(|name| (name.into(), TooltipTint::Gold)));
    let levels = vec![
        KeyedArg::D(i64::from(stone.min_level)),
        KeyedArg::D(i64::from(stone.max_level)),
    ];
    lines.push((
        TooltipText::keyed("MEETINGSTONE_LEVEL", levels),
        TooltipTint::Gold,
    ));
    lines
}

/// The builder's lock lines (`0x52aa20`): "Locked" iff `GO_FLAG_LOCKED` (`0x52aae5`), then one line
/// from `Lock.dbc` slot 0 only, if it passes the Action gate (`0x52ab7e`): a key in white
/// (`0x52acd9`), named by its item, which waits for the record (`0x52aca6`); a skill, named by its
/// `LockType.dbc` row or the literal `UNKNOWN` (`0x838044`), as `LOCKED_WITH_SPELL_KNOWN` in the
/// margin ramp's colour when a known spell opens its lock type, sufficient or not
/// (`0x52abcb`-`0x52abe2`), else as `LOCKED_WITH_SPELL` in red, and on a flag-locked object not at
/// all (`0x52abe9`-`0x52ac04`), so a locked door names its key, never a lockpicking the player
/// lacks. The keys all read "Requires %s" in enUS; only the binary tells them apart. A line whose
/// template is missing or fills to nothing adds nothing (`0x5302a9`).
pub(super) fn go_lock_lines(
    lock: GoLock<'_>,
    key_name: &dyn Fn(u32) -> Option<String>,
    lock_type_name: &dyn Fn(u32) -> Option<String>,
) -> Vec<(TooltipText, TooltipTint)> {
    let mut lines = Vec::new();
    if lock.flag_locked {
        lines.push((
            TooltipText::keyed("LOCKED", Vec::new()),
            locked_line_tint(lock.outcome, lock.matched),
        ));
    }
    let Some(slot0) = lock
        .slots
        .map(|s| s[0])
        .filter(|s| s.available(lock.state, lock.flag_locked))
    else {
        return lines;
    };
    match slot0.key_type {
        benilla_formats::LOCK_KEY_ITEM => {
            if let Some(name) = key_name(slot0.index) {
                let text = TooltipText::keyed("LOCKED_WITH_ITEM", vec![KeyedArg::S(name)]);
                lines.push((text, TooltipTint::White));
            }
        }
        benilla_formats::LOCK_KEY_SKILL => {
            let line = match lock.matched {
                Some(m) => Some((
                    "LOCKED_WITH_SPELL_KNOWN",
                    TooltipTint::Skill {
                        required: m.required,
                        value: m.points,
                    },
                )),
                None if lock.flag_locked => None,
                None => Some(("LOCKED_WITH_SPELL", TooltipTint::Red)),
            };
            if let Some((key, tint)) = line {
                let word = lock_type_name(slot0.index).unwrap_or_else(|| "UNKNOWN".into());
                lines.push((TooltipText::keyed(key, vec![KeyedArg::S(word)]), tint));
            }
        }
        _ => {}
    }
    lines
}

/// What the game object and corpse builders read as they run: the object by its guid as of the
/// place they run, its record and lock chain (shared with the click router, so hover and click
/// agree on whether a lock can be opened), the known spells the resolver's skill arm scans, the
/// player, the area names a meeting stone reads, and the name cache a corpse names its owner by.
#[derive(SystemParam)]
pub(crate) struct PlateLive<'w, 's> {
    index: Res<'w, crate::net::GuidIndex>,
    stores: crate::ui_unit::views::ViewStores<'w, 's>,
    me: Query<'w, 's, Entity, With<SelfPlayer>>,
    /// The stored `GAMEOBJECT_STATE` the lock lines' Action gate reads.
    anims: Query<'w, 's, &'static crate::go_anim::GoAnim>,
    go: crate::target::lock::GoLockInputs<'w, 's>,
    actions: Res<'w, crate::ui_action::PlayerActions>,
    areas: Option<Res<'w, crate::area::AreaTableRes>>,
    names: Res<'w, NameCache>,
    commands: Option<Res<'w, NetCommands>>,
}

impl PlateLive<'_, '_> {
    /// `guid`'s store as of here, with its entity.
    fn held(&self, guid: u64) -> Option<(Entity, &crate::net::ObjectStore)> {
        let entity = self.index.0.get(&guid).copied()?;
        Some((entity, self.stores.get(entity).ok()?))
    }

    /// Where a miss is asked: inside the build, with its callback.
    fn ask(&self) -> Ask<'_> {
        self.commands.as_deref().map_or(Ask::Read, Ask::New)
    }

    /// `0x52aa20` over the game object `guid` names (mask 0x20, `0x52aa5e`): its record's name
    /// (`0x5f8100`), the type handler's lines, then the lock lines. An object whose record has not
    /// landed has no name (`0x5f8100` gives `""`, which adds no line, `0x5302a9`), no lock row
    /// (`0x5f8180`) and no opener (`0x5f83d0` returns 0, `0x5f8418`), so its plate is the
    /// descriptor's "Locked" line alone, in green (`0x52ab22`).
    fn object_plate(&self, guid: u64) -> Option<ObjectPlate> {
        let (entity, store) = self.held(guid)?;
        if store.0.object_type_mask().unwrap_or(0) & 0x20 == 0 {
            return None;
        }
        let template = self.go.templates.of(&store.0);
        let flag_locked = store.0.gameobject_flags() & GO_FLAG_LOCKED != 0;
        let state = crate::go_anim::go_state(self.anims.get(entity).ok(), store);
        let slots = template
            .map(|t| t.lock_id)
            .filter(|&id| id != 0)
            .zip(self.go.locks.as_ref())
            .and_then(|(id, l)| l.0.slots(id));
        // The resolver the click uses (`0x5f83d0`), which the builder asks for both lines
        // (`0x52ab14`, `0x52abc6`): its verdict and the opener it last matched.
        let facts = crate::target::lock::go_facts(Some((store, state)));
        let me = self.me.single().ok().and_then(|e| self.stores.get(e).ok());
        let mut matched = None;
        let outcome = slots.map(|slots| {
            crate::target::lock::resolve_lock(
                slots,
                &self.actions.openers,
                self.go.spells.as_deref(),
                self.go.skill_lines.as_ref().map(|s| &s.catalog),
                me,
                &self.go.objects,
                facts,
                &mut matched,
            )
        });
        let mut lines = go_handler_lines(template.and_then(|t| t.meeting_stone), &|id| {
            self.areas
                .as_deref()
                .and_then(|a| a.0.name(id))
                .map(str::to_string)
        });
        lines.extend(go_lock_lines(
            GoLock {
                slots,
                state,
                flag_locked,
                outcome,
                matched,
            },
            &|entry| {
                // The key's record, asked with the tooltip's callback on a miss (`0x52aca6`).
                let name = self
                    .go
                    .items
                    .template(entry, guid, self.ask())
                    .map(|t| t.name.clone());
                if name.is_none() {
                    self.go
                        .items
                        .wait_once(entry, crate::items::RecordCallback::LockKey);
                }
                name
            },
            &|index| {
                self.go
                    .lock_types
                    .as_deref()
                    .and_then(|c| c.0.row_name(index))
                    .map(str::to_string)
            },
        ));
        Some(ObjectPlate {
            name: template.map(|t| t.name.clone()).unwrap_or_default(),
            lines,
        })
    }

    /// `0x52aef0` over the corpse `guid` names (mask 0x80, `0x52af1b`): its owner's name from the
    /// name cache (`CORPSE_FIELD_OWNER`, the corpse's fields `+0`), asked with the tooltip's callback
    /// on a miss (`0x52af77`).
    fn corpse_plate(&self, guid: u64) -> Option<Option<String>> {
        let (_, store) = self.held(guid)?;
        if store.0.object_type_mask().unwrap_or(0) & 0x80 == 0 {
            return None;
        }
        let Some(owner) = store.0.corpse_owner() else {
            return Some(None);
        };
        let name = self
            .names
            .resolve_player(owner, self.ask())
            .map(str::to_string);
        if name.is_none() {
            self.names
                .wait_for_player(owner, crate::names::NameCallback::CorpsePlate);
        }
        Some(name)
    }
}

impl crate::script_calls::live::LiveFamily for PlateLive<'_, '_> {
    fn answer(&mut self, ask: &mut LiveAsk<'_>) -> bool {
        ask.answer(|q: &ObjectPlateQuery| Some(self.object_plate(q.0)))
            || ask.answer(|q: &CorpsePlateQuery| Some(self.corpse_plate(q.0)))
    }
}
