//! The **inspector surface** — the dev-chord `I` overlay: an "armed" pill plus a compact identity
//! card that follows the cursor over whatever [`MouseoverTarget`] picked. Split from the module
//! face for size only; the toggle, the card, and its readout lines live here unchanged.

use bevy::platform::collections::HashSet;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_egui::{egui, EguiContexts};

use super::{overlay_text, OVERLAY_FILL, OVERLAY_TEXT_DIM};
use crate::net::ObjectStore;
use crate::ui_script::{InspectMode, PointerOverUi};
use benilla_world::interact::{WorldObject, WorldPick};
use benilla_world::model_render::ModelKind;
use benilla_world::modkeys::DEV_CHORD;
use benilla_world::view::WorldCamera;

/// The **dev chord + `I`** (for *inspect*) arms/disarms the inspector — off the bare-letter plane the
/// game's own bindings own, on whichever plane this OS leaves free. Unmistakable as a
/// chord, so unlike the old bare `i` it needs no chat-bar/EditBox gate.
pub(super) fn toggle_inspect(keys: Res<ButtonInput<KeyCode>>, mut inspect: ResMut<InspectMode>) {
    if benilla_world::modkeys::dev_chord(&keys, KeyCode::KeyI) {
        inspect.enabled = !inspect.enabled;
    }
}

/// How long the inspector card shows its "copied to clipboard" confirmation after a left-click.
/// Shared with the journal's row-copy flash ([`super::journal`]).
pub(super) const COPY_FLASH_SECS: f32 = 1.2;

/// A per-kind accent so the card's header is glanceable (which *sort* of thing am I over?) before you
/// even read the label.
fn kind_color(kind: ModelKind) -> egui::Color32 {
    match kind {
        ModelKind::Doodad => egui::Color32::from_rgb(140, 220, 140), // green — props/trees
        ModelKind::Wmo => egui::Color32::from_rgb(150, 185, 240),    // blue — buildings
        ModelKind::Creature => egui::Color32::from_rgb(240, 205, 130), // gold — NPCs
        ModelKind::GameObject => egui::Color32::from_rgb(220, 165, 220), // violet — GameObjects
    }
}

/// The granted movement modes as the card names them — `None` when the unit has
/// none, which is nearly all of them, so the common line is unchanged. Named rather than derived
/// from the raw word on the card: `0x40001000` says nothing to the eye, and the whole reason this
/// is on the card is that the modes explain a body that looks wrong and is not.
fn granted_modes(modes: Option<&crate::net::UnitMoveModes>) -> Option<String> {
    use crate::creature_anim::move_flags as f;
    let w = modes?.0;
    let named = [
        (f::ROOT, "rooted"),
        (f::HOVER, "hover"),
        (f::WATER_WALKING, "waterwalk"),
        (f::SAFE_FALL, "feather"),
        (f::WALK_MODE, "walk-mode"),
        (f::SWIMMING, "swim"),
    ]
    .into_iter()
    .filter(|(bit, _)| w & bit != 0)
    .map(|(_, name)| name)
    .collect::<Vec<_>>();
    (!named.is_empty()).then(|| format!("granted {}", named.join("+")))
}

/// The inspector's GameObject collision readout: does a hull exist, is it disabled
/// right now, and what stored state does the passability gate see. Named so the bundled `stores`
/// param stays readable.
type GoCollisionReadout = (
    Has<avian3d::prelude::Collider>,
    Has<avian3d::prelude::ColliderDisabled>,
    Option<&'static crate::go_anim::GoAnim>,
);

/// The inspector's **motion** readout: the server-dictated path a creature is riding
/// ([`crate::net::Spline`]) and the dead-reckoned state of a remote mover
/// ([`crate::net::RemoteMotion`]) — the two producers of "this thing is moving" that the animation
/// selector itself unifies. On the card because the identity lines alone cannot answer the one
/// question a "that creature looks wrong" report always turns on: *is it actually moving, and how
/// fast?* The AQ drakes are the case that earned it — a Brood of Nozdormu 190 yd overhead beating
/// its wings at a flat 1× while its path crawls at walk pace reads as frozen, and nothing on the
/// card said the path was the slow half.
///
/// …and, since 1780, the third producer: the modes the server **granted** this unit
/// ([`crate::net::UnitMoveModes`]). They belong on the same line because they answer the same
/// report from the other side — a body a yard off the floor, standing on water, or refusing every
/// path the server sends it looks broken and is not, and the mode word is the only place that
/// difference is written down. A unit with none reads nothing extra, which is nearly all of them.
type MotionReadout = (
    Option<&'static crate::net::Spline>,
    Option<&'static crate::net::RemoteMotion>,
    Option<&'static crate::net::UnitMoveModes>,
    // The clamp's memo, for the card's `ground` line — the only place a "this mob is under the
    // world" sighting becomes a measurement instead of a picture.
    Option<&'static crate::net::GroundClamped>,
    // Where the unit is standing *now*, which the `ground` line reads the terrain against.
    &'static Transform,
);

/// The inspector's entity LIGHT readout: the lane this object's parts render
/// under, and which attach found the room — "two identical GameObjects a few yards apart, one lit
/// like the room and one like the street" is the report that made this a card line rather than a
/// rebuild with `WOW_INTERIOR_LOG`.
type EntityLightReadout = (
    &'static benilla_world::interior::InteriorAnchor,
    Has<benilla_world::interior::ContainmentAttach>,
);

