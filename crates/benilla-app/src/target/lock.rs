//! The GameObject **lock chain** — the client's resolver `0x5f83d0`, its per-slot **Action** gate
//! (`0x5f81d0`), and the refusal-toast routing (`0x5f3427..`; decisions 0239 / 0545 / **0752**).
//!
//! **One chain, two consumers — exactly as the reference.** `CGGameObject`'s per-type strategy
//! calls the same resolver twice: from **`usable` `0x5f3130`** (which decides the cursor's grayed
//! twin *and* whether the right-click is sent at all), and from the **USE sender
//! `0x5f33e0`** (which picks between `CMSG_GAMEOBJ_USE`, an `OPEN_LOCK` cast, and a client-local
//! toast). That is why it lives here rather than inside either caller: the icon and the
//! click agree by construction only if they ask the same question.
//!
//! ## The Action gate — the piece that was missing
//!
//! Before the resolver will *consider* a `Lock.dbc` slot it asks `0x5f81d0(GO, Action[i])`, which
//! answers from the GameObject's own **state** and its `GO_FLAG_LOCKED` wire bit — see
//! [`benilla_formats::LockSlot::available`]. Without it, the ladder is over-permissive in a way
//! that looks arbitrary from the chair: nearly every keyed door in 5875 carries a spare
//! `Quick Open` slot (`LockType 10`, `Skill 0`, **`Action 0`**), *every* character knows spell 6247
//! "Opening", and Action 0 means "only when the object is NOT flagged locked" — so skipping the
//! gate hands every padlocked door in the game to every player. The Searing Gorge gate (lock 84)
//! was the one that refused because it is the one that carries no `Action 0` slot.
//!
//! ## Satisfaction is the SPELL's value — and the value's level term IS the player's skill
//!
//! `0x5f850f` compares the matched spell's own OPEN_LOCK **effect value**
//! ([`benilla_formats::SpellDisplay::open_lock_skill`]) against the slot's requirement — which is
//! `Skill[i]`, or **`GAMEOBJECT_LEVEL × 5`** when `Skill[i]` is zero (`0x5f84be`). The sentence
//! that used to stand here — "it never reads the skill block" — is wrong at the bytes: the
//! value's level term routes through the CGPlayer vtable (`0x6e384d → 0x6e3130
//! → [vtbl+0xa8] = 0x5ea690`) into `PLAYER_SKILL_INFO` for the spell's own SkillLineAbility
//! line ([`spell_skill_value`]). At skill cap the two readings coincide (`skill/5 == level`),
//! which is how the old one survived every at-cap cross-check while a level-60 with 1 Mining
//! satisfied a 300-skill vein (decision 1320 named the bug; this module now carries the fix).

use std::collections::BTreeSet;

use benilla_formats::{LockSlot, LOCK_KEY_ITEM, LOCK_KEY_SKILL};
use bevy::prelude::*;

use crate::net::ObjectStore;

/// The lock chain's full data set as ONE [`SystemParam`]: the
/// ask-once GO-template cache, `Lock.dbc` + `LockType.dbc`, the spell catalog, and the ask-once
/// item-template cache (key-item names for the "Requires \<key\>" toast, and the key's own ON_USE
/// spell). The `Option` members are absent without client data.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct GoLockInputs<'w, 's> {
    // Shared: the tooltip arm's ask-once template request on a miss marks itself through
    // `&self`, so neither arm needs the store exclusively.
    pub(crate) templates: Res<'w, crate::go_templates::GameObjectTemplates>,
    pub(crate) locks: Option<Res<'w, crate::go_templates::Locks>>,
    pub(crate) lock_types: Option<Res<'w, crate::go_templates::LockTypes>>,
    pub(crate) spells: Option<Res<'w, crate::ui_action::Spells>>,
    /// The skill-line catalog — the opener-value level term's spell→line hop
    /// ([`spell_skill_value`]). Absent without client data, which reads as skill 0 (fail-closed).
    pub(crate) skill_lines: Option<Res<'w, crate::ui_spellbook::SkillLines>>,
    pub(crate) items: Res<'w, crate::items::Items>,
    /// The one object index (2334) — the key-item scan walks the player's bags through it.
    pub(crate) objects: crate::net::Objects<'w, 's>,
}

