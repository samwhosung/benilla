//! `ItemDisplayInfo.dbc` adapter — displayId → held-item / worn-equipment visual identity (decision
//! 0072 slice 1: combat presence, held items via the display chain).
//!
//! Layout — build 5875 (`decisions/0072`: our raw dump `record_count=29604`; the client's
//! loader `0x547590` asserts the field count and record size): 23 fields / 92-byte records.
//! Columns: `0` id (key) · `1`/`2` model name L/R (string) ·
//! `3`/`4` model texture L/R (string) · `5` icon (string, unread — no consumer yet) · `6`-`8`
//! geosetGroup[0..2] (u32) · `9` **flags** (u32 — the client's `ItemDisplayInfo_inmem[+0x24]`, of
//! which only **bit 0** is read: the guild-emblem tabard, [`ItemDisplay::takes_guild_emblem`]) ·
//! `10` the ranged-weapon `SpellVisual.dbc`
//! id (u32 — the substitute visual a RANGED-attribute spell with no own visual borrows for its
//! fire animation; `0x60d493: mov eax,[eax+0x28]`) · `11` the `ItemGroupSounds.dbc` id (u32 — the
//! pickup/place sound group; `0x458008: mov eax,[edx+0x2c]`, 20513/20513 valid ids on the
//! real DBC) · `12`/`13` helm-vis (u32) · `14`-`21` the 8 body-region textures, in **ArmUpper,
//! ArmLower, Hand, TorsoUpper, TorsoLower, LegUpper, LegLower, Foot** order (string) · `22` the
//! **`ItemVisuals.dbc` id** — the item's intrinsic glow (i32; byte-verified `0x47a200: mov
//! edx,[esi+0x58]`, the held-item attach handing it on — decision 0805,
//! [`crate::ItemVisualCatalog`]).
//!
//! **Trap, load-bearing:** `model` and `model_texture` are **independently-resolved basenames** in
//! the same `Item\ObjectComponents\<dir>\` folder — never derive one from the other. Real
//! counter-example: the Worn Wooden Shield row carries model `Shield_Round_A_01.mdx` (→ `.m2` on
//! disk) alongside texture `Buckler_Damaged_A_01Purple` (no relation to the model's own name).

use std::collections::HashMap;

use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, RecordSet, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};
use crate::models::model_path;

const ITEM_DISPLAY_INFO: &str = "DBFilesClient\\ItemDisplayInfo.dbc";