/// One pickable part as the card's `parts alive` and glow-card lines read it: the object, its
/// draw verdict, its transform, whether it is a billboard card, the material + mesh tag the glow
/// line names, and the visibility class the STACKED count reads. A tuple alias like
/// [`EntityLightReadout`] above, for the same reason: as an inline `Query<>` the field trips
/// clippy's `type_complexity` at the workspace gate.
type PartReadout = (
    &'static benilla_world::interact::WorldObject,
    &'static bevy::camera::visibility::ViewVisibility,
    &'static GlobalTransform,
    Has<benilla_world::billboard::BillboardCard>,
    Option<&'static MeshMaterial3d<benilla_assets::materials::WowModelMaterial>>,
    Option<&'static bevy::mesh::MeshTag>,
    Option<&'static bevy::camera::visibility::VisibilityClass>,
);

/// Everything the identity card reads off the **net entity** under the cursor, as one named
/// [`SystemParam`] — the descriptor store and the coarse kind the line gates go by, the GameObject
/// collision readout, the light readout, and the two remaining inputs of the
/// **interact gate** ([`crate::target::cursor_mode::go_highlightable`]): the faction catalog and our
/// own store. A bundle because `inspect_ui` sits at Bevy's 16-param ceiling; a named struct rather
/// than the tuple it grew out of, so each member says what it is at the point of use.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct InspectStores<'w, 's> {
    stores: Query<'w, 's, &'static ObjectStore>,
    kinds: Query<'w, 's, &'static crate::net::NetEntity>,
    /// Every pickable part with its draw verdict, for the card's `parts alive` line: a prop
    /// spawned twice reads twice its model's batch count here, and nowhere else.
    objects: Query<'w, 's, PartReadout>,
    /// The realized materials + images, for the card line: which material a glow card is bound
    /// to, whether that material still says ADDITIVE, and whether its texture is resident.
    model_mats: Res<'w, Assets<benilla_assets::materials::WowModelMaterial>>,
    images: Res<'w, Assets<bevy::image::Image>>,
    collision: Query<'w, 's, GoCollisionReadout>,
    lit: Query<'w, 's, EntityLightReadout>,
    motion: Query<'w, 's, MotionReadout>,
    /// The MCNK heightfield under the hovered unit — the `ground` line's third number, and the one
    /// that separates "the server put it there" from "we sank it".
    points: benilla_world::world_point::WorldPoint<'w, 's>,
    factions: Option<Res<'w, crate::target::ring::Factions>>,
    self_store: Query<'w, 's, &'static ObjectStore, With<crate::net::SelfPlayer>>,
    /// The live standings — the other half of the reaction resolve (`ring_reaction` takes the
    /// reputation branch first, and a rank that came off a standing rather than a faction-template
    /// comparison is the single most confusing thing about a unit's colour).
    reputations: Res<'w, crate::net::Reputations>,
    /// This frame's plate verdict ([`crate::vplates::VPlates`]) — the *rendered* answer, not a
    /// re-derivation, so the card can never disagree with what is on screen.
    plates: Res<'w, crate::vplates::VPlates>,
    /// Which of the two master bits are on, so a `plate ✗` can say whether the unit lost its plate
    /// on the category gate or on something else.
    plate_mode: Res<'w, crate::vplates::VPlateMode>,
    /// The ask-once GO template cache — the readable head a TEXT object's line reports,
    /// and the highlight column + name the tooltip ladder reports (2246).
    go_templates: Res<'w, crate::go_templates::GameObjectTemplates>,
    /// `[0xb72038]` — the meeting-stone queue, the other half of MEETINGSTONE(23)'s own
    /// highlightable term, so the card's `interact` verdict reads the same
    /// predicate the cursor and the click do.
    stone: Option<Res<'w, crate::ui_dialog_verbs::MeetingStone>>,
    /// **The published GameObject mouseover** — the one the tooltip actually reads
    /// ([`crate::target::HoveredObject`]). The card's own pick is a dev pick and does not go
    /// through the publish, so without this the card can show an object the game is not hovering
    /// at all and give no sign of the difference (2246).
    hovered_go: Res<'w, crate::target::HoveredObject>,
    /// The GameObject **animation** readout — what the state machine's arm
    /// (`0x5f3930`) is playing right now, for the card's `anim` line. Its own query rather than a
    /// `collision` member because it needs the model components: they sit on the same entity as
    /// [`crate::go_anim::GoAnim`], but a GO whose model authors no skeleton renders as a static
    /// mesh and has none of them.
    go_anims: Query<
        'w,
        's,
        (
            &'static crate::go_anim::GoAnim,
            &'static AnimationPlayer,
            &'static benilla_assets::ModelAnimations,
        ),
    >,
    /// The **picked submesh's own `MeshTag`** — the per-instance shading payload, decoded by its
    /// owning module ([`benilla_world::mesh_tag::describe`]).
    ///
    /// The `light` line above names which *law* an object is on; this names what its parts are
    /// actually carrying under that law, and the two together are what settle a "why is this body
    /// lit wrong?" report. The payload's meaning switches on material state, which a tag alone
    /// cannot know, so the decode prints BOTH readings of bits 6..=18 side by side — a shade byte
    /// of 255 (the MCSH-shadowed half-intensity) sits inside a probe slot of 2047, and a writer
    /// using the wrong law for its material is invisible in either reading alone and obvious in
    /// the pair. Read off the picked entity itself, not the parent: `WorldObject` and `MeshTag`
    /// both ride the submesh.
    tags: Query<'w, 's, &'static bevy::mesh::MeshTag>,
}

