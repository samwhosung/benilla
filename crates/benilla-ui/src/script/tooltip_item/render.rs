//! The item tooltip's line law — [`render_view`], the byte-verified emission order of the
//! client's shared renderer `0x52b650` (see the parent module doc for the law's provenance and
//! the compare/red/SET summaries).
//!
//! **Every sentence here is a key, never a Rust string**. The builder names its
//! keys outright — the census of the string pointers reachable from `[0x52b650, 0x52e600)` is
//! where each one below came from, so a line's key is the *reference's* choice and not a match on
//! English. That distinction is load-bearing: `ITEM_REQ_SKILL`, `LOCKED_WITH_ITEM`,
//! `LOCKED_WITH_SPELL` and `LOCKED_WITH_SPELL_KNOWN` are all "Requires %s" in enUS and four
//! different strings anywhere else.
//!
//! A key the player's `GlobalStrings.lua` does not carry emits **no line at all** — never an
//! invented one. That is `duration_text`'s disposition and the reference's own: its
//! `FrameScript_GetText` hands back the pre-seeded empty string and `AddLine` drops an empty row
//! before it ever counts it.

use mlua::{Lua, Table};

use crate::script::tooltip::{append_line, duration_text, plural_template};
use crate::script::{ItemTemplateView, Model};
use crate::strings::{fill, Arg};

use super::names::*;

/// The REAL instance's contribution to the line law — the fields only a streamed item OBJECT
/// carries. A template/link hover passes `None` at the call site: the ref gates the whole
/// creator + openable/readable tail on the item-object pointer (`0x52e1c7`/`0x52e2e0` — no
/// object, no lines), and keeps the authored full max/max durability.
#[derive(Default)]
pub(super) struct ItemInstance {
    /// The source's own display NAME, when it knows one the template cannot compose — today that
    /// is exactly the **random-suffix roll**: "Chipped Claw" + `ItemRandomProperties[id].suffix`
    /// = "Chipped Claw of the Bear". The reference composes it inside the builder (`0x52b7bf`
    /// keeps the suffix row for the name, and `0x5d8b00` joins it through `ITEM_SUFFIX_TEMPLATE`);
    /// here the app owns every DBC join, so it arrives already joined — the same division the
    /// enchant lines below already use. `None` = the template's own name (`ItemTemplateView::name`).
    pub name: Option<String>,
    /// Live `(current, max)` durability (director-reported: the spirit healer's 25% loss
    /// showed nowhere); `None` = indestructible (max 0) or the create not yet landed.
    pub durability: Option<(u32, u32)>,
    /// The RESOLVED `ITEM_FIELD_CREATOR` name (the app's ask-once name cache — the ref's
    /// `0x55f080` probe). `None` = authorless OR the name query is still in flight: the ref
    /// emits no line either way (`0x52e209`; its resolve callback repaints the tooltip, ours
    /// is the container re-enter loop).
    pub creator: Option<String>,
    /// Instance `ITEM_FIELD_ITEM_TEXT_ID` ≠ 0 (a mail permanent copy): flips the creator line
    /// to WRITTEN_BY (`0x52e223`) and satisfies the READABLE gate (`0x52e348`).
    pub has_text: bool,
    /// Instance `ITEM_FIELD_FLAGS` — the openable lock sub-gate reads UNLOCKED `0x4`
    /// (`0x52e30c`) and the wrapped-gift arm WRAPPED `0x8` (`0x52e31d`).
    pub flags: u32,
    /// `0x5da2c0` — **this instance is runtime-bound**: `ITEM_FIELD_FLAGS & 1`, or a live
    /// enchant slot naming a `SpellItemEnchantment` row that binds. The binding half needs a DBC
    /// join, so the whole predicate arrives app-resolved
    /// ([`crate::script::ContainerSlot::already_bound`]) rather than being re-derived here off
    /// [`Self::enchants`] — that list is a *display* view and drops rows the line law hides.
    /// Drives the Soulbound override; `false` on every template/link source.
    pub already_bound: bool,
    /// The petition this charter names — **line 3**, between the NAME and `ITEM_SIGNABLE`. See
    /// [`crate::script::PetitionSlotView`], which carries the reason its third line is unbuilt.
    pub petition: Option<crate::script::PetitionSlotView>,
    /// The instance's enchant slots, app-resolved and in slot order (`0x52c991`,
    /// decisions 0915/0920) — see [`crate::script::EnchantView`]. Empty on an unenchanted item and
    /// on every template/link source (no instance, nothing enchanted).
    pub enchants: Vec<crate::script::EnchantView>,
    /// May this source emit the ITEM_OPENABLE line? = the reference's `p6 == 0` — "this tooltip
    /// carries **no** caller-supplied instance block" (`[this+0x440]`, tested at `0x52e2e8`: when
    /// it is set, the builder evaluates only READABLE and skips the openable tree entirely).
    ///
    /// **This is not a per-binding constant**, though it looks like one: the old p6 table treated
    /// `SetBagItem` as passing `p6=1` unconditionally, and so never showed the line — which is why
    /// our bag hover had no green line at all, and which the director's screenshot of a clam
    /// falsified. The cause: the old p6 table was enumerated per *binding* from the instance-block
    /// **writers**, so it only ever saw the p6=1 leg. A per-**call-site** census of all 31
    /// `0x52b650` sites finds five callers with two or more legs, and `SetBagItem 0x534620` is one:
    /// `0x534900` p6=1 and `0x53493e` p6=0.
    ///
    /// What selects the leg is the item-cooldown query `0x6e2ed0` at `0x53483a` — p6=1 iff
    /// **enable, start and duration are all non-zero**, i.e. iff the item has a *running
    /// cooldown*. That same block writes `this+0x408 = start + duration − now`, whose sole
    /// consumer is the ITEM_COOLDOWN_TIME line and the builder's `hasCooldown` return. So on the
    /// bag binding the two lines are structurally exclusive: **an item on cooldown shows
    /// "Cooldown remaining", the same item off cooldown shows `<Right Click to Open>`.**
    /// Decision 0896.
    pub openable_source: bool,
    /// **The instance's remaining LIFETIME in milliseconds** — a duration-limited item (a
    /// conjured stone, a holiday gift, a timed quest item) counting down to its own destruction.
    /// `None` = no timer, which is every ordinary item and every template/link source.
    ///
    /// App-resolved from the client-local deadline `SMSG_ITEM_TIME_UPDATE` parks
    /// ([`crate::script::ContainerSlot::duration_ms`]), not from the instance's
    /// `ITEM_FIELD_DURATION` field: vmangos's own writer says the field is not what the client
    /// displays from — *"Though the client has the information in the item's data field, we have
    /// to send SMSG_ITEM_TIME_UPDATE to display the remaining time"* (`Item::SendTimeUpdate`,
    /// `Objects/Item.cpp:1094`) — which is the same split the temporary-enchant countdown one
    /// field up already takes.
    pub duration_ms: Option<u64>,
}

