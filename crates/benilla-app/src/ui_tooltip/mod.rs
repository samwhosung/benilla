//! The world-mouseover tooltip and the spell tooltip views. The world tooltip rebuilds once per
//! hover-target change and a lost hover arms a fade; the pick is [`Hovered`] or
//! [`HoveredObject`], arbitrated by [`go_is_nearest`] as the click router does. The spell views
//! are built and pushed ahead of a hover by [`spell_feed`].

use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use benilla_ui::script::{TooltipTint, UiScript, UnitState};
use benilla_ui::strings::Arg;

use crate::names::NameCache;
use crate::net::{NetCommands, ObjectStore, SelfPlayer};
use crate::target::{go_is_nearest, Hovered, HoveredObject, GO_FLAG_LOCKED, GO_TYPE_GENERIC};
use crate::ui_script::UiFeed;
use crate::ui_trainer::TrainerFeed;
use crate::ui_unit::{enrich_unit, snapshot, unit_reaction, SnapshotTables, UnitFeed};

mod spell_deps;
mod spell_feed;
use spell_feed::feed_spell_tooltips;

pub struct UiTooltipPlugin;

impl Plugin for UiTooltipPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                drive_mouseover_tooltip.in_set(UnitFeed),
                // After the trainer and quest feeds, so a list that lands this frame is hoverable
                // in its tick; outside `UnitFeed`, which the trainer feed follows, so it takes that
                // set's gate.
                feed_spell_tooltips
                    .in_set(UiFeed)
                    .after(TrainerFeed)
                    .after(crate::ui_quest::feed_quest)
                    .run_if(crate::ui_script::ingame_ui_up),
            ),
        );
    }
}

/// Fill a key's template from the VM's `GlobalStrings.lua`; no string shows nothing.
fn keyed(get: &dyn Fn(&str) -> Option<String>, key: &str, args: &[Arg<'_>]) -> Option<String> {
    let text = benilla_ui::strings::fill(&get(key)?, args);
    (!text.is_empty()).then_some(text)
}

/// What the world tooltip was last built for; the reference rebuilds once per hover-target change.
#[derive(Default, PartialEq, Clone, Copy)]
enum LastHover {
    #[default]
    None,
    Unit(u64),
    Go(u64),
    /// A hovered corpse object, keyed on the corpse's guid.
    Corpse(u64),
}

/// The world-hover driver's memory, one `SystemParam` because the driver is at Bevy's 16-param
/// ceiling.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct HoverMemo<'s> {
    /// Which world plate is up. On an unchanged mouseover the reference makes no call (`0x482090`
    /// returns at `0x4820b5`), so a plate Lua takes mid-hover stays gone.
    last: Local<'s, crate::ui_script::VmMemo<LastHover>>,
    /// The line-affecting fields the current unit plate was built from, so a late-arriving name or
    /// creature-info rebuilds it under the same hover.
    last_lines: Local<'s, crate::ui_script::VmMemo<Option<UnitState>>>,
    /// The hover probe's last trace line, so a stationary probe logs it once.
    trace: Local<'s, String>,
    /// Whether the VM holds a `"mouseover"` unit, so the clear is pushed once.
    published: Local<'s, crate::ui_script::VmMemo<bool>>,
}

fn lines_view(s: &UnitState) -> UnitState {
    UnitState {
        health: 0,
        max_health: 0,
        power: 0,
        max_power: 0,
        ..s.clone()
    }
}

/// The "Locked" line's colour (`0x52ab03`-`0x52ab43`): red `0xc0d3a8` unless the resolver finds an
/// opener, then green `0xc0d420`, for a key (`0x52ab29`) or no requirement (`0x52ab22`); `None`, a
/// flag-locked object with no `Lock.dbc` row, is no requirement. A skill opener takes the
/// reference's difficulty ramp `0x529fa0` (grey, green, yellow, orange, red at +0/25/50/100);
/// benilla shows its green, since the resolver discards the margin.
fn locked_line_tint(outcome: Option<crate::target::lock::LockOutcome>) -> TooltipTint {
    match outcome {
        Some(crate::target::lock::LockOutcome::Unmet) => TooltipTint::Red,
        _ => TooltipTint::LockOpen,
    }
}

