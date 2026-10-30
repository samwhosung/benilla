//! Targeting: the selection, the mouseover pick, click-to-select and the ground selection ring
//! ([`ring`]). The pick is the reference's (`0x481190` → `0x7089c0`, [`hover::update_hover`]): a
//! nameplate under the pointer first, then the posed-mesh pick, clamped by the world trace; a click
//! acts on what the press was over ([`PressPick`]).

use bevy::prelude::*;

use crate::net::{ClientCommand, Guid, NetCommands, ObjectStore, SelfPlayer};
use crate::ui_script::InspectMode;
use benilla_assets::AssetSet;
use benilla_world::schedule::WorldStage;

mod arm;
#[cfg(test)]
pub(crate) use crate::script_calls::{
    click_plate, error_lines, record_error_lines, release_click, take_error_lines,
};
pub(crate) use arm::{
    begin_records, right_press_hook, run_world_clicks, world_steps, ArmTracer, ClickArm, ClickLegs,
    WorldClickLegs,
};
mod by_name;
// `pub(crate)` for the chest live probe, which drives the mouse's own `click::resolve_go_action`.
pub(crate) mod click;
#[cfg(test)]
mod click_order_tests;
#[cfg(test)]
mod record_click_tests;
// `pub(crate)` for the hover inspector, which runs the cursor's `cursor_mode::go_highlightable`.
pub(crate) mod cursor_mode;
mod died;
mod flash;
mod front;
#[cfg(test)]
mod front_tests;
#[cfg(test)]
mod gate_tests;
mod highlight;
pub(crate) mod hover;
mod live;
pub(crate) use live::SelectionLive;
mod hover_probe;

/// The hover probe's aim for a cursorless window, as this frame's pick published it.
pub(crate) fn hover_probe_point() -> Option<bevy::math::Vec2> {
    hover_probe::now()
}

/// Whether the headless hover probe is armed.
pub(crate) fn hover_probe_armed() -> bool {
    hover_probe::armed()
}
pub(crate) mod lock;
mod price_discount;
pub(crate) mod relations;
mod reticle;
// `pub(crate)` for the hover inspector too: its faction catalog feeds `go_highlightable`.
pub(crate) mod ring;
mod scan;
mod script;
pub(crate) use script::ScriptSelect;
mod release;
#[cfg(test)]
pub(crate) use release::Releases;
pub(crate) use release::{
    drop_parked_releases, member as raid_member_or_pet, object_held, unit_released, OwedReleases,
};
#[cfg(test)]
mod unheld_tests;

pub(crate) use cursor_mode::{
    corpse_mouseover_eligible, CursorKind, WorldCursor, GO_TYPE_GENERIC, SERVICE_RANGE_SQ,
};
/// `GO_FLAG_LOCKED`, the bit the lock chain and the GameObject tooltip's "Locked" line read.
pub(crate) use lock::GO_FLAG_LOCKED;
// `OnDied`, which the update packet's field callbacks run on a unit's death edge, and the health
// watcher's precast end after it.
pub(crate) use died::{dead_selection_ends_precast, died};
// This frame's combat-flash verdict, for the ring's material and the nameplate colour gate.
pub(crate) use flash::CombatFlash;
// The discount `0x612b80` the repair cost and the taxi fare take off their DBC prices.
pub(crate) use price_discount::vendor_price_discount;
#[cfg(test)]
pub(crate) use price_discount::{stormwind_fixture, HUMAN_WARRIOR};
// The one relation model, two units in: reaction, attack, assist, cooperation, interaction.
pub(crate) use relations::{
    corpse_friendly, is_possessed_by, is_selectable, RelUnit, RelationWorld, Relations, UnitLookup,
    UNIT_FLAG_POSSESSED,
};
// The attack validator's target half, run in the call that asks, for the player's Attack and the
// pet bar's; the melee probe's press with nothing selected.
#[cfg(test)]
pub(crate) use scan::{fire_owed_selects_lane, take_owed_deselects, LastEnemy};
pub(crate) use scan::{
    Acquire, AttackNearestRequest, AttackPick, CreatureTypes, Owed, OwedSelect, OwedSelects,
    OwedWire,
};
// The typemask `TargetByName`'s candidate feed shares.
pub(crate) use by_name::{assistable_alive, Candidacy, NameSearch, PlayerLookup};
// The one SetSelection path, for the loot response's select (`0x48f3a0`), and what a selecting
// call owes after its signals.
pub(crate) use by_name::{SelectCommit, SelectDebt};
// The follow's start gate, for `/follow` and for `FollowUnit` and `FollowByName` in their calls.
pub(crate) use by_name::{FollowStart, FollowSteps};
// The selector's colour and the faction catalog it and the model read.
#[cfg(test)]
pub(crate) use ring::{ring_variant, PlayerPath, SelectorInput};
pub(crate) use ring::{selection_variant, Factions, RingVariant};

