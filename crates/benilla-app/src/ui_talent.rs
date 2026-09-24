//! The talent window feed (decision 0304) — the app half of the `benilla_ui::script::talent`
//! seam, the `ui_spellbook` shape: build the class's talent pages from the `Talent.dbc` ×
//! `TalentTab.dbc` catalog joined with the known-spell set (`PlayerActions.spells`) and the
//! self descriptor's `PLAYER_CHARACTER_POINTS1/2`, push the snapshot, fire the refresh events,
//! and drain `LearnTalent` clicks into `CMSG_LEARN_TALENT`.
//!
//! The resolves the engine trusts verbatim:
//! - **rank** = the highest rank whose spell is known (learn-up-to grants every lower rank —
//!   vmangos `Player::LearnTalent`).
//! - **name/icon** ride the display rank's spell (`Spell.dbc`), max(rank, 1) — an unlearned
//!   talent wears rank 1's face, the reference's own look.
//! - **`learnable`** (the tooltip's green "Click to learn" hint) = unspent points > 0 ∧
//!   `rank < maxRank` — the hint's own gate (SetTalent `0x535170`); the frame's gold/green/gray
//!   availability compose Lua-side from the transcribed reference (tier gate, prereq triplets,
//!   meetsPrereq). The learn SEND gates on not-at-max only (LearnTalent `0x4f36a0`) — the server
//!   enforces the rest.
//! - **tooltip req lines** (red, shown while locked): `TOOLTIP_TALENT_TIER_POINTS` (`0x854a40`)
//!   for a short tab total, `TOOLTIP_TALENT_PREREQ[_P1]` (`0x854a5c`) per unmet prereq slot, and
//!   `ITEM_REQ_SKILL` (`0x84e338`, reused from the item tooltip) for an unmet `required_spell` —
//!   the three keys `0x52b390` emits, in the reference's own order. **Keys, resolved off
//!   the player's own `GlobalStrings.lua` at the feed** (decision 2045), never sentences written
//!   here; a key the install does not carry renders no line, which is the reference's own
//!   data-suppression face (the desaturation still communicates the lock).
//! - The tooltip's spell parts (display + next rank) ride `ui_tooltip`'s spell channel, which
//!   pre-feeds every rank spell of the class's pages at arrival — a `SetTalent` hover hits on
//!   the FIRST enter (the reference's all-local instancy); its ask-once miss path stays as the
//!   odd-case fallback.
//!
//! The free-professions chat line (`CHARACTER_POINTS_CHANGED` → `LEVEL_UP_SKILL_POINTS[_P1]`,
//! the reference ChatFrame.lua:1326-1337) is composed here on a cp2 rise: this feed owns the
//! points diff, and the chat arc's own kinds carry it as a SYSTEM line. The token is **not** a
//! message-catalog row — the reference resolves it in Lua and calls `AddMessage` itself — so it
//! rides no keyed queue and carries no surface or sound of its own (decision 2054's `/ginfo`
//! shape); only the wording comes off the install.

use std::collections::BTreeSet;

use bevy::prelude::*;

use benilla_formats::{Talent, TalentCatalog};
use benilla_ui::script::{
    ScriptValue, TalentPrereqView, TalentTabView, TalentUiState, TalentView, UiScript,
};
use benilla_ui::strings::{fill, Arg};

use crate::net::{ClientCommand, NetCommands, ObjectStore, SelfPlayer};
use crate::ui_action::{PlayerActions, Spells};
use crate::ui_script::{gate, UiInput};
use crate::ui_unit::UnitFeed;
use benilla_assets::{AssetSet, LockRecover, WorldAssets};

/// The talent catalog (`Talent.dbc` × `TalentTab.dbc`) — absent when the client data failed to
/// load (every consumer tolerates that; the `SkillLines` precedent).
#[derive(Resource)]
pub(crate) struct Talents {
    pub(crate) catalog: TalentCatalog,
}

pub(crate) struct UiTalentPlugin;

impl Plugin for UiTalentPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, load_talents.after(AssetSet::Open))
            .add_systems(
                Update,
                (
                    // Feed before UiInput (an N-key open this frame sees a populated window);
                    // the learn drain after (a click's wire goes out the same frame).
                    feed_talents.in_set(UnitFeed),
                    drain_talent_learns.after(UiInput),
                ),
            );
    }
}

fn load_talents(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_talent_catalog(&mut chain)
    };
    match loaded {
        Ok(catalog) => {
            info!("ui_talent: {} talent(s) in the catalog", catalog.len());
            commands.insert_resource(Talents { catalog });
        }
        Err(e) => warn!("ui_talent: Talent.dbc failed to load — no talent window: {e:#}"),
    }
}

