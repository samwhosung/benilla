//! Overhead unit names, the 1.12 world-text name system.
//!
//! - Geometry: a camera-facing glyph mesh in the world pass, depth-tested like the reference's name
//!   batch (`0x6c7470` calls `0x5c1d60(1,1)`), so walls occlude names. Deviation:
//!   `AlphaMode::Blend` writes no depth where the reference does, because overlapping names
//!   resolving by sort order look the same.
//! - Colour: the render `0x6c6e90` calls the ground ring's selector (`0x605960`).
//! - Anchor: the posed PlayerName attachment, re-read every frame with no smoothing, as the
//!   reference does.
//! - Seat: the top line's baseline is at `anchor.z + lineCount*scale` and the block hangs down one
//!   pitch per line (`0x5cdc20`'s rotated branch).
//! - Show gate (`ShouldShowName`, `0x6070a0`): the own unit by `UnitNameOwn`, before the target
//!   rescue; the current target (`[0xb4e2d8]`, the selection) regardless of CVars; a dead creature
//!   only through that rescue; others by `UnitNamePlayer` or `UnitNameNPC`.
//! - Lines (`0x608f50`): an NPC's name and `<Subname>`; a player's `<AFK>`/`<DND>`/`<GM>`
//!   prefixes, the a4 rank prefix (`UnitNamePlayerPVPTitle`, bit `0x20`) and name, and `<Guild>`.
//!   The guild (a5) and subname (a6) slots share the format `"\n<%s>"` (`0x860f9c`) and are never
//!   both reached; a5 alone is CVar-gated (`0x609085`).
//! - The a4 slot is `0x609370`'s ranked-player leg (A, `0x6093a5`): `UNIT_PVP_NAME` filled rank
//!   first, `PVP_RANK_<rank>_<team>` off the unit's public `PLAYER_BYTES_3` byte 3 and its race's
//!   team digit, and never a creature's.
//!
//! Not built: `0x609370`'s civilian leg (`0x609449`, `PVP_RANK_CIVILIAN` and a space before a
//! hostile PvP-flagged civilian NPC's name) and its city-protector line (`0x6093ef`, `"\n"` and
//! `PVP_MEDAL<n>` for a set `PLAYER_BYTES_3` byte 2, which vmangos's `.character citytitle` sets);
//! a7 and a8 have no cross-realm wire here.

use std::collections::HashMap;

use bevy::prelude::*;

use benilla_protocol::EntityKind;
use benilla_ui::script::{JustifyH, JustifyV, Outline};

use crate::entities::{overhead_anchor, BoneAttach, OverheadFallback};
use crate::names::NameCache;
use crate::net::{Guid, NetCommands, NetEntity, ObjectStore, Reputations, SelfPlayer};
use crate::target::{selection_variant, CombatFlash, Factions, RingVariant, Selection};
use crate::ui_text::{layout_text_quads, FontSpec, Justify, TextSeat, UiFontAtlas};
use benilla_world::view::WorldCamera;

/// The height-scale law (`0x6c6e90`): `d > KNEE ? d/KNEE · RATE · FLOOR : FLOOR`, `d` the anchor's
/// height above the feet in world units.
const SCALE_FLOOR: f32 = 0.2; // [0x80679c]
const SCALE_KNEE: f32 = 4.0; // [0x8112a8]
const SCALE_RATE: f32 = 1.5; // [0x8112ac]

/// The `UnitNamePlayer`/`UnitNameNPC`/`UnitNameOwn` CVars (the reference's `0xce8720` mask), with
/// the reference's defaults `"1"`, `"0"`, `"0"` (`0x6c7470`).
#[derive(Resource, Clone, Copy)]
pub(crate) struct NameConfig {
    pub(crate) player: bool,
    pub(crate) npc: bool,
    pub(crate) own: bool,
    /// `UnitNamePlayerGuild`, mask bit `0x10`, registered `"1"`: gates the a5 guild line
    /// (`0x609085`), not a whole name.
    pub(crate) player_guild: bool,
    /// `UnitNamePlayerPVPTitle` (`0x86c668`, registered `"1"`), mask bit `0x20`: gates the a4 rank
    /// prefix on the main name line (`0x609370`).
    pub(crate) player_pvp_title: bool,
}

impl Default for NameConfig {
    fn default() -> Self {
        Self {
            player: true,
            npc: false,
            own: false,
            player_guild: true,
            player_pvp_title: true,
        }
    }
}

/// The a1-a3 prefix slots of the line stack (`0x608f50`), CGPlayer vtable `+0x7c/+0x80/+0x84`
/// (`0x5ec9e0`, `0x5eca40`, `0x5eca80`): each set `PLAYER_FLAGS` bit (vmangos `Player.h:316-318`)
/// stacks in slot order with no space before the name; the base CGUnit slots are stubs, so NPCs
/// never decorate. The tags are `CHAT_FLAG_AFK/DND/GM` (`GlobalStrings.lua:534-536`).
/// The own player's AFK slot also reads the client-side mirror `[0xb6e5cc]` ([`drive_nameplates`]).
const FLAG_PREFIXES: [(u32, &str); 3] = [(0x2, "<AFK>"), (0x4, "<DND>"), (0x8, "<GM>")];

/// The name-line prefix for a player's `PLAYER_FLAGS`; empty when unflagged.
fn flag_prefix(player_flags: u32) -> String {
    FLAG_PREFIXES
        .iter()
        .filter(|(bit, _)| player_flags & bit != 0)
        .map(|(_, s)| *s)
        .collect()
}

/// The strings the a4 rank prefix resolves (`UNIT_PVP_NAME` and the `PVP_RANK_*` titles), which
/// the honor feed reads off each VM once ([`crate::ui_honor`]), where `0x609370` reads them live
/// at every rebuild: the world-text pass runs without the VM.
#[derive(Resource, Default)]
pub(crate) struct PvpNameStrings {
    /// The VM session the strings were read from; `None` before the first VM.
    session: Option<u64>,
    /// Moves when a read changes the strings, so a line built from the old ones rebuilds.
    generation: u64,
    strings: HashMap<String, String>,
}

