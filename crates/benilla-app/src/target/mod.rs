//! Targeting: the selection, the mouseover pick, click-to-select and the ground selection ring
//! ([`ring`]). The pick is the reference's (`0x481190` → `0x7089c0`, [`hover::update_hover`]): a
//! nameplate under the pointer first, then the posed-mesh pick, clamped by the world trace; a click
//! acts on what the press was over ([`PressPick`]).

use bevy::prelude::*;

use benilla_ui::script::UiScript;

use crate::creature_anim::Engaged;
use crate::net::{ClientCommand, Guid, NetCommands, ObjectStore, SelfPlayer};
use crate::ui_script::InspectMode;
use benilla_assets::AssetSet;
use benilla_world::interact::{WorldClick, WorldRightClick};
use benilla_world::schedule::WorldStage;

mod by_name;
// `pub(crate)` for the chest live probe, which drives the mouse's own `click::resolve_go_action`.
pub(crate) mod click;
// `pub(crate)` for the hover inspector, which runs the cursor's `cursor_mode::go_highlightable`.
pub(crate) mod cursor_mode;
mod flash;
mod highlight;
pub(crate) mod hover;
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
mod relations;
mod reticle;
// `pub(crate)` for the hover inspector too: its faction catalog feeds `go_highlightable`.
pub(crate) mod ring;
mod scan;

/// `Faction.dbc` and our reputation table as one [`SystemParam`], since every reaction function
/// takes both; `factions` is `Option` so a UI-only harness runs without client data.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct ReactionInputs<'w> {
    pub(crate) factions: Option<Res<'w, Factions>>,
    pub(crate) reputations: Res<'w, crate::net::Reputations>,
}

pub(crate) use cursor_mode::{
    corpse_mouseover_eligible, CursorKind, WorldCursor, GO_TYPE_GENERIC, SERVICE_RANGE_SQ,
};
/// `GO_FLAG_LOCKED`, the bit the lock chain and the GameObject tooltip's "Locked" line read.
pub(crate) use lock::GO_FLAG_LOCKED;
// This frame's combat-flash verdict, for the ring's material and the nameplate colour gate.
pub(crate) use flash::CombatFlash;
// The discount `0x612b80` the repair cost and the taxi fare take off their DBC prices.
pub(crate) use price_discount::vendor_price_discount;
#[cfg(test)]
pub(crate) use price_discount::{stormwind_fixture, HUMAN_WARRIOR};
// The attack-with-no-target request, and the same nearest-enemy core called synchronously for the
// pet bar's Attack, whose order must leave in the frame it was pressed.
pub(crate) use relations::{can_assist, can_attack, can_interact};
pub(crate) use scan::{attack_order_target, AttackNearestRequest, TargetScan};
// The chat layer's by-name asks (`/target`, `/assist`).
pub(crate) use by_name::{AssistRequest, TargetByNameRequest};
// The reaction decode and its faction catalog, which also tint the target frame
// (`TargetFrame_CheckFaction`); `duel_rung` is the same walk, for `/reaction`.
pub(crate) use ring::{duel_rung, ring_reaction, ring_variant, Factions, RingVariant};

pub(crate) use click::DeselectGuid;

/// Our target, set the instant we click, as the 1.12 client does without waiting for the server,
/// and cleared on deselect or when it streams out. `CMSG_SET_SELECTION` carries the guid, which the
/// server records in our `UNIT_FIELD_TARGET`.
#[derive(Resource, Default)]
pub(crate) struct Selection {
    pub(crate) target: Option<Entity>,
    pub(crate) guid: Option<u64>,
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
    /// To [`click::select_on_click`] it is an object hit, which clears no selection.
    pub(crate) refused: bool,
}

