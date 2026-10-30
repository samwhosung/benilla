//! The world-mouseover tooltip and the spell tooltip views. The world tooltip is built once per
//! hover-target change, published through the VM ([`UiScript::publish_world_mouseover`]), whose
//! change requests the fade, and built again under the same hover only inside the packets, by the
//! callbacks its subject's lookups and watchers run there ([`plate`],
//! [`crate::field_watch::FieldNotice::TooltipUnit`]); the pick is [`Hovered`] or
//! [`HoveredObject`], arbitrated by [`go_is_nearest`] as the click router does. The pick stands for the WorldFrame's own update
//! (`0x481790`), which picks only while the WorldFrame holds the hover (`0x4817ae`); the loss when
//! the cursor moves onto a window is published by the WorldFrame's leave in the UI's walk, before
//! the window's `OnEnter`. The spell views are built at the setter's call ([`live`]).

use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use crate::names::NameCache;
use crate::net::{NetCommands, ObjectStore, SelfPlayer};
use crate::query_cache::Ask;
use crate::target::{go_is_nearest, Hovered, HoveredObject, GO_TYPE_GENERIC};
use crate::ui_unit::{
    enrich_unit, player_reaction, snapshot, unit_reaction, SnapshotTables, UnitFeed,
};

mod live;
mod plate;
mod spell_view;
pub(crate) use live::TooltipLive;
pub(crate) use plate::PlateLive;
pub(crate) use spell_view::tooltip_text;

pub struct UiTooltipPlugin;

/// The frame's world-mouseover publish ([`UiScript::publish_world_mouseover`]), which the readers
/// of the published guid follow.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct MouseoverPublish;

impl Plugin for UiTooltipPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            drive_mouseover_tooltip
                .in_set(UnitFeed)
                .in_set(MouseoverPublish),
        );
    }
}

/// The world-hover driver's memory, one `SystemParam` because the driver is at Bevy's 16-param
/// ceiling. What it last published is the VM's ([`UiScript::world_mouseover`]), which a hover
/// leave in the UI's walk also writes.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct HoverMemo<'s> {
    /// The hover probe's last trace line, so a stationary probe logs it once.
    trace: Local<'s, String>,
    /// Whether the VM holds a `"mouseover"` unit, so the clear is pushed once.
    published: Local<'s, crate::ui_script::VmMemo<bool>>,
    /// What the last frame hovered, unit, corpse and object: a hover onto anything else is the
    /// build, whose name lookup passes a callback (`GetUnitName` `0x609210` from `0x529fe0`); the
    /// frames under the same hover read.
    hovering: Local<'s, Option<HoverKey>>,
}

/// What the pointer is over: the picked unit, corpse and object guids.
type HoverKey = (Option<u64>, Option<u64>, Option<u64>);

/// A fresh hover's unit, seated where it is published: the publisher writes the mouseover pair
/// (`0x492890`) before it signals (`0x4929b3`), and `CanInspect` (`0x48a1b0`),
/// `CheckInteractDistance` (`0x48ba00`) and the slot getters (`0x4c8328`) read the object it names
/// at once. So the resolver holds it from the publish on; the range verbs measure it and the slot
/// getters read its visible items through the lent host as they run.
fn seat_fresh_hover(
    script: &mut benilla_ui::script::UiScript,
    guid: u64,
    entity: Entity,
    stores: &Query<&ObjectStore>,
) {
    let Ok(store) = stores.get(entity) else {
        return;
    };
    script.seat_world_mouseover_unit(guid, store.0.unit_target().unwrap_or(0));
}

/// What the hover reads of the relation inputs, taken before any Lua runs, as a call its
/// handlers make can write them.
struct HoverRelations {
    factions: Option<crate::target::Factions>,
    /// The hovered unit's.
    unit: Option<HoveredRelations>,
}

#[derive(Clone, Copy)]
struct HoveredRelations {
    /// `UnitReaction(unit, "player")` and the player's toward it.
    reaction: u8,
    player_reaction: u8,
}