pub use click::Interact;
pub(crate) use click::{
    clear_selection_on_world_teardown, clear_with_no_world, deselect_on_ui_shutdown,
    signal_ui_shutdown_deselect, CursorLeg,
};

/// Our target, set the instant we click, as the 1.12 client does without waiting for the server.
/// The selection is the guid `[0xb4e2d8]`: `SetSelection 0x493540` stores it whether or not the
/// object manager holds its object (`0x4935bf` → `0x493605`), and the client reads the object by
/// that guid where it needs one. `CMSG_SET_SELECTION` carries the guid, which the server records in
/// our `UNIT_FIELD_TARGET`. The handlers that hold and release objects keep [`Self::target`] on the
/// guid's object and end the selection where the client's teardowns do ([`release`]).
#[derive(Resource, Default, Clone, Copy, Debug, PartialEq)]
pub(crate) struct Selection {
    /// The object [`Self::guid`] names while the object manager holds it.
    pub(crate) target: Option<Entity>,
    pub(crate) guid: Option<u64>,
    /// The object `SetSelection` lit as the target (`SetLight` bit 0, `0x493701`) and the teardown
    /// unlights (`0x4939a7`): an object that streams in after the select is not lit.
    pub(crate) lit: Option<Entity>,
    /// The last-target pair `[0xb4e2e0]` that `TargetLastTarget` reads: `SetSelection` stamps the
    /// outgoing selection, none included, on every change it makes, a select or a deselect
    /// (`0x49361d`-`0x493628`); a teardown clear (`0x493910` alone) leaves it.
    pub(crate) last: Option<u64>,
}

/// This frame's character-model pick, by [`hover::update_hover`]. At most one slot is set: the
/// reference makes one pick over all CGObjects and switches on type at the end (`0x480df0` →
/// `0x7089c0`), and a corpse (`CGCorpse_C`) is not a unit, so it never answers as `target`.
#[derive(Resource, Default, Clone, Copy)]
pub(crate) struct Hovered {
    /// The hovered unit or player, the selectable slot.
    pub(crate) target: Option<Entity>,
    pub(crate) guid: Option<u64>,
    /// Ray distance to the pick, refused or not; a plate hover is `0.0`, so UI wins any tie.
    pub(crate) distance: f32,
    /// The hovered corpse: the reference's `CGCorpse_C` fills the interact slot `+0x60`
    /// (`0x5d6bf0`), never `SetTarget`.
    pub(crate) corpse: Option<Entity>,
    /// The hovered corpse's guid, which `CMSG_LOOT` and `CMSG_RECLAIM_CORPSE` carry.
    pub(crate) corpse_guid: Option<u64>,
    /// The `IsSelectable` grader refused the winning pick (`0x482982`): both slots are empty but
    /// [`Self::distance`] holds the hit, so a chest behind the unit does not inherit the mouseover.
    /// To [`click::ClickSelect::select`] it is an object hit, which clears no selection.
    pub(crate) refused: bool,
    /// The pick is the hovered plate's unit, the plate's enter publishing it (`0x7cb850`), not the
    /// world's own pick: it stands only while that plate still holds the hover.
    pub(crate) plate: bool,
}

impl Hovered {
    /// The picked entity, whichever slot holds it.
    pub(crate) fn any(&self) -> Option<Entity> {
        self.target.or(self.corpse)
    }