/// The SET block's blank gold spacer — the reference's own literal `0x854b2c`, and it is **not**
/// the empty string. It is a SPACE followed by a newline, and the difference is a whole row.
///
/// Both halves:
/// `AddLine`'s core `0x530270` takes an empty left text with no right text and **bails before it
/// ever increments the line count** (`5302a9: test ebx,ebx; 5302ab: je 0x530378`, the shared exit,
/// which never reaches the `inc [esi+0x31c]` at `0x530372`) — an empty string here is silently
/// dropped, not even a zero-height slot. So the reference cannot spell this line `""`, and it
/// doesn't: `" \n"` is one real line carrying one space. It measures as ONE row, not two, because
/// the width-based stepper `0x5c7470` consumes the trailing break inside the same call that
/// scanned the space (`5c7659: add esi,ecx`) and the kernel's next top-of-loop read hits the NUL.
///
/// benilla lands on the same row count from the same law: the app's `fontstring_lines`
/// trailing-break rule drops the empty segment a plain split would leave, so
/// `" \n"` is one line here too. We wrote `String::new()`, which our own engine calls zero lines
/// of zero height — so the set block's two spacers were invisible where the reference shows two
/// blank rows.
const SET_SPACER: &str = " \n";

/// The builder's two independent render flags, p4 and p5 of `0x52b650` — a struct rather than two
/// adjacent `bool`s because collapsing them into one was decision 2216's bug, and two positional
/// bools at a call site is the same mistake with an extra step.
#[derive(Clone, Copy, Default)]
pub(super) struct BuilderFlags {
    /// **p5 `[arg+0x18]`** — prepend the gray `CURRENTLY_EQUIPPED` line. Set by the two shopping
    /// plates (`SetMerchantCompareItem`, `SetAuctionCompareItem`) and by nothing else in the
    /// image; those two sites pass [`name_only`](Self::name_only) ZERO.
    pub currently_equipped: bool,
    /// **p4 `[arg+0x14]`** — the compact mode the binary itself calls `nameOnly`
    /// (`0x8552dc`: `"Usage: SetInventoryItem(unit, slot [, nameOnly])"`). Reachable from exactly
    /// one binding — `SetInventoryItem`'s optional third argument, a number `> 0` — and from no
    /// stock FrameXML caller at all, but it is live code an addon can ask for (benilla
    /// decision 2224).
    ///
    /// It is *trimmed*, not bare, and the trim is two non-contiguous jumps plus an early return:
    /// the name goes white, the bind/lock region and the whole stat body are cut, and everything
    /// after the cooldown line is skipped. What survives is the name, the slot/type cell,
    /// durability, duration, every requirement line, the spell triggers and the set block.
    pub name_only: bool,
}