/// The inspector overlay, drawn only while armed: a weak top-centre "armed" pill (so it's obvious the
/// mode is on and how to leave it) and, whenever the cursor is over an identified object, a compact
/// identity card pinned to the cursor. No chrome, no panel — its own lightweight surface.
pub(super) fn inspect_ui(
    mut contexts: EguiContexts,
    inspect: Res<InspectMode>,
    mouseover: Res<MouseoverTarget>,
    // The pickable mesh is a child of the net entity; its descriptor store (`ObjectStore`) lives on the
    // parent, so the readout hops child → parent.
    parents: Query<&ChildOf>,
    stores: InspectStores,
    guids: Query<&crate::net::Guid>,
    castings: Query<&crate::creature_anim::Casting>,
    drivers: Query<&crate::creature_anim::AnimDriver>,
    anim_data: Option<Res<crate::creature_anim::AnimData>>,
    spells: Option<Res<crate::ui_action::Spells>>,
    names: Res<crate::names::NameCache>,
    net_commands: Res<crate::net::NetCommands>,
    // Bundled into one param (Bevy's system-function arity ceiling): the copy-click button, and
    // the flag it must yield to — a left press this frame the UI already consumed as a
    // cursor-payload world drop must not ALSO land as an inspector copy-click, the same
    // yield every other world left-press consumer gives it (see `PointerOverUi` above for the
    // hover-time twin).
    click_input: (
        Res<ButtonInput<MouseButton>>,
        Res<crate::ui_script::PlayerUiClickConsumed>,
    ),
    time: Res<Time>,
    mut copied_at: Local<Option<f32>>,
) -> Result {
    let (buttons, click_consumed) = (&click_input.0, &click_input.1);
    if !inspect.enabled {
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?;

    // Armed indicator — small + dim, so it states "inspect is on" without competing with the world.
    egui::Area::new(egui::Id::new("inspect_armed"))
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 8.0))
        .show(ctx, |ui| {
            egui::Frame::NONE
                .inner_margin(egui::Margin::symmetric(8, 4))
                .corner_radius(5.0)
                .fill(OVERLAY_FILL)
                .show(ui, |ui| {
                    overlay_text(ui);
                    // Spelled out, not ⌃⌘ — egui's default font stack has no glyph for U+2303 and
                    // would draw tofu.
                    ui.label(
                        egui::RichText::new(format!("inspect · {DEV_CHORD}+I to exit")).small(),
                    );
                });
        });

    // The identity card: only when hovering a picked object, pinned just off the cursor tip.
    let Some(obj) = mouseover.object.as_ref() else {
        return Ok(());
    };
    let Some(cursor) = ctx.pointer_latest_pos() else {
        return Ok(());
    };

    // A unit's decoded server vitals from its descriptor store (`ObjectStore`), if the picked mesh's
    // parent has them — proof the descriptor pipeline (UpdateFields → ObjectValues → ECS) reached it.
    let net_entity = mouseover
        .entity
        .and_then(|e| parents.get(e).ok())
        .map(|c| c.parent());
    let (factions, self_store) = (stores.factions.as_deref(), stores.self_store.single().ok());
    let (reputations, plates, plate_mode) =
        (&*stores.reputations, &stores.plates.0, &*stores.plate_mode);
    let go_templates = &*stores.go_templates;
    let queued_area = stores.stone.as_deref().map_or(0, |s| s.area);
    let hovered_go = &*stores.hovered_go;
    // The picked submesh's shading payload — off the hit entity itself (see the field's doc).
    let tag_line = mouseover
        .entity
        .and_then(|e| stores.tags.get(e).ok())
        .map(|t| format!("tag {}", benilla_world::mesh_tag::describe(t.0)));
    // The duplicate readout: every live part naming this same object, and how many of them
    // drew this frame. A doodad has one part per render batch; a doubled placement shows twice
    // that here — the census the FPS probe prints as `orphan_parts=`, at the cursor.
    let parts_line = {
        let (mut alive, mut drawn, mut stacked) = (0usize, 0usize, 0usize);
        let mut cards: Vec<String> = Vec::new();
        for (w, vv, gt, card, mat, tag, class) in stores.objects.iter() {
            if w.kind == obj.kind && w.id == obj.id {
                alive += 1;
                drawn += usize::from(vv.get());
                // A part whose `VisibilityClass` lists its mesh class more than once is queued
                // that many times — drawn stacked on itself (`model_render::park`'s dedup).
                stacked += usize::from(class.is_some_and(|c| c.len() > 1));
                if card {
                    // One glow card, as the draw sees it: its live world scale (the placement
                    // scale times the bone's pulse), its tag alpha, and the bound material's
                    // marker word — ADDITIVE is `clutter_fade.z` bit 2, the bit `specialize`
                    // keys the (ONE, ONE) blend on and the shader keys the alpha fold on; a card
                    // whose material lost it draws its texture unweighted, a hard bright disc.
                    let scale = gt.compute_transform().scale.x;
                    let alpha = tag.map_or(-1.0, |t| benilla_world::mesh_tag::alpha_of(t.0));
                    let m = mat.and_then(|m| stores.model_mats.get(&m.0));
                    let state = match m {
                        Some(m) => {
                            let z = m.extension.clutter_fade.z as u32;
                            let tex = m.base.base_color_texture.as_ref().map_or(
                                "none".to_string(),
                                |h| {
                                    let loaded = stores.images.get(h).is_some();
                                    format!("{}{}", h.id(), if loaded { "" } else { " MISSING" })
                                },
                            );
                            format!(
                                "add {} fade {:.0} unlit {:.0} nodw {} tex {tex}",
                                u8::from(z & 4 != 0),
                                m.extension.model_flags.y,
                                m.extension.model_flags.w,
                                u8::from(z & 1 != 0),
                            )
                        }
                        None => match mat {
                            Some(_) => "material UNREALIZED".to_string(),
                            None => "no material".to_string(),
                        },
                    };
                    cards.push(format!(
                        "[scale {scale:.2} α {alpha:.2} vis {} {state}]",
                        u8::from(vv.get())
                    ));
                }
            }
        }
        let stacked = if stacked > 0 {
            format!(", STACKED {stacked}")
        } else {
            String::new()
        };
        if cards.is_empty() {
            format!("parts alive {alive}, drawn {drawn}{stacked}")
        } else {
            format!(
                "parts alive {alive}, drawn {drawn}{stacked}, cards {} {}",
                cards.len(),
                cards.join(" ")
            )
        }
    };
    let (stores, kinds, collision, lit, motion, points, go_anims) = (
        &stores.stores,
        &stores.kinds,
        &stores.collision,
        &stores.lit,
        &stores.motion,
        &stores.points,
        &stores.go_anims,
    );
    let store = net_entity.and_then(|p| stores.get(p).ok());
    // The unit's server name through the query cache — asks on first hover, fills on a later frame
    // (the same ask-once path the unit frames use).
    let name_line = net_entity
        .and_then(|p| guids.get(p).ok())
        .and_then(|g| names.resolve_unit(g.0, store, &net_commands))
        .map(str::to_string);
    // Line gates go by the entity's KIND, not field presence: a create-seeded store answers every
    // field (absent = 0, the descriptor truth), so "is the health field there" stopped meaning
    // "is this a unit".
    let kind = net_entity.and_then(|p| kinds.get(p).ok()).map(|n| n.kind);
    let is_unit = matches!(
        kind,
        Some(benilla_protocol::EntityKind::Unit | benilla_protocol::EntityKind::Player)
    );
    let is_player = kind == Some(benilla_protocol::EntityKind::Player);
    // The GameObject collision + state readout — the line that closes the loop on
    // "this door is drawn open but I can't walk through it". It answers, for the object under the
    // cursor, the three facts the passability gate turns on: the client's stored `GAMEOBJECT_STATE`
    // (`go_anim::go_state`, i.e. what we believe open/closed is), whether a collision hull exists at
    // all, and whether it is currently disabled. A door reading `state 0 open · SOLID` is the bug
    // live on screen; `state 0 open · passable` says the gate ran and something else is blocking.
    let go_line = store
        .filter(|_| kind == Some(benilla_protocol::EntityKind::GameObject))
        .map(|s| {
            let (has_hull, disabled, anim) = net_entity
                .and_then(|p| collision.get(p).ok())
                .unwrap_or((false, false, None));
            let state = crate::go_anim::go_state(anim, s);
            let word = match state {
                0 => "open(ACTIVE)",
                1 => "closed(READY)",
                2 => "alt(DESTROYED)",
                _ => "?",
            };
            let solidity = match (has_hull, disabled) {
                (false, _) => "no hull",
                (true, true) => "passable",
                (true, false) => "SOLID",
            };
            let flags = s.0.gameobject_flags();
            let mut named = Vec::new();
            for (bit, name) in [
                (0x1, "IN_USE"),
                (0x2, "LOCKED"),
                (0x4, "INTERACT_COND"),
                (0x10, "NO_INTERACT"),
            ] {
                if flags & bit != 0 {
                    named.push(name);
                }
            }
            let flag_text = if named.is_empty() {
                String::new()
            } else {
                format!(" [{}]", named.join("|"))
            };
            // The **interact gate**, stated rather than inferred (`0x5f2f80`): the
            // strategy vtable's `+0x14` **highlightable** slot is the single predicate behind the
            // cursor, the +64 brighten, the right-click USE and the pick priority — so a GO reading
            // `interact ✗` is saying all four are off *by design*, and one reading `interact ✓` while
            // showing no gear says the fault is downstream in the cursor naming. That distinction is
            // exactly what the type-8 anvil report cost a hand-derivation to make: an anvil hovers
            // (this card is up) and must still read `interact ✗`, because SPELL_FOCUS is one of the
            // types whose `+0x14` is a constant `xor al,al`.
            let reaction =
                crate::target::cursor_mode::go_reaction(factions, s.0.gameobject_faction(), self_store);
            let go_guid = net_entity.and_then(|p| guids.get(p).ok()).map(|g| g.0);
            let overrides = crate::target::cursor_mode::GoOverrides {
                channel_owned: crate::target::cursor_mode::fishing_channel_owned(
                    self_store, go_guid,
                ),
                meeting_stone_queued: crate::target::cursor_mode::meeting_stone_queued(
                    go_guid.and_then(|g| Some(go_templates.get(g)?.meeting_stone?.area)),
                    queued_area,
                ),
            };
            let interact = if crate::target::cursor_mode::go_highlightable(s, reaction, overrides) {
                "interact ✓"
            } else {
                "interact ✗"
            };
            // **The TOOLTIP's own ladder**, which `interact` above is not and is
            // routinely mistaken for. "No tooltip on this" has three possible stages and the card
            // could name none of them, so every report of it cost a session of code reading:
            //
            //  · `hover ✗` — the **mouseover-eligibility** slot `+0x54`
            //    ([`crate::target::cursor_mode::mouseover_eligible`]) said no, so the reference
            //    publishes the NULL mouseover and there is no tooltip *by design*. For a
            //    GENERIC(5) signpost this is the template's `data[1]` highlight column, which is
            //    why the `tmpl` field sits beside it.
            //  · `hover ✓` but `shown ✗` — eligible, and the publish still did not take it: the
            //    pick lost to the occlusion verdict, to a nearer unit, or to the pointer being
            //    over UI. The fault is in the pick, not the gate.
            //  · `shown ✓` and still no plate on screen — the fault is downstream in
            //    [`crate::ui_tooltip`], and `tmpl` says whether the name it needs has arrived.
            //
            // `tmpl —` is the one that looks like a bug and is not: the template query is
            // ask-once and answers a frame or two later, and until it does the tooltip has no
            // name to draw.
            let tmpl = go_guid.and_then(|g| go_templates.get(g));
            let hover_gate = crate::target::cursor_mode::mouseover_eligible(
                s.0.gameobject_type_id(),
                flags,
                s.0.gameobject_dynamic_flags(),
                tmpl.map(|t| t.highlight_column),
                reaction,
                overrides,
            );
            let published = go_guid.is_some_and(|g| hovered_go.guid == Some(g));
            let tooltip_line = format!(
                " · hover {} · shown {} · tmpl {}",
                if hover_gate { "✓" } else { "✗" },
                if published { "✓" } else { "✗" },
                match tmpl {
                    None => "—".to_string(),
                    Some(t) if t.name.is_empty() => "(unnamed)".to_string(),
                    Some(t) => format!("{:?}", t.name),
                }
            );
            // TEXT (type 9) only: the **readable head**. A book that opens no
            // window is either "no page in the template" or a fault downstream, and only this
            // line tells the two apart — the symptom is identical from the chair, and the first
            // one is the reference behaving correctly. `page —` = the template says none;
            // `page ?` = its ask-once query hasn't answered yet.
            let page_text = if s.0.gameobject_type_id() == crate::target::cursor_mode::GO_TYPE_TEXT
            {
                let go_guid = net_entity.and_then(|p| guids.get(p).ok()).map(|g| g.0);
                match go_guid
                    .and_then(|g| go_templates.get(g))
                    .map(|t| t.text_page.map_or(0, |p| p.page_id))
                {
                    None => " · page ?".to_string(),
                    Some(0) => " · page —".to_string(),
                    Some(id) => format!(" · page {id}"),
                }
            } else {
                String::new()
            };
            // The **placement tilt** — shown only when there is one. A GameObject
            // is placed by its `GAMEOBJECT_ROTATION` quaternion, and 96.7% of live spawns encode a
            // plain yaw in it; the rest carry a tilt that swings the model's off-origin geometry
            // yards from the spawn point. So `tilt 70°` on a prop that looks misplaced says "this
            // one's pose is quaternion-only" — the question B89 cost a DB round-trip to answer —
            // and its absence says the placement is a bare facing and the fault is elsewhere.
            let tilt = s
                .0
                .gameobject_rotation()
                // The angle the quaternion leans the model's own up-axis off world up:
                // `acos(m22)`, i.e. `2·asin(|x, y|)` — zero for every pure-yaw spawn.
                .map(|q| (2.0 * q[0].hypot(q[1]).min(1.0).asin()).to_degrees())
                .filter(|deg| *deg >= 0.5)
                .map(|deg| format!(" · tilt {deg:.0}°"))
                .unwrap_or_default();
            format!(
                "go type {} · state {state} {word} · {solidity} · flags {flags:#x}{flag_text} · {interact}{tooltip_line}{page_text}{tilt}",
                s.0.gameobject_type_id()
            )
        });
    let vitals_line = store.filter(|_| is_unit).map(|s| {
        format!(
            "hp {}/{} · level {}",
            s.0.unit_health().unwrap_or(0),
            s.0.unit_max_health().unwrap_or(0),
            s.0.unit_level().unwrap_or(0)
        )
    });
    // Raw bytes (not name-mapped): creature race/class don't share the player-race enum, so a label
    // would mislead. The character model will name-map these for players specifically.
    let appearance_line = store.filter(|_| is_unit).map(|s| {
        format!(
            "race {} · class {} · sex {}",
            s.0.unit_race().unwrap_or(0),
            s.0.unit_class().unwrap_or(0),
            s.0.unit_gender().unwrap_or(0)
        )
    });
    // **Why this unit does or does not carry a V-plate** — the whole input set of
    // `vplates::drive_vplates`'s gate on one line, plus the verdict it actually reached this frame
    // (read out of [`crate::vplates::VPlates`], never re-derived, so the card cannot disagree with
    // the screen). "These plates shouldn't be here" is otherwise a question no amount of looking
    // can answer: the plate is drawn from a REACTION rank whose provenance (a reputation standing
    // vs a faction-template comparison) is invisible, over unit flags nothing displays, and the
    // two master bits are off-screen state. Reading `reaction 3 neutral(rep)` off a green-looking
    // city NPC is the entire diagnosis.
    let plate_line = store.filter(|_| is_unit).map(|s| {
        let rank = crate::target::ring::ring_reaction(factions, reputations, Some(s), self_store);
        let word = match rank {
            0 => "hated",
            1 => "hostile",
            2 => "unfriendly",
            3 => "neutral",
            4 => "friendly",
            5 => "honored",
            6 => "revered",
            _ => "exalted",
        };
        // Which branch produced it: a faction WITH a reputation slot colours by our standing
        // (`0x605fc0` → `0x4d63a0`, before any template comparison), everything else by the
        // template comparator. The two disagree constantly — a battleground emissary is `friendly`
        // by template and `neutral` by standing — and only this tag says which one you are seeing.
        let branch = s
            .0
            .unit_faction_template()
            .and_then(|t| factions?.catalog().template(t))
            .map(|t| {
                if factions.is_some_and(|f| f.catalog().reputation_faction(t.faction).is_some()) {
                    "rep"
                } else {
                    "tpl"
                }
            })
            .unwrap_or("?");
        let ctype = net_entity
            .and_then(|p| guids.get(p).ok())
            .and_then(|g| benilla_protocol::guid::entry(g.0))
            .and_then(|e| names.creature_type(e));
        let ctype = ctype.map_or_else(|| "type ?".to_string(), |t| format!("type {t}"));
        let verdict = match net_entity {
            Some(p) if plates.contains(&p) => "plate ✓".to_string(),
            _ => {
                // The category the gate would have put it in, and whether that bit is on — the
                // first thing to check when a plate is missing (or present) unexpectedly.
                let (category, bit) = if rank >= 4 {
                    ("friendly", plate_mode.friends)
                } else {
                    ("enemy", plate_mode.enemies)
                };
                if bit {
                    "plate ✗".to_string()
                } else {
                    format!("plate ✗ ({category} bit off)")
                }
            }
        };
        format!(
            "{verdict} · reaction {rank} {word}({branch}) · faction {} · unitflags {:#x} · {ctype}",
            s.0.unit_faction_template().unwrap_or(0),
            s.0.unit_flags(),
        )
    });
    // Player-only customization: the compositor's input, shown raw so we can
    // confirm the PLAYER_BYTES decode against an in-game character.
    let customization_line = store.filter(|_| is_player).map(|s| {
        format!(
            "skin {} · face {} · hair {}/{} · facial {}",
            s.0.player_skin().unwrap_or(0),
            s.0.player_face().unwrap_or(0),
            s.0.player_hair_style().unwrap_or(0),
            s.0.player_hair_color().unwrap_or(0),
            s.0.player_facial_hair().unwrap_or(0)
        )
    });

    // A unit mid-cast (`SMSG_SPELL_START` .. GO — the `Casting` wire seam): which spell, by id and
    // display name. The director's "what is it casting?" answered on hover; the finished cast's
    // trail lives in the journal.
    let casting_line = net_entity.and_then(|p| castings.get(p).ok()).map(|c| {
        match spells.as_ref().and_then(|s| s.catalog.get(c.spell_id)) {
            Some(d) => format!("casting {} \"{}\"", c.spell_id, d.name),
            None => format!("casting {}", c.spell_id),
        }
    });
    // **Is it moving, and how fast** ([`MotionReadout`]) — the input the `anim` line below is a
    // consequence of. A creature riding a server path reports the path's constant speed (its whole
    // length over its whole duration, the same number the gait selector reads) and how far through
    // it is, so a unit that looks static is either `still` (no path at all — the server has not
    // launched one, or the last one expired) or a path so slow it cannot be seen. A remote mover
    // reports its dead-reckoning speed instead. Units and players only: a GameObject has no mover.
    let motion_line = net_entity
        .filter(|_| is_unit)
        .and_then(|p| motion.get(p).ok())
        .map(|(spline, remote, modes, _, _)| {
            let moving = match (spline, remote) {
                (Some(sp), _) => format!(
                    "motion {:.2} yd/s · {} path {:.0}% of {:.0}s",
                    sp.speed(),
                    if sp.grounded { "ground" } else { "flying" },
                    100.0 * sp.elapsed_frac(),
                    sp.duration.as_secs_f32(),
                ),
                (None, Some(r)) if r.speed > 0.0 => {
                    format!("motion {:.2} yd/s · dead-reckoned", r.speed)
                }
                _ => "motion still".to_string(),
            };
            match granted_modes(modes) {
                Some(m) => format!("{moving} · {m}"),
                None => moving,
            }
        });
    // **Where its feet came from** ([`crate::net::GroundClamped`]) — the ground clamp's own memo,
    // shown because "that mob is standing inside the hill" is a claim about three numbers and no
    // screenshot carries them:
    //
    //   `ground z 12.34 · seat 12.42 (drop +0.08) · terrain 12.34 · walk hit`
    //
    // - **`z`** is where we are drawing it; **`seat`** is the pose the server last wrote, before
    //   the clamp had its say, and **`drop`** = `seat − z` is the clamp's own correction. A big
    //   positive drop is *ours*; a `z` far under `terrain` with `drop ≈ 0` is the server's.
    // - **`terrain`** is the MCNK height under it — so "is it under the world?" reads off the card
    //   rather than off a guess about what the hill looks like from here.
    // - **the arm and the probe verdict**: `walk` is the swept step continuing a server path,
    //   `idle` the settle from the seat, and they answer a MISS differently — an idle miss leaves
    //   the unit at its seat, a walk miss descends. `MISS` is upper-case because on the `walk` arm
    //   it is the interesting state, not a neutral one.
    //
    // The same three numbers `WOW_GROUND_CENSUS` prints per unit, on the unit the director is
    // actually pointing at: the census answers "is anything sunk in this scene", this answers "why
    // is *that* one".
    let ground_line = net_entity
        .filter(|_| is_unit)
        .and_then(|p| motion.get(p).ok())
        .and_then(|(_, _, _, clamped, t)| clamped.map(|c| (c, t)))
        .map(|(c, t)| {
            let z = t.translation.y;
            format!(
                "ground z {z:.2} · seat {:.2} (drop {:+.2}) · terrain {} · {} {}",
                c.seat_y,
                c.seat_y - z,
                points
                    .terrain_height_under(t.translation)
                    .map_or_else(|| "none".to_string(), |v| format!("{v:.2}")),
                if c.walking() { "walk" } else { "idle" },
                if c.hit { "hit" } else { "MISS" },
            )
        });
    // An `AnimationData` id as the card names it — shared by the creature and GameObject anim
    // lines below, which read the same id space.
    let fmt = |id: u16| match anim_data.as_ref().and_then(|a| a.0.name(id)) {
        Some(name) => format!("{name}({id})"),
        None => format!("{id}"),
    };
    // The animation slots this frame (requested `AnimationData` ids — the selector's choice,
    // before missing-clip substitution): the full-body base + any masked upper-body overlay.
    let anim_line = net_entity.and_then(|p| drivers.get(p).ok()).map(|d| {
        let (base, overlay) = d.playing();
        let base = base.map(&fmt).unwrap_or_else(|| "—".into());
        // The base slot's live playback rate — `speed / (moveSpeed · modelScale)`
        // on a locomotion clip, a flat 1× on everything else. Shown so a "its walk is too fast"
        // report is a hover away from a number instead of a hand-worked divisor; suppressed at
        // exactly 1× so the ordinary case doesn't carry noise.
        let rate = d.rate();
        let rate = if (rate - 1.0).abs() > 1e-3 {
            format!(" · rate {rate:.2}×")
        } else {
            String::new()
        };
        match overlay {
            Some(o) => format!("anim {base} + overlay {}{rate}", fmt(o)),
            None => format!("anim {base}{rate}"),
        }
    });
    // The same line for a **GameObject**, which is driven by `GoAnim` rather than
    // `AnimDriver` and so never reached the branch above: the sequence the state machine's arm
    // (`0x5f3930`) is playing, whether it is a transient one (a transition motion / a Custom block
    // — something the completion advance `0x5f4120` must end) or the state's held rest pose, and
    // the repeat it is running under. A `transition · loops` reading is the "stuck open/closing"
    // bug, stated.
    let anim_line = anim_line.or_else(|| {
        let (id, transient, repeat) = net_entity
            .and_then(|p| go_anims.get(p).ok())
            .and_then(|(go, player, anims)| crate::go_anim::armed_anim(go, player, anims))?;
        let kind = if transient { "transition" } else { "rest" };
        let repeat = match repeat {
            bevy::animation::RepeatAnimation::Forever => " · loops".to_string(),
            bevy::animation::RepeatAnimation::Never => String::new(),
            bevy::animation::RepeatAnimation::Count(n) => format!(" · ×{n}"),
        };
        Some(format!("anim {} · {kind}{repeat}", fmt(id)))
    });
    // The light lane + the attach that found it. Absent until the classifier has
    // resolved the anchor once — a freshly streamed object shows no line rather than a wrong one.
    let light_line = net_entity
        .and_then(|p| lit.get(p).ok())
        .map(|(anchor, containment)| {
            format!(
                "light {} · {} attach",
                anchor.law_label(),
                if containment {
                    "containment"
                } else {
                    "down-ray"
                }
            )
        });

    // The lines shown in the card — also exactly what a left-click copies to the clipboard.
    let mut lines = vec![format!("{:?}", obj.kind), obj.label.clone()];
    if let Some(name) = &name_line {
        lines.push(format!("\"{name}\""));
    }
    if obj.id != 0 {
        lines.push(format!("id {}", obj.id));
    }
    if !obj.detail.is_empty() {
        lines.push(obj.detail.clone());
    }
    if let Some(line) = &go_line {
        lines.push(line.clone());
    }
    if let Some(line) = &vitals_line {
        lines.push(line.clone());
    }
    if let Some(line) = &appearance_line {
        lines.push(line.clone());
    }
    if let Some(line) = &plate_line {
        lines.push(line.clone());
    }
    if let Some(line) = &customization_line {
        lines.push(line.clone());
    }
    if let Some(line) = &casting_line {
        lines.push(line.clone());
    }
    if let Some(line) = &motion_line {
        lines.push(line.clone());
    }
    if let Some(line) = &ground_line {
        lines.push(line.clone());
    }
    if let Some(line) = &anim_line {
        lines.push(line.clone());
    }
    if let Some(line) = &light_line {
        lines.push(line.clone());
    }
    if let Some(line) = &tag_line {
        lines.push(line.clone());
    }
    lines.push(parts_line.clone());
    lines.push(format!("{:.1} yd away", mouseover.distance));

    // The inspector owns left-click while armed (player::control suppresses left-orbit during inspect),
    // so a press over the hovered object copies the whole card to the clipboard.
    if buttons.just_pressed(MouseButton::Left) && !click_consumed.0 {
        ctx.copy_text(lines.join("\n"));
        *copied_at = Some(time.elapsed_secs());
    }
    let just_copied = copied_at.is_some_and(|t| time.elapsed_secs() - t < COPY_FLASH_SECS);

    egui::Area::new(egui::Id::new("inspect_card"))
        .fixed_pos(cursor + egui::vec2(18.0, 18.0))
        .show(ctx, |ui| {
            egui::Frame::NONE
                .inner_margin(egui::Margin::symmetric(8, 6))
                .corner_radius(5.0)
                .fill(OVERLAY_FILL)
                .show(ui, |ui| {
                    overlay_text(ui);
                    ui.colored_label(kind_color(obj.kind), format!("{:?}", obj.kind));
                    ui.label(egui::RichText::new(&obj.label).monospace());
                    if obj.id != 0 {
                        ui.label(
                            egui::RichText::new(format!("id {}", obj.id)).color(OVERLAY_TEXT_DIM),
                        );
                    }
                    if !obj.detail.is_empty() {
                        ui.label(egui::RichText::new(&obj.detail).color(OVERLAY_TEXT_DIM));
                    }
                    if let Some(line) = &go_line {
                        ui.label(egui::RichText::new(line).color(OVERLAY_TEXT_DIM));
                    }
                    if let Some(line) = &vitals_line {
                        ui.label(egui::RichText::new(line).color(OVERLAY_TEXT_DIM));
                    }
                    if let Some(line) = &appearance_line {
                        ui.label(egui::RichText::new(line).color(OVERLAY_TEXT_DIM));
                    }
                    if let Some(line) = &plate_line {
                        ui.label(egui::RichText::new(line).color(OVERLAY_TEXT_DIM));
                    }
                    if let Some(line) = &customization_line {
                        ui.label(egui::RichText::new(line).color(OVERLAY_TEXT_DIM));
                    }
                    if let Some(line) = &casting_line {
                        ui.label(egui::RichText::new(line).color(OVERLAY_TEXT_DIM));
                    }
                    if let Some(line) = &anim_line {
                        ui.label(egui::RichText::new(line).color(OVERLAY_TEXT_DIM));
                    }
                    if let Some(line) = &light_line {
                        ui.label(egui::RichText::new(line).color(OVERLAY_TEXT_DIM));
                    }
                    ui.label(
                        egui::RichText::new(format!("{:.1} yd away", mouseover.distance))
                            .color(OVERLAY_TEXT_DIM),
                    );
                    // Copy affordance, swapped for a brief confirmation after a left-click.
                    if just_copied {
                        ui.label(
                            egui::RichText::new("copied to clipboard")
                                .small()
                                .color(egui::Color32::from_rgb(140, 220, 140)),
                        );
                    } else {
                        ui.label(
                            egui::RichText::new("left-click to copy")
                                .small()
                                .color(OVERLAY_TEXT_DIM),
                        );
                    }
                });
        });
    Ok(())
}