    /// The unit `"mouseover"` names: the hovered unit, unless a nearer GameObject won the pick.
    /// The publisher `0x492890` writes whatever won to `0xb4e2c8`/`0xb4e2cc` (`0x492927`,
    /// `0x492938`), and the token resolver re-resolves it as a unit (`0x515bca mov ecx,8`), so a
    /// GameObject, a corpse or nothing names nobody (`0x515bd9 je`).
    pub(crate) fn mouseover(&self, go: &HoveredObject) -> Option<(Entity, u64)> {
        self.target
            .zip(self.guid)
            .filter(|_| !go_is_nearest(self, go))
    }
}

/// The GameObject under the cursor this frame, by [`hover::update_hovered_object`]: usable, never
/// selected. It acts only when nearer than the character pick ([`go_is_nearest`]).
#[derive(Resource, Default, Clone, Copy)]
pub(crate) struct HoveredObject {
    pub(crate) target: Option<Entity>,
    pub(crate) guid: Option<u64>,
    /// World-space ray distance, compared against [`Hovered::distance`].
    pub(crate) distance: f32,
}

/// This frame's world hit along the cursor ray, by [`hover::update_pick_occlusion`]: a unit or
/// GameObject behind a wall is not hoverable (`0x480df0`).
#[derive(Resource, Clone, Copy)]
pub(crate) struct PickOcclusion {
    pub(crate) distance: f32,
    /// The world hit point, which the ground-targeting cursor rides, as in the reference.
    pub(crate) point: Option<Vec3>,
    /// The cursor ray's direction, which a right-click on the sky walks along (`0x492d6c`).
    pub(crate) ray: Option<Dir3>,
}

impl Default for PickOcclusion {
    fn default() -> Self {
        Self {
            distance: f32::INFINITY,
            point: None,
            ray: None,
        }
    }
}

/// The pick latched at the press, which a world click of either button acts on: the reference
/// picks once per press, as the mouse command arms the click (`0x514810` → `0x481f00`), into the
/// WorldFrame (state `+0x350`, guid `+0x358`,
/// point `+0x360`, distance `+0x36c`), so a drag can orbit the camera and still select. The hover
/// is another matter: the reference blanks it during freelook (`0x483e80` sets `[wf+0x38c]&2`).
#[derive(Resource, Default, Clone, Copy)]
pub(crate) struct PressPick {
    pub(crate) hovered: Hovered,
    pub(crate) object: HoveredObject,
    pub(crate) occlusion: PickOcclusion,
    /// The context cursor as of the press, which the right-click ladder forks on. No select's
    /// re-swing reads it: `Attack 0x5ecb70` judges the new unit itself (`scan::reswing_allowed`).
    pub(crate) cursor: cursor_mode::WorldCursor,
    /// The unit dispatcher's attack fork as of the press, which the sword alone cannot carry: the
    /// sword also needs the player's own legs, and a refused player still takes the attack arm.
    pub(crate) attack_fork: cursor_mode::AttackFork,
}

impl PressPick {
    /// The router's pick state is an object (`[this+0x350]` 2, `0x481fb8`): a unit, a corpse, a
    /// unit the `IsSelectable` grader refused (an object hit that selects nothing) or a
    /// GameObject. Anything else is the ground or the sky.
    pub(crate) fn over_object(&self) -> bool {
        self.hovered.any().is_some() || self.hovered.refused || self.object.target.is_some()
    }
}

/// Whether the hovered GameObject, not the character pick, is what a click acts on. The reference
/// makes one pick over all CGObjects; benilla makes two and keeps the GameObject only when strictly
/// nearer. A corpse or a refused pick holds the ground as a unit does.
pub(crate) fn go_is_nearest(character: &Hovered, go: &HoveredObject) -> bool {
    match (character.any().is_some() || character.refused, go.target) {
        (true, Some(_)) => go.distance < character.distance,
        (false, Some(_)) => true,
        _ => false,
    }
}

/// A unit's model-local ring radius, `sqrt(0.5 · sqrt(dx² + dy²))` over the Stand sequence box's
/// horizontal extents (`0x608e00`/`0x60aee0`); the ring scales it by `OBJECT_FIELD_SCALE_X`.
/// Stamped at attach; a model-less unit has none and takes the ring's fallback radius.
#[derive(Component, Clone, Copy)]
pub(crate) struct SelectionRadius(pub(crate) f32);