fn drive_mouseover_tooltip(
    script: Option<NonSendMut<UiScript>>,
    hovered: Res<Hovered>,
    hovered_go: Res<HoveredObject>,
    // Seats the cursor-anchored GameObject plate.
    window: Query<&Window, With<PrimaryWindow>>,
    stores: Query<&ObjectStore>,
    // The stored GAMEOBJECT_STATE the lock lines' Action gate reads.
    anims: Query<&crate::go_anim::GoAnim>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    rx: crate::target::ReactionInputs,
    // The lock chain's data, shared with the click router (`target::lock`) so hover and click
    // agree on whether a lock can be opened.
    go_inputs: crate::target::lock::GoLockInputs,
    // The known-spell set the resolver's SKILL arm scans.
    player_actions: Res<crate::ui_action::PlayerActions>,
    mut memo: HoverMemo,
    // `ChrClasses.dbc` field 16, `UnitHasRelicSlot`'s input, and the form table the creature type
    // reads.
    tables: SnapshotTables,
) {
    let Some(mut script) = script else {
        return;
    };
    let (last, last_lines, trace) = (&mut memo.last, &mut memo.last_lines, &mut *memo.trace);
    let last = last.get(&script);
    let last_lines = last_lines.get(&script);
    let published = memo.published.get(&script);
    let self_store = self_q.iter().next();
    let chr = tables.classes();
    let types = tables.types(&names);

    let unit = hovered.mouseover(&hovered_go).and_then(|(entity, guid)| {
        let store = stores.get(entity).ok()?;
        let name = names
            .resolve_unit(guid, Some(store), &commands)
            .map(str::to_string);
        let reaction = unit_reaction(rx.factions.as_deref(), &rx.reputations, store, self_store);
        // The hovered guid, the pair `0x492890` writes to `0xb4e2c8`/`0xb4e2cc` and the token
        // resolver `0x515970` reads for `"mouseover"`, so `UnitIsUnit` can match it.
        let mut s = snapshot(store, guid, name, reaction, chr, types);
        enrich_unit(
            &mut s,
            guid,
            &names,
            store,
            rx.factions.as_deref(),
            self_store,
        );
        Some((guid, s))
    });
    // The hovered GameObject, when nothing else was picked or it is nearer. Not gated on
    // highlightability: the publisher `0x492890` sends every object to the builder by kind, so a
    // signpost or an in-use object shows its plate with no interact cursor.
    let go = hovered_go.target.zip(hovered_go.guid).filter(|_| {
        (unit.is_none() && hovered.corpse.is_none()) || go_is_nearest(&hovered, &hovered_go)
    });

    if let Some((guid, state)) = unit {
        // Push first: the builder reads the token. Rebuild on a new target or a late name or
        // creature answer; health and power stay out of the key, since the watcher drives the bar.
        let key = lines_view(&state);
        script.set_unit("mouseover", Some(state));
        *published = true;
        if *last != LastHover::Unit(guid) || last_lines.as_ref() != Some(&key) {
            script.world_tooltip_unit("mouseover");
            *last = LastHover::Unit(guid);
            *last_lines = Some(key);
        }
        return;
    }
    // No unit won the pick: the publisher's teardown zeroes the mouseover pair (`0x4928e8`,
    // `0x4928f2`) and writes a null, corpse or GameObject guid, none of which resolves as a unit,
    // so `"mouseover"` names nobody from this frame. A fading plate keeps its lines and its bar,
    // which follows the unit's guid, not the token; the null publish fires no event (the sole
    // `UPDATE_MOUSEOVER_UNIT` is the unit arm's, `0x4929b3`).
    if std::mem::take(published) {
        script.set_unit("mouseover", None);
    }
    // The corpse plate, "Corpse of <owner>" (`0x52aef0`): a name and nothing else, corner-seated.
    // The publisher fires no event for a corpse, so `UnitName("mouseover")` on one stays nil.
    if let Some((entity, guid)) = hovered
        .corpse
        .zip(hovered.corpse_guid)
        .filter(|_| go.is_none())
    {
        let Some(store) = stores.get(entity).ok() else {
            return;
        };
        // A bone pile with nothing to take publishes no mouseover: no plate, no brighten.
        if !crate::target::corpse_mouseover_eligible(store) {
            if !matches!(*last, LastHover::None) {
                script.world_tooltip_fade();
                *last = LastHover::None;
            }
            return;
        }
        if *last == LastHover::Corpse(guid) {
            return; // already up; a corner plate never re-seats
        }
        // A corpse carries `CORPSE_FIELD_OWNER`, not a name; the plate waits for the name query.
        let Some(owner) = store.0.corpse_owner() else {
            return;
        };
        let Some(name) = names.resolve(owner, &commands).map(str::to_string) else {
            return;
        };
        // `CORPSE_TOOLTIP`, the builder's key; no string, no plate.
        let Some(plate) = keyed(
            &|key: &str| benilla_ui::strings::global(script.lua(), key),
            "CORPSE_TOOLTIP",
            &[Arg::S(&name)],
        ) else {
            return;
        };
        script.world_tooltip_gameobject(&plate, &[], None);
        *last = LastHover::Corpse(guid);
        return;
    }
    if let Some((entity, guid)) = go {
        // The plate follows the cursor iff `[vtbl+0x5c]` (`0x5f8630`) finds
        // `data[0x621b00(type, 0x13)]` set, and semantic `0x13` exists only for GENERIC(5), at
        // `data[0]`. `data[1]` (`0x5f4830`) is whether it can be hovered, not where it sits.
        let cursor_seated = stores.get(entity).map(|s| s.0.gameobject_type_id())
            == Ok(GO_TYPE_GENERIC)
            && go_inputs
                .templates
                .get(guid)
                .is_some_and(|t| t.floating_tooltip);
        // Window px to the VM's y-up root units, as the input seam converts.
        let cursor_ui = cursor_seated
            .then(|| {
                window.iter().next().and_then(|w| {
                    // The hover probe's aim stands in for a missing cursor; a real pointer wins.
                    w.cursor_position()
                        .or_else(crate::target::hover_probe_point)
                        .map(|c| {
                            let at = crate::ui_script::window_to_ui(w.height(), c);
                            (at.x, at.y)
                        })
                })
            })
            .flatten();
        if *last == LastHover::Go(guid) {
            if crate::target::hover_probe_armed() {
                let line = format!(
                    "held Go({guid:#x}) — plate up {}",
                    script.world_tooltip_up()
                );
                if *trace != line {
                    info!("hover probe/tooltip: {line}");
                    *trace = line;
                }
            }
            // The cursor arm follows the pointer; the corner arm has nothing to re-seat.
            if let Some((x, y)) = cursor_ui {
                script.world_tooltip_move(x, y);
            }
            return;
        }
        if cursor_seated && cursor_ui.is_none() {
            return; // cursor off-window: nothing to seat the plate against
        }
        if crate::target::hover_probe_armed() {
            info!(
                "hover probe/tooltip: guid {guid:#x} cursor_seated {cursor_seated} cursor_ui \
                 {cursor_ui:?} template {:?}",
                go_inputs.templates.get(guid).map(|t| t.name.clone()),
            );
        }
        let Some(template) = go_inputs.templates.get(guid).cloned() else {
            // Template in flight: ask once, and show it when it lands.
            go_inputs.templates.request(guid, &commands);
            return;
        };
        // The builder's lock lines (`0x52aa20`): "Locked" iff `GO_FLAG_LOCKED` (`0x52aae5`), then
        // one line from `Lock.dbc` slot 0 only, if it passes the Action gate (`0x52ab7e`): a key in
        // white (`0x52acd9`), an unknown skill in red but only on an unflagged object (`0x52abf7`),
        // so a locked door names its key, never its lockpicking.
        let go_store = stores.get(entity).ok();
        let flags = go_store.map_or(0, |s| s.0.gameobject_flags());
        let flag_locked = flags & GO_FLAG_LOCKED != 0;
        let state = go_store.map_or(benilla_formats::GO_STATE_ACTIVE, |s| {
            crate::go_anim::go_state(anims.get(entity).ok(), s)
        });
        let slots = go_inputs
            .locks
            .as_ref()
            .filter(|_| template.lock_id != 0)
            .and_then(|l| l.0.slots(template.lock_id));
        let mut lines: Vec<(String, TooltipTint)> = Vec::new();
        // The lock lines' keys all read "Requires %s" in enUS; only the binary tells them apart.
        let go_get = |key: &str| benilla_ui::strings::global(script.lua(), key);
        if flag_locked {
            // Coloured by whether the player can open it: the builder asks the resolver the click
            // uses (`0x52ab14` → `0x5f83d0`), and `locked_line_tint` maps the answer.
            let facts = crate::target::lock::go_facts(go_store.map(|s| (s, state)));
            let mut matched = None;
            let outcome = slots.map(|slots| {
                crate::target::lock::resolve_lock(
                    slots,
                    &player_actions.spells,
                    go_inputs.spells.as_deref(),
                    go_inputs.skill_lines.as_ref().map(|s| &s.catalog),
                    self_store,
                    &go_inputs.objects,
                    facts,
                    &mut matched,
                )
            });
            if let Some(text) = keyed(&go_get, "LOCKED", &[]) {
                lines.push((text, locked_line_tint(outcome)));
            }
        }
        if let Some(slot0) = slots
            .map(|s| s[0])
            .filter(|s| s.available(state, flag_locked))
        {
            match slot0.key_type {
                benilla_formats::LOCK_KEY_ITEM => {
                    if let Some(t) = go_inputs.items.template(slot0.index, 0, &commands) {
                        // `LOCKED_WITH_ITEM`, at `0x52acb8`.
                        if let Some(text) = keyed(&go_get, "LOCKED_WITH_ITEM", &[Arg::S(&t.name)]) {
                            lines.push((text, TooltipTint::White));
                        }
                    }
                }
                // Every skill lock takes the opener-unknown arm, silent on a flag-locked object;
                // the known arm, `LOCKED_WITH_SPELL_KNOWN` in the margin ramp's colour
                // (`0x529fa0`), is not built.
                benilla_formats::LOCK_KEY_SKILL if !flag_locked => {
                    // `LOCKED_WITH_SPELL` (`0x52ac04`), named by `LockType.dbc` ("Pick Lock"); the
                    // reference takes the known arm iff any known spell opens it (`0x52abcb`).
                    let word = go_inputs
                        .lock_types
                        .as_deref()
                        .and_then(|c| c.0.name(slot0.index));
                    if let Some(text) =
                        word.and_then(|w| keyed(&go_get, "LOCKED_WITH_SPELL", &[Arg::S(w)]))
                    {
                        lines.push((text, TooltipTint::Red));
                    }
                }
                _ => {}
            }
        }
        script.world_tooltip_gameobject(&template.name, &lines, cursor_ui);
        *last = LastHover::Go(guid);
        return;
    }
    if !matches!(*last, LastHover::None) {
        // Hover lost: arm the fade (`0x492909` → `0x530ae0`).
        script.world_tooltip_fade();
        *last = LastHover::None;
    }
}

#[cfg(test)]
mod bench;
#[cfg(test)]
mod deps_tests;
#[cfg(test)]
mod feed_tests;
#[cfg(test)]
mod tests;
