//! The inspector, the dev-chord `I` overlay: an "armed" pill and an identity card that follows
//! the cursor over whatever [`MouseoverTarget`] picked.

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

/// The dev chord + `I` arms and disarms the inspector; a chord needs no EditBox gate.
pub(super) fn toggle_inspect(keys: Res<ButtonInput<KeyCode>>, mut inspect: ResMut<InspectMode>) {
    if benilla_world::modkeys::dev_chord(&keys, KeyCode::KeyI) {
        inspect.enabled = !inspect.enabled;
    }
}

/// How long a "copied to clipboard" confirmation shows, on the card and the journal's rows.
pub(super) const COPY_FLASH_SECS: f32 = 1.2;

/// A per-kind accent for the card's header.
fn kind_color(kind: ModelKind) -> egui::Color32 {
    match kind {
        ModelKind::Doodad => egui::Color32::from_rgb(140, 220, 140), // green: props, trees
        ModelKind::Wmo => egui::Color32::from_rgb(150, 185, 240),    // blue: buildings
        ModelKind::Creature => egui::Color32::from_rgb(240, 205, 130), // gold: NPCs
        ModelKind::GameObject => egui::Color32::from_rgb(220, 165, 220), // violet: GameObjects
    }
}

/// The granted movement modes by name, `None` for a unit with none (nearly all of them).
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

/// A GameObject's collision readout: hull present, hull disabled, and the stored state the
/// passability gate sees.
type GoCollisionReadout = (
    Has<avian3d::prelude::Collider>,
    Has<avian3d::prelude::ColliderDisabled>,
    Option<&'static crate::go_anim::GoAnim>,
);

/// A unit's motion readout: its server path ([`crate::net::Spline`]), its dead-reckoned remote
/// motion ([`crate::net::RemoteMotion`]) and the modes the server granted it
/// ([`crate::net::UnitMoveModes`]), which explain a hovering, water-walking or rooted body.
type MotionReadout = (
    Option<&'static crate::net::Spline>,
    Option<&'static crate::net::RemoteMotion>,
    Option<&'static crate::net::UnitMoveModes>,
    // The ground clamp's memo, for the card's `ground` line.
    Option<&'static crate::net::GroundClamped>,
    // Where the unit stands now, which the `ground` line reads the terrain against.
    &'static Transform,
);

/// An object's light readout: the lane its parts render under, and which attach found the room.
type EntityLightReadout = (
    &'static benilla_world::interior::InteriorAnchor,
    Has<benilla_world::interior::ContainmentAttach>,
);

/// One pickable part as the card's `parts alive` and glow-card lines read it; an alias because an
/// inline `Query<>` trips clippy's `type_complexity`.
type PartReadout = (
    &'static benilla_world::interact::WorldObject,
    &'static bevy::camera::visibility::ViewVisibility,
    &'static GlobalTransform,
    Has<benilla_world::billboard::BillboardCard>,
    Option<&'static MeshMaterial3d<benilla_assets::materials::WowModelMaterial>>,
    Option<&'static bevy::mesh::MeshTag>,
    Option<&'static bevy::camera::visibility::VisibilityClass>,
);