/// The set the targeting chain's frame-time half runs in: readers of [`Selection`], [`Hovered`]
/// or [`CombatFlash`] order after it, or they can read last frame's verdict.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct TargetUpdate;

/// `deselectOnClick`, 1.12's CVar behind the "Sticky Targeting" checkbox, which shows it inverted
/// (`UIOptionsFrame.lua:250`). On by default, as in the reference: an empty-world click clears the
/// target.
#[derive(Resource)]
pub(crate) struct ClickConfig {
    pub(crate) deselect_on_click: bool,
}

impl Default for ClickConfig {
    fn default() -> Self {
        Self {
            deselect_on_click: true,
        }
    }
}

/// `assistAttack`, 1.12's "Attack on assist" option (`[0xb4d8f8]`, registered at `0x48fc50` with
/// default "0"). When set, `/assist`'s shared tail also calls `StartAttack 0x5ecb70` on the new
/// target (`0x489c02`, `0x489d02`), which stands you and sends `CMSG_ATTACKSWING`.
#[derive(Resource, Default)]
pub(crate) struct AssistAttack(pub(crate) bool);

/// The targeting rows' change callback: two flags.
pub(crate) fn on_cvar(
    ev: &crate::cvars::CvarChanged,
    mut click: bevy::prelude::Mut<'_, ClickConfig>,
    mut assist: bevy::prelude::Mut<'_, AssistAttack>,
) {
    match ev.key().as_str() {
        "deselectonclick" => click.deselect_on_click = ev.flag(),
        "assistattack" => assist.0 = ev.flag(),
        _ => {}
    }
}

/// Targeting: click-to-select + the ground selection ring.
pub(crate) struct TargetPlugin;