impl PvpNameStrings {
    /// The VM session the strings were read from; `None` before the first VM.
    pub(crate) fn session(&self) -> Option<u64> {
        self.session
    }

    /// A VM's strings, every key it defines; a missing one is the builder's own miss. The
    /// generation moves only when they differ, so a new VM with the same strings rebuilds no plate.
    pub(crate) fn set(&mut self, session: u64, strings: HashMap<String, String>) {
        self.session = Some(session);
        if strings != self.strings {
            self.strings = strings;
            self.generation += 1;
        }
    }

    /// One held string, for the feed's tests.
    #[cfg(test)]
    pub(crate) fn get(&self, key: &str) -> Option<&str> {
        self.strings.get(key).map(String::as_str)
    }

    /// Whether the snapshot holds the template: no install strings, no prefix, as `UnitPVPName`
    /// answers the plain name with `_G` missing.
    fn ready(&self) -> bool {
        self.strings.contains_key("UNIT_PVP_NAME")
    }

    /// `0x609370`'s ranked-player leg for `key`; `None` where the builder declines.
    fn decorated(&self, key: TitleKey, name: &str) -> Option<String> {
        benilla_ui::script::decorated_name(
            |global| self.strings.get(global).cloned(),
            key.title,
            name,
        )
    }
}

/// The a4 rank-prefix inputs a live line was built with: the steady frame compares these fields,
/// so a rank change, a CVar flip or a new VM's different strings rebuild without reading the
/// strings again.
#[derive(Clone, Copy, PartialEq, Eq)]
struct TitleKey {
    /// The rank, team and gender the title lookup keyed on
    /// ([`benilla_ui::script::RankTitle`]).
    title: benilla_ui::script::RankTitle,
    /// The a4 bit (`0x20`) was set and the strings held the template when the line was built.
    on: bool,
    /// The [`PvpNameStrings`] generation the line was built from.
    strings: u64,
}

/// The a4 inputs for one unit, cheap enough for the steady frame: `None` off a player or before
/// its descriptor arrives.
fn title_key(
    cfg: &NameConfig,
    strings: &PvpNameStrings,
    net: &NetEntity,
    store: Option<&ObjectStore>,
) -> Option<TitleKey> {
    let store = store?;
    (net.kind == EntityKind::Player).then(|| TitleKey {
        title: benilla_ui::script::RankTitle {
            rank: store.0.player_pvp_rank().unwrap_or(0),
            team: store
                .0
                .unit_race()
                .map_or(-1, crate::ui_unit::race_pvp_team),
            female: store.0.unit_gender() == Some(1),
        },
        on: cfg.player_pvp_title && strings.ready(),
        strings: strings.generation,
    })
}

/// `0x608f50`'s main name line: the a1-a3 flag tags glued on with no separator, then the a4 slot,
/// `0x609370`'s ranked-player leg where `key` is on and the builder resolves, else the plain name.
fn name_line(flags: u32, key: Option<TitleKey>, strings: &PvpNameStrings, name: &str) -> String {
    let decorated = key
        .filter(|key| key.on)
        .and_then(|key| strings.decorated(key, name));
    let mut line = flag_prefix(flags);
    line.push_str(decorated.as_deref().unwrap_or(name));
    line
}

/// A live plate: its entity and stack, and the inputs the stack was built from, so a steady frame
/// compares fields and re-reads neither the name cache nor the install strings.
struct LiveName {
    plate: Entity,
    lines: Vec<String>,
    paint: NamePaint,
    /// `0x608f50`'s line inputs at build time.
    flags: u32,
    name: String,
    bracketed: Option<String>,
    key: Option<TitleKey>,
}

/// The name's world scale for a unit whose overhead anchor sits `d` world units above its feet.
/// Shared with the raid-target marker ([`crate::raid_marks`]), which it raises (`0x6c70d8`).
pub(crate) fn height_scale(d: f32) -> f32 {
    if d > SCALE_KNEE {
        (d / SCALE_KNEE) * SCALE_RATE * SCALE_FLOOR
    } else {
        SCALE_FLOOR
    }
}

/// What a name line is painted with: the ring's selector ([`selection_variant`]), which the
/// reference's name render also calls (`0x605960`), or the combat flash.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum NamePaint {
    /// The selector's answer.
    Variant(RingVariant),
    /// The combat-flash pulse, the selector's first-priority branch; its tint is rewritten each
    /// frame from [`CombatFlash::color`].
    Flash,
}

impl NamePaint {
    // `linear_rgb` passes the authored bytes raw into the gamma framebuffer: unlit writes
    // base_color as-is, and the frame decodes once at FFXGlow.
    fn color(self) -> Color {
        match self {
            Self::Variant(v) => v.color(),
            Self::Flash => Color::linear_rgb(1.0, 0.0, 0.0), // seed: the wave's G=0 endpoint
        }
    }
}

/// Materials per colour, meshes per line stack and the live plate per unit. The meshes bake glyph
/// UVs, so they die when the glyph sheet resets; the sheet's texture handle never changes.
#[derive(Resource, Default)]
pub(crate) struct Nameplates {
    materials: HashMap<NamePaint, Handle<StandardMaterial>>,
    meshes: HashMap<Vec<String>, Handle<Mesh>>,
    /// unit → its live plate and the inputs the stack was built from.
    live: bevy::ecs::entity::EntityHashMap<LiveName>,
    /// The atlas generation the meshes' UVs were built from; `None` before the first build.
    baked_from: Option<u64>,
}

impl Nameplates {
    /// Whether `unit` shows an overhead name (the reference's `unit+0xc7c` name object is live);
    /// the questgiver marker's raised (anim 190) bob keys on it (`0x6076c0` checks `0x6c7950`).
    pub(crate) fn shows(&self, unit: Entity) -> bool {
        self.live.contains_key(&unit)
    }

    /// The line count of `unit`'s live name, for the raid marker's seat (`0x6c70d8`: one pitch
    /// above the block, or at the bare anchor when no name shows).
    pub(crate) fn line_count(&self, unit: Entity) -> Option<usize> {
        self.live.get(&unit).map(|live| live.lines.len())
    }
}