/// Render one item template into the tooltip — the emission law of the shared
/// renderer `0x52b650` (the enchant lines are decision 0915's), minus the instance-only families
/// still unfed (soulbound override, cooldown-remaining, the gift-wrap family).
pub(super) fn render_view(
    lua: &Lua,
    this: &Table,
    v: &ItemTemplateView,
    flags: BuilderFlags,
    // `None` = a template/link source (the ref's no-object path).
    inst: Option<&ItemInstance>,
) -> mlua::Result<()> {
    let (req, known_spell, taught_known, set_view, equipped) = {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        let knows = |id: u32| model.spellbook.slots.iter().any(|s| s.spell_id == id);
        let known = v.required_spell != 0 && knows(v.required_spell);
        // A taught spell (trigger 6, SPELL_EFFECT_LEARN) the player already knows — the
        // unconditional-red ITEM_SPELL_KNOWN "Already known" line (recipes).
        let taught = v
            .spell_triggers
            .iter()
            .any(|&(t, id, _)| t == 6 && id != 0 && knows(id));
        // The SET block's inputs: the app-resolved set view (a miss records the ask-once) and
        // the player's equipped item ids — the (owned/total) count + per-member highlight.
        let set_view = (v.item_set != 0)
            .then(|| {
                let view = model.item_sets.get(&v.item_set).cloned();
                if view.is_none() {
                    model.item_set_asks.insert(v.item_set);
                }
                view
            })
            .flatten();
        let equipped: std::collections::HashSet<u32> = model
            .inventory_slots
            .iter()
            .flatten()
            .map(|s| s.item_id)
            .collect();
        (model.player_req.clone(), known, taught, set_view, equipped)
    };
    let add = |l: (String, [f32; 4])| append_line(lua, this, l, None, false);
    let addw = |l: (String, [f32; 4])| append_line(lua, this, l, None, true);
    let add2 =
        |l: (String, [f32; 4]), r: (String, [f32; 4])| append_line(lua, this, l, Some(r), false);
    // The VM's own `GlobalStrings.lua`, off the player's patch chain — the one source of every
    // sentence below.
    let get = |key: &str| crate::strings::global(lua, key);
    // One line, named by KEY and filled from the template the install carries. A key the table
    // does not hold emits nothing: the reference's empty `FrameScript_GetText` return reaches
    // `AddLine`'s `0x530270`, which bails before it increments the line count.
    let keyed = |key: &str, args: &[Arg<'_>], color: [f32; 4], wrap: bool| -> mlua::Result<()> {
        match get(key) {
            Some(t) => append_line(lua, this, (fill(&t, args), color), None, wrap),
            None => Ok(()),
        }
    };

    // The gray CURRENTLY_EQUIPPED header (`[arg+0x18]≠0`, color ptr `0xc0d3c4`) — and it is the
    // ONLY thing a shopping-tooltip fill adds. **Against a plain `SetInventoryItem` of the same
    // equipped item, one extra line, first, is the whole difference**: the two callers that set
    // the header (`SetMerchantCompareItem`'s arg vector at `0x5362d4`, `SetAuctionCompareItem`'s
    // at `0x53603e`) pass **p4 = 0**, so compact/compare mode is OFF — the NAME keeps its quality
    // color, the stat body is not jumped, and nothing is cut at `0x52e14c`. A downstream client
    // that renders a "compare mode" abbreviated tooltip here is wrong — the per-call-site census
    // over all 31 sites of `0x52b650` is what settles it: those two are
    // the only sites in the image passing a literal non-zero p5, and both pass p4 zero.
    if flags.currently_equipped {
        keyed("CURRENTLY_EQUIPPED", &[], GRAY, false)?;
    }
    let name = inst
        .and_then(|i| i.name.clone())
        .unwrap_or_else(|| v.name.clone());
    // The NAME's one recolor: `nameOnly` paints it white (`0x52b8b3`, the `je 0x52b8ca` target
    // being the quality arm `lea ecx,[eax*4+0xc0d3c8]`). Nothing else ever recolors it — an
    // unusable item's name stays quality-colored.
    let name_color = if flags.name_only {
        WHITE
    } else {
        quality_color(v.quality)
    };
    add((name, name_color))?;
    // Line 3 — the petition block, ABOVE the green line and below the name: "Guild Name: X" then
    // "Guild Master: Y" for a charter, "Petition: X" / "Created by Y" for a plain petition. The
    // keys are picked by the record's own charter bit, the same bit `GetPetitionInfo`'s first
    // return reads.
    //
    // Each line is withheld while its source is unresolved rather than printed with a hole: an
    // uncached owner name shows the title alone, and the repaint fills it in — the creator line's
    // rule, and the reference's own (its resolve callback repaints the tooltip; ours is the
    // container re-enter loop).
    if let Some(p) = inst.and_then(|i| i.petition.as_ref()) {
        let (title_key, creator_key) = if p.is_charter {
            ("GUILD_CHARTER_TITLE", "GUILD_CHARTER_CREATOR")
        } else {
            ("PETITION_TITLE", "PETITION_CREATOR")
        };
        if !p.title.is_empty() {
            keyed(title_key, &[Arg::S(&p.title)], WHITE, false)?;
        }
        if let Some(owner) = p.owner.as_deref().filter(|o| !o.is_empty()) {
            keyed(creator_key, &[Arg::S(owner)], WHITE, false)?;
        }
    }
    // ITEM_SIGNABLE (green) — Flags bit 0x2000 (petitions).
    if v.flags & 0x2000 != 0 {
        keyed("ITEM_SIGNABLE", &[], GREEN, false)?;
    }
    // **p4's FIRST cut** (`0x52babe`; `0x52bac3 jne 0x52bfad` skips `[0x52bac9, 0x52bfad)`) — the
    // whole bind/lock region. `ITEM_SIGNABLE` just above is NOT in the jumped range and survives.
    if !flags.name_only {
        // ITEM_CONJURED (Flags bit 0x2).
        if v.flags & 0x2 != 0 {
            keyed("ITEM_CONJURED", &[], WHITE, false)?;
        }
        // The bind line (white, one line). Bonding `[record+0x194]` ∈ {1..5} is what decides
        // whether a line prints at ALL — a Bonding-0 item says nothing here however it is held.
        // Within that, a **runtime-bound instance** (`0x5da2c0` — [`ItemInstance::already_bound`])
        // overrides the whole line to ITEM_SOULBOUND, and to ITEM_BIND_QUEST for the two quest
        // kinds, which is the same text 4|5 print anyway; only then does the jump table `0x52e4fc`
        // pick 1→picked up · 2→equipped · 3→used · 4/5→Quest Item.
        //
        // Before this arm an equipped Binds-when-equipped piece kept saying *Binds when equipped*
        // forever: the template's `bonding` never changes when the item binds —
        // the instance's flag is the only thing that does.
        match v.bonding {
            4 | 5 => keyed("ITEM_BIND_QUEST", &[], WHITE, false)?,
            1..=3 if inst.is_some_and(|i| i.already_bound) => {
                keyed("ITEM_SOULBOUND", &[], WHITE, false)?
            }
            1 => keyed("ITEM_BIND_ON_PICKUP", &[], WHITE, false)?,
            2 => keyed("ITEM_BIND_ON_EQUIP", &[], WHITE, false)?,
            3 => keyed("ITEM_BIND_ON_USE", &[], WHITE, false)?,
            _ => {}
        }
        match v.max_count {
            1 => keyed("ITEM_UNIQUE", &[], WHITE, false)?,
            n if n > 1 => keyed("ITEM_UNIQUE_MULTIPLE", &[Arg::D(n.into())], WHITE, false)?,
            _ => {}
        }
        if v.start_quest != 0 {
            keyed("ITEM_STARTS_QUEST", &[], WHITE, false)?;
        }
        // LOCKED (red) — suppressed once the INSTANCE carries UNLOCKED `0x4` (the law's "and the
        // item is not already unlocked"; the same bit the openable sub-gate reads). The key-item
        // "Requires %s" sub-line joins with the Lock.dbc resolve (the GO-locks follow-up).
        if v.lock_id != 0 && inst.is_none_or(|i| i.flags & 0x4 == 0) {
            keyed("LOCKED", &[], RED, false)?;
        }
    }
    // Slot | type — or, for a bag, the single CONTAINER_SLOTS line in the same seat. The type
    // cell is suppressed for cloaks (InventoryType 16) and displayFlags-hidden subclasses
    // (rings/trinkets/shirts — the "Miscellaneous" family), both builder gates. The two cells
    // recolor independently (byte-read at the builder's `0x52c143..0x52c1f9` legs against the
    // verified `0x530270(this, left, right, leftColor, rightColor, wrap)` signature — NB the cells
    // are swapped): a proficiency-mask miss (`0xc4d4a0[class]` bit
    // `1 << subclass`; our SMSG_SET_PROFICIENCY-fed map; no mask entry never reds) reds the
    // TYPE cell — unless a weapon's alternate subclass (ItemSubClass prereq/postreq) is
    // proficient, which reds the SLOT cell instead. Independently, an off-hand weapon
    // (InventoryType 22) reds the SLOT cell without Dual Wield (`0x5eab70` = the learned
    // effect-40 spell), even when the type itself is proficient.
    // The bag line's gate is `InventoryType == 0x12` **alone** (`0x52b754 sete`, read at
    // `0x52bffe`) — never the slot count, which the gate does not test at all: a 0-slot bag
    // prints "0 Slot Bag". Every shipped container carries 18, quivers and ammo pouches
    // included, which is why `INVTYPE_QUIVER` is a dead slot name in 1.12 (InventoryType 27
    // occurs in none of the 848 records of the reference install's own
    // `itemcache.wdb`, and `0x809200[27]` maps to no equipment slot at all).
    if v.inventory_type == 18 {
        // `%d` is `ContainerSlots`; `%s` is the SAME `ItemSubClass` DisplayName the type cell
        // reads — so the noun is per-subclass ("24 Slot Soul Bag", "8 Slot Quiver", "6 Slot
        // Ammo Pouch"), never the constant "Bag" this line used to print for every container.
        // A container whose row has no display name emits NEITHER this line nor the ordinary
        // slot|type one (`0x52c006`/`0x52c018`/`0x52c021` all jump past the whole region), and
        // displayFlags bit 0 is not consulted on this path.
        if let Some(name) = v.sub_class_display.as_deref() {
            keyed(
                "CONTAINER_SLOTS",
                &[Arg::D(v.container_slots.into()), Arg::S(name)],
                WHITE,
                false,
            )?;
        }
    } else {
        // The slot cell. For **`ItemClass == 6` the InventoryType key table is not consulted at
        // all** (`0x52c0bc cmp dword [esi],6`): the reference reads `ItemClass.dbc` row 6's own
        // localized name — "Projectile" — verbatim out of the store at `[0xc0dc24]`, never
        // through `FrameScript_GetText`. That is the same `[classRow + 4·locale + 0xc]` column
        // `GetItemInfo`'s `itemType` answers with, which the view already carries.
        //
        // Every other class takes the `0x83ddb0` key table for its InventoryType, so the types
        // whose keys `GlobalStrings.lua` does not carry (24 ammo, 25 thrown, 26 ranged-right,
        // 27 quiver) draw no slot cell — but 15 `INVTYPE_RANGED` *does* ship, so a bow reads
        // "Ranged | Bow" while a gun reads "Gun" alone. Decision 2080 removed three invented
        // words here and was right about the KEYS; an arrow's cell is nonetheless not empty,
        // because ammunition never reaches them.
        let slot = if v.class == 6 {
            v.item_type.clone()
        } else {
            invtype_key(v.inventory_type).and_then(&get)
        };
        // The type cell is `ItemSubClass.dbc`'s DisplayName for the pair, app-resolved (the
        // builder's `0xc0db90` row cache read) — never a table in here: 2080 found the same
        // shape one file over, where a hand-typed "Lockpicking" shadowed `LockType.dbc`'s own
        // "Pick Lock".
        let ty = if v.inventory_type == 16 || v.hide_subclass {
            None
        } else {
            v.sub_class_display.as_deref()
        };
        let mut left_red = false;
        let mut right_red = false;
        if let Some(&mask) = req.proficiency.get(&v.class) {
            if mask & (1 << v.subclass) == 0 {
                let alt_ok =
                    v.class == 2 && v.proficiency_alt.is_some_and(|a| mask & (1 << a) != 0);
                if alt_ok {
                    left_red = true;
                } else {
                    right_red = true;
                }
            }
        }
        if v.inventory_type == 22 && !req.can_dual_wield {
            left_red = true;
        }
        match (slot, ty) {
            (Some(s), Some(t)) => add2(
                (s, req_color(!left_red)),
                (t.to_string(), req_color(!right_red)),
            )?,
            (Some(s), None) => add((s, req_color(!left_red)))?,
            // No slot name: the type stands alone and takes the hard-miss color (the
            // builder's single-cell fallback keeps flag-1).
            (None, Some(t)) => add((t.to_string(), req_color(!right_red)))?,
            _ => {}
        }
    }
    // **p4's SECOND cut** (`0x52c220`; `0x52c225 jne 0x52cc5b` skips `[0x52c22b, 0x52cc5b)`) —
    // damage/speed/DPS, armor, block, the stat mods, the resistances and the whole enchant family.
    // The two cuts are not contiguous: the slot/type cell between them is emitted either way.
    if !flags.name_only {
        // **The damage block** — five slots, a five-arm template matrix and a first/PLUS_ flag
        // (`[0x52c22b, 0x52c5a1)`). Every arm is a key, and every hole's shape is the
        // binary's own push list — this block composed its English in Rust until it was converted.
        //
        // The three predicates and the flag: **hasSchool** is the slot's school field being nonzero
        // (school 0 takes the no-school subtree — `SPELL_SCHOOL0_CAP` is never looked up);
        // **isAmmo** is `ItemClass == 6` and *only* that (not InventoryType, not a flag);
        // **isSingle** is `floor(min) == ceil(max)` on the ROUNDED integers, tested only on the
        // no-school non-ammo leaf (there is no `SINGLE_…_WITH_SCHOOL`); and **isFirst** is per-ITEM,
        // cleared on the first EMITTED slot, so a skipped slot does not consume it.
        let mut first = true;
        let mut dps_acc = 0.0f32;
        for &(min, max, school) in v.damages.iter().take(5) {
            // `floor(min)` / `ceil(max)` — NOT a round-half pair, which is what this block used to
            // do on both bounds. The reference biases by `0x808120` (0.9999899864196777, neither 0.5
            // nor 1.0 — the epsilon is what stops an exactly-integral max being bumped) and converts
            // with `__ftol`'s truncate-toward-zero. Fang of the Mystics' 38.7–85.7 reads "38 - 86";
            // rounding to nearest would say "39 - 86" (43 shipped items carry fractional damage).
            let (lo, hi) = (floor_min(min), ceil_max(max));
            // A slot is emitted iff either rounded bound is nonzero (`0x52c292`).
            if lo == 0 && hi == 0 {
                continue;
            }
            // The school WORD is the resolved key; the school NUMBER is what picks the arm. A chain
            // that carries no `SPELL_SCHOOL%d_CAP` still takes the with-school template and fills
            // the hole with the empty string, exactly as `FrameScript_GetText` does.
            let school_name = school_key(school).and_then(|k| get(&k)).unwrap_or_default();
            // `avg` is the two ROUNDED bounds averaged, round-tripped through f32 — and it is NOT
            // divided by anything: `AMMO_DAMAGE_TEMPLATE`'s shipped "Adds %g damage per second" is
            // FrameXML's phrasing over a number the binary never makes per-second (Rough Arrow's
            // 1–2 reads "Adds 1.5 damage per second").
            let avg = f64::from((lo + hi) as f32 * 0.5);
            let plus = |k: &str| {
                if first {
                    k.to_string()
                } else {
                    format!("PLUS_{k}")
                }
            };
            let (key, args): (String, Vec<Arg<'_>>) = if school != 0 {
                if v.class == 6 {
                    (
                        plus("AMMO_SCHOOL_DAMAGE_TEMPLATE"),
                        vec![Arg::F(avg), Arg::S(&school_name)],
                    )
                } else {
                    (
                        plus("DAMAGE_TEMPLATE_WITH_SCHOOL"),
                        vec![Arg::D(lo.into()), Arg::D(hi.into()), Arg::S(&school_name)],
                    )
                }
            } else if v.class == 6 {
                (plus("AMMO_DAMAGE_TEMPLATE"), vec![Arg::F(avg)])
            } else if lo == hi {
                (plus("SINGLE_DAMAGE_TEMPLATE"), vec![Arg::D(lo.into())])
            } else {
                (
                    plus("DAMAGE_TEMPLATE"),
                    vec![Arg::D(lo.into()), Arg::D(hi.into())],
                )
            };
            // The RIGHT cell is the FIRST emitted line's alone, and weapons' alone (`0x52c494`/
            // `0x52c49c` — the only two conjuncts): every later damage line, and every line of a
            // non-weapon, renders left-text-only. `"%s %.2f"` is an `.rdata` LITERAL, not a key —
            // only the word "Speed" is looked up — over `Delay × 0.001`.
            let speed = (first && v.class == 2).then(|| {
                let word = get("SPEED").unwrap_or_default();
                let secs = f64::from(v.delay_ms) * f64::from(0.001_f32);
                format!("{word} {secs:.2}")
            });
            // The damage line is white in BOTH cells unconditionally — it never reddens for an item
            // the player cannot use (`0x52c4fa`–`0x52c516`).
            if let Some(t) = get(&key) {
                let line = (fill(&t, &args), WHITE);
                match speed {
                    Some(s) => add2(line, (s, WHITE))?,
                    None => add(line)?,
                }
            }
            // The DPS accumulator runs on the emit path, and it uses the RAW floats — so the
            // printed range and the printed DPS are computed from different numbers by design.
            dps_acc += (max + min) * 0.5;
            first = false;
        }
        // DPS — two conjuncts: a line was emitted AND `ItemClass == 2`. Ammo can satisfy neither
        // (its arms require class 6), so an arrow gets no DPS line and no Speed cell.
        if !first && v.class == 2 {
            // The precision is DPS_TEMPLATE's own `%.1f`, not ours — a locale that respells it gets
            // its own number of decimals with no code change (`0x854c4c`: "the print precision is
            // FRAMEXML-DATA"). There is no divide-by-zero guard in the reference either.
            let secs = f64::from(v.delay_ms) * f64::from(0.001_f32);
            keyed(
                "DPS_TEMPLATE",
                &[Arg::F(f64::from(dps_acc) / secs)],
                WHITE,
                false,
            )?;
        }
        if v.armor > 0 {
            keyed("ARMOR_TEMPLATE", &[Arg::D(v.armor.into())], WHITE, false)?;
        }
        if v.block > 0 {
            keyed(
                "SHIELD_BLOCK_TEMPLATE",
                &[Arg::D(v.block.into())],
                WHITE,
                false,
            )?;
        }
        // Stat mods (+N Stamina …) in the client's DISPLAY order — the `0x808e88` table (byte-read:
        // 4,3,7,5,6,1,0 then 8,9,2,10 + zero padding; the builder's outer loop walks the table,
        // the inner loop scans the item's raw slots — `0x52c6b0..0x52c801`). So Strength, Agility,
        // Stamina, Intellect, Spirit, Health, Mana — never the wire order. (The table's trailing
        // ZERO entries would re-match a mana slot once per pass — a dormant client quirk nothing
        // shipped can reach: the only mana-stat item in the whole 1.12 DB is the internal "Test MP
        // Ring" 6674. Not emulated.)
        const STAT_DISPLAY_ORDER: [u32; 7] = [4, 3, 7, 5, 6, 1, 0];
        for &want in &STAT_DISPLAY_ORDER {
            for &(t, val) in &v.stats {
                if t != want || val == 0 {
                    continue;
                }
                // The `ITEM_MOD_*` template is the whole line, sign hole included
                // (`"%c%d Agility"`) — the sign is an argument, not a prefix we glue on.
                let Some(key) = stat_key(t) else { continue };
                let sign = if val > 0 { "+" } else { "-" };
                keyed(key, &[Arg::S(sign), Arg::D(val.abs().into())], WHITE, false)?;
            }
        }
        // Resistances: six equal nonzero values collapse to the ALL line; otherwise one line per
        // nonzero school with HOLY excluded from the singles loop (both byte-verified). The six
        // fields are schools 1..6, so slot `i` names `SPELL_SCHOOL{i+1}_CAP` — the same `%d`-composed
        // key the damage line uses one block up.
        let first_res = v.resistances[0];
        if first_res != 0 && v.resistances.iter().all(|&r| r == first_res) {
            let sign = if first_res > 0 { "+" } else { "-" };
            keyed(
                "ITEM_RESIST_ALL",
                &[Arg::S(sign), Arg::D(first_res.abs().into())],
                WHITE,
                false,
            )?;
        } else {
            // **The singles come out in the BUILDER's order, not the field order.** `0x52c8ad–
            // 0x52c93e` runs `edi` 1..5 and reads school `esi = (edi == 1) ? 6 : edi`, so the lines
            // are Arcane, Fire, Nature, Frost, Shadow — and that 1→6 remap is *how* Holy is excluded:
            // it is displaced by Arcane rather than skipped by a test. Read while converting the
            // block's keys and named in decision 2080; we emitted plain field order (Fire first,
            // Arcane last) until it was converted.
            const RESIST_EMIT_ORDER: [u32; 5] = [6, 2, 3, 4, 5];
            for school in RESIST_EMIT_ORDER {
                let r = v.resistances[school as usize - 1];
                if r == 0 {
                    continue;
                }
                let sign = if r > 0 { "+" } else { "-" };
                let school = school_key(school).and_then(|k| get(&k)).unwrap_or_default();
                keyed(
                    "ITEM_RESIST_SINGLE",
                    &[Arg::S(sign), Arg::D(r.abs().into()), Arg::S(&school)],
                    WHITE,
                    false,
                )?;
            }
        }
        // **Line 17 — the enchant family**. One contiguous block
        // `[0x52c991, 0x52cc69)` between the resistances and the durability precompute, and three arms
        // that are mutually exclusive by construction — the per-slot loop falls through to the
        // proposed-enchant pair and jumps the block's end, so RANDOM_ENCHANT is reachable only when
        // there was no id source at all.
        //
        // The **colour is per slot** — the value is a
        // computed local, defaulting to WHITE, overwritten **only for slots 0 and 1** — green
        // `0xc0d3ac` for a positive id, pure-red `0xc0d398` for a negative one. Slots 2..6 — the
        // random-property suffix enchants — are **always white**, whatever the sign. (Our first cut
        // painted every slot green.) The sign never picks a different DBC row; the app already
        // resolved that off `abs(id)`.
        //
        // Two gates sit above the loop. **ITEM_SIGNABLE** (template Flags bit `0x2000`, a petition
        // or guild charter) forces every id to 0 with no fallback (`0x52c9e0: test ah,0x20`) — such
        // an item shows no enchant line even if its instance carries ids. And with **no id source
        // at all** the block instead prints the template-only `ITEM_RANDOM_ENCHANT` placeholder
        // (`0x52cc33`).
        let signable = v.flags & 0x2000 != 0;
        let enchant_slots = match signable {
            true => &[][..],
            false => inst.map(|i| i.enchants.as_slice()).unwrap_or_default(),
        };
        // "No id source" is the reference's own three-way fork (`0x52c991`): a wrapped gift, or no
        // item object AND no caller-supplied instance block (`+0x440 == 0`). Ours reads the same: a
        // hover
        // that passes NO [`ItemInstance`] is a p6=0 leg — the template sources (merchant, quest,
        // craft, buyback, send-mail, the compare legs, `BenillaSetItemById`) — plus the wrapped-gift bit.
        //
        // **A block-supplying source never prints the placeholder, even carrying no ids at all.** The
        // fork tests the block's presence, not its contents, so `SetLootItem`/`SetHyperlink`/
        // `SetInboxItem`/`SetAuctionItem`/`SetLootRollItem`/the trade legs fall into the slot loop and
        // print whatever their slots hold — nothing, when the roll is absent. Decision 0920's prose
        // put a hyperlink hover on the placeholder arm; the `0x52c9a3` fork says otherwise, and
        // that is the drift 1547 corrects (a linked or looted "of the Monkey" showed the placeholder
        // where the reference shows the rolled lines).
        let no_id_source = inst.is_none_or(|i| i.flags & 0x8 != 0);
        if no_id_source && !signable && v.random_property != 0 {
            keyed("ITEM_RANDOM_ENCHANT", &[], GREEN, false)?;
        }
        for e in enchant_slots {
            let color = match (e.slot < 2, e.negative) {
                (true, false) => GREEN,
                (true, true) => ENCHANT_RED,
                (false, _) => WHITE,
            };
            // A TEMPORARY enchant's countdown REPLACES the plain name in the same line and keeps
            // that colour — it is never a second line (`0x52ca49`). The bucket ladder
            // (day/hour/min/sec) and its
            // ceil-vs-truncate split are [`enchant_time_left`]'s; its source is
            // `SMSG_ITEM_ENCHANT_TIME_UPDATE`, never the item's own duration field.
            let mut text = match e.remaining_ms {
                // The countdown IS the line — `ITEM_ENCHANT_TIME_LEFT_MIN = "%s (%d min)"` carries
                // the enchant's own name in its first hole. Without that template there is no line
                // to compose: the reference printf's an empty format into a zeroed buffer and
                // `AddLine` drops the empty row, so a missing key skips the slot rather than falling
                // back to the bare name.
                Some(ms) => match enchant_time_left(&e.name, ms, &get) {
                    Some(t) => t,
                    None => continue,
                },
                None => e.name.clone(),
            };
            // " (N Charges)" — the slot's own charges dword through ITEM_SPELL_CHARGES, then the
            // literal `" (%s)" 0x854820` (`0x52caa6–0x52cb38`), which is the engine's own format and
            // not a string table entry. Only an owned item object carries charges; the session/inspect
            // legs ship ids alone, so this is naturally absent there.
            if let Some(charges) = charges_phrase(e.charges, &get) {
                text.push_str(&format!(" ({charges})"));
            }
            add((text, color))?;
        }
    }
    if let Some((cur, max)) = inst.and_then(|i| i.durability).filter(|&(_, max)| max > 0) {
        // Red iff BROKEN (durability 0) — the byte law ("durability (red iff broken==0)", the
        // AddLine colour pointer `0xc0d390`, the same red as the unmet-requirement lines).
        let color = if cur == 0 { RED } else { WHITE };
        keyed(
            "DURABILITY_TEMPLATE",
            &[Arg::D(cur.into()), Arg::D(max.into())],
            color,
            false,
        )?;
    } else if v.max_durability > 0 {
        let full = i64::from(v.max_durability);
        keyed(
            "DURABILITY_TEMPLATE",
            &[Arg::D(full), Arg::D(full)],
            WHITE,
            false,
        )?;
    }
    // **ITEM_DURATION `0x854bb4` — the item's own expiry countdown** (`0x52ce0d`, one of the four
    // sites that set the builder's `[ebp-0x38]` return). It sits here by that address: after the
    // durability precompute, pinned at `0x52cd0e–0x52cd22`, and well before `ITEM_COOLDOWN_TIME` at
    // `0x52e140` — the reference's own numbered emission order omits the line entirely, which is
    // the gap decision 1933 names.
    // Formatted through the same `0x52fa50` ladder as the enchant countdown, with key prefix
    // `ITEM_DURATION`; unlike its siblings that prefix ships **no `_P1` plural twin**, so every
    // count reads "days"/"hrs" (`GlobalStrings.lua:2401-2404`) — which `plural_template`'s
    // fall-back-to-the-bare-token arm produces without a special case.
    //
    // **This is `tooltip::duration_text` itself**, not a second copy of the ladder: the two had
    // drifted into separate implementations of the same four arms, and the local one always
    // emitted a line (an invented sentence off an install) where the shared one returns `None`.
    // The one seam is the width — the deadline is parked as `u64` milliseconds and the reference's
    // formatter takes a dword, so a timer past 49.7 days saturates rather than wrapping. No 1.12
    // item carries one; the longest shipped duration is a fortnight.
    if let Some(ms) = inst.and_then(|i| i.duration_ms) {
        let ms = u32::try_from(ms).unwrap_or(u32::MAX);
        if let Some(line) = duration_text(ms, "ITEM_DURATION", true, &get) {
            add((line, WHITE))?;
        }
    }
    // Class/race lists — red when the player's own bit is absent (the usable ask).
    if v.allowable_class > 0
        && (v.allowable_class & full_mask(&CLASS_NAMES)) != full_mask(&CLASS_NAMES)
    {
        let list: Vec<&str> = CLASS_NAMES
            .iter()
            .filter(|&&(id, _)| v.allowable_class & (1 << (id - 1)) != 0)
            .map(|&(_, n)| n)
            .collect();
        if !list.is_empty() {
            let ok = req.class_id > 0 && v.allowable_class & (1 << (req.class_id - 1)) != 0;
            keyed(
                "ITEM_CLASSES_ALLOWED",
                &[Arg::S(&list.join(", "))],
                req_color(ok),
                false,
            )?;
        }
    }
    if v.allowable_race > 0 && (v.allowable_race & full_mask(&RACE_NAMES)) != full_mask(&RACE_NAMES)
    {
        let list: Vec<&str> = RACE_NAMES
            .iter()
            .filter(|&&(id, _)| v.allowable_race & (1 << (id - 1)) != 0)
            .map(|&(_, n)| n)
            .collect();
        if !list.is_empty() {
            let ok = req.race_id > 0 && v.allowable_race & (1 << (req.race_id - 1)) != 0;
            keyed(
                "ITEM_RACES_ALLOWED",
                &[Arg::S(&list.join(", "))],
                req_color(ok),
                false,
            )?;
        }
    }
    // ITEM_MIN_LEVEL prints only for RequiredLevel > 1 (byte-VERIFIED `0x52d2cf`: `cmp esi,0x1 /
    // jle skip`) — a level-1 requirement gates nothing a logged-in player could fail, so the
    // real client hides it on all the level-1 consumables that carry one.
    if v.required_level > 1 {
        keyed(
            "ITEM_MIN_LEVEL",
            &[Arg::D(v.required_level.into())],
            req_color(req.level >= v.required_level),
            false,
        )?;
    }
    // The skill requirement is TWO keys, picked by whether a rank is asked for — `ITEM_MIN_SKILL`
    // = "Requires %s (%d)" and `ITEM_REQ_SKILL` = "Requires %s" (the builder's own pair at
    // `0x52d34e`/`0x52d355`). `ITEM_REQ_SKILL` is also what the required-SPELL line below takes
    // (`0x52d650`): three other 1.12 keys read "Requires %s" in enUS — the `LOCKED_WITH_*` trio —
    // and none of them is this line.
    if let Some(skill) = &v.required_skill_name {
        let have = req.skills.get(&v.required_skill).copied().unwrap_or(0);
        let color = req_color(have >= v.required_skill_rank.max(1));
        if v.required_skill_rank > 0 {
            let args = [Arg::S(skill), Arg::D(v.required_skill_rank.into())];
            keyed("ITEM_MIN_SKILL", &args, color, false)?;
        } else {
            keyed("ITEM_REQ_SKILL", &[Arg::S(skill)], color, false)?;
        }
    }
    if v.required_spell != 0 {
        if let Some(name) = &v.required_spell_name {
            keyed(
                "ITEM_REQ_SKILL",
                &[Arg::S(name)],
                req_color(known_spell),
                false,
            )?;
        }
    }
    if let Some(rep) = &v.required_rep_line {
        // Red when the player's rank with the faction is below the requirement (`0xc0d390`;
        // standing `< [+0x58]`); an unfed faction reads as unmet, like the real client's
        // empty store at login (INITIALIZE_FACTIONS lands before the world does).
        let rank = req
            .rep_ranks
            .get(&v.required_rep_faction)
            .copied()
            .unwrap_or(0);
        add((
            rep.clone(),
            req_color(u32::from(rank) >= v.required_rep_rank),
        ))?;
    }
    // ITEM_SPELL_KNOWN — a taught spell the player already knows, UNCONDITIONALLY red.
    if taught_known {
        keyed("ITEM_SPELL_KNOWN", &[], RED, false)?;
    }
    // Green trigger lines, wrapped — the text is app-resolved (the spell's name in P1; its
    // $-substituted description via the token engine in P2). The prefix is a key, joined to the
    // text by the engine's own `"%s %s"` literal `0x82ef08` (`0x52da7e`) — which is why the
    // shipped values ("Use:", "Equip:", "Chance on hit:") carry no trailing space of their own.
    for &(trigger, _, ref text) in &v.spell_triggers {
        let key = match trigger {
            0 | 5 => "ITEM_SPELL_TRIGGER_ONUSE",
            1 => "ITEM_SPELL_TRIGGER_ONEQUIP",
            2 => "ITEM_SPELL_TRIGGER_ONPROC",
            _ => continue,
        };
        let Some(prefix) = get(key) else { continue };
        addw((format!("{prefix} {text}"), GREEN))?;
    }
    if let Some(charges) = charges_phrase(v.charges.max(0) as u32, &get) {
        add((charges, WHITE))?;
    }
    // The item-SET block (above the binary's p4 cut at `0x52e14c`), byte-read at the builder's
    // `0x52d8a0..0x52e0f5`: a blank gold line ([`SET_SPACER`]), the gold "name (owned/total)"
    // header, the set-level skill line (white, red when short), the member ladder ("  name" —
    // pale-cream `0xc0d368` when equipped, gray otherwise; a member whose template is still in
    // flight waits — the app re-pushes the view as names land), a second blank, then the threshold
    // bonuses sorted THRESHOLD-ASCENDING (the builder qsorts the slot indices via `0x52e5c0` —
    // never the DBC's stored order), green only when the skill requirement is met (`0x5eaae0`)
    // AND owned ≥ threshold. "Owned" counts EQUIPPED members on both builder paths (the
    // owner-unit 19-slot scan / the hyperlink walk with section mask 0x1).
    //
    // **The two bonus arms are two different KEYS, and this is where reading the byte law paid**
    // (`0x52e056..0x52e0d6`): the met arm formats `ITEM_SET_BONUS` = "Set: %s" with the text
    // alone, and only the unmet arm formats `ITEM_SET_BONUS_GRAY` = "(%d) Set: %s" with the
    // count. We had printed the counted form in both colours, so an ACTIVE set bonus read
    // "(3) Set: …" where 1.12 reads "Set: …".
    if let Some(set) = &set_view {
        let owned = set
            .members
            .iter()
            .filter(|(id, _)| equipped.contains(id))
            .count();
        addw((SET_SPACER.into(), GOLD))?;
        keyed(
            "ITEM_SET_NAME",
            &[
                Arg::S(&set.name),
                Arg::D(owned as i64),
                Arg::D(set.members.len() as i64),
            ],
            GOLD,
            false,
        )?;
        let skill_met = set.required_skill == 0 || {
            let have = req.skills.get(&set.required_skill).copied().unwrap_or(0);
            have >= set.required_skill_rank
        };
        if let Some(skill) = &set.required_skill_name {
            let color = req_color(skill_met);
            if set.required_skill_rank > 0 {
                let args = [Arg::S(skill), Arg::D(set.required_skill_rank.into())];
                keyed("ITEM_MIN_SKILL", &args, color, false)?;
            } else {
                keyed("ITEM_REQ_SKILL", &[Arg::S(skill)], color, false)?;
            }
        }
        // The member ladder's two-space indent is the engine's own `"  %s"` literal `0x854b14`,
        // not a string-table entry.
        for (id, name) in &set.members {
            let Some(name) = name else { continue };
            let color = if equipped.contains(id) { CREAM } else { GRAY };
            add((format!("  {name}"), color))?;
        }
        addw((SET_SPACER.into(), GOLD))?;
        let mut bonuses: Vec<&(u32, String)> = set.bonuses.iter().collect();
        bonuses.sort_by_key(|&&(threshold, _)| threshold);
        for &(threshold, ref text) in bonuses {
            if skill_met && owned as u32 >= threshold {
                keyed("ITEM_SET_BONUS", &[Arg::S(text)], GREEN, true)?;
            } else {
                let args = [Arg::D(threshold.into()), Arg::S(text)];
                keyed("ITEM_SET_BONUS_GRAY", &args, GRAY, true)?;
            }
        }
    }
    // **p4's early return** — `0x52e147` reads the flag and `0x52e14c je 0x52e170` takes the
    // NON-compact leg, so compact is the fall-through `0x52e14e`: it stamps `[esi+0xd0]=1`, lays
    // out, and returns `[ebp-0x38]` (the `hasCooldown` answer is produced on this path too).
    // Everything from here down — flavor text, creator/gift, OPENABLE/READABLE, money — is below
    // `0x52e170` and never runs.
    if flags.name_only {
        return Ok(());
    }
    // The quoted flavor text — gold, wrapped, literal quotes (all three byte-verified).
    if !v.description.is_empty() {
        addw((format!("\"{}\"", v.description), GOLD))?;
    }
    // Everything below rides the REAL-INSTANCE gate (`0x52e1c7`/`0x52e2e0`): a template/link
    // hover emits no creator line and no openable/readable line — you can't right-click a
    // hyperlink open.
    let Some(inst) = inst else { return Ok(()) };
    // The creator line (`0x52e1b1..0x52e2db`): the resolved
    // `ITEM_FIELD_CREATOR` name — a letter (instance text id) is ITEM_WRITTEN_BY, anything
    // else ITEM_CREATED_BY, both the literal 1.12 GlobalStrings (the Made-by green is the
    // string's OWN `|cff00ff00` escape; the AddLine color pointer is white `0xc0cf60`, wrap 0).
    // A WRAPPED instance (`ITEM_FIELD_FLAGS` bit `0x8` — the arm gate `0x52b7b0`, instance bit
    // only) switches the whole line to ITEM_WRAPPED_BY off `ITEM_FIELD_GIFTCREATOR`; that
    // field has no feed yet, so a wrapped instance emits NOTHING here rather than a wrong
    // "Made by" (gifts join with the wrap/unwrap arc).
    if inst.flags & 0x8 == 0 {
        if let Some(name) = &inst.creator {
            let key = if inst.has_text {
                "ITEM_WRITTEN_BY"
            } else {
                "ITEM_CREATED_BY"
            };
            keyed(key, &[Arg::S(name)], WHITE, false)?;
        }
    }
    // ITEM_OPENABLE / ITEM_READABLE — ONE line, openable wins outright (the `jmp 0x52e35d` past
    // the READABLE test; `0x52e2f2..0x52e35d`): openable = a p6=0 source ([`ItemInstance::
    // openable_source`]) AND the template loot flag `0x4` behind its lock sub-gate (LockID set →
    // only once the instance carries UNLOCKED `0x4`), or a wrapped gift (template WRAPPER `0x200`
    // + instance WRAPPED `0x8`); readable = template PageText (the CGItem vtable `+0x74` getter
    // `0x5d9e10`) OR the instance letter text. Both green.
    //
    // Note the lock sub-gate is the LINE's, not the CLICK's: the send arm tests the bare template
    // bit, so a locked junkbox stays silent here while its right-click still goes out and draws
    // the server's "Item is locked" (`ItemInfo::opens_loot`). The click order is
    // also the inverse of this one — READABLE wins there.
    let openable = inst.openable_source
        && ((v.flags & 0x4 != 0 && (v.lock_id == 0 || inst.flags & 0x4 != 0))
            || (v.flags & 0x200 != 0 && inst.flags & 0x8 != 0));
    if openable {
        keyed("ITEM_OPENABLE", &[], GREEN, false)?;
    } else if v.page_text != 0 || inst.has_text {
        keyed("ITEM_READABLE", &[], GREEN, false)?;
    }
    Ok(())
}