/// One `ItemDisplayInfo` row: the visual identity a held item or worn equipment display resolves to.
///
/// `model`/`model_texture` carry **no directory prefix** — the app layer owns the
/// `Item\ObjectComponents\<dir>\` join, since `<dir>` depends on the item's inventory type (Weapon,
/// Shield, Head, …), which isn't a column in this DBC.
#[derive(Debug, Clone, Default)]
pub struct ItemDisplay {
    /// Left/right model basename (index 0/1), the existing `.mdx`→`.m2` normalizer applied
    /// (lowercased, extension swapped) — see [`crate::models::model_path`]. No directory. `None` for
    /// an empty column (most rows carry only the left slot; two-handed/dual visuals use both).
    pub model: [Option<String>; 2],
    /// Left/right model-texture basename (index 0/1) — raw, **no extension** (the app appends
    /// `.blp`) and **not** derived from [`Self::model`] (see the module trap note).
    pub model_texture: [Option<String>; 2],
    /// `geosetGroup[0..2]` — worn-equipment geoset selectors (robes etc.); `0` where unauthored.
    /// Unused by held items.
    pub geoset_groups: [u32; 3],
    /// The 8 body-region texture overrides this display paints on, in ArmUpper/ArmLower/Hand/
    /// TorsoUpper/TorsoLower/LegUpper/LegLower/Foot order. `None` per-slot where this display
    /// doesn't touch that region (most held-item rows: all 8 empty).
    pub region_textures: [Option<String>; 8],
    /// A helm's `HelmetGeosetVisData` row ids — `[male, female]` (cols 12/13; `0x4799a0` reads
    /// `ItemDisplayInfo_inmem[+0x30 + sex*4]`). `0` = no vis row (non-helm displays).
    pub helmet_vis: [u32; 2],
    /// The inventory icon (col 5) as a ready `Interface\Icons\…` MPQ path, extensionless as the
    /// DBC stores names (the BLP loader appends it). Unlike the model columns there is no
    /// variable directory to join — the icon dir is fixed — so this one ships app-ready, same as
    /// `SpellCatalog`'s icons. `None` on the ~26% of rows that are icon-less visual attachments.
    /// Anchored on live server-truth pairings (vmangos `item_template.display_id`, the values the
    /// real wire answers): Worn Shortsword 1542 → `INV_Sword_04`, Tough Jerky 2473 →
    /// `INV_Misc_Food_16`, Worn Wooden Shield 18730 → `INV_Shield_09`, Hearthstone 6418 →
    /// `INV_Misc_Rune_01`.
    pub icon: Option<String>,
    /// The `ItemGroupSounds.dbc` id (col 11) — the item's pickup/place/use sound group
    /// ([`crate::item_sounds`]). `0` = no group (the display drags silently), matching the client's
    /// bounds-check-and-return (`0x45800b`).
    pub group_sounds: u32,
    /// The ranged-weapon `SpellVisual.dbc` id (col 10) — the SUBSTITUTE visual a RANGED-attribute
    /// spell with no own visual borrows from the equipped ranged weapon (the client's `0x60d450`
    /// fallback: how Throw/Auto Shot get their fire clips).
    /// `0` on every non-ranged display; only three distinct nonzero ids exist across the real
    /// table (thrown 98 · bow 5 · gun/rifle 224).
    pub spell_visual: u32,
    /// The flags word (col 9) — the client's `ItemDisplayInfo_inmem[+0x24]`. Only **bit 0** has a
    /// reader in the image, and it means *this garment takes the wearer's guild emblem*
    /// ([`Self::takes_guild_emblem`]); bit 1 appears on four chest/robe rows with no reader found.
    /// Carried whole rather than as a bool so the unread bits stay visible.
    pub flags: u32,
    /// The `ItemVisuals.dbc` id (col 22) — the display's **intrinsic glow**: the permanent weapon
    /// glows, resolved to up to five `Spells\Enchantments\*.mdx` models by
    /// [`crate::ItemVisualCatalog`]. **Signed**, because the client reads it that
    /// way (`0x4798c0`'s `jle` gate): `0` = none on 29 239 of the 29 604 rows, and **five shipped
    /// rows carry `-1`**, which is also none.
    pub item_visual: i32,
}

impl ItemDisplay {
    /// The `HelmetGeosetVisData` row pair (`[male, female]`) this display hides hair / facial hair /
    /// ears with — **only when the display is actually a worn helm**, i.e. it names a head model.
    ///
    /// The gate is the point. `helmet_vis` is authored on 1314 of the 29604 shipped rows, and **12
    /// of those name no model at all** — jewellery-shaped rows (15676 is an
    /// `INV_Jewelry_Amulet_01` icon with no mesh) that still carry a full hide mask. Nothing wears
    /// them: with no `ModelName` there is no helm to attach and no helm to tuck hair under. But
    /// `CreatureDisplayInfoExtra`'s head column points **126 character-model NPC displays** at
    /// exactly those rows, and honouring the mask there strips the NPC's hairstyle to the bare
    /// scalp (geoset 1), its ears to the tucked variant (701) and its earrings/beard to their
    /// group bases — while 1.12.1 renders them in full (Jubie Gadgetspring, display 7969 →
    /// extra 5503 → head display **15676** → vis row **306** = `[446,478,510,222,238]`, every
    /// column with the gnome bit `1<<7` set).
    ///
    /// **The gate is `ModelName[0]` alone** — on-disk column 1, the LEFT slot (`0x4799c1`): the
    /// head-slot handler `0x4799a0` loads
    /// `[[cc+0x4a8] + 4]` and `cmp byte ptr [ecx],0`, a **string-emptiness** test, jumping straight
    /// to the epilogue `0x479b33` past the whole geoset-vis tail. Column 2 (the right slot) is
    /// never consulted, and neither is the helm M2's load result — `0x4798c0`'s return is clobbered
    /// untested, so this is decided synchronously off the DBC row, never off whether the model
    /// resolved. On the shipped table the two readings coincide (41 rows fill only the right slot;
    /// **none** of them carries a vis pair), which the test asserts rather than assumes.
    pub fn worn_helm_vis(&self) -> Option<[u32; 2]> {
        self.model[0].is_some().then_some(self.helmet_vis)
    }