/// Marker on a plate entity (a world-pass billboard mesh, root-level, following its unit).
#[derive(Component)]
struct NamePlate;

/// Deviation: a mounted plate keeps only this fraction of its seat's oscillation about a slow
/// mean, because the raw seat, which the reference also rides, read more intense than the
/// reference side by side. `1.0` is the raw seat; on-foot plates are untouched.
const ROCK_KEEP: f32 = 0.7;
/// The mean tracker's rate (1/s): slow against the ~1 Hz gallop, fast enough to follow a stance.
const ROCK_MEAN_RATE: f32 = 1.5;
/// A residual this large (yd) is a stance change (mount swap, teleport): snap the mean to it.
const ROCK_SNAP: f32 = 1.0;

/// The raster size (logical px) of the mesh bake, which only sets crispness. Deviation: larger
/// than the reference's 32 px `UNIT_NAME_FONT` raster, which it magnifies (`0x6c749b`), because a
/// larger source is crisper up close.
const BAKE_PX: f32 = 36.0;

/// Build one name-block mesh in pitch units: x centered, y up, line `i`'s baseline at
/// `lineCount - i`, one pitch being the font size. The baseline sits `TextEngine::ascent_ratio`
/// into the layout cell (the `[0x17c]` load param), as in the 2-D path.
fn build_name_mesh(atlas: &mut UiFontAtlas, lines: &[String]) -> Mesh {
    use bevy::asset::RenderAssetUsages;
    use bevy::mesh::{Indices, PrimitiveTopology};

    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let n = lines.len() as f32;
    // The baseline's place in the one-pitch cell, `round(size * asc/(asc+|desc|))`; it only has to
    // agree with `layout_text_quads`, since the baseline stays on the grid either way.
    let mut e = atlas.lock();
    // The size the glyphs are laid out at: `BAKE_PX` rounded to whole device pixels and back.
    let src = e.drawn_size(BAKE_PX);
    let baseline_frac =
        ((f64::from(src) * f64::from(e.ascent_ratio(None)) + 0.5).floor() / f64::from(src)) as f32;
    for (i, line) in lines.iter().enumerate() {
        let glyphs = layout_text_quads(
            &mut e,
            line,
            Rect::from_center_size(Vec2::ZERO, Vec2::ZERO),
            [1.0, 1.0, 1.0, 1.0], // the material tints; glyph alpha rides the texture
            Justify {
                h: JustifyH::Center,
                v: JustifyV::Middle,
            },
            0,
            FontSpec {
                path: None, // UNIT_NAME_FONT = Friz Quadrata, the default face
                height: Some(BAKE_PX),
                outline: Outline::None,
                alpha_gradient: None,
            },
            // A world billboard: the glyphs are re-seated in pitch units below, off the UI grid.
            TextSeat::Exact,
        );
        // Recenter the ink box on x = 0, then normalize px → pitch units, flipping y-down px into
        // y-up locals within this line's band.
        let Some(bounds) = glyphs.iter().map(|q| q.rect).reduce(|a, b| a.union(b)) else {
            continue;
        };
        let cx = (bounds.min.x + bounds.max.x) * 0.5;
        // Line `i`'s baseline lands at local y = n - i; the layout puts it `baseline_frac` of a
        // cell below the cell top, so the cell top maps to n - i + baseline_frac.
        let line_top = n - i as f32 + baseline_frac;
        for q in &glyphs {
            let x0 = (q.rect.min.x - cx) / src;
            let x1 = (q.rect.max.x - cx) / src;
            let y0 = line_top - q.rect.min.y / src; // px top → local (higher)
            let y1 = line_top - q.rect.max.y / src; // px bottom → local (lower)
            let base = positions.len() as u32;
            positions.extend([[x0, y0, 0.0], [x1, y0, 0.0], [x1, y1, 0.0], [x0, y1, 0.0]]);
            let [tl, tr, br, bl] = q.uv.corners;
            uvs.extend([tl, tr, br, bl]);
            normals.extend([[0.0, 0.0, 1.0]; 4]);
            indices.extend([base, base + 2, base + 1, base, base + 3, base + 2]);
        }
    }
    drop(e);
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

/// Drive the plates: gate, resolve lines and colour, and rebuild the plate on change, in Update
/// where mesh churn is supported.
#[allow(clippy::type_complexity)] // one Bevy system's full input set
pub(crate) fn drive_nameplates(
    mut commands: Commands,
    units: Query<(
        Entity,
        &NetEntity,
        &Guid,
        &Transform,
        Option<&ObjectStore>,
        Has<SelfPlayer>,
        // Whether the body is drawn: a unit outside the scene has nothing to float a name over.
        Option<&InheritedVisibility>,
    )>,
    self_store: Query<&ObjectStore, With<SelfPlayer>>,
    // The optimistic AFK mirror (`[0xb6e5cc]`), the own player's `<AFK>` override.
    mirror: Res<crate::ui_chat::AfkMirror>,
    // The show-gate inputs, one tuple param for Bevy's 16-param ceiling.
    gates: (
        Res<Selection>,
        Res<CombatFlash>,
        Option<Res<crate::player::CameraControl>>,
        Res<crate::vplates::VPlates>,
        Res<crate::chat_bubble::BubblesActive>,
        // The selector's party roster (`0xbc6f48`), for the pale party variants.
        Res<crate::ui_party::GroupState>,
        // The UnitName* CVar mask.
        Res<NameConfig>,
        // The rank strings, read off the current VM ([`PvpNameStrings`]).
        Res<PvpNameStrings>,
    ),
    names: Res<NameCache>,
    // The guild cache, the a5 line's text; `ResMut` because a miss sends `CMSG_GUILD_QUERY`.
    mut guilds: ResMut<crate::ui_guild::GuildState>,
    net_commands: Res<NetCommands>,
    factions: Option<Res<Factions>>,
    reputations: Res<Reputations>,
    camera: Query<&Transform, With<WorldCamera>>,
    mut atlas: Option<ResMut<UiFontAtlas>>,
    mut plates: ResMut<Nameplates>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    // The `overhead_anchor` inputs, for the spawn-frame seat only.
    anchor_q: (
        Query<&BoneAttach>,
        Query<&benilla_world::rig_anim::RigPose>,
        Query<&OverheadFallback>,
        Query<&GlobalTransform>,
        Query<(), With<crate::entities::mount::MountChild>>,
    ),
) {
    let (selection, flash, rig, vplates, bubbles, group, name_cfg, pvp_strings) = gates;
    let (Ok(cam_tf), Some(atlas)) = (camera.single(), atlas.as_mut()) else {
        return;
    };
    // A glyph-sheet reset moves every cached UV.
    drop_stale_glyph_caches(&mut plates, atlas.generation, &mut commands);
    // The palette materials, once (they need the atlas image).
    if plates.materials.is_empty() {
        let image = atlas.image();
        for kind in RingVariant::ALL
            .map(NamePaint::Variant)
            .into_iter()
            .chain([NamePaint::Flash])
        {
            plates.materials.insert(
                kind,
                materials.add(StandardMaterial {
                    base_color: kind.color(),
                    base_color_texture: Some(image.clone()),
                    unlit: true,
                    // Depth-tested; Blend skips the depth write (the module doc's deviation).
                    alpha_mode: AlphaMode::Blend,
                    cull_mode: None,
                    // Sorted after every ordinary transparent (sign law in `sky_order`), as the
                    // reference draws world text after the liquid; water cannot cover a name.
                    depth_bias: benilla_world::sky_order::Rung::NAMEPLATE,
                    ..default()
                }),
            );
        }
    }
    // Written only while a flash is live, so an idle frame does not re-upload the material.
    if flash.unit.is_some() {
        if let Some(mat) = plates
            .materials
            .get(&NamePaint::Flash)
            .and_then(|h| materials.get_mut(h))
        {
            mat.base_color = flash.color;
        }
    }
    // The billboard facing: screen-aligned (the camera's own rotation), this frame's camera.
    let facing = cam_tf.rotation;
    let mut seen = bevy::ecs::entity::EntityHashSet::default();
    for (entity, net, guid, tf, store, is_self, drawn) in &units {
        // A name needs a drawn body; this precedes the ShouldShowName ladder.
        if !drawn.is_none_or(|v| v.get()) {
            continue;
        }
        // No distance cull: the name's update, cull and build (`0x6c6d40`, `0x6c6e00`,
        // `0x6c6e90`) compare none; the 20-yard cap (`0x60f600`) is the V-key frame's.
        // ShouldShowName's order: own unit by its CVar, the target (`[0xb4e2d8]`), the kind CVar.
        // A V-key plate or chat bubble (`+0xe64`) suppresses the name (`0x6070a0`). A dead
        // creature shows only as the target, as in the reference; that gate leg is untraced.
        let show = if vplates.0.contains(&entity) || bubbles.0.contains(&entity) {
            false
        } else if store.is_some_and(|s| s.0.unit_is_ghost_visual()) {
            // The ghost vis-flag suppresses the name, target rescue included: `0x607101` and
            // `0x60f62e` both test `bytes_1` byte 3 `& 3`.
            false
        } else if is_self {
            // The own CVar, before the rescue; no name over a fully faded first-person avatar
            // (our reading of `ShouldShowName`'s untraced `vtable+0x58` leg).
            name_cfg.own
                && rig
                    .as_deref()
                    .map_or(1.0, crate::player::CameraControl::self_fade)
                    > 0.0
        } else if selection.target == Some(entity) {
            true
        } else if net.kind == EntityKind::Unit && store.is_some_and(|s| s.0.unit_is_dead()) {
            false
        } else {
            match net.kind {
                EntityKind::Player => name_cfg.player,
                EntityKind::Unit => name_cfg.npc,
                _ => false,
            }
        };
        if !show {
            continue;
        }
        // Re-read by `peek`: resolve's `&mut`-tied return cannot outlive the subname read.
        if names.resolve_unit(guid.0, store, &net_commands).is_none() {
            continue;
        }
        let Some(name) = names.peek_unit(guid.0, store) else {
            continue;
        };
        // The player flag prefixes (a1-a3), glued onto the name with no space.
        let flags = if net.kind == EntityKind::Player {
            store.map_or(0, |s| s.0.player_flags())
        } else {
            0
        };
        // The AFK slot `0x5ec9e0` alone emits `<AFK>` for the active player while the mirror
        // `[0xb6e5cc]` is set, whatever the bit (`0x5ec9fd`). Folded into `flags`, so the cache
        // compare and [`name_line`] read the same bit.
        let flags = if is_self && mirror.is_afk() {
            flags | 0x2
        } else {
            flags
        };
        // The one `"\n<%s>"` slot: a6 subtitle for an NPC, a5 guild for a player (CVar-gated).
        let bracketed = match net.kind {
            EntityKind::Unit => benilla_protocol::guid::entry(guid.0)
                .and_then(|e| names.creature_subname(e))
                // vmangos sends "" for most templates: an empty subname is no line.
                .filter(|s| !s.trim().is_empty()),
            EntityKind::Player if name_cfg.player_guild => store
                .and_then(|s| crate::ui_guild::unit_guild_name(&s.0, &mut guilds, &net_commands)),
            _ => None,
        };
        // The selector's first-priority branch: the combat flash while we melee this unit.
        let color = if flash.unit == Some(entity) {
            NamePaint::Flash
        } else {
            NamePaint::Variant(selection_variant(
                factions.as_deref(),
                &reputations,
                store,
                self_store.single().ok(),
                net.kind == EntityKind::Player,
                group.members.iter().any(|m| m.guid == guid.0),
            ))
        };

        seen.insert(entity);
        let key = title_key(&name_cfg, &pvp_strings, net, store);
        match plates.live.get(&entity) {
            // The steady frame compares the inputs in place, allocating nothing; a rebuild from
            // these inputs is the stack `LiveName` holds.
            Some(live)
                if live.paint == color
                    && live.flags == flags
                    && live.key == key
                    && live.name == name
                    && live.bracketed.as_deref() == bracketed => {}
            stale => {
                if let Some(live) = stale {
                    if let Ok(mut e) = commands.get_entity(live.plate) {
                        e.despawn();
                    }
                }
                let mut lines = vec![name_line(flags, key, &pvp_strings, name)];
                if let Some(bracketed) = bracketed {
                    lines.push(format!("<{bracketed}>"));
                }
                debug!("nameplates: rebuild {entity} -> {lines:?} ({color:?})");
                let mesh = plates
                    .meshes
                    .entry(lines.clone())
                    .or_insert_with(|| meshes.add(build_name_mesh(atlas, &lines)))
                    .clone();
                let material = plates.materials[&color].clone();
                // The spawn-frame seat: placement can miss a plate whose unit despawns later
                // this Update, and a plate must never render at the origin.
                let anchor = overhead_anchor(
                    entity,
                    tf,
                    &anchor_q.0,
                    &anchor_q.1,
                    &anchor_q.2,
                    &anchor_q.3,
                    &anchor_q.4,
                );
                let scale = height_scale(anchor.y - tf.translation.y);
                let place = Transform {
                    translation: anchor,
                    rotation: facing,
                    scale: Vec3::splat(scale),
                };
                let plate = commands
                    .spawn((Mesh3d(mesh), MeshMaterial3d(material), place, NamePlate))
                    .id();
                plates.live.insert(
                    entity,
                    LiveName {
                        plate,
                        lines,
                        paint: color,
                        flags,
                        name: name.to_owned(),
                        bracketed: bracketed.map(str::to_owned),
                        key,
                    },
                );
            }
        }
    }
    // Plates whose unit despawned or gated off this frame.
    plates.live.retain(|unit, live| {
        if seen.contains(unit) {
            true
        } else {
            if let Ok(mut e) = commands.get_entity(live.plate) {
                e.despawn();
            }
            false
        }
    });
}

/// Seat every live plate from this frame's propagated pose, so a moving name does not trail;
/// plates are roots, so writing `GlobalTransform` directly is exact.
#[allow(clippy::type_complexity)] // one Bevy system's full input set
fn place_nameplates(
    plates: Res<Nameplates>,
    camera: Query<&Transform, (With<WorldCamera>, Without<NamePlate>)>,
    units: Query<&Transform, (Without<NamePlate>, Without<WorldCamera>)>,
    mut plate_tfs: Query<(&mut Transform, &mut GlobalTransform), With<NamePlate>>,
    anchor_q: (
        Query<&BoneAttach>,
        Query<&benilla_world::rig_anim::RigPose>,
        Query<&OverheadFallback>,
        Query<&GlobalTransform, Without<NamePlate>>, // disjoint from the plate global write
        Query<(), With<crate::entities::mount::MountChild>>,
    ),
    trace: (
        Res<NameAnchorTrace>,
        Res<Time>,
        Query<(), With<crate::net::SelfPlayer>>,
    ),
    // unit → the slow mean of (anchor - root) while mounted, the rock-damping state.
    mut rock: Local<bevy::ecs::entity::EntityHashMap<Vec3>>,
) {
    let Ok(cam_tf) = camera.single() else {
        return;
    };
    let facing = cam_tf.rotation;
    let blend = 1.0 - (-ROCK_MEAN_RATE * trace.1.delta_secs()).exp();
    for (&unit, live) in plates.live.iter() {
        let (Ok(tf), Ok((mut ptf, mut pglobal))) = (units.get(unit), plate_tfs.get_mut(live.plate))
        else {
            continue; // spawned this frame and not yet flushed, or the unit is despawning
        };
        let raw = overhead_anchor(
            unit,
            tf,
            &anchor_q.0,
            &anchor_q.1,
            &anchor_q.2,
            &anchor_q.3,
            &anchor_q.4,
        );
        // Mounted: mean + ROCK_KEEP * residual, root-relative so the plate never lags.
        let anchor = if anchor_q.4.contains(unit) {
            let off = raw - tf.translation;
            let mean = rock.entry(unit).or_insert(off);
            if (off - *mean).length_squared() > ROCK_SNAP * ROCK_SNAP {
                *mean = off;
            } else {
                *mean += (off - *mean) * blend;
            }
            tf.translation + *mean + (off - *mean) * ROCK_KEEP
        } else {
            raw
        };
        if trace.0 .0 && trace.2.contains(unit) {
            // Root, seated (damped) anchor and camera per frame; `raw` is the undamped joint read.
            let (r, a, c) = (tf.translation, anchor, cam_tf.translation);
            info!(
                "NAME_TRACE t={:.4} root=({:.4},{:.4},{:.4}) anchor=({:.4},{:.4},{:.4}) cam=({:.3},{:.3},{:.3}) raw=({:.4},{:.4},{:.4})",
                trace.1.elapsed_secs(), r.x, r.y, r.z, a.x, a.y, a.z, c.x, c.y, c.z, raw.x, raw.y, raw.z
            );
        }
        let scale = height_scale(anchor.y - tf.translation.y);
        let place = Transform {
            translation: anchor,
            rotation: facing,
            scale: Vec3::splat(scale),
        };
        // Write only on a bit-level change, so a parked plate is not marked changed every frame.
        {
            let t = ptf.bypass_change_detection();
            if *t != place {
                *t = place;
                ptf.set_changed();
                *pglobal = GlobalTransform::from(place);
            }
        }
    }
    // Dismounted or despawned units drop their damping state (a re-mount starts fresh).
    rock.retain(|e, _| anchor_q.4.contains(*e));
}

/// Registers the plate driver (Update) and the per-frame placer (PostUpdate, after propagation,
/// before visibility culling).
pub(crate) struct NameplatesPlugin;

/// `WOW_PROBE_NAME_TRACE=1`: per-frame `NAME_TRACE` lines for the self player's plate seat.
#[derive(Resource)]
struct NameAnchorTrace(bool);

/// The overhead-name rows' change callback: the name trio, the guild line and the rank prefix.
pub(crate) fn on_cvar(ev: On<crate::cvars::CvarChanged>, mut names: ResMut<NameConfig>) {
    match ev.key().as_str() {
        "unitnameplayer" => names.player = ev.flag(),
        "unitnamenpc" => names.npc = ev.flag(),
        "unitnameown" => names.own = ev.flag(),
        "unitnameplayerguild" => names.player_guild = ev.flag(),
        "unitnameplayerpvptitle" => names.player_pvp_title = ev.flag(),
        _ => {}
    }
}

impl Plugin for NameplatesPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_cvar);
        app.insert_resource(NameAnchorTrace(
            std::env::var_os("WOW_PROBE_NAME_TRACE").is_some(),
        ))
        .init_resource::<Nameplates>()
        .init_resource::<NameConfig>()
        .init_resource::<PvpNameStrings>()
        // After targeting, V-key plates and chat bubbles: the gate reads their verdicts.
        .add_systems(
            Update,
            drive_nameplates
                .after(crate::target::TargetUpdate)
                .after(crate::vplates::VPlateSet)
                .after(crate::chat_bubble::BubbleSet),
        )
        .add_systems(Update, evict_name_meshes)
        .add_systems(
            PostUpdate,
            place_nameplates
                .after(bevy::transform::TransformSystems::Propagate)
                .before(bevy::camera::visibility::VisibilitySystems::CheckVisibility),
        );
    }
}