/// A temporary enchant's line text — the name with its countdown, the reference's own bucket
/// ladder (`0x52fa50`): a runtime key
/// `ITEM_ENCHANT_TIME_LEFT_<DAYS|HOURS|MIN|SEC>` chosen by the largest unit that fits, with the
/// count taken as **ceil** in the day/hour/minute arms and **truncated** in the seconds arm, and
/// the `_P1` plural twin picked by [`plural_template`]'s byte-pinned rule. The day and hour keys
/// ship a twin; minutes and seconds do not, so those read "(1 min)" at one minute exactly as they
/// read "(9 min)".
///
/// **Two holes, which is why this cannot be `duration_text`**: the templates are
/// `"%s (%d min)"` — the enchant's own name fills the first, the count the second. That name is
/// the `SpellItemEnchantment` row's, DBC data rather than a string-table sentence, so it arrives
/// as an argument. `None` = the family is absent from the install's string table, and the caller
/// draws no line at all.
fn enchant_time_left(name: &str, ms: u64, get: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    let (suffix, n) = time_bucket(ms);
    let template = plural_template(&format!("ITEM_ENCHANT_TIME_LEFT_{suffix}"), n, get)?;
    Some(fill(&template, &[Arg::S(name), Arg::D(i64::from(n))]))
}