    /// Whether this display is a **guild-emblem tabard** — the garment whose torso art the wearer's
    /// guild tabard replaces (`flags & 1`, the client's `0x472ca4 test byte ptr [rec+0x24],1` on the
    /// tabard equip slot). A tabard that clears the bit keeps its own `TorsoUpper`/`TorsoLower`
    /// columns; one that sets it has those columns overwritten by the three
    /// `Textures\GuildEmblems\` layers ([`crate::GuildEmblem`]) whenever the wearer's guild
    /// identity is known.
    ///
    /// **Eleven** of the 29604 shipped rows carry a nonzero `flags`; **seven** set bit 0, and they
    /// are exactly the tabard-shaped ones (Crusader ×2, Default ×2, Stromgard, Hillsbrad, and
    /// **20621** — the row item 5976 *Guild Tabard* wears, the only one any 1.12.1 item template
    /// points at). The other four set bit **1** on chest/robe rows (Samurai plate, the Horde robe
    /// family) and nothing reads it.
    pub fn takes_guild_emblem(&self) -> bool {
        self.flags & FLAG_GUILD_EMBLEM_TABARD != 0
    }
}

/// [`ItemDisplay::flags`] bit 0 — see [`ItemDisplay::takes_guild_emblem`].
const FLAG_GUILD_EMBLEM_TABARD: u32 = 0x1;

/// `ItemDisplayInfo.dbc`, keyed by `displayId` (the id `ItemDisplayInfoID`/`UNIT_VIRTUAL_ITEM_SLOT_DISPLAY`
/// resolve into).
pub struct ItemDisplayCatalog {
    displays: HashMap<u32, ItemDisplay>,
}

impl ItemDisplayCatalog {
    /// Build a catalog from an explicit row map — for tests and synthetic fixtures, the twin of
    /// `SpellCatalog::from_displays`. The live path is [`load_item_display_catalog`].
    pub fn from_displays(displays: HashMap<u32, ItemDisplay>) -> Self {
        ItemDisplayCatalog { displays }
    }

    /// Look up a display id, or `None` if unknown.
    pub fn get(&self, display_id: u32) -> Option<&ItemDisplay> {
        self.displays.get(&display_id)
    }

    pub fn len(&self) -> usize {
        self.displays.len()
    }

    pub fn is_empty(&self) -> bool {
        self.displays.is_empty()
    }

    /// Iterate `(displayId, row)` (order unspecified) — the cross-DBC join checks use it, and so
    /// does the corpus sweep that has to name *which* display reaches a model
    /// (`benilla-extract entityuvscan`). The id is carried because a census that cannot join back
    /// to a row id answers "how many" and never "which one".
    pub fn iter(&self) -> impl Iterator<Item = (u32, &ItemDisplay)> {
        self.displays.iter().map(|(&id, d)| (id, d))
    }
}

/// `ItemDisplayInfo.dbc` — 23 fields / 92-byte records in build 5875 (see the module doc for the
/// column pins). Unpinned integer columns are still typed `UInt32` (matching the file's own record
/// stride) even though nothing reads them, so the schema's field-count check against the real header
/// stays exact.
pub(crate) fn item_display_info_schema() -> Schema {
    let mut s = Schema::new("ItemDisplayInfo");
    for (name, ty) in [
        ("ID", FieldType::UInt32),
        ("ModelNameLeft", FieldType::String),
        ("ModelNameRight", FieldType::String),
        ("ModelTextureLeft", FieldType::String),
        ("ModelTextureRight", FieldType::String),
        ("Icon", FieldType::String),
        ("GeosetGroup0", FieldType::UInt32),
        ("GeosetGroup1", FieldType::UInt32),
        ("GeosetGroup2", FieldType::UInt32),
    ] {
        s.add_field(SchemaField::new(name, ty));
    }
    // fields 9..14 (positional): 9 the flags word, 10 the ranged-weapon SpellVisual id, 11 the
    // ItemGroupSounds id, 12/13 helm-vis.
    for name in [
        "Flags",
        "SpellVisualID",
        "ItemGroupSoundsID",
        "HelmVisMale",
        "HelmVisFemale",
    ] {
        s.add_field(SchemaField::new(name, FieldType::UInt32));
    }
    for name in [
        "ArmUpperTexture",
        "ArmLowerTexture",
        "HandTexture",
        "TorsoUpperTexture",
        "TorsoLowerTexture",
        "LegUpperTexture",
        "LegLowerTexture",
        "FootTexture",
    ] {
        s.add_field(SchemaField::new(name, FieldType::String));
    }
    s.add_field(SchemaField::new("ItemVisualID", FieldType::UInt32));
    s
}