/// The nearest identified thing under the cursor this frame, or `None`. `point`/`distance` are the
/// world-space hit.
///
/// The identity is the resource's own (`object`), not something the consumer looks up off `entity`:
/// most of the static world draws from a consolidated lane and has **no entity** to look anything
/// up on. `entity` is `Some` only when one owns the geometry — which is exactly
/// when the extra per-entity readouts (a unit's descriptor store, a GameObject's collision) mean
/// anything.
#[derive(Resource, Default)]
pub struct MouseoverTarget {
    pub object: Option<WorldObject>,
    pub entity: Option<Entity>,
    pub point: Vec3,
    pub distance: f32,
}

/// Ray-cast from the cursor into the world and record the nearest [`WorldObject`] hit. Restricted to
/// entities carrying `WorldObject` (so terrain, particle billboards, and other un-identified meshes are
/// transparent to the pick), and skipped entirely unless inspection is active.
pub(super) fn update_mouseover(
    inspect: Res<InspectMode>,
    pointer_over_ui: Res<PointerOverUi>,
    mut target: ResMut<MouseoverTarget>,
    window: Query<&Window, With<PrimaryWindow>>,
    camera: Query<(&Camera, &GlobalTransform), With<WorldCamera>>,
    objects: Query<(Entity, &WorldObject)>,
    // Everything drawn, entity-owned or not — see [`WorldPick`]: most of the static world draws
    // from a consolidating lane and would be invisible to a bare `PickParts` cast (1534).
    pick: WorldPick,
) {
    if !inspect.enabled {
        if target.object.is_some() {
            *target = MouseoverTarget::default();
        }
        return;
    }
    target.object = None;
    target.entity = None;
    // The pointer is over the dev UI (e.g. the now-overlaid debug panel), not the world — don't pick
    // behind it. This replaces the old "is the cursor in the inset world viewport?" test, which no
    // longer means anything now the panel overlays a full-screen view.
    if pointer_over_ui.0 {
        return;
    }
    let Ok((camera, cam_tf)) = camera.single() else {
        return;
    };
    let Ok(window) = window.single() else {
        return;
    };
    let Some(cursor) = window.cursor_position() else {
        return; // cursor left the window, or we're in mouselook (hidden)
    };
    let identified: HashSet<Entity> = objects.iter().map(|(e, _)| e).collect();
    if let Some(hit) = pick.at_cursor(cursor, camera, cam_tf, &identified) {
        target.object = hit.object;
        target.entity = hit.entity;
        target.point = hit.hit.point;
        target.distance = hit.hit.distance;
    }
}