/// The GameObject facts the Action gate and the requirement fallback read off the wire — gathered
/// once by the caller so the resolver stays pure.
#[derive(Clone, Copy, Debug)]
pub(crate) struct GoFacts {
    /// The client's stored `GAMEOBJECT_STATE` (`go+0x27c`) — [`crate::go_anim::go_state`].
    pub(crate) state: u32,
    /// `GAMEOBJECT_FLAGS & GO_FLAG_LOCKED (0x2)`.
    pub(crate) flag_locked: bool,
    /// `GAMEOBJECT_LEVEL` — the `Skill[i] == 0` requirement fallback's `× 5` base. vmangos leaves
    /// it 0 on everything but transports, so on this server the fallback resolves to "no
    /// requirement" and the server does the real gating; the client law is modelled regardless.
    pub(crate) level: u32,
}

/// What the resolver says about a GameObject's lock — the reference's return plus its spell-id
/// out-param, made explicit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LockOutcome {
    /// No `Lock.dbc` row, or every slot empty — `0x5f8180` null / `[ebp-1]` never set. The object
    /// opens by `CMSG_GAMEOBJ_USE`.
    Unlocked,
    /// A skill slot the player satisfies with a known `OPEN_LOCK` spell — cast it at the GO.
    OpenBySpell(u32),
    /// A key slot whose item the player carries; the value is the **key's entry**. The reference
    /// resolves the item's ON_USE spell here (`0x5d8c80`) and casts that.
    OpenByKey(u32),
    /// A lock is present and no slot is satisfied — the client-local refusal, **no packet**.
    Unmet,
}

impl LockOutcome {
    /// The `usable` half: `0x5f3130`'s lock arm returns *not usable* only for [`Self::Unmet`], and
    /// **only when `GO_FLAG_LOCKED` is set** (`0x5f32a6`: `shr 1; test al,1; je`). With the flag
    /// clear the arm is skipped entirely — which is exactly why a herb node you cannot gather still
    /// shows the lit `GatherHerbs` cursor and only toasts on the click.
    pub(super) fn blocks_usable(self, flag_locked: bool) -> bool {
        flag_locked && self == LockOutcome::Unmet
    }
}

/// Read the Action gate's inputs off a hovered GameObject — its store plus the stored state the
/// caller already resolved ([`crate::go_anim::go_state`]). `None` (nothing hovered / no store yet)
/// answers with the wire defaults, which make every gated slot inapplicable rather than free.
pub(crate) fn go_facts(go: Option<(&ObjectStore, u32)>) -> GoFacts {
    match go {
        Some((store, state)) => GoFacts {
            state,
            flag_locked: store.0.gameobject_flags() & GO_FLAG_LOCKED != 0,
            level: store.0.gameobject_level(),
        },
        None => GoFacts {
            state: benilla_formats::GO_STATE_ACTIVE,
            flag_locked: false,
            level: 0,
        },
    }
}

/// `GO_FLAG_LOCKED` (vmangos `GameObjectFlags`) — the wire bit that both selects the Action-1
/// ("unlock") slots and arms `usable`'s lock check (`0x5f32a6`).
pub(crate) const GO_FLAG_LOCKED: u32 = 0x2;