/// Build the catalog from an already-parsed record set — the testable core (no `Chain` needed);
/// [`load_item_display_catalog`] is the chain-reading wrapper.
fn catalog_from_records(rs: RecordSet) -> ItemDisplayCatalog {
    let mut displays = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        let model = [
            str_at(&rs, r, 1).map(|s| model_path(&s)),
            str_at(&rs, r, 2).map(|s| model_path(&s)),
        ];
        let model_texture = [str_at(&rs, r, 3), str_at(&rs, r, 4)];
        let geoset_groups = [
            u32_at(r, 6).unwrap_or(0),
            u32_at(r, 7).unwrap_or(0),
            u32_at(r, 8).unwrap_or(0),
        ];
        let region_textures = std::array::from_fn(|i| str_at(&rs, r, 14 + i));
        let helmet_vis = [u32_at(r, 12).unwrap_or(0), u32_at(r, 13).unwrap_or(0)];
        let icon = str_at(&rs, r, 5).map(|i| format!("Interface\\Icons\\{i}"));
        let group_sounds = u32_at(r, 11).unwrap_or(0);
        let spell_visual = u32_at(r, 10).unwrap_or(0);
        let flags = u32_at(r, 9).unwrap_or(0);
        let item_visual = u32_at(r, 22).unwrap_or(0) as i32;
        displays.insert(
            id,
            ItemDisplay {
                model,
                model_texture,
                geoset_groups,
                region_textures,
                helmet_vis,
                icon,
                group_sounds,
                spell_visual,
                flags,
                item_visual,
            },
        );
    }
    ItemDisplayCatalog { displays }
}