/// The feed's memory: the last pushed snapshot (the refresh diff) and the last points pair
/// (the CHARACTER_POINTS_CHANGED edge + the professions chat line).
#[derive(Default)]
struct FeedMemory {
    pushed: TalentUiState,
    /// `None` until the first descriptor read lands (a login's initial value must not chat).
    points: Option<(u32, u32)>,
}

fn feed_talents(
    script: Option<NonSendMut<UiScript>>,
    talents: Option<Res<Talents>>,
    actions: Res<PlayerActions>,
    spells: Option<Res<Spells>>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    changed_self: Query<(), (With<SelfPlayer>, Changed<ObjectStore>)>,
    mut memory: Local<crate::ui_script::VmMemo<FeedMemory>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let (memory, vm_reset) = memory.get_reset(&script);
    // The gate (1439): every input the build below reads — the known-spell set, the two
    // catalogs (`Added ⊆ Changed` covers their one-time landing), and the self descriptor
    // (points, race/class; its ABSENCE needs no reopen — the body returns pushless without it).
    let self_changed = !changed_self.is_empty();
    let actions_changed = actions.is_changed();
    let talents_changed = talents.as_ref().is_some_and(|r| r.is_changed());
    let spells_changed = spells.as_ref().is_some_and(|r| r.is_changed());
    gate::trace(
        "feed_talents",
        &[
            ("vm_reset", vm_reset),
            ("self", self_changed),
            ("actions", actions_changed),
            ("talents", talents_changed),
            ("spells", spells_changed),
        ],
    );
    let gate = gate::Gate::new(
        vm_reset || self_changed || actions_changed || talents_changed || spells_changed,
    );
    if gate.skip() {
        return;
    }
    let (Some(talents), Some(spells)) = (talents.as_deref(), spells.as_deref()) else {
        return;
    };
    let Ok(store) = self_q.single() else {
        return;
    };
    let race = store.0.unit_race().unwrap_or(0);
    let class = store.0.unit_class().unwrap_or(0);
    let points = (
        store.0.player_talent_points().unwrap_or(0),
        store.0.player_free_professions().unwrap_or(0),
    );
    // The reference strings the pages carry are read off the player's own `GlobalStrings.lua`
    // (2045). Scoped, because the lookup borrows the VM and the push below needs it mutably.
    let fresh = {
        let get = |key: &str| {
            script
                .lua()
                .globals()
                .get::<String>(key)
                .ok()
                .filter(|t| !t.is_empty())
        };
        build_pages(
            &talents.catalog,
            &actions.spells,
            &spells.catalog,
            race,
            class,
            points,
            &get,
        )
    };
    if fresh != memory.pushed {
        gate.audit("feed_talents", "the talent pages");
        debug!(
            "ui_talent: fed {} tab(s), {} points",
            fresh.tabs.len(),
            points.0
        );
        script.set_talents(fresh.clone());
        memory.pushed = fresh;
        // `%d%d` — the talent-point and profession-point DELTAS, per the reference's own fire
        // site (SignalEvent2, decision 1884). `ChatFrame.lua:1326` opens its branch with
        // `if ( arg2 > 0 )`, unguarded, so an argless fire is not merely ignored there: it
        // compares nil with a number and raises. `memory.points` still holds the previous pair
        // here — it is updated at the end of this function — and a first observation seeds at
        // zero, the same rule the professions line below uses.
        let (talent_delta, profession_delta) = match memory.points {
            Some((old_talent, old_professions)) => (
                i64::from(points.0) - i64::from(old_talent),
                i64::from(points.1) - i64::from(old_professions),
            ),
            None => (0, 0),
        };
        script.fire_event(
            "CHARACTER_POINTS_CHANGED",
            vec![
                ScriptValue::Int(talent_delta),
                ScriptValue::Int(profession_delta),
            ],
        );
    }
    // **The professions line is not composed here, and must not be.** The event fired above IS
    // the line: stock `ChatFrame_OnEvent`'s `CHARACTER_POINTS_CHANGED` arm (`ChatFrame.lua`
    // l.1324-1334) reads `UnitCharacterPoints("player")` on `arg2 > 0` and prints
    // `GetText("LEVEL_UP_SKILL_POINTS", nil, cp2)` itself, and `benilla.toc` sources that file
    // off the player's own chain (1948). This feed used to print a second copy beside it —
    // `ui_chat::tests::the_free_professions_line_is_printed_once` is what asks the window rather
    // than reasoning about it, and it was the ding block's twin (see that test's neighbour).
    if memory.points != Some(points) {
        gate.audit("feed_talents", "the points memo");
        memory.points = Some(points);
    }
}