/// Everything the identity card reads off the net entity under the cursor, bundled because
/// `inspect_ui` sits at Bevy's 16-param ceiling.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct InspectStores<'w, 's> {
    stores: Query<'w, 's, &'static ObjectStore>,
    kinds: Query<'w, 's, &'static crate::net::NetEntity>,
    /// Every pickable part with its draw verdict; a prop spawned twice reads twice its batches.
    objects: Query<'w, 's, PartReadout>,
    /// For a glow card: its bound material, whether that is still additive, and whether its
    /// texture is resident.
    model_mats: Res<'w, Assets<benilla_assets::materials::WowModelMaterial>>,
    images: Res<'w, Assets<bevy::image::Image>>,
    collision: Query<'w, 's, GoCollisionReadout>,
    lit: Query<'w, 's, EntityLightReadout>,
    motion: Query<'w, 's, MotionReadout>,
    /// The MCNK height under the hovered unit, the `ground` line's `terrain`.
    points: benilla_world::world_point::WorldPoint<'w, 's>,
    factions: Option<Res<'w, crate::target::ring::Factions>>,
    self_store: Query<'w, 's, &'static ObjectStore, With<crate::net::SelfPlayer>>,
    /// The live standings: `ring_reaction` takes the reputation branch before the template one.
    reputations: Res<'w, crate::net::Reputations>,
    /// This frame's rendered plate verdict, not a re-derivation, so the card matches the screen.
    plates: Res<'w, crate::vplates::VPlates>,
    /// The two master bits, so a `plate ✗` can say whether the category gate refused it.
    plate_mode: Res<'w, crate::vplates::VPlateMode>,
    /// The GO template cache: a TEXT object's page and the tooltip ladder's highlight column
    /// and name.
    go_templates: Res<'w, crate::go_templates::GameObjectTemplates>,
    /// The meeting-stone queue (`[0xb72038]`), part of MEETINGSTONE(23)'s highlightable term, so
    /// the card's `interact` verdict reads the same predicate as the cursor.
    stone: Option<Res<'w, crate::ui_dialog_verbs::MeetingStone>>,
    /// The published GameObject mouseover the tooltip reads; the card's own dev pick bypasses the
    /// publish, so this shows when the two differ.
    hovered_go: Res<'w, crate::target::HoveredObject>,
    /// What the GameObject state machine's arm (`0x5f3930`) is playing, for the `anim` line. Its
    /// own query: a GO whose model has no skeleton is a static mesh without these components.
    go_anims: Query<
        'w,
        's,
        (
            &'static crate::go_anim::GoAnim,
            &'static AnimationPlayer,
            &'static benilla_assets::ModelAnimations,
        ),
    >,
    /// The picked submesh's own `MeshTag`, its per-instance shading payload. Its meaning depends
    /// on material state, so [`benilla_world::mesh_tag::describe`] prints both readings of bits
    /// 6..=18. Read off the submesh itself, not the parent.
    tags: Query<'w, 's, &'static bevy::mesh::MeshTag>,
}