impl Plugin for TargetPlugin {
    fn build(&self, app: &mut App) {
        // The update packet's field callbacks, a death's among them, after the net plugin's
        // update end, which writes their edges.
        crate::field_callbacks::register(app);
        release::register(app);
        app.init_resource::<Selection>()
            .init_resource::<ClickConfig>()
            .init_resource::<AssistAttack>()
            .init_resource::<Hovered>()
            .init_resource::<HoveredObject>()
            .init_resource::<PickOcclusion>()
            .init_resource::<hover_probe::ProbeAim>()
            .init_resource::<PressPick>()
            .init_resource::<ClickArm>()
            .init_resource::<hover::StickyPicks>()
            .init_resource::<hover::GoPickable>()
            // The GameObject pick set, kept ahead of the frame's input records and the hover.
            .add_systems(
                Update,
                hover::track_go_pick_parts
                    .before(crate::ui_script::UiInput)
                    .before(hover::update_hovered_object),
            )
            .init_resource::<WorldCursor>()
            .init_resource::<cursor_mode::AttackFork>()
            .init_resource::<CombatFlash>()
            .init_resource::<scan::TabHistory>()
            .init_resource::<scan::LastEnemy>()
            .add_message::<AttackNearestRequest>()
            .add_message::<Interact>()
            .add_systems(
                Startup,
                (
                    ring::setup_ring,
                    reticle::setup_reticle,
                    (ring::load_factions, scan::load_creature_types).after(AssetSet::Open),
                ),
            )
            // What no call dispatches, where the frame's records end, ahead of the packets, as
            // the reference's world-frame handlers run in the window input (`0x423920` at
            // `0x420d23`), in the dispatcher's order: a crate's interacts and the casts their
            // dispatchers queued.
            .edit_schedule(WorldClickLegs, |legs| {
                legs.set_executor_kind(bevy::ecs::schedule::ExecutorKind::SingleThreaded);
            })
            .add_systems(
                WorldClickLegs,
                (
                    click::act_on_interact,
                    // The casts the dispatchers queued (`0x5f35c0`, `0x5f06dc` → `0x6e5a90`).
                    crate::ui_action::drain_click_casts,
                )
                    .chain(),
            )
            // The frame time's half, after the controller's step and the stream: the picks, the
            // cursor, a walk's arrival, the non-mouse selection writers, then the flash and the
            // ring off the frame's final selection. Nested tuples keep the chain within Bevy's
            // 20-tuple limit.
            .add_systems(
                Update,
                (
                    // The trace reads the static world after this frame's stream reshapes it.
                    hover::aim_and_trace().after(WorldStage::Stream),
                    hover::update_hover,
                    hover::update_hovered_object,
                    cursor_mode::classify_cursor,
                    // Overwrites the classifier's verdict while the targeting cursor is up, as the
                    // reference's dispatcher runs this branch before the object classifier.
                    crate::spell::targeting::drive_targeting_cursor,
                    // Reads that verdict: `WorldCursor.unable` is the frame's range state, as the
                    // reference's one `CheckGroundPointInRange` caller feeds both.
                    reticle::update_reticle,
                    // The verb a walk owes runs at its stop, in the frame time's movement, after
                    // the frame's clicks; a unit it parked with the targeting cursor up is bound,
                    // and a cast it queued sent, as a click's are.
                    (
                        click::act_on_arrival,
                        crate::script_calls::bind_arrival_picks,
                        crate::ui_action::drain_arrival_casts,
                    )
                        .chain(),
                    // The melee probe's press, the player's Attack; the TAB keys and ATTACKTARGET
                    // run their bodies' `TargetNearestEnemy` and `AttackTarget` as script calls.
                    crate::ui_action::attack_nearest_probe,
                    flash::drive_flash,
                    // The last-enemy stamp, then the ring off the frame's final selection.
                    (scan::remember_last_enemy, ring::update_ring).chain(),
                )
                    .chain()
                    .in_set(TargetUpdate)
                    .after(WorldStage::Input),
            )
            // PostUpdate: the brighten sets MeshTag bit 31 after every Update writer that
            // overwrites the whole tag.
            .add_systems(PostUpdate, highlight::apply_highlight)
            // The ring's stream push, after the frame's stream clear.
            .add_systems(
                PostUpdate,
                (ring::push_ring, reticle::push_reticle)
                    .after(benilla_world::particles::buffer::begin_effect_frame),
            );
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gameobject_wins_the_click_only_when_the_nearer_object() {
        let hov = |t: bool, d: f32| Hovered {
            target: t.then_some(Entity::PLACEHOLDER),
            distance: d,
            ..Hovered::default()
        };
        // The same hover landed on a corpse, the other slot of the one pick.
        let corpse = |t: bool, d: f32| Hovered {
            corpse: t.then_some(Entity::PLACEHOLDER),
            distance: d,
            ..Hovered::default()
        };
        let obj = |t: bool, d: f32| HoveredObject {
            target: t.then_some(Entity::PLACEHOLDER),
            guid: None,
            distance: d,
        };
        // No GameObject hovered: the unit path keeps the click.
        assert!(!go_is_nearest(&hov(true, 5.0), &obj(false, 0.0)));
        assert!(!go_is_nearest(&hov(false, 0.0), &obj(false, 0.0)));
        // A GameObject with no competing unit takes the click.
        assert!(go_is_nearest(&hov(false, 0.0), &obj(true, 42.0)));
        // Both hovered: only a strictly nearer GameObject wins; a tie goes to the unit.
        assert!(go_is_nearest(&hov(true, 10.0), &obj(true, 4.0)));
        assert!(!go_is_nearest(&hov(true, 4.0), &obj(true, 10.0)));
        assert!(!go_is_nearest(&hov(true, 5.0), &obj(true, 5.0)));
        // A corpse competes on the same terms as a unit.
        assert!(!go_is_nearest(&corpse(true, 5.0), &obj(false, 0.0)));
        assert!(!go_is_nearest(&corpse(true, 4.0), &obj(true, 10.0)));
        assert!(!go_is_nearest(&corpse(true, 5.0), &obj(true, 5.0)));
        assert!(go_is_nearest(&corpse(true, 10.0), &obj(true, 4.0)));
        // So does a pick the `IsSelectable` grader refused, and then nothing is hovered at all.
        let refused = |d: f32| Hovered {
            refused: true,
            distance: d,
            ..Hovered::default()
        };
        assert!(!go_is_nearest(&refused(4.0), &obj(true, 10.0)));
        assert!(!go_is_nearest(&refused(5.0), &obj(true, 5.0)));
        assert!(go_is_nearest(&refused(10.0), &obj(true, 4.0)));
    }
}