/// `0x52fa50`'s bucket + count for the ENCHANT countdown — the key suffix and the number that
/// fills it. The largest unit that fits wins (`>= 1 day` · `>= 1 h` · `>= 1 min` · else seconds);
/// the count is **ceil** in the day/hour/minute arms (the `roundUp` argument, which all seven of
/// the function's callers pass as 1) and plain **truncation** in the seconds arm.
///
/// The same ladder [`crate::script::tooltip::duration_text`] walks — it stays separate only
/// because that one fills a single-hole template and this family has two holes; every other
/// countdown in the tooltips goes through `duration_text`.
fn time_bucket(ms: u64) -> (&'static str, u32) {
    const SEC: u64 = 1_000;
    const MIN: u64 = 60 * SEC;
    const HOUR: u64 = 60 * MIN;
    const DAY: u64 = 24 * HOUR;
    let (suffix, n) = match ms {
        _ if ms >= DAY => ("DAYS", ms.div_ceil(DAY)),
        _ if ms >= HOUR => ("HOURS", ms.div_ceil(HOUR)),
        _ if ms >= MIN => ("MIN", ms.div_ceil(MIN)),
        _ => ("SEC", ms / SEC),
    };
    // The count is a dword in the reference; a timer long enough to overflow one does not exist.
    (suffix, u32::try_from(n).unwrap_or(u32::MAX))
}