/// One talent's current rank: the highest rank whose spell is known (module doc).
fn rank_of(t: &Talent, known: &BTreeSet<u32>) -> u32 {
    t.ranks
        .iter()
        .enumerate()
        .filter(|(_, &s)| s != 0 && known.contains(&s))
        .map(|(i, _)| i as u32 + 1)
        .max()
        .unwrap_or(0)
}

/// Build the pushed snapshot — the app's whole resolve (module doc).
pub(crate) fn build_pages(
    catalog: &TalentCatalog,
    known: &BTreeSet<u32>,
    // The DISPLAY catalog only — `Spell.dbc` for each talent's name and icon. Narrowed from the
    // whole `Spells` resource (2167) so the addon-corpus survey can build the same snapshot off
    // the player's chain without standing up five more DBCs it has no use for.
    spells: &benilla_formats::SpellCatalog,
    race: u8,
    class: u8,
    points: (u32, u32),
    // The VM's own `GlobalStrings.lua`, for the three red requirement lines — `None` for a key
    // the install does not carry, which renders no line (decision 2045).
    get: &dyn Fn(&str) -> Option<String>,
) -> TalentUiState {
    let mut tabs = Vec::new();
    let mut pages = Vec::new();
    for tab in catalog.tabs_for_class(race, class) {
        let list = catalog.talents_in_tab(tab.id);
        let spent: u32 = list.iter().map(|t| rank_of(t, known)).sum();
        let mut views = Vec::with_capacity(list.len());
        for t in list {
            let rank = rank_of(t, known);
            let max_rank = t.max_rank();
            let display_spell = t.ranks[rank.max(1) as usize - 1];
            let next_spell = if rank > 0 && rank < max_rank {
                t.ranks[rank as usize]
            } else {
                0
            };
            let d = spells.get(display_spell);
            // meetsPrereq is the requiredSpell known-check ONLY (GetTalentInfo `0x4f3200` — the
            // 0305 fold-back; talent prereqs live in the triplets).
            let meets_prereq = t.required_spell == 0 || known.contains(&t.required_spell);
            // The tier gate reads the tab's own spent sum; prereqs read the prereq's rank.
            let tier_unlocked = t.row * 5 <= spent;
            let mut req_lines = Vec::new();
            if !tier_unlocked {
                req_lines.extend(
                    get("TOOLTIP_TALENT_TIER_POINTS")
                        .map(|f| fill(&f, &[Arg::D(i64::from(t.row * 5)), Arg::S(&tab.name)])),
                );
            }
            // The requiredSpell line rides ITEM_REQ_SKILL (`0x84e338`, the item tooltip's own
            // key reused) — emitted by the SetTalent requirement builder `0x52b390`. It is NOT the
            // `LOCKED_WITH_*`/`SPELL_FAILED_*` family, which reads identically in enUS.
            if !meets_prereq {
                if let Some(req) = spells.get(t.required_spell) {
                    req_lines.extend(get("ITEM_REQ_SKILL").map(|f| fill(&f, &[Arg::S(&req.name)])));
                }
            }
            let mut prereqs = Vec::new();
            if t.prereq_talent != 0 {
                if let Some(p) = catalog.talent(t.tab, t.prereq_talent) {
                    let need = t.prereq_rank + 1;
                    let learnable = rank_of(p, known) >= need;
                    prereqs.push(TalentPrereqView {
                        tier: p.row + 1,
                        column: p.col + 1,
                        learnable,
                    });
                    if !learnable {
                        let p_name = spells
                            .get(p.ranks[0])
                            .map(|d| d.name.clone())
                            .unwrap_or_default();
                        req_lines.extend(
                            benilla_ui::strings::plural("TOOLTIP_TALENT_PREREQ", Some(need), get)
                                .map(|f| fill(&f, &[Arg::D(i64::from(need)), Arg::S(&p_name)])),
                        );
                    }
                }
            }
            // The green learn hint's own gate is points-available && not-maxed ONLY
            // (SetTalent `0x535170`) — the frame's gold/green/gray
            // availability law stays the transcribed Lua's (tier/prereq/meetsPrereq).
            let learnable = rank < max_rank && points.0 > 0;
            views.push(TalentView {
                name: d.map(|d| d.name.clone()).unwrap_or_default(),
                texture: d.and_then(|d| d.icon.clone()),
                tier: t.row + 1,
                column: t.col + 1,
                rank,
                max_rank,
                exceptional: t.exceptional,
                meets_prereq,
                prereqs,
                display_spell,
                next_spell,
                req_lines,
                learnable,
            });
        }
        tabs.push(TalentTabView {
            name: tab.name.clone(),
            background: tab.background.clone(),
            points_spent: spent,
        });
        pages.push(views);
    }
    TalentUiState {
        tabs,
        talents: pages,
        points,
    }
}