/// The client's lock resolver **`0x5f83d0`**, transcribed.
///
/// Walks the 8 `Lock.dbc` slots **in order**, dispatching each by `Type`:
/// - **SKILL (2)** — gate on [`LockSlot::available`], then linear-scan the player's known spells
///   for one whose `SPELL_EFFECT_OPEN_LOCK` `EffectMiscValue` equals the slot's `Index`; the first
///   such match sets `matched_spell` unconditionally (`0x5f84f8`, *before* the rank test — that
///   nonzero-ness is the toast routing's `0xdf`-vs-`0xe0` discriminator), then the spell's effect
///   value is compared against the requirement (`0x5f850f`). Sufficient → satisfied.
/// - **KEY (1)** — gate the same way, then look for the key item in our bags/keyring.
/// - **NONE (0)** — skipped without marking the lock real.
///
/// Any SKILL/KEY slot marks the lock **real** even when its Action gate rejects it (the binary
/// writes `[ebp-1] = 1` *before* calling `0x5f81d0`), so a door whose only opener is gated out
/// refuses rather than falling through to `CMSG_GAMEOBJ_USE`.
///
/// `matched_spell` is the out-param the toast routing needs; it is written for a LockType match
/// even when the value test then fails.
///
/// **The scan order is part of the answer, not an implementation detail**. The
/// reference walks its known-spell **array** in index order and *returns on the first sufficient
/// match*, so when two known spells both match the LockType and both clear the requirement, the
/// order decides which one is cast — and that spell's name is what the cast bar reads. This was
/// documented here for a long time as a harmless difference ("it only decides which spell lands in
/// `matched_spell`, never the outcome"), which is false: every character knows both 6478 "Opening"
/// and 22810 **"Opening - No Text"** — both `SPELL_EFFECT_OPEN_LOCK` on `LockType 13` (Open
/// Kneeling), both trivially sufficient against the `Skill == 0` slots the ground containers carry
/// — so iterating a `HashSet` put Blizzard's placeholder name on the cast bar at the hash's whim.
/// `known` is a [`BTreeSet`] for exactly that reason: ascending spell id is the reference
/// array's own order after login, the server building `SMSG_INITIAL_SPELLS` out of a `std::map`.
pub(crate) fn resolve_lock(
    slots: &[LockSlot],
    known: &BTreeSet<u32>,
    spells: Option<&crate::ui_action::Spells>,
    skill_lines: Option<&benilla_formats::SkillLineCatalog>,
    me: Option<&ObjectStore>,
    objects: &crate::net::Objects,
    go: GoFacts,
    matched_spell: &mut Option<u32>,
) -> LockOutcome {
    let mut real = false;
    for slot in slots {
        match slot.key_type {
            LOCK_KEY_SKILL => {
                real = true;
                if !slot.available(go.state, go.flag_locked) {
                    continue;
                }
                let Some(spells) = spells else { continue };
                for &id in known {
                    let Some(spell) = spells.catalog.get(id) else {
                        continue;
                    };
                    if spell.open_lock_type() != Some(slot.index) {
                        continue;
                    }
                    // `0x5f84f8` — the out-param is written on the LockType match, before the
                    // value test.
                    matched_spell.get_or_insert(id);
                    let skill = spell_skill_value(me, skill_lines, id);
                    let provides = spell.open_lock_skill(skill).unwrap_or(0);
                    if provides >= required_skill(slot, go.level) {
                        return LockOutcome::OpenBySpell(id);
                    }
                }
            }
            LOCK_KEY_ITEM => {
                real = true;
                if !slot.available(go.state, go.flag_locked) {
                    continue;
                }
                if me.is_some_and(|s| holds_item(&s.0, objects, slot.index)) {
                    return LockOutcome::OpenByKey(slot.index);
                }
            }
            _ => {}
        }
    }
    if real {
        LockOutcome::Unmet
    } else {
        LockOutcome::Unlocked
    }
}

/// The player's skill value for `spell_id`'s own line — the opener-value level term's source:
/// `0x5ea690` hops spell → SkillLineAbility line
/// (`0x6de040`, `[+4]`), then `0x5ea520` scans `PLAYER_SKILL_INFO` (`PLAYER_SKILL_INFO_1_1 +
/// 3·slot`) and returns value **plus both bonus halves** (`0x5ea56d`/`0x5ea578`/`0x5ea580`).
/// Every absent leg is fail-closed like the reference's null paths: no catalog, no store, no
/// SLA record, or a line the player does not carry all answer **0** — the opener then provides
/// only its flat terms.
fn spell_skill_value(
    me: Option<&ObjectStore>,
    skill_lines: Option<&benilla_formats::SkillLineCatalog>,
    spell_id: u32,
) -> u32 {
    let Some(line) = skill_lines.and_then(|c| c.spell_to_line(spell_id)) else {
        return 0;
    };
    let Some(store) = me else { return 0 };
    line_skill_value(
        (0..benilla_protocol::messages::PLAYER_SKILL_SLOTS)
            .filter_map(|slot| store.0.player_skill(slot)),
        line,
    )
}

/// The `0x5ea520` sum on one line's slot: `value + temp_bonus + perm_bonus`, floored at 0 (the
/// bonuses are signed — a curse can push the sum below the base). First matching slot wins;
/// a line absent from the block reads 0.
fn line_skill_value(
    slots: impl Iterator<Item = benilla_protocol::messages::PlayerSkillSlot>,
    line: u32,
) -> u32 {
    for s in slots {
        if u32::from(s.skill_id) == line {
            let v = i32::from(s.value) + i32::from(s.temp_bonus) + i32::from(s.perm_bonus);
            return v.max(0) as u32;
        }
    }
    0
}