impl Hovered {
    /// The picked entity, whichever slot holds it.
    pub(crate) fn any(&self) -> Option<Entity> {
        self.target.or(self.corpse)
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
}

impl Default for PickOcclusion {
    fn default() -> Self {
        Self {
            distance: f32::INFINITY,
            point: None,
        }
    }
}

/// The pick latched on the button's down edge, which a world click of either button acts on: the
/// reference picks once per press (`0x481f00`) into the WorldFrame (state `+0x350`, guid `+0x358`,
/// point `+0x360`, distance `+0x36c`), so a drag can orbit the camera and still select. The hover
/// is another matter: the reference blanks it during freelook (`0x483e80` sets `[wf+0x38c]&2`).
#[derive(Resource, Default, Clone, Copy)]
pub(crate) struct PressPick {
    pub(crate) hovered: Hovered,
    pub(crate) object: HoveredObject,
    pub(crate) occlusion: PickOcclusion,
    /// The context cursor as of the press, which the right-click ladder forks on; the reference's
    /// new-target validation (`0x5ecb70`) also reads the press's pick, not a live one.
    pub(crate) cursor: cursor_mode::WorldCursor,
    /// The unit dispatcher's attack fork as of the press, which the sword alone cannot carry: the
    /// sword also needs the player's own legs, and a refused player still takes the attack arm.
    pub(crate) attack_fork: cursor_mode::AttackFork,
}

impl PressPick {
    /// Whether the press was an Attack-cursor press, for `0x5ecb70`'s new-target validation.
    pub(crate) fn attack(&self) -> bool {
        self.cursor.kind == cursor_mode::CursorKind::Attack
    }
}

/// Latch the frame's pick as a press begins. It must run at the head of the target chain: input has
/// already engaged the look session on the press frame, and [`hover::update_hover`] is about to
/// clear the hover, so what stands here is last frame's pick at the same cursor, the press's.
pub(crate) fn latch_press_pick(
    buttons: Res<ButtonInput<MouseButton>>,
    hovered: Res<Hovered>,
    object: Res<HoveredObject>,
    occlusion: Res<PickOcclusion>,
    cursor: Res<cursor_mode::WorldCursor>,
    attack_fork: Res<cursor_mode::AttackFork>,
    mut press: ResMut<PressPick>,
) {
    // Either button's down edge arms a pick (`0x514810`, clickModes 1 and 2); a chord does not
    // re-latch, as `0x51481a` refuses to arm while the other button is held.
    let left = buttons.just_pressed(MouseButton::Left) && !buttons.pressed(MouseButton::Right);
    let right = buttons.just_pressed(MouseButton::Right) && !buttons.pressed(MouseButton::Left);
    if !(left || right) {
        return;
    }
    *press = PressPick {
        hovered: *hovered,
        object: *object,
        occlusion: *occlusion,
        cursor: *cursor,
        attack_fork: *attack_fork,
    };
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

/// The set the targeting chain runs in: readers of [`Selection`], [`Hovered`] or [`CombatFlash`]
/// order after it, or they can read last frame's verdict.
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
    ev: On<crate::cvars::CvarChanged>,
    mut click: ResMut<ClickConfig>,
    mut assist: ResMut<AssistAttack>,
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
        app.add_observer(on_cvar);
        app.init_resource::<Selection>()
            .init_resource::<ClickConfig>()
            .init_resource::<AssistAttack>()
            .init_resource::<Hovered>()
            .init_resource::<HoveredObject>()
            .init_resource::<PickOcclusion>()
            .init_resource::<PressPick>()
            .init_resource::<WorldCursor>()
            .init_resource::<cursor_mode::AttackFork>()
            .init_resource::<CombatFlash>()
            .init_resource::<scan::TabHistory>()
            .init_resource::<scan::LastEnemy>()
            .add_message::<AttackNearestRequest>()
            .add_message::<TargetByNameRequest>()
            .add_message::<AssistRequest>()
            .add_message::<click::DeselectGuid>()
            .add_systems(
                Startup,
                (
                    ring::setup_ring,
                    reticle::setup_reticle,
                    (ring::load_factions, scan::load_creature_types).after(AssetSet::Open),
                ),
            )
            // After input, so this frame's `WorldClick` is here, in order: the picks, the cursor,
            // the clicks, the non-mouse selection writers, then the flash and the ring off the
            // frame's final selection. Nested tuples keep the chain within Bevy's 20-tuple limit.
            .add_systems(
                Update,
                (
                    // The latch first, before any pick refresh clears what the press was over.
                    (latch_press_pick, hover::update_pick_occlusion).chain(),
                    hover::update_hover,
                    hover::update_hovered_object,
                    cursor_mode::classify_cursor,
                    // The right press's two legs of the reference's OnMouseDown hook (`0x492c20`),
                    // the targeting cancel and the repair-mode reset, before the cursor drive, so
                    // the press frame already reads both modes cleared.
                    (
                        crate::spell::targeting::cancel_targeting_on_right_press,
                        crate::ui_merchant::end_repair_mode_on_right_press,
                    ),
                    // Overwrites the classifier's verdict while the targeting cursor is up, as the
                    // reference's dispatcher runs this branch before the object classifier.
                    crate::spell::targeting::drive_targeting_cursor,
                    // Reads that verdict: `WorldCursor.unable` is the frame's range state, as the
                    // reference's one `CheckGroundPointInRange` caller feeds both.
                    reticle::update_reticle,
                    click::world_right_click_payload,
                    // A plate click replayed as a click, then the select that reads it.
                    (click::select_on_plate_click, click::select_on_click).chain(),
                    // After the gated select, which must read the targeting mode before a commit
                    // clears it. The pending spell feeds only one of the two legs, so their mutual
                    // order is free.
                    crate::spell::targeting::commit_ground_cast_on_click,
                    crate::spell::targeting::commit_object_cast_on_click,
                    click::act_on_right_click,
                    click::clear_target_requests,
                    // The unit-token asks (`TargetUnit`, `AssistUnit`, `TargetLastEnemy`: one
                    // drain, as the reference has one `0x489a40`) and `DropItemOnUnit`'s pet leg,
                    // independent of each other.
                    (
                        click::selection_requests,
                        crate::ui_action::drop_item::drop_item_on_unit,
                    ),
                    // The by-name asks: `/target`, the Lua `TargetByName` and `/assist` commit
                    // through `scan::commit`; `/follow` hands its subject to `crate::player`.
                    (
                        by_name::target_by_name_requests,
                        by_name::script_target_by_name_requests,
                        by_name::assist_requests,
                        by_name::follow_requests,
                    )
                        .chain(),
                    scan::auto_acquire_attacker,
                    // The one cycler's two sides (`0x493f60`, modes 1 and 2), chained: they share
                    // `TabHistory`, which a side switch clears.
                    (scan::tab_target, scan::target_nearest_friend_requests).chain(),
                    scan::acquire_and_attack,
                    flash::drive_flash,
                    // The last-enemy stamp before the ring's death-clear, so a hostile that dies
                    // selected is still remembered (the reference's `TargetLastEnemy` has no
                    // liveness gate).
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

    /// A drag blanks the live hover for its whole length, and a click at its end must still act on
    /// what the press was over.
    #[test]
    fn the_press_latch_outlives_the_hover_a_drag_clears() {
        let mut world = World::new();
        world.init_resource::<Hovered>();
        world.init_resource::<HoveredObject>();
        world.init_resource::<PickOcclusion>();
        world.init_resource::<cursor_mode::WorldCursor>();
        world.init_resource::<cursor_mode::AttackFork>();
        world.init_resource::<PressPick>();
        world.init_resource::<ButtonInput<MouseButton>>();
        let id = world.register_system(latch_press_pick);

        // A boar under the cursor, and the cursor classified as Attack over it.
        const BOAR: u64 = 0xB0A2;
        world.resource_mut::<Hovered>().guid = Some(BOAR);
        world.resource_mut::<cursor_mode::WorldCursor>().kind = cursor_mode::CursorKind::Attack;

        // Press left: the latch takes the pick.
        world
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        world.run_system(id).unwrap();
        assert_eq!(world.resource::<PressPick>().hovered.guid, Some(BOAR));
        assert!(world.resource::<PressPick>().attack(), "Attack rode along");

        // The drag begins: the look session blanks the live hover.
        *world.resource_mut::<Hovered>() = Hovered::default();
        world.resource_mut::<ButtonInput<MouseButton>>().clear();
        world.run_system(id).unwrap();
        assert_eq!(
            world.resource::<PressPick>().hovered.guid,
            Some(BOAR),
            "the release must still know what the press was over"
        );
    }

    /// The reference refuses to arm while another button is down (`0x51481a`) and kills the
    /// pending click (`0x514ac1`), so neither release of a both-button run selects.
    #[test]
    fn a_chord_does_not_relatch_the_pick() {
        let mut world = World::new();
        world.init_resource::<Hovered>();
        world.init_resource::<HoveredObject>();
        world.init_resource::<PickOcclusion>();
        world.init_resource::<cursor_mode::WorldCursor>();
        world.init_resource::<cursor_mode::AttackFork>();
        world.init_resource::<PressPick>();
        world.init_resource::<ButtonInput<MouseButton>>();
        let id = world.register_system(latch_press_pick);

        const FIRST: u64 = 0x1111;
        const SECOND: u64 = 0x2222;
        world.resource_mut::<Hovered>().guid = Some(FIRST);
        world
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        world.run_system(id).unwrap();
        assert_eq!(world.resource::<PressPick>().hovered.guid, Some(FIRST));

        // Right joins while left is still held, over something else entirely.
        world.resource_mut::<Hovered>().guid = Some(SECOND);
        let mut buttons = world.resource_mut::<ButtonInput<MouseButton>>();
        buttons.clear(); // drop last frame's just_pressed, keep `pressed`
        buttons.press(MouseButton::Right);
        world.run_system(id).unwrap();
        assert_eq!(
            world.resource::<PressPick>().hovered.guid,
            Some(FIRST),
            "the chord's second press must not steal the latch"
        );
    }
}