/// Load `ItemDisplayInfo.dbc` off the patch chain into an [`ItemDisplayCatalog`].
pub fn load_item_display_catalog(chain: &mut Chain) -> Result<ItemDisplayCatalog> {
    let bytes = chain
        .read_file(ITEM_DISPLAY_INFO)
        .with_context(|| format!("reading {ITEM_DISPLAY_INFO}"))?;
    let rs = parse(&bytes, item_display_info_schema(), "ItemDisplayInfo")?;
    Ok(catalog_from_records(rs))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal synthetic WDBC (20-byte header + fixed-width records + a string block) — the same
    /// shape `benilla-dbc`'s own tests build, reproduced here so this adapter is testable without a
    /// real client install.
    fn build_wdbc(
        record_count: u32,
        field_count: u32,
        record_size: u32,
        records: &[u8],
        strings: &[u8],
    ) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(b"WDBC");
        b.extend_from_slice(&record_count.to_le_bytes());
        b.extend_from_slice(&field_count.to_le_bytes());
        b.extend_from_slice(&record_size.to_le_bytes());
        b.extend_from_slice(&(strings.len() as u32).to_le_bytes());
        b.extend_from_slice(records);
        b.extend_from_slice(strings);
        b
    }

    const FIELD_COUNT: u32 = 23;
    const RECORD_SIZE: u32 = FIELD_COUNT * 4;
    const OFF_EMPTY: u32 = 0;

    fn u32le(v: u32) -> [u8; 4] {
        v.to_le_bytes()
    }

    /// A growable string block: offset 0 is always `""` (an absent column resolves there); each
    /// `push` appends a NUL-terminated string and returns its offset.
    #[derive(Default)]
    struct StringBlock(Vec<u8>);
    impl StringBlock {
        fn new() -> Self {
            Self(vec![0u8]) // offset 0 = ""
        }
        fn push(&mut self, s: &str) -> u32 {
            let off = self.0.len() as u32;
            self.0.extend_from_slice(s.as_bytes());
            self.0.push(0);
            off
        }
    }

    /// The ammo-display **shape rule** on the real build-5875 rows (decision 0099 phase 5): a
    /// projectile display carries its flight model in the **right** slot only (`Ammo\` dir) —
    /// verified over every display a class-6 projectile item resolves to (17 rows, vmangos
    /// `item_template`) — while a thrown weapon fills the **left** slot (`Weapon\` dir; the weapon
    /// itself flies). The missile spawner keys its dir choice on that shape; the client keys the
    /// same fork on the wire ammo block's InventoryType (`0x19`=THROWN, `0x60ba30`) — identical
    /// output on every real row. Skips without client data.
    #[test]
    fn real_ammo_displays_carry_flight_models_right_thrown_left() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_item_display_catalog(&mut chain).expect("load ItemDisplayInfo");

        // Rough Arrow's display: flight model + its object skin, right slot, left empty.
        let arrow = cat.get(5996).expect("arrow display");
        assert_eq!(arrow.model, [None, Some("arrowflight_01.m2".into())]);
        assert_eq!(arrow.model_texture[1].as_deref(), Some("Arrow_A_01Brown"));
        // Light Shot's display: the bullet pair.
        let shot = cat.get(5998).expect("bullet display");
        assert_eq!(shot.model[1].as_deref(), Some("bulletflight_01.m2"));

        // Balanced Throwing Dagger's display: the weapon model in the LEFT slot (it lives in
        // `Item\ObjectComponents\Weapon\`, verified against the MPQ listing).
        let thrown = cat.get(16752).expect("thrown display");
        assert_eq!(thrown.model[0].as_deref(), Some("thrown_1h_dagger_a_01.m2"));
        assert_eq!(
            thrown.model_texture[0].as_deref(),
            Some("Thrown_1H_Dagger_A_01Copper")
        );
    }

    #[test]
    fn parses_model_and_texture_independently_and_skips_empty_columns() {
        let mut strings = StringBlock::new();
        let off_model = strings.push("Shield_Round_A_01.mdx");
        let off_texture = strings.push("Buckler_Damaged_A_01Purple");

        let mut rec = Vec::with_capacity(RECORD_SIZE as usize);
        rec.extend(u32le(18730)); // ID — the Worn Wooden Shield's real displayId
        rec.extend(u32le(off_model)); // ModelNameLeft
        rec.extend(u32le(OFF_EMPTY)); // ModelNameRight — absent
        rec.extend(u32le(off_texture)); // ModelTextureLeft
        rec.extend(u32le(OFF_EMPTY)); // ModelTextureRight — absent
        rec.extend(u32le(OFF_EMPTY)); // Icon — unread
        rec.extend(u32le(1)); // GeosetGroup0
        rec.extend(u32le(0)); // GeosetGroup1
        rec.extend(u32le(0)); // GeosetGroup2
        rec.extend(u32le(0)); // Unk9
        rec.extend(u32le(0)); // Unk10
        rec.extend(u32le(21)); // ItemGroupSoundsID
        rec.extend(u32le(0)); // HelmVisMale
        rec.extend(u32le(0)); // HelmVisFemale
        for _ in 0..8 {
            rec.extend(u32le(OFF_EMPTY)); // region textures — this row paints none
        }
        rec.extend(u32le(0)); // ItemVisualID
        assert_eq!(rec.len(), RECORD_SIZE as usize);

        let bytes = build_wdbc(1, FIELD_COUNT, RECORD_SIZE, &rec, &strings.0);
        let rs = parse(&bytes, item_display_info_schema(), "test").expect("synthetic DBC parses");
        let catalog = catalog_from_records(rs);

        assert_eq!(catalog.len(), 1);
        let display = catalog.get(18730).expect("displayId 18730 present");

        // The model normalizer applied (lowercased, `.mdx` → `.m2`) — reused, not duplicated.
        assert_eq!(
            display.model[0].as_deref(),
            Some("shield_round_a_01.m2"),
            "model name normalized like any other model path"
        );
        assert_eq!(display.model[1], None, "empty ModelNameRight column ⇒ None");

        // The texture is an independently-resolved basename — NOT derived from the model name (the
        // module-doc trap this test guards).
        assert_eq!(
            display.model_texture[0].as_deref(),
            Some("Buckler_Damaged_A_01Purple"),
            "model texture is unrelated to the model's own basename"
        );
        assert_eq!(display.model_texture[1], None);

        assert_eq!(display.geoset_groups, [1, 0, 0]);
        assert!(
            display.region_textures.iter().all(Option::is_none),
            "a held-item row paints no body regions"
        );
        assert_eq!(
            display.group_sounds, 21,
            "field 11 is the ItemGroupSounds id"
        );
    }

    #[test]
    fn unknown_display_id_misses() {
        let rec = vec![0u8; RECORD_SIZE as usize]; // ID 0, everything else empty/zero
        let bytes = build_wdbc(1, FIELD_COUNT, RECORD_SIZE, &rec, &StringBlock::new().0);
        let rs = parse(&bytes, item_display_info_schema(), "test").expect("synthetic DBC parses");
        let catalog = catalog_from_records(rs);
        assert_eq!(catalog.len(), 1);
        assert!(catalog.get(999).is_none());
    }
}