/// **`0x5f8260`** — the *targeting cursor's* lock question, and this chain's **third** consumer.
/// Reached from the object dispatcher `0x4828d0`'s targeting step
/// (`0x482910 → 0x6e6460`, whose GameObject arm calls it at `6e670f` with the GO, the cast-item
/// guid `0xceac48` and the pending spell id `[0xceac58]`).
///
/// Where [`resolve_lock`] (`0x5f83d0`) asks *"can I open this with anything I have?"*, this asks
/// the narrower *"does **this pending spell** open **this lock**?"*. Three differences, all in the
/// bytes:
///
/// - It walks the **spell's** three effect slots, not the player's known spells — `5f82c0`'s outer
///   loop over `SpellRec+0xf4/f8/fc` with `cmp …, 0x21` (`SPELL_EFFECT_OPEN_LOCK`), matching
///   `EffectMiscValue[k]` (`[ebx+0xb4]`) against the slot's `Index[i]`.
/// - **It never compares skill values.** `0x5f83d0`'s satisfaction test (`0x5f850f`,
///   [`required_skill`]) has no counterpart here. A matched `LockType` that passes the Action gate
///   is the whole predicate — so the cursor lights on a chest your skill cannot yet open, the
///   click sends, and the *server* refuses. That is the same "no gate on the object leg" 0939
///   found at `BindTarget`, and it is why this is not simply [`resolve_lock`] reused.
/// - A GameObject with **no `Lock.dbc` row is `false`**, not "open" (`0x5f8180` null → `5f8273`
///   returns 0) — the opposite of [`resolve_lock`]'s lockless arm, and correct: you cannot Pick
///   Lock a mailbox, and the cursor greys over one.
///
/// The Action gate ([`LockSlot::available`]) is shared with both siblings — same `0x5f81d0`, so
/// the cursor, the tooltip and the click cannot disagree about whether a slot applies.
///
/// **Not transcribed:** the `SPELL_EFFECT_OPEN_LOCK_ITEM` (59) arm at `5f831c`, which matches the
/// **cast item's** entry against a `LOCK_KEY_ITEM` slot (`5f8360`: `Type[i] == 1`, `5f836c`: the
/// item's entry `== Index[i]`, then the same gate). It needs a per-effect id the catalog does not
/// carry yet, and it only fires for a key's own ON_USE spell armed at a GameObject — a seam 0939
/// built but no 5875 key is known to use. Named residual.
pub(crate) fn spell_opens_lock(
    slots: &[LockSlot],
    spell: &benilla_formats::SpellDisplay,
    go: GoFacts,
) -> bool {
    let Some(lock_type) = spell.open_lock_type() else {
        return false;
    };
    slots.iter().any(|slot| {
        slot.key_type == LOCK_KEY_SKILL
            && slot.index == lock_type
            && slot.available(go.state, go.flag_locked)
    })
}

/// The slot's required skill: `Skill[i]`, or **`GAMEOBJECT_LEVEL × 5`** when it is zero
/// (`0x5f84be..0x5f84ca` — the resolver; `0x5f3490..0x5f349f` recomputes the same value for the
/// `0xe0` toast's `%d`). A zero `Skill` is *not* "no requirement": every gathering node in the game
/// stores 0 there and leans on the object's level.
pub(crate) fn required_skill(slot: &LockSlot, go_level: u32) -> i32 {
    if slot.skill != 0 {
        slot.skill as i32
    } else {
        (go_level * 5) as i32
    }
}