/// `ITEM_SPELL_CHARGES` and its `_P1` plural twin (`0x84e3b4`, pushed at `0x52cae8` for the
/// enchant suffix and `0x52db61` for the standalone line). One rule for both consumers: the
/// charges line and the enchant line's ` (…)` suffix.
///
/// Zero answers `None` — the reference never reaches this with a zero count, and neither
/// consumer here has a line to draw without one.
fn charges_phrase(n: u32, get: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    if n == 0 {
        return None;
    }
    let template = plural_template("ITEM_SPELL_CHARGES", n, get)?;
    Some(fill(&template, &[Arg::D(i64::from(n))]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in string table, **deliberately not the shipped wording** — what is under test is
    /// which key is reached and what fills it, never what the sentence says (
    /// "assert the identifier, not the sentence"). Every value carries angle brackets so a
    /// resolved line cannot be mistaken for a composed one.
    fn table(key: &str) -> Option<String> {
        Some(
            match key {
                "ITEM_ENCHANT_TIME_LEFT_DAYS" => "<%s :: %d D>",
                "ITEM_ENCHANT_TIME_LEFT_DAYS_P1" => "<%s :: %d DD>",
                "ITEM_ENCHANT_TIME_LEFT_HOURS" => "<%s :: %d H>",
                "ITEM_ENCHANT_TIME_LEFT_HOURS_P1" => "<%s :: %d HH>",
                "ITEM_ENCHANT_TIME_LEFT_MIN" => "<%s :: %d M>",
                "ITEM_ENCHANT_TIME_LEFT_SEC" => "<%s :: %d S>",
                "ITEM_SPELL_CHARGES" => "<%d chg>",
                "ITEM_SPELL_CHARGES_P1" => "<%d chgs>",
                _ => return None,
            }
            .to_string(),
        )
    }

    /// The countdown's bucket ladder (`0x52fa50`): the largest unit that
    /// fits wins, the count is **ceil** in the day/hour/minute arms and **truncated** in seconds,
    /// and the day/hour arms have `_P1` twins while minutes and seconds do not — so those two
    /// suffixes reach the same key at every count.
    #[test]
    fn enchant_countdown_buckets_and_rounding() {
        let t = |ms| enchant_time_left("Rockbiter", ms, &table);
        // Seconds truncate: 1900 ms is 1, not 2.
        assert_eq!(t(1_900).as_deref(), Some("<Rockbiter :: 1 S>"));
        assert_eq!(t(59_999).as_deref(), Some("<Rockbiter :: 59 S>"));
        // Minutes ceil: one second past 4 minutes already reads 5.
        assert_eq!(t(60_000).as_deref(), Some("<Rockbiter :: 1 M>"));
        assert_eq!(t(241_000).as_deref(), Some("<Rockbiter :: 5 M>"));
        // Hours ceil, and only the non-singular count takes the `_P1` key.
        assert_eq!(t(3_600_000).as_deref(), Some("<Rockbiter :: 1 H>"));
        assert_eq!(t(3_600_001).as_deref(), Some("<Rockbiter :: 2 HH>"));
        // Days ceil.
        assert_eq!(t(86_400_000).as_deref(), Some("<Rockbiter :: 1 D>"));
        assert_eq!(t(86_400_001).as_deref(), Some("<Rockbiter :: 2 DD>"));
        // Zero never reaches here (the app drops an expired timer), but it must not panic — and
        // zero takes the PLURAL arm, which is `GetPluralIndex`'s rule, not a rounding accident.
        assert_eq!(t(0).as_deref(), Some("<Rockbiter :: 0 S>"));
    }

    /// A string table that does not carry the family draws **no line**, never an invented one —
    /// the whole point of resolving by key rather than composing (2045).
    #[test]
    fn a_missing_countdown_family_draws_nothing() {
        assert_eq!(enchant_time_left("Rockbiter", 60_000, &|_| None), None);
        assert_eq!(charges_phrase(3, &|_| None), None);
    }

    /// `ITEM_SPELL_CHARGES` and its `_P1` plural twin — one rule, both consumers (the charges
    /// line and the enchant line's suffix). A zero count has no line either way.
    #[test]
    fn charges_phrase_picks_the_plural() {
        assert_eq!(charges_phrase(1, &table).as_deref(), Some("<1 chg>"));
        assert_eq!(charges_phrase(5, &table).as_deref(), Some("<5 chgs>"));
        assert_eq!(charges_phrase(0, &table), None);
    }
}
