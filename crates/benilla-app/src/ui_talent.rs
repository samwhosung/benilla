//! The talent window feed: the class's pages from `Talent.dbc` and `TalentTab.dbc`, the known
//! spells and `PLAYER_CHARACTER_POINTS1/2`, and `LearnTalent` clicks as `CMSG_LEARN_TALENT`. A
//! talent's rank is its highest known rank spell (learning grants every lower rank, vmangos
//! `Player::LearnTalent`); an unlearned one wears rank 1's name and icon.
//!
//! The red requirement lines are the three keys `0x52b390` emits, in its order:
//! `TOOLTIP_TALENT_TIER_POINTS` (`0x854a40`), `TOOLTIP_TALENT_PREREQ[_P1]` (`0x854a5c`) and
//! `ITEM_REQ_SKILL` (`0x84e338`), resolved off the install's `GlobalStrings.lua`; a key it lacks
//! renders no line.

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

/// The talent catalog (`Talent.dbc`, `TalentTab.dbc`), absent when the client data failed to load.
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
                    // Feed before the input pass, so an open this frame sees a populated
                    // window; drain after it, so a click sends the same frame.
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

/// The last pushed snapshot, and the points pair `CHARACTER_POINTS_CHANGED`'s deltas come from.
#[derive(Default)]
struct FeedMemory {
    pushed: TalentUiState,
    /// `None` until the first read, which gives zero deltas.
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
    // Reopened by every input the build reads: the known spells, both catalogs (their landing
    // counts as a change) and the self descriptor.
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
    // Scoped: the `GlobalStrings.lua` lookup borrows the VM, which the push needs mutably.
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
        // The talent and profession point deltas, as the reference's `SignalEvent2` sends them:
        // `ChatFrame.lua:1326` tests `arg2 > 0` unguarded, so an argless fire raises.
        // `memory.points` still holds the previous pair here.
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
    // The free-professions line must not be composed here: the stock `ChatFrame_OnEvent` arm
    // (`ChatFrame.lua:1324-1334`) prints `LEVEL_UP_SKILL_POINTS` itself on `arg2 > 0`.
    if memory.points != Some(points) {
        gate.audit("feed_talents", "the points memo");
        memory.points = Some(points);
    }
}

/// A talent's rank: the highest rank whose spell is known.
fn rank_of(t: &Talent, known: &BTreeSet<u32>) -> u32 {
    t.ranks
        .iter()
        .enumerate()
        .filter(|(_, &s)| s != 0 && known.contains(&s))
        .map(|(i, _)| i as u32 + 1)
        .max()
        .unwrap_or(0)
}

/// Builds the pushed snapshot.
pub(crate) fn build_pages(
    catalog: &TalentCatalog,
    known: &BTreeSet<u32>,
    // `Spell.dbc` alone, for names and icons, so the addon-corpus survey can build the snapshot
    // without the other spell catalogs.
    spells: &benilla_formats::SpellCatalog,
    race: u8,
    class: u8,
    points: (u32, u32),
    // The VM's `GlobalStrings.lua`, for the red requirement lines.
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
            // `meetsPrereq` is the `requiredSpell` check alone (`GetTalentInfo` `0x4f3200`).
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
            // The item tooltip's `ITEM_REQ_SKILL`, not the `LOCKED_WITH_*` or `SPELL_FAILED_*`
            // keys that read the same in enUS.
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
            // The green hint is points left and not maxed alone (`SetTalent` `0x535170`); the
            // frame's gold, green and gray come from the stock Lua.
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

/// Drains `LearnTalent(tab, index)` clicks into `CMSG_LEARN_TALENT` for the current rank count,
/// the 0-based next rank, which vmangos learns up to.
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
        // Not at max is the client's only send gate (`LearnTalent` `0x4f36a0`); points and
        // prereqs are the server's to enforce.
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
        // Learning up to a rank leaves no gap on the wire, but a gap still reads by the max.
        let holey: BTreeSet<u32> = [102].into_iter().collect();
        assert_eq!(rank_of(&t, &holey), 3);
    }

    /// The rank reads the known spells alone, so a wipe shows once the `SMSG_REMOVED_SPELL` burst
    /// empties them; the points are a descriptor field and arrive on their own.
    #[test]
    fn an_emptied_book_reads_as_a_wiped_tree() {
        let t = talent(1, 201, 0, 0, &[14522, 14788, 14789]);
        let spent: BTreeSet<u32> = [14522, 14788, 14789].into_iter().collect();
        assert_eq!(rank_of(&t, &spent), 3);
        assert_eq!(rank_of(&t, &BTreeSet::new()), 0);
    }
}