/// Whether we carry item `entry` — the key-slot scan's inventory walk. This IS the reference's own
/// walker (`0x622270` → `0x622420`, mode 0 ⇒ `0x47` = equipment | bags | backpack | keyring), which
/// benilla already transcribes once as [`crate::ui_items::find_item`]; it used to be a second,
/// hand-rolled walk here that missed the equipment slots and read 32 keyring slots where the wire
/// has 16. One walker, one answer — and the caller needs the POSITION it returns anyway, to address
/// `CMSG_USE_ITEM`.
fn holds_item(
    store: &benilla_protocol::messages::ObjectFields,
    objects: &crate::net::Objects,
    entry: u32,
) -> bool {
    crate::ui_items::find_item(
        store,
        objects,
        entry,
        crate::ui_items::ItemSearch::default(),
    )
    .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_formats::{GO_STATE_ACTIVE, GO_STATE_READY, LOCK_KEY_NONE};

    fn skill_slot(index: u32, skill: u32, action: u32) -> LockSlot {
        LockSlot {
            key_type: LOCK_KEY_SKILL,
            index,
            skill,
            action,
        }
    }

    /// **`0x5f8260` is not `0x5f83d0`**. The cursor's question differs from the
    /// right-click resolver's in three ways that all show on screen, and getting any of them
    /// wrong greys the wrong chest.
    #[test]
    fn the_cursor_lock_predicate_ignores_skill_and_refuses_the_lockless() {
        use benilla_formats::SpellDisplay;
        // Pick Lock: LockType 1 (Lockpicking), providing 300 at level 60.
        let pick_lock = SpellDisplay {
            open_lock: Some(benilla_formats::OpenLock {
                effect: 0,
                lock_type: 1,
            }),
            ..SpellDisplay::default()
        };
        let unlocked = GoFacts {
            state: GO_STATE_READY,
            flag_locked: true,
            level: 60,
        };

        // A Lockpicking slot demanding 300 skill, Action 1 (applies while flagged locked).
        let matching = [skill_slot(1, 300, 1), LockSlot::default()];
        assert!(
            spell_opens_lock(&matching, &pick_lock, unlocked),
            "the right LockType through an applicable slot lights the cursor"
        );

        // **The skill value is never read** — `0x5f850f` has no counterpart in `0x5f8260`. A slot
        // demanding far more than Pick Lock provides STILL lights: the click sends and the server
        // refuses. This is the single biggest difference from `resolve_lock`.
        let brutal = [skill_slot(1, 9999, 1), LockSlot::default()];
        assert!(
            spell_opens_lock(&brutal, &pick_lock, unlocked),
            "0x5f8260 compares no skill values — an out-of-reach lock still lights"
        );

        // Wrong LockType (3 = Mining) — a lockpick greys over an ore vein.
        let mining = [skill_slot(3, 0, 1), LockSlot::default()];
        assert!(!spell_opens_lock(&mining, &pick_lock, unlocked));

        // The shared Action gate still applies: Action 0 means "only when NOT flagged locked".
        assert!(!spell_opens_lock(
            &[skill_slot(1, 0, 0)],
            &pick_lock,
            unlocked
        ));
        assert!(spell_opens_lock(
            &[skill_slot(1, 0, 0)],
            &pick_lock,
            GoFacts {
                flag_locked: false,
                ..unlocked
            }
        ));

        // **No lock row at all is FALSE, not "open"** (`0x5f8180` null → `5f8273`) — the exact
        // opposite of `resolve_lock`'s lockless arm. You cannot Pick Lock a mailbox.
        assert!(!spell_opens_lock(&[], &pick_lock, unlocked));
        assert!(!spell_opens_lock(
            &[LockSlot::default()],
            &pick_lock,
            unlocked
        ));

        // A spell with no OPEN_LOCK effect opens nothing, whatever the lock says.
        assert!(!spell_opens_lock(
            &matching,
            &SpellDisplay::default(),
            unlocked
        ));

        // A KEY slot is not the SKILL arm's business (the effect-59 arm is a named residual).
        let key = [LockSlot {
            key_type: LOCK_KEY_ITEM,
            index: 1,
            skill: 0,
            action: 1,
        }];
        assert!(!spell_opens_lock(&key, &pick_lock, unlocked));
    }

    /// The requirement fallback (`0x5f84be`): a zero `Skill[i]` means GO-level × 5, not "free".
    #[test]
    fn zero_skill_falls_back_to_go_level_times_five() {
        assert_eq!(required_skill(&skill_slot(3, 0, 0), 0), 0);
        assert_eq!(required_skill(&skill_slot(3, 0, 0), 20), 100);
        // A nonzero Skill wins outright — the level is never consulted.
        assert_eq!(required_skill(&skill_slot(1, 280, 1), 60), 280);
    }

    /// **The 1320 bug, fixed**: the opener's level
    /// term is the player's SKILL in the spell's own line, never their character level — so a
    /// level-60 with 1 Mining is refused by a 250-skill vein the old caster-level reading handed
    /// them. Also pins the two fail-closed legs (`0x623b70`-style: absent data reads skill 0)
    /// and the bonus halves of the `0x5ea520` sum.
    #[test]
    fn the_lock_value_tracks_the_players_skill_not_their_level() {
        use benilla_formats::{OpenLock, SkillLineCatalog, SpellCatalog, SpellDisplay};
        use benilla_protocol::messages::{ObjectFields, FIELD_PLAYER_SKILL_INFO_1_1};

        // Mining 2575, real shape: `−1 + 1 + 5.0·Δ`, baseLevel 0, LockType 3.
        let mining = SpellDisplay {
            open_lock: Some(OpenLock {
                lock_type: 3,
                effect: 0,
            }),
            effect_base_points: [-1, 0, 0],
            effect_base_dice: [1, 0, 0],
            effect_real_points_per_level: [5.0, 0.0, 0.0],
            ..Default::default()
        };
        let spells = crate::ui_action::Spells {
            catalog: SpellCatalog::from_displays([(2575, mining)].into_iter().collect()),
            ..crate::ui_action::Spells::empty_for_tests()
        };
        // Spell → line: Mining is SkillLine 186.
        let lines = SkillLineCatalog::from_spell_lines([(2575, 186)]);
        let known = BTreeSet::from([2575]);
        // Nothing streamed: no case here holds a key, and the key branch is only reached
        // with a store, which these pass as `None` (2334).
        let mut objs = crate::ui_items::TestObjects::new();
        let objects = objs.get();
        // A 250-skill vein, available (Action 0, READY, not flagged).
        let mut vein = [LockSlot::default(); 8];
        vein[0] = skill_slot(3, 250, 0);
        let facts = GoFacts {
            state: GO_STATE_READY,
            flag_locked: false,
            level: 0,
        };
        // A level-60 store (UNIT_FIELD_LEVEL = 34) whose skill block holds Mining at `value`,
        // with the bonus dwords in the third word ([`PlayerSkillSlot`]'s verified packing).
        let store_with_mining = |value: u32, bonus_word: u32| {
            ObjectStore(ObjectFields::from_pairs(&[
                (34, 60),
                (FIELD_PLAYER_SKILL_INFO_1_1, 186),
                (FIELD_PLAYER_SKILL_INFO_1_1 + 1, value | (300 << 16)),
                (FIELD_PLAYER_SKILL_INFO_1_1 + 2, bonus_word),
            ]))
        };
        let resolve = |store: Option<&ObjectStore>, lines: Option<&SkillLineCatalog>| {
            resolve_lock(
                &vein,
                &known,
                Some(&spells),
                lines,
                store,
                &objects,
                facts,
                &mut None,
            )
        };

        // 300 Mining opens; 1 Mining — the level-60 the old reading waved through — is refused.
        let skilled = store_with_mining(300, 0);
        assert_eq!(
            resolve(Some(&skilled), Some(&lines)),
            LockOutcome::OpenBySpell(2575)
        );
        let unskilled = store_with_mining(1, 0);
        assert_eq!(resolve(Some(&unskilled), Some(&lines)), LockOutcome::Unmet);

        // The `0x5ea520` sum counts both bonus halves: 235 + 10 temp + 5 perm = 250, exactly
        // enough.
        let buffed = store_with_mining(235, 10 | (5 << 16));
        assert_eq!(
            resolve(Some(&buffed), Some(&lines)),
            LockOutcome::OpenBySpell(2575)
        );

        // Fail-closed legs: no skill-line catalog, or no store, reads skill 0 — refused, never
        // a fall-back to the character level.
        assert_eq!(resolve(Some(&skilled), None), LockOutcome::Unmet);
        assert_eq!(resolve(None, Some(&lines)), LockOutcome::Unmet);
    }

    /// A door whose only opener is gated out by its Action must refuse, **not** fall through to
    /// `CMSG_GAMEOBJ_USE`: the binary marks the lock real (`[ebp-1] = 1`) *before* asking
    /// `0x5f81d0`. This is the difference between "locked door refuses" and "locked door opens".
    #[test]
    fn a_gated_out_slot_still_makes_the_lock_real() {
        let slots = [
            skill_slot(10, 0, 0), // Quick Open — Action 0, gated out on a flagged-locked door
            LockSlot::default(),
        ];
        // Nothing streamed: no case here holds a key, and the key branch is only reached
        // with a store, which these pass as `None` (2334).
        let mut objs = crate::ui_items::TestObjects::new();
        let objects = objs.get();
        let mut matched = None;
        let out = resolve_lock(
            &slots,
            &BTreeSet::new(),
            None,
            None,
            None,
            &objects,
            GoFacts {
                state: GO_STATE_READY,
                flag_locked: true,
                level: 0,
            },
            &mut matched,
        );
        assert_eq!(out, LockOutcome::Unmet);
        assert!(
            out.blocks_usable(true),
            "a flagged-locked unmet lock grays the cursor"
        );
        // …and with the flag clear the SAME lock is not a `usable` blocker (the arm is skipped) —
        // the herb-node case: lit cursor, toast on click.
        assert!(!out.blocks_usable(false));
    }

    /// The reported bug, end to end, on the **real** shipped `Lock.dbc` / `Spell.dbc` values.
    /// `benilla-formats`' own `real_lock_catalog_reads_the_action_column` and
    /// `real_spell_catalog_computes_the_lock_skill_an_opener_provides` pin these numbers against
    /// the files, so this stays a pure unit test while still describing real data.
    ///
    /// Before the Action gate, every one of these doors opened for every character: "Opening"
    /// (6247) satisfied their spare `Quick Open` slot with a flat 100 ≥ 0.
    #[test]
    fn a_keyed_door_refuses_the_universally_known_opening_spell() {
        use benilla_formats::{OpenLock, SpellCatalog, SpellDisplay};

        // Scholomance Door, lockId 1159 — key 13704 / Pick Lock 280 / Quick Open / Quick Close /
        // Blasting 300; the template ships GO_FLAG_LOCKED.
        let mut scholomance = [LockSlot::default(); 8];
        scholomance[0] = LockSlot {
            key_type: LOCK_KEY_ITEM,
            index: 13704,
            skill: 0,
            action: 1,
        };
        scholomance[1] = skill_slot(1, 280, 1);
        scholomance[2] = skill_slot(10, 0, 0);
        scholomance[3] = skill_slot(11, 0, 2);
        scholomance[4] = skill_slot(16, 300, 1);

        // Two openers with their real value inputs: the "Opening" every character is created
        // with, and Pick Lock (whose value tracks 5×level).
        let opening = SpellDisplay {
            open_lock: Some(OpenLock {
                lock_type: 10,
                effect: 0,
            }),
            effect_base_points: [99, 0, 0],
            effect_base_dice: [1, 0, 0],
            ..Default::default()
        };
        let pick_lock = SpellDisplay {
            open_lock: Some(OpenLock {
                lock_type: 1,
                effect: 0,
            }),
            effect_base_points: [4, 0, 0],
            effect_base_dice: [1, 0, 0],
            effect_real_points_per_level: [5.0, 0.0, 0.0],
            base_level: 1,
            ..Default::default()
        };
        let spells = crate::ui_action::Spells {
            catalog: SpellCatalog::from_displays(
                [(6247, opening), (1804, pick_lock)].into_iter().collect(),
            ),
            ..crate::ui_action::Spells::empty_for_tests()
        };
        // Nothing streamed: no case here holds a key, and the key branch is only reached
        // with a store, which these pass as `None` (2334).
        let mut objs = crate::ui_items::TestObjects::new();
        let objects = objs.get();
        let locked_shut = GoFacts {
            state: GO_STATE_READY,
            flag_locked: true,
            level: 0,
        };

        // A character who knows only "Opening" — i.e. everybody — is refused.
        let mut matched = None;
        assert_eq!(
            resolve_lock(
                &scholomance,
                &BTreeSet::from([6247]),
                Some(&spells),
                None,
                None,
                &objects,
                locked_shut,
                &mut matched,
            ),
            LockOutcome::Unmet,
        );
        assert_eq!(
            matched, None,
            "a gated-out slot never even reaches the known-spell scan"
        );

        // Clear the flag and the SAME door opens to the SAME spell — proof that the gate is what
        // refuses, not the value test. (No shipped door does this; it isolates the mechanism.)
        assert_eq!(
            resolve_lock(
                &scholomance,
                &BTreeSet::from([6247]),
                Some(&spells),
                None,
                None,
                &objects,
                GoFacts {
                    flag_locked: false,
                    ..locked_shut
                },
                &mut None,
            ),
            LockOutcome::OpenBySpell(6247),
        );

        // Pick Lock sits on an Action 1 slot, so the flag *selects* it — but with no store and no
        // skill-line catalog the skill reads 0 (the fail-closed leg), Pick Lock provides its flat
        // 5, and 5 < 280 refuses. The out-param is still written (`0x5f84f8`), which is the toast
        // routing's `0xdf`-vs-`0xe0` discriminator. (The satisfied-by-skill path is
        // `the_lock_value_tracks_the_players_skill_not_their_level` below.)
        let mut matched = None;
        assert_eq!(
            resolve_lock(
                &scholomance,
                &BTreeSet::from([1804]),
                Some(&spells),
                None,
                None,
                &objects,
                locked_shut,
                &mut matched,
            ),
            LockOutcome::Unmet,
        );
        assert_eq!(
            matched,
            Some(1804),
            "a LockType match writes the out-param before the value test"
        );

        // The Searing Gorge gate (lockId 84) — the reporter's counter-example. No Action-0 slot at
        // all, which is why it refused even before the gate existed; it must still refuse.
        let mut searing_gorge = [LockSlot::default(); 8];
        searing_gorge[0] = LockSlot {
            key_type: LOCK_KEY_ITEM,
            index: 5396,
            skill: 0,
            action: 1,
        };
        searing_gorge[1] = skill_slot(1, 225, 1);
        assert_eq!(
            resolve_lock(
                &searing_gorge,
                &BTreeSet::from([6247]),
                Some(&spells),
                None,
                None,
                &objects,
                locked_shut,
                &mut None,
            ),
            LockOutcome::Unmet,
        );

        // And the gate must not touch what already worked: a Copper Vein (lockId 38 — one Mining
        // slot, Skill 0, Action 0) on an unflagged, READY object still opens for a miner.
        let mining = SpellDisplay {
            open_lock: Some(OpenLock {
                lock_type: 3,
                effect: 0,
            }),
            effect_base_points: [-1, 0, 0],
            effect_base_dice: [1, 0, 0],
            effect_real_points_per_level: [5.0, 0.0, 0.0],
            ..Default::default()
        };
        let with_mining = crate::ui_action::Spells {
            catalog: SpellCatalog::from_displays([(2575, mining)].into_iter().collect()),
            ..crate::ui_action::Spells::empty_for_tests()
        };
        let mut vein = [LockSlot::default(); 8];
        vein[0] = skill_slot(3, 0, 0);
        assert_eq!(
            resolve_lock(
                &vein,
                &BTreeSet::from([2575]),
                Some(&with_mining),
                None,
                None,
                &objects,
                GoFacts {
                    state: GO_STATE_READY,
                    flag_locked: false,
                    level: 0
                },
                &mut None,
            ),
            LockOutcome::OpenBySpell(2575),
        );
    }

    /// **B247**: two known openers on one LockType, both sufficient — the LOWEST
    /// spell id wins, deterministically, because that is the reference array's order and the scan
    /// returns on its first sufficient match.
    ///
    /// The real values: lock 43 (Hyacinth Mushroom and every other ground container) carries one
    /// SKILL slot, `LockType 13` "Open Kneeling", `Skill 0`, `Action 0`. Three shipped spells carry
    /// `SPELL_EFFECT_OPEN_LOCK` on type 13 — 6478 "Opening", 17667 "Light On Fire", and 22810
    /// **"Opening - No Text"** — and `playercreateinfo_spell` grants 6478 *and* 22810 to every
    /// race/class. Whichever this scan returns is the spell the click casts, so its Spell.dbc name
    /// is what the cast bar prints: with a `HashSet` here the bar read Blizzard's placeholder about
    /// half the time.
    #[test]
    fn two_sufficient_openers_pick_the_lower_spell_id() {
        use benilla_formats::{OpenLock, SpellCatalog, SpellDisplay};

        let mut mushroom = [LockSlot::default(); 8];
        mushroom[0] = skill_slot(13, 0, 0);

        // Both "Opening" spells: OPEN_LOCK on type 13, flat value 100 (base 99 + dice 1), which
        // clears the slot's `Skill 0`. Identical in every input the resolver reads — the pick is
        // decided by visit order and nothing else, which is the whole point.
        let kneeling_opener = || SpellDisplay {
            open_lock: Some(OpenLock {
                lock_type: 13,
                effect: 0,
            }),
            effect_base_points: [99, 0, 0],
            effect_base_dice: [1, 0, 0],
            ..Default::default()
        };
        let spells = crate::ui_action::Spells {
            catalog: SpellCatalog::from_displays(
                [(6478, kneeling_opener()), (22810, kneeling_opener())]
                    .into_iter()
                    .collect(),
            ),
            ..crate::ui_action::Spells::empty_for_tests()
        };
        // Nothing streamed: no case here holds a key, and the key branch is only reached
        // with a store, which these pass as `None` (2334).
        let mut objs = crate::ui_items::TestObjects::new();
        let objects = objs.get();
        let unlocked = GoFacts {
            state: GO_STATE_READY,
            flag_locked: false,
            level: 0,
        };

        let mut matched = None;
        assert_eq!(
            resolve_lock(
                &mushroom,
                &BTreeSet::from([6478, 22810]),
                Some(&spells),
                None,
                None,
                &objects,
                unlocked,
                &mut matched,
            ),
            LockOutcome::OpenBySpell(6478),
            "6478 \"Opening\", never 22810 \"Opening - No Text\""
        );
        assert_eq!(matched, Some(6478), "the toast's out-param agrees");

        // Insertion order cannot change it — the set is ordered, so the scan is too.
        let mut matched = None;
        assert_eq!(
            resolve_lock(
                &mushroom,
                &BTreeSet::from([22810, 6478]),
                Some(&spells),
                None,
                None,
                &objects,
                unlocked,
                &mut matched,
            ),
            LockOutcome::OpenBySpell(6478),
        );
    }

    /// An all-empty row is not a lock at all — `CMSG_GAMEOBJ_USE`.
    #[test]
    fn an_empty_row_is_unlocked() {
        let slots = [LockSlot::default(); 8];
        // Nothing streamed: no case here holds a key, and the key branch is only reached
        // with a store, which these pass as `None` (2334).
        let mut objs = crate::ui_items::TestObjects::new();
        let objects = objs.get();
        let mut matched = None;
        assert_eq!(
            resolve_lock(
                &slots,
                &BTreeSet::new(),
                None,
                None,
                None,
                &objects,
                GoFacts {
                    state: GO_STATE_ACTIVE,
                    flag_locked: false,
                    level: 0
                },
                &mut matched,
            ),
            LockOutcome::Unlocked
        );
        assert_eq!(slots[0].key_type, LOCK_KEY_NONE);
    }
}