pub(crate) fn drive_mouseover_tooltip(
    // With `widescreen`, a CVar knob the call host's callbacks write, which seats the
    // cursor-anchored GameObject plate on the interface's screen.
    mut vm: crate::script_calls::VmWith<(
        crate::target::RelationWorld<'static, 'static>,
        Option<Res<'static, crate::video::Widescreen>>,
    )>,
    hovered: Res<Hovered>,
    hovered_go: Res<HoveredObject>,
    window: Query<&Window, With<PrimaryWindow>>,
    stores: Query<&ObjectStore>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    // The game object records, whose floating-tooltip column seats the plate (`0x5f8630`).
    templates: Res<crate::go_templates::GameObjectTemplates>,
    mut memo: HoverMemo,
    // `ChrClasses.dbc` field 16, `UnitHasRelicSlot`'s input, and the form table the creature type
    // reads.
    tables: SnapshotTables,
    // The guild cache, the hovered player's guild leg.
    guild: Res<crate::ui_guild::GuildState>,
) {
    let (hover_relations, widescreen) = {
        let (relations, widescreen) = vm.own();
        let me = relations.me();
        let hover_relations = HoverRelations {
            factions: relations.factions().cloned(),
            unit: hovered.mouseover(&hovered_go).and_then(|(entity, guid)| {
                let store = stores.get(entity).ok()?;
                let rel = relations.get();
                let subject = relations.unit_of(guid, store);
                Some(HoveredRelations {
                    reaction: unit_reaction(&rel, &subject, me.as_ref()),
                    player_reaction: player_reaction(&rel, &subject, me.as_ref()),
                })
            }),
        };
        (hover_relations, widescreen.as_deref().copied())
    };
    vm.run(|script| {
        let Some(script) = script else {
            return;
        };
        // The WorldFrame picks nothing in freelook (`0x483e80` sets `[wf+0x38c]&2`), from the
        // call that entered it (`0x483ec3`): a pick taken before that call names nobody.
        let (hovered, hovered_go) = if script.freelook_now() == Some(true) {
            let blank = Hovered {
                distance: f32::MAX,
                ..Default::default()
            };
            let go = HoveredObject {
                distance: f32::MAX,
                ..Default::default()
            };
            (blank, go)
        } else {
            (*hovered, *hovered_go)
        };
        let trace = &mut *memo.trace;
        // The publisher's change check (`0x482090`) compares against the VM's last publish.
        let last = script.world_mouseover();
        let published = memo.published.get(script);
        let self_store = self_q.iter().next();
        let chr = tables.unit_names();
        let types = tables.types(&names);

        // A plate's pick stands for its enter's publish (`0x7cb850`), so it holds only while that
        // plate holds the hover: a plate retired after the pick was taken has run its leave
        // (`0x7cb8b0`), which published the loss, and the hover re-picks to the WorldFrame.
        if hovered.plate && script.hovered_nameplate() != hovered.guid {
            if std::mem::take(published) {
                script.set_unit("mouseover", None);
            }
            script.publish_world_mouseover(None);
            return;
        }
        let hover: HoverKey = (
            hovered.mouseover(&hovered_go).map(|(_, guid)| guid),
            hovered.corpse_guid,
            hovered_go.guid,
        );
        let ask = Ask::at(memo.hovering.replace(hover) != Some(hover), &commands);
        let unit = hovered.mouseover(&hovered_go).and_then(|(entity, guid)| {
            let store = stores.get(entity).ok()?;
            let name = names
                .resolve_unit(guid, Some(store), ask)
                .map(str::to_string);
            let hovered_rel = hover_relations.unit?;
            // The hovered guid, the pair `0x492890` writes to `0xb4e2c8`/`0xb4e2cc` and the token
            // resolver `0x515970` reads for `"mouseover"`, so `UnitIsUnit` can match it.
            let mut s = snapshot(store, guid, name, hovered_rel.reaction, chr, types);
            s.player_reaction = hovered_rel.player_reaction;
            // `GetGuildInfo` reads any token's object (`0x4c9382`), its guild from the cache.
            s.guild = crate::ui_guild::cached_unit_guild(&store.0, &guild);
            enrich_unit(
                &mut s,
                guid,
                &names,
                store,
                hover_relations.factions.as_ref(),
                self_store,
            );
            let object = |g: u64| stores.get(tables.entity(g)?).ok();
            let sources = crate::ui_unit::tooltip_facts::TooltipSources {
                names: &names,
                catalog: tables.catalog(),
                object: &object,
                study: tables.study(),
            };
            s.tooltip =
                crate::ui_unit::tooltip_facts::tooltip_facts(store, s.creature_type, &sources);
            Some((guid, s))
        });
        // The hovered GameObject, when nothing else was picked or it is nearer. Not gated on
        // highlightability: the publisher `0x492890` sends every object to the builder by kind, so
        // a signpost or an in-use object shows its plate with no interact cursor.
        let go = hovered_go.target.zip(hovered_go.guid).filter(|_| {
            (unit.is_none() && hovered.corpse.is_none()) || go_is_nearest(&hovered, &hovered_go)
        });

        if let Some((guid, state)) = unit {
            // Publish, which tears the old hover down, then push: the builder reads the token.
            // The plate is built on a change of the mouseover alone (`0x482090`); under the same
            // hover it is rebuilt inside the packets, by the unit's callbacks
            // ([`crate::field_watch::FieldNotice::TooltipUnit`]), and its bar by its health watcher.
            let changed = script.publish_world_mouseover(Some(guid));
            if let Some((entity, _)) = hovered.mouseover(&hovered_go).filter(|_| changed) {
                seat_fresh_hover(script, guid, entity, &stores);
            }
            script.set_unit("mouseover", Some(state));
            *published = true;
            if changed {
                script.world_tooltip_unit("mouseover");
            }
            return;
        }
        // No unit won the pick: the publisher's teardown zeroes the mouseover pair (`0x4928e8`,
        // `0x4928f2`) and writes a null, corpse or GameObject guid, none of which resolves as a
        // unit, so `"mouseover"` names nobody from this frame. A fading plate keeps its lines and
        // its bar, which follows the unit's guid, not the token; the null publish fires no event
        // (the sole `UPDATE_MOUSEOVER_UNIT` is the unit arm's, `0x4929b3`).
        if std::mem::take(published) {
            script.set_unit("mouseover", None);
        }
        // The corpse plate, "Corpse of <owner>" (`0x52aef0`): a name and nothing else,
        // corner-seated, built on a change of the mouseover alone (`0x482090`); an owner's name the
        // cache lacks builds it again at the name's answer
        // ([`crate::field_watch::FieldNotice::CorpsePlate`]). The publisher fires no event for a
        // corpse, so `UnitName("mouseover")` on one stays nil.
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
                script.publish_world_mouseover(None);
                return;
            }
            if script.publish_world_mouseover(Some(guid)) {
                script.world_tooltip_corpse(guid);
            }
            return;
        }
        if let Some((entity, guid)) = go {
            // The plate follows the cursor iff `[vtbl+0x5c]` (`0x5f8630`) finds
            // `data[0x621b00(type, 0x13)]` set, and semantic `0x13` exists only for GENERIC(5), at
            // `data[0]`. `data[1]` (`0x5f4830`) is whether it can be hovered, not where it sits.
            let go_fields = stores.get(entity).ok().map(|s| &s.0);
            let cursor_seated = go_fields.map(|f| f.gameobject_type_id()) == Some(GO_TYPE_GENERIC)
                && go_fields
                    .and_then(|f| templates.of(f))
                    .is_some_and(|t| t.floating_tooltip);
            // Window px to the VM's y-up root units, as the input seam converts.
            let cursor_ui = cursor_seated
                .then(|| {
                    window.iter().next().and_then(|w| {
                        // The hover probe's aim stands in for a missing cursor; a real pointer
                        // wins.
                        w.cursor_position()
                            .or_else(crate::target::hover_probe_point)
                            .map(|c| {
                                let screen =
                                    crate::ui_script::UiScreen::of_window(w, widescreen.as_ref());
                                let at = crate::ui_script::window_to_ui(screen, c);
                                (at.x, at.y)
                            })
                    })
                })
                .flatten();
            if last == Some(guid) {
                if crate::target::hover_probe_armed() {
                    let line = format!("held Go({guid:#x})");
                    if *trace != line {
                        info!("hover probe/tooltip: {line}");
                        *trace = line;
                    }
                }
                // A cursor-seated plate follows the pointer in its own update (mode 6); the corner
                // arm has nothing to re-seat.
                return;
            }
            if cursor_seated && cursor_ui.is_none() {
                return; // cursor off-window: nothing to seat the plate against
            }
            if crate::target::hover_probe_armed() {
                info!(
                    "hover probe/tooltip: guid {guid:#x} cursor_seated {cursor_seated} cursor_ui \
                 {cursor_ui:?} template {:?}",
                    go_fields
                        .and_then(|f| templates.of(f))
                        .map(|t| t.name.clone()),
                );
            }
            if go_fields.is_none() {
                return; // not streamed: no descriptor, no entry
            }
            // The builder reads the object, its record and its lock chain as it runs
            // ([`PlateLive`]): a record not yet landed names nothing (`0x5f8100`), and a key the
            // cache lacks builds the plate again at the key's answer
            // ([`crate::item_watch::ItemNotice::LockKeyRecord`]).
            script.publish_world_mouseover(Some(guid));
            script.world_tooltip_object(guid, cursor_seated);
            return;
        }
        // Hover lost: the same teardown, with no plate after it.
        script.publish_world_mouseover(None);
    })
}

#[cfg(test)]
mod go_lock_tests;
#[cfg(test)]
mod hover_order_tests;
#[cfg(test)]
mod live_tests;
#[cfg(test)]
mod tests;