/// The inspector overlay while armed: a top-centre "armed" pill and, over an identified object,
/// an identity card pinned to the cursor.
pub(super) fn inspect_ui(
    mut contexts: EguiContexts,
    inspect: Res<InspectMode>,
    mouseover: Res<MouseoverTarget>,
    // The pickable mesh is a child of the net entity, whose `ObjectStore` is on the parent.
    parents: Query<&ChildOf>,
    stores: InspectStores,
    guids: Query<&crate::net::Guid>,
    castings: Query<&crate::creature_anim::Casting>,
    drivers: Query<&crate::creature_anim::AnimDriver>,
    anim_data: Option<Res<crate::creature_anim::AnimData>>,
    spells: Option<Res<crate::ui_action::Spells>>,
    names: Res<crate::names::NameCache>,
    net_commands: Res<crate::net::NetCommands>,
    // Bundled for the arity ceiling: the copy-click button and the flag it yields to, a left
    // press the UI already consumed as a cursor-payload drop.
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

    // The armed indicator, small and dim.
    egui::Area::new(egui::Id::new("inspect_armed"))
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 8.0))
        .show(ctx, |ui| {
            egui::Frame::NONE
                .inner_margin(egui::Margin::symmetric(8, 4))
                .corner_radius(5.0)
                .fill(OVERLAY_FILL)
                .show(ui, |ui| {
                    overlay_text(ui);
                    // Spelled out: egui's default fonts have no glyph for U+2303.
                    ui.label(
                        egui::RichText::new(format!("inspect · {DEV_CHORD}+I to exit")).small(),
                    );
                });
        });

    // The identity card, only over a picked object, pinned just off the cursor tip.
    let Some(obj) = mouseover.object.as_ref() else {
        return Ok(());
    };
    let Some(cursor) = ctx.pointer_latest_pos() else {
        return Ok(());
    };

    // The picked mesh's parent, the net entity carrying the descriptor store.
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
    // The picked submesh's shading payload, off the hit entity itself.
    let tag_line = mouseover
        .entity
        .and_then(|e| stores.tags.get(e).ok())
        .map(|t| format!("tag {}", benilla_world::mesh_tag::describe(t.0)));
    // Every live part naming this object and how many drew; a doodad has one per render batch,
    // so a doubled placement shows twice that.
    let parts_line = {
        let (mut alive, mut drawn, mut stacked) = (0usize, 0usize, 0usize);
        let mut cards: Vec<String> = Vec::new();
        for (w, vv, gt, card, mat, tag, class) in stores.objects.iter() {
            if w.kind == obj.kind && w.id == obj.id {
                alive += 1;
                drawn += usize::from(vv.get());
                // A `VisibilityClass` listing its mesh class twice is queued, and drawn, twice.
                stacked += usize::from(class.is_some_and(|c| c.len() > 1));
                if card {
                    // A glow card as drawn: world scale (placement times the bone's pulse), tag
                    // alpha, and the material word. Additive is `clutter_fade.z` bit 2, which keys
                    // the (ONE, ONE) blend and the shader's alpha fold.
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
    // The unit's name through the ask-once cache; it fills on a later frame.
    let name_line = net_entity
        .and_then(|p| guids.get(p).ok())
        .and_then(|g| names.resolve_unit(g.0, store, &net_commands))
        .map(str::to_string);
    // Gate by kind, not field presence: a create-seeded store answers every field (absent is 0).
    let kind = net_entity.and_then(|p| kinds.get(p).ok()).map(|n| n.kind);
    let is_unit = matches!(
        kind,
        Some(benilla_protocol::EntityKind::Unit | benilla_protocol::EntityKind::Player)
    );
    let is_player = kind == Some(benilla_protocol::EntityKind::Player);
    // A GameObject's stored `GAMEOBJECT_STATE` and its hull, the passability gate's inputs: a door
    // reading `state 0 open · SOLID` is the gate failing.
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
            // The interact gate (`0x5f2f80`): the strategy vtable's `+0x14` highlightable slot is
            // the one predicate behind the cursor, the +64 brighten, right-click USE and pick
            // priority. SPELL_FOCUS's slot is a constant `xor al,al`, so an anvil reads `✗`.
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
            // The tooltip's own ladder, distinct from `interact`:
            //  · `hover ✗`: the mouseover-eligibility slot `+0x54` said no, so the reference
            //    publishes a NULL mouseover; for a GENERIC(5) signpost it is the template's
            //    `data[1]` highlight column.
            //  · `hover ✓`, `shown ✗`: the publish lost to occlusion, a nearer unit or UI.
            //  · `shown ✓` and no tooltip: the fault is in [`crate::ui_tooltip`].
            // `tmpl —` is the ask-once template query not answered yet.
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
            // TEXT (type 9) only, the template's page: `page —` means none, so no window opens;
            // `page ?` means the query has not answered yet.
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
            // The tilt of the `GAMEOBJECT_ROTATION` quaternion, shown only when there is one: most
            // spawns are a plain yaw, and a tilt swings off-origin geometry yards from the spawn.
            let tilt = s
                .0
                .gameobject_rotation()
                // The model's up-axis off world up, `acos(m22)`, i.e. `2·asin(|x, y|)`.
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
    // Raw bytes: creature race and class do not share the player-race enum.
    let appearance_line = store.filter(|_| is_unit).map(|s| {
        format!(
            "race {} · class {} · sex {}",
            s.0.unit_race().unwrap_or(0),
            s.0.unit_class().unwrap_or(0),
            s.0.unit_gender().unwrap_or(0)
        )
    });
    // Why this unit does or does not carry a plate: every input of `vplates::drive_vplates`'s
    // gate and the verdict read from [`crate::vplates::VPlates`].
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
        // A faction with a reputation slot reacts by our standing (`0x605fc0` -> `0x4d63a0`),
        // before any template comparison; everything else by the template comparator.
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
                // The category the gate would have put it in, and whether that bit is on.
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
    // Player customization, the compositor's input, raw from `PLAYER_BYTES`.
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

    // A unit mid-cast, between `SMSG_SPELL_START` and GO: the spell's id and name.
    let casting_line = net_entity.and_then(|p| castings.get(p).ok()).map(|c| {
        match spells.as_ref().and_then(|s| s.catalog.get(c.spell_id)) {
            Some(d) => format!("casting {} \"{}\"", c.spell_id, d.name),
            None => format!("casting {}", c.spell_id),
        }
    });
    // Motion: a server path's constant speed (length over duration, as the gait selector reads
    // it) and progress, or a remote mover's dead-reckoning speed; `still` means no path at all.
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
    // The ground clamp's memo ([`crate::net::GroundClamped`]): `z` is drawn, `seat` the server's
    // last pose, `drop` = `seat - z` the clamp's correction, `terrain` the MCNK height. `walk` is
    // the swept step on a server path and `idle` the settle from the seat; an idle miss stays at
    // the seat, a walk miss descends.
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
    // An `AnimationData` id by name, for both anim lines.
    let fmt = |id: u16| match anim_data.as_ref().and_then(|a| a.0.name(id)) {
        Some(name) => format!("{name}({id})"),
        None => format!("{id}"),
    };
    // The requested `AnimationData` ids, before missing-clip substitution: the full-body base and
    // any masked upper-body overlay.
    let anim_line = net_entity.and_then(|p| drivers.get(p).ok()).map(|d| {
        let (base, overlay) = d.playing();
        let base = base.map(&fmt).unwrap_or_else(|| "—".into());
        // The base slot's playback rate, `speed / (moveSpeed · modelScale)` on a locomotion clip;
        // hidden at 1x.
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
    // A GameObject's line, from `GoAnim`: the sequence the state machine's arm (`0x5f3930`) plays,
    // a transient one the completion advance `0x5f4120` must end or the held rest pose, and its
    // repeat. `transition · loops` is a door stuck mid-motion.
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
    // The light lane and the attach that found it; absent until the anchor resolves once.
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

    // The card's lines, which a left-click copies.
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

    // The inspector owns left-click while armed; `player::control` suppresses left-orbit.
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

/// The nearest identified thing under the cursor this frame. `entity` is `Some` only when one
/// owns the geometry: most of the static world draws from a consolidated lane with none.
#[derive(Resource, Default)]
pub struct MouseoverTarget {
    pub object: Option<WorldObject>,
    pub entity: Option<Entity>,
    pub point: Vec3,
    pub distance: f32,
}

/// Ray-cast from the cursor to the nearest [`WorldObject`], only while inspect is armed; terrain
/// and other unidentified meshes are transparent to it.
pub(super) fn update_mouseover(
    inspect: Res<InspectMode>,
    pointer_over_ui: Res<PointerOverUi>,
    mut target: ResMut<MouseoverTarget>,
    window: Query<&Window, With<PrimaryWindow>>,
    camera: Query<(&Camera, &GlobalTransform), With<WorldCamera>>,
    objects: Query<(Entity, &WorldObject)>,
    // Everything drawn, entity-owned or not.
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
    // No pick behind UI.
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
        return; // outside the window, or hidden in mouselook
    };
    let identified: HashSet<Entity> = objects.iter().map(|(e, _)| e).collect();
    if let Some(hit) = pick.at_cursor(cursor, camera, cam_tf, &identified) {
        target.object = hit.object;
        target.entity = hit.entity;
        target.point = hit.hit.point;
        target.distance = hit.hit.distance;
    }
}