/// Empty the UV-bearing caches when the glyph sheet resets, despawning the live plates too (they
/// hold the old `Mesh3d`); the materials survive. The first build only seeds.
fn drop_stale_glyph_caches(plates: &mut Nameplates, generation: u64, commands: &mut Commands) {
    if plates.baked_from == Some(generation) {
        return;
    }
    if plates.baked_from.is_some() {
        debug!(
            "nameplates: glyph sheet reset (generation {generation}) — dropping {} meshes and {} \
             live plates",
            plates.meshes.len(),
            plates.live.len(),
        );
        for live in plates.live.values() {
            if let Ok(mut e) = commands.get_entity(live.plate) {
                e.despawn();
            }
        }
        plates.live.clear();
        plates.meshes.clear();
    }
    plates.baked_from = Some(generation);
}

/// Drop the line-stack mesh dedup on a cross-map transition: the old map's names never return.
fn evict_name_meshes(
    mut changes: MessageReader<benilla_world::world_map::MapChange>,
    mut plates: ResMut<Nameplates>,
) {
    if changes.is_empty() {
        return;
    }
    changes.clear();
    plates.meshes.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::target::{ring_variant, PlayerPath, SelectorInput};

    /// `>` not `>=` at the knee and `0.075*d` beyond: the jump at the knee is the reference's.
    #[test]
    fn height_scale_matches_the_byte_law() {
        assert_eq!(height_scale(1.8), 0.2, "human: the floor");
        assert_eq!(height_scale(4.0), 0.2, "knee is > not >=");
        let big = height_scale(8.0);
        assert!((big - 0.6).abs() < 1e-6, "8/4 · 1.5 · 0.2 = 0.6, got {big}");
        assert!(height_scale(4.1) > 0.2, "past the knee: the jump is real");
    }

    /// Bare concatenation in slot order AFK, DND, GM (`0x608f50`); other bits ignored.
    #[test]
    fn flag_prefix_matches_the_slot_law() {
        assert_eq!(flag_prefix(0), "");
        assert_eq!(flag_prefix(0x8), "<GM>");
        assert_eq!(flag_prefix(0x2), "<AFK>");
        assert_eq!(flag_prefix(0x4), "<DND>");
        assert_eq!(flag_prefix(0x8 | 0x2), "<AFK><GM>", "slot order, a1 first");
        assert_eq!(
            flag_prefix(0x2 | 0x4 | 0x8),
            "<AFK><DND><GM>",
            "all three stack — the client enforces no exclusivity"
        );
        assert_eq!(
            flag_prefix(0x10 | 0x20 | 0x1),
            "",
            "ghost/resting/leader don't decorate"
        );
    }

    /// A PvP-flagged friendly player's name is the ring's green, not the soft blue.
    #[test]
    fn name_color_is_the_ring_selector_itself() {
        let player = |attacks_us, attackable, pvp, in_party| {
            NamePaint::Variant(ring_variant(SelectorInput::Player(PlayerPath {
                attacks_us,
                attackable,
                pvp,
                in_party,
            })))
        };
        let npc = |rank, dead| NamePaint::Variant(ring_variant(SelectorInput::Npc { rank, dead }));
        // Flagged is green, unflagged the soft blue, and the two differ.
        let flagged = player(false, false, true, false);
        let unflagged = player(false, false, false, false);
        assert_eq!(flagged, NamePaint::Variant(RingVariant::Friendly));
        assert_eq!(unflagged, NamePaint::Variant(RingVariant::Player));
        assert_ne!(flagged.color(), unflagged.color());
        assert_eq!(
            flagged.color(),
            Color::linear_rgb(0.0, 1.0, 0.0),
            "0xFF00FF00 — the ring's own green, byte for byte"
        );
        assert_eq!(
            player(false, false, true, true),
            NamePaint::Variant(RingVariant::PartyPvp)
        );
        assert_eq!(
            player(false, false, false, true),
            NamePaint::Variant(RingVariant::Party)
        );
        assert_eq!(npc(0, false).color(), RED);
        assert_eq!(
            player(true, true, true, false).color(),
            RED,
            "mutual attack"
        );
        assert_eq!(
            player(false, true, false, false).color(),
            Color::linear_rgb(1.0, 1.0, 0.0),
            "attackable, not attacking: yellow"
        );
        assert_eq!(npc(6, true), NamePaint::Variant(RingVariant::Dead));
    }

    const RED: Color = Color::linear_rgb(1.0, 0.0, 0.0);

    /// The rank strings a VM defining `pairs` hands over, as the honor feed sets them.
    fn strings_of(session: u64, pairs: &[(&str, &str)]) -> PvpNameStrings {
        let mut strings = PvpNameStrings::default();
        strings.set(session, set_of(pairs));
        strings
    }

    fn set_of(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|&(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    const TEMPLATE: (&str, &str) = ("UNIT_PVP_NAME", "%s %s");
    const SERGEANT: (&str, &str) = ("PVP_RANK_7_1", "Sergeant");

    /// A streamed unit's kind and descriptor.
    fn unit(kind: EntityKind, fields: &[(u16, u32)]) -> (NetEntity, ObjectStore) {
        use benilla_protocol::messages::ObjectFields;
        (
            NetEntity {
                kind,
                display_id: None,
                scale: 1.0,
            },
            ObjectStore(ObjectFields::from_pairs(fields)),
        )
    }

    /// `UNIT_FIELD_BYTES_0` (36): race human, class warrior, male.
    const HUMAN_MALE: (u16, u32) = (36, 0x0101);
    /// `PLAYER_BYTES_3` (195) byte 3: the current honor rank, internal 7.
    const RANK_7: (u16, u32) = (195, 7 << 24);

    /// The a4 inputs off the descriptor: the public rank byte, the race's team digit, the sex and
    /// the CVar's strings-ready bit; a creature has no a4 leg.
    #[test]
    fn title_key_reads_the_rank_byte_the_race_and_the_cvar() {
        let cfg = NameConfig::default();
        let ready = strings_of(1, &[TEMPLATE]);
        let bare = PvpNameStrings::default();
        // Human (race 1 → team 1), rank 7.
        let (net, human) = unit(EntityKind::Player, &[HUMAN_MALE, RANK_7]);
        let key = title_key(&cfg, &ready, &net, Some(&human)).unwrap();
        assert_eq!(
            (key.title.rank, key.title.team, key.title.female, key.on),
            (7, 1, false, true)
        );
        // Orc (race 2 → team 0), female; the CVar off, and the strings unloaded, each drop the bit.
        let (_, orc) = unit(EntityKind::Player, &[(36, 0x010102), RANK_7]);
        let orc = title_key(&cfg, &ready, &net, Some(&orc)).unwrap();
        assert_eq!((orc.title.team, orc.title.female), (0, true));
        let off = NameConfig {
            player_pvp_title: false,
            ..cfg
        };
        assert!(!title_key(&off, &ready, &net, Some(&human)).unwrap().on);
        assert!(!title_key(&cfg, &bare, &net, Some(&human)).unwrap().on);
        let (creature, _) = unit(EntityKind::Unit, &[]);
        assert!(
            title_key(&cfg, &ready, &creature, Some(&human)).is_none(),
            "a creature has no a4"
        );
    }

    /// The composed main line, from the descriptor up: `0x608f50` glues the a1-a3 tags onto
    /// `0x609370`'s a4 with no separator; the CVar off, rank 0 or a creature keep the plain name.
    #[test]
    fn the_name_line_glues_the_flag_tags_onto_the_ranked_name() {
        let strings = strings_of(
            1,
            &[TEMPLATE, SERGEANT, ("PVP_RANK_7_1_FEMALE", "Sergeant (f)")],
        );
        let line = |cfg: &NameConfig, (net, store): &(NetEntity, ObjectStore), flags, name| {
            name_line(
                flags,
                title_key(cfg, &strings, net, Some(store)),
                &strings,
                name,
            )
        };
        const AFK: u32 = 0x2;
        let cfg = NameConfig::default();
        let sergeant = unit(EntityKind::Player, &[HUMAN_MALE, RANK_7]);
        assert_eq!(line(&cfg, &sergeant, AFK, "Bob"), "<AFK>Sergeant Bob");
        let off = NameConfig {
            player_pvp_title: false,
            ..cfg
        };
        assert_eq!(
            line(&off, &sergeant, AFK, "Bob"),
            "<AFK>Bob",
            "the CVar off"
        );
        let unranked = unit(EntityKind::Player, &[HUMAN_MALE]);
        assert_eq!(line(&cfg, &unranked, 0, "Bob"), "Bob", "rank 0");
        // The same bytes on a creature: the kind gates the leg, not the descriptor.
        let wolf = unit(EntityKind::Unit, &[HUMAN_MALE, RANK_7]);
        assert_eq!(line(&cfg, &wolf, 0, "Young Wolf"), "Young Wolf");
        // `UNIT_FIELD_BYTES_0` byte 2 = 1: the `_FEMALE` twin.
        let female = unit(EntityKind::Player, &[(36, 0x01_0101), RANK_7]);
        assert_eq!(line(&cfg, &female, 0, "Alice"), "Sergeant (f) Alice");
    }

    /// The decoration is `0x609370`'s: title first, `None` off rank 0 or a key the install lacks.
    #[test]
    fn the_snapshot_decorates_a_ranked_name() {
        let strings = strings_of(1, &[TEMPLATE, SERGEANT]);
        let key = TitleKey {
            title: benilla_ui::script::RankTitle {
                rank: 7,
                team: 1,
                female: false,
            },
            on: true,
            strings: strings.generation,
        };
        assert_eq!(
            strings.decorated(key, "Bob").as_deref(),
            Some("Sergeant Bob"),
            "the rank rides in front of the name"
        );
        assert_eq!(
            strings.decorated(
                TitleKey {
                    title: benilla_ui::script::RankTitle {
                        rank: 0,
                        ..key.title
                    },
                    ..key
                },
                "Bob"
            ),
            None,
            "unranked: no a4 line"
        );
        assert_eq!(
            strings.decorated(
                TitleKey {
                    title: benilla_ui::script::RankTitle {
                        team: 0,
                        ..key.title
                    },
                    ..key
                },
                "Bob"
            ),
            None,
            "the Horde key is not the Alliance one"
        );
    }

    /// A VM's strings move the key the plate cache compares only when they differ: the same
    /// strings from a new VM rebuild nothing, and a renamed title reaches the line.
    #[test]
    fn a_changed_string_set_moves_the_key_and_reaches_the_line() {
        let (net, store) = unit(EntityKind::Player, &[HUMAN_MALE, RANK_7]);
        let cfg = NameConfig::default();
        let built = |strings: &PvpNameStrings| {
            let key = title_key(&cfg, strings, &net, Some(&store));
            (key, name_line(0, key, strings, "Bob"))
        };
        let mut strings = strings_of(1, &[TEMPLATE, SERGEANT]);
        let (boot_key, boot_line) = built(&strings);
        assert_eq!(boot_line, "Sergeant Bob");
        strings.set(2, set_of(&[TEMPLATE, SERGEANT]));
        assert!(built(&strings).0 == boot_key, "a new VM, the same strings");
        strings.set(3, set_of(&[TEMPLATE, ("PVP_RANK_7_1", "Feldwebel")]));
        let (key, line) = built(&strings);
        assert_eq!(line, "Feldwebel Bob", "the new VM's title reaches the line");
        assert!(
            key != boot_key,
            "the cached key moves, so the live plate rebuilds"
        );
    }

    /// A parked plate's second placement leaves both change ticks alone; a moved unit still writes.
    #[test]
    fn a_parked_plates_seat_takes_no_write() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        world.init_resource::<Time>();
        world.insert_resource(NameAnchorTrace(false));
        world.spawn((Transform::default(), WorldCamera));
        let unit = world.spawn(Transform::from_xyz(3.0, -1.0, 5.0)).id();
        let plate = world
            .spawn((Transform::default(), GlobalTransform::default(), NamePlate))
            .id();
        let mut plates = Nameplates::default();
        plates.live.insert(
            unit,
            LiveName {
                plate,
                lines: vec!["Bob".to_string()],
                paint: NamePaint::Flash,
                flags: 0,
                name: "Bob".into(),
                bracketed: None,
                key: None,
            },
        );
        world.insert_resource(plates);

        // First seat: the anchor fallback (no model) is the unit's own translation.
        world.run_system_once(place_nameplates).unwrap();
        assert_eq!(
            world.get::<Transform>(plate).unwrap().translation,
            Vec3::new(3.0, -1.0, 5.0),
        );

        // A parked frame: bit-identical seat, so neither transform's tick may move.
        world.clear_trackers();
        world.run_system_once(place_nameplates).unwrap();
        let e = world.entity(plate);
        assert!(
            !e.get_ref::<Transform>().unwrap().is_changed(),
            "an unchanged seat must not re-mark the plate's transform"
        );
        assert!(
            !e.get_ref::<GlobalTransform>().unwrap().is_changed(),
            "…nor its global"
        );

        // A real move still lands, and marks.
        world.get_mut::<Transform>(unit).unwrap().translation.x += 2.0;
        world.clear_trackers();
        world.run_system_once(place_nameplates).unwrap();
        let e = world.entity(plate);
        assert_eq!(
            e.get::<Transform>().unwrap().translation,
            Vec3::new(5.0, -1.0, 5.0)
        );
        assert!(e.get_ref::<Transform>().unwrap().is_changed());
        assert!(e.get_ref::<GlobalTransform>().unwrap().is_changed());
    }

    /// A populated cache, as a re-bake finds it: one live plate, its mesh, its material.
    fn primed(generation: Option<u64>, unit: Entity, plate: Entity) -> Nameplates {
        let lines = vec!["Young Wolf".to_string()];
        let mut plates = Nameplates {
            baked_from: generation,
            ..Default::default()
        };
        plates.meshes.insert(lines.clone(), Handle::default());
        plates.materials.insert(NamePaint::Flash, Handle::default());
        plates.live.insert(
            unit,
            LiveName {
                plate,
                lines,
                paint: NamePaint::Flash,
                flags: 0,
                name: "Young Wolf".into(),
                bracketed: None,
                key: None,
            },
        );
        plates
    }

    /// The live plates hold the old `Mesh3d`, so a reset must despawn them, not only clear the map.
    #[test]
    fn a_sheet_reset_drops_the_meshes_and_the_live_plates_but_not_the_materials() {
        let mut world = World::new();
        let unit = world.spawn_empty().id();
        let plate = world.spawn_empty().id();
        let mut plates = primed(Some(7), unit, plate);

        let mut queue = bevy::ecs::world::CommandQueue::default();
        {
            let mut commands = Commands::new(&mut queue, &world);
            drop_stale_glyph_caches(&mut plates, 8, &mut commands);
        }
        queue.apply(&mut world);

        assert!(plates.meshes.is_empty(), "stale UVs must not survive");
        assert_eq!(
            plates.materials.len(),
            1,
            "the material binds a texture whose handle never moves — dropping it is pure churn"
        );
        assert!(plates.live.is_empty());
        assert!(
            world.get_entity(plate).is_err(),
            "a live plate holds the old mesh handle: clearing the map around it leaves the wrong \
             cells on screen"
        );
        assert_eq!(plates.baked_from, Some(8));
    }

    /// A reset on the first build would despawn the plates the frame they spawn.
    #[test]
    fn the_first_build_seeds_without_dropping_anything() {
        let mut world = World::new();
        let unit = world.spawn_empty().id();
        let plate = world.spawn_empty().id();
        let mut plates = primed(None, unit, plate);

        let mut queue = bevy::ecs::world::CommandQueue::default();
        {
            let mut commands = Commands::new(&mut queue, &world);
            drop_stale_glyph_caches(&mut plates, 0, &mut commands);
        }
        queue.apply(&mut world);

        assert_eq!(plates.meshes.len(), 1);
        assert_eq!(plates.live.len(), 1);
        assert!(world.get_entity(plate).is_ok());
        assert_eq!(plates.baked_from, Some(0));
    }

    #[test]
    fn an_unmoved_generation_is_a_no_op() {
        let mut world = World::new();
        let unit = world.spawn_empty().id();
        let plate = world.spawn_empty().id();
        let mut plates = primed(Some(3), unit, plate);

        let mut queue = bevy::ecs::world::CommandQueue::default();
        {
            let mut commands = Commands::new(&mut queue, &world);
            drop_stale_glyph_caches(&mut plates, 3, &mut commands);
        }
        queue.apply(&mut world);

        assert_eq!(plates.meshes.len(), 1);
        assert_eq!(plates.materials.len(), 1);
        assert_eq!(plates.live.len(), 1);
        assert!(world.get_entity(plate).is_ok());
    }
}