/// Drain the window's `LearnTalent(tab, index)` clicks into `CMSG_LEARN_TALENT` — requested
/// rank = the current rank count (the 0-based next rank, vmangos's learn-up-to semantics). The
/// gate mirrors the pushed `learnable` (the server re-validates regardless).
fn drain_talent_learns(
    script: Option<NonSendMut<UiScript>>,
    talents: Option<Res<Talents>>,
    actions: Res<PlayerActions>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    commands: Res<NetCommands>,
) {
    let Some(mut script) = script else {
        return;
    };
    let clicks = script.take_talent_learns();
    if clicks.is_empty() {
        return;
    }
    let Some(talents) = talents.as_deref() else {
        return;
    };
    let Ok(store) = self_q.single() else {
        return;
    };
    let race = store.0.unit_race().unwrap_or(0);
    let class = store.0.unit_class().unwrap_or(0);
    let tabs = talents.catalog.tabs_for_class(race, class);
    for (tab, index) in clicks {
        // 1-based Lua-facing pair; a 0 from a stray script is a miss, never an underflow.
        let Some(t) = (tab as usize)
            .checked_sub(1)
            .and_then(|i| tabs.get(i))
            .and_then(|tab| {
                (index as usize)
                    .checked_sub(1)
                    .and_then(|i| talents.catalog.talents_in_tab(tab.id).get(i))
            })
        else {
            continue;
        };
        let rank = rank_of(t, &actions.spells);
        // The client's ONLY send gate is not-at-max (byte-verified LearnTalent 0x4f36a0 — the
        // 0305 fold-back); points/prereqs are the server's to enforce.
        if rank >= t.max_rank() {
            continue;
        }
        debug!("ui_talent: learn talent {} rank {}", t.id, rank);
        let _ = commands.0.send(ClientCommand::LearnTalent {
            talent_id: t.id,
            rank,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_formats::MAX_TALENT_RANK;

    fn talent(id: u32, tab: u32, row: u32, col: u32, ranks: &[u32]) -> Talent {
        let mut r = [0u32; MAX_TALENT_RANK];
        r[..ranks.len()].copy_from_slice(ranks);
        Talent {
            id,
            tab,
            row,
            col,
            ranks: r,
            prereq_talent: 0,
            prereq_rank: 0,
            required_spell: 0,
            exceptional: false,
        }
    }

    #[test]
    fn rank_is_the_highest_known_rank_spell() {
        let t = talent(1, 81, 0, 0, &[100, 101, 102]);
        let known: BTreeSet<u32> = [100, 101].into_iter().collect();
        assert_eq!(rank_of(&t, &known), 2);
        assert_eq!(rank_of(&t, &BTreeSet::new()), 0);
        // A gap never happens on the live wire (learn-up-to), but the max() read stays honest.
        let holey: BTreeSet<u32> = [102].into_iter().collect();
        assert_eq!(rank_of(&t, &holey), 3);
    }

    /// **The respec, at the seam that made it look like it hadn't happened** (decision 1584). The
    /// window's rank is a read of the known-spell set and nothing else, so a wipe only shows here
    /// once the server's `SMSG_REMOVED_SPELL` burst has emptied it — one packet per rank, all of
    /// them dropped before 1584. That is why the director's screenshot could show a 20-point
    /// Discipline tree beside "Talent Points: 51": the points are a descriptor field and arrived,
    /// the ranks are this set and did not. The arm that empties it has its own test beside it
    /// (`spell::net::tests::a_removal_shrinks_the_book_and_dirties_the_feeds`).
    #[test]
    fn an_emptied_book_reads_as_a_wiped_tree() {
        let t = talent(1, 201, 0, 0, &[14522, 14788, 14789]);
        let spent: BTreeSet<u32> = [14522, 14788, 14789].into_iter().collect();
        assert_eq!(rank_of(&t, &spent), 3);
        assert_eq!(rank_of(&t, &BTreeSet::new()), 0);
    }
}
