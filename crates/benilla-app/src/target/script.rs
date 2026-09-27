//! The selection calls of the script call stream ([`crate::script_calls`]), each committed through
//! the one SetSelection path when its turn comes, so a later call in the same pass reads it.

use bevy::prelude::*;

use benilla_ui::script::SelectionRequest;

use super::by_name::{self, ByNameScan, SelectCommit};
use super::scan;

/// Everything the selection calls read and write.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct ScriptSelect<'w, 's> {
    /// The one unit-token resolver, shared with the reach feed so `TargetUnit("target")` and
    /// `CheckInteractDistance("target", …)` name the same unit.
    tokens: crate::ui_unit::UnitTokens<'w, 's>,
    /// `TargetLastEnemy`'s memory (`[0xb4e2e8]`), stamped by `scan::remember_last_enemy`.
    last_enemy: Res<'w, scan::LastEnemy>,
    /// The SetSelection tail shared with the world click, `/target` and `/assist`, classification
    /// included.
    commit: SelectCommit<'w, 's>,
    by_name: ByNameScan<'w, 's>,
    /// The TAB cycler's inputs, for `TargetNearestFriend`.
    scan: scan::TargetScan<'w, 's>,
    history: ResMut<'w, scan::TabHistory>,
    time: Res<'w, Time>,
}

impl ScriptSelect<'_, '_> {
    /// `TargetUnit` (`0x4899d0`), `AssistUnit` (`0x489b80`) and `TargetLastEnemy` share the
    /// reference's helper `0x489a40`: a resolved unit or a roster member is committed, anything
    /// else is a no-op, never a deselect. Here a token must name a streamed unit, so an
    /// out-of-range roster member is a no-op (the guid-only selection is not built).
    pub(crate) fn select(&mut self, request: SelectionRequest) {
        match request {
            SelectionRequest::Unit(token) => {
                if let Some((entity, guid)) = self.tokens.resolve(&token, &self.commit.selection) {
                    self.commit.commit(entity, guid);
                }
            }
            // `0x489ba9 call 0x515940(token)`, then the shared tail. An unresolvable token is
            // silent here; the reference shows message `0xb8` `ERR_GENERIC_NO_TARGET` (`0x489c0e`).
            SelectionRequest::Assist(token) => {
                match self.tokens.resolve(&token, &self.commit.selection) {
                    Some((basis, _)) => self.commit.assist(basis, "AssistUnit"),
                    None => info!("assist (AssistUnit): \"{token}\" names nothing; silent no-op"),
                }
            }
            // `0x489c40`, `/assist <name>`: a player name, prefix-matched by the app's name scan.
            SelectionRequest::AssistByName(name) => {
                by_name::assist_named(&self.by_name, &mut self.commit, Some(&name), "AssistByName");
            }
            // `0x489b45` → `0x489a40`. Empty memory is a no-op (the shim `0x489b40` tests nothing),
            // unlike `TargetLastTarget 0x489b00`, which reaches `0x493540(0,0)` and deselects.
            SelectionRequest::LastEnemy => {
                let Some(guid) = self.last_enemy.0 else {
                    info!("TargetLastEnemy: nothing hostile has been targeted yet; no-op");
                    return;
                };
                match self.tokens.held(guid) {
                    Some((entity, guid)) => self.commit.commit(entity, guid),
                    // Stale: a no-op, and the memory stays, as the reference never clears it.
                    None => info!(
                        "TargetLastEnemy: {guid:#x} is no longer streamed; target left untouched"
                    ),
                }
            }
        }
    }

    /// `TargetByName(name, exactMatch)` (`0x489d60`), the stock `/target`.
    pub(crate) fn target_by_name(&mut self, name: &str, exact: bool) {
        by_name::target_named(&self.by_name, &mut self.commit, name, exact);
    }

    /// `TargetNearestFriend([reverse])` (`0x489aa0` → `0x493f60(reverse, 2)`), one cycle.
    pub(crate) fn target_nearest_friend(&mut self, reverse: bool) {
        let engaged = self.commit.engaged();
        scan::cycle(
            scan::ScanSide::Friend,
            reverse,
            self.time.elapsed_secs_f64(),
            &self.scan,
            &mut self.history,
            &mut self.commit.selection,
            &mut self.commit.seam,
            engaged,
        );
    }

    /// `ClearTarget()` (`0x489ff0`): deselect whatever the calls before it left selected.
    pub(crate) fn clear_target(&mut self) {
        self.commit.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The selection calls from Lua, through the binding bodies of ASSISTTARGET (`AssistUnit`) and
    /// TARGETLASTHOSTILE (`TargetLastEnemy`): an empty basis, a garbage token and a stale memory
    /// are each a no-op (`0x489a40`'s bare `ret`), never a deselect or a panic.
    #[test]
    fn the_selection_calls_run_assist_and_last_enemy_without_ever_deselecting() {
        use crate::net::{Guid, NetCommands, ObjectStore, SelfPlayer};
        use benilla_ui::script::UiScript;
        use bevy::ecs::system::RunSystemOnce;

        const ME: u64 = 1;
        const BASIS: u64 = 0xB0A2;
        const VICTIM: u64 = 0xC0DE;
        const GONE: u64 = 0x6017;
        // `UNIT_FIELD_TARGET` is a 2-field guid at index 16; HEALTH/MAXHEALTH keep the units live.
        let store =
            |pairs: &[(u16, u32)]| ObjectStore(benilla_protocol::ObjectFields::from_pairs(pairs));

        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut world = World::new();
        world.insert_resource(NetCommands(tx));
        world.init_resource::<crate::spell::QueuedMeleeSpell>();
        world.init_resource::<crate::spell::AutoRepeatActive>();
        world.init_resource::<crate::ui_loot::LootState>();
        world.init_resource::<crate::ui_loot::LootLatch>();
        world.init_resource::<Messages<crate::creature_anim::SheathRequest>>();
        world.init_resource::<crate::ui_party::GroupState>();
        world.init_resource::<crate::net::GuidIndex>();
        world.init_resource::<crate::net::Reputations>();
        world.init_resource::<super::super::AssistAttack>();
        world.init_resource::<super::super::Selection>();
        world.init_resource::<scan::LastEnemy>();
        world.init_resource::<crate::names::NameCache>();
        world.init_resource::<scan::TabHistory>();
        world.init_resource::<Time>();
        world.insert_non_send_resource(UiScript::new().expect("a bare VM"));

        world.spawn((SelfPlayer, Guid(ME), store(&[(22, 100), (28, 100)])));
        // The basis is pointing at the victim; the victim points at nobody.
        let basis = world
            .spawn((
                Guid(BASIS),
                store(&[(22, 100), (28, 100), (16, VICTIM as u32)]),
            ))
            .id();
        let victim = world
            .spawn((Guid(VICTIM), store(&[(22, 100), (28, 100)])))
            .id();
        let index = &mut world.resource_mut::<crate::net::GuidIndex>().0;
        index.insert(BASIS, basis);
        index.insert(VICTIM, victim);

        let run = |world: &mut World, lua: &str| {
            world
                .non_send_resource_mut::<UiScript>()
                .eval::<()>(lua)
                .expect("the binding runs");
            world
                .run_system_once(
                    |mut script: NonSendMut<UiScript>, mut select: ScriptSelect| {
                        for request in script.take_selection_requests() {
                            select.select(request);
                        }
                    },
                )
                .expect("the applier runs as a one-shot system");
            world.resource::<super::super::Selection>().guid
        };
        let set = |world: &mut World, target: Option<(Entity, u64)>| {
            let mut sel = world.resource_mut::<super::super::Selection>();
            sel.target = target.map(|(e, _)| e);
            sel.guid = target.map(|(_, g)| g);
        };

        // Assist the current target: one hop onto its target.
        set(&mut world, Some((basis, BASIS)));
        assert_eq!(run(&mut world, r#"AssistUnit("target")"#), Some(VICTIM));

        // The victim targets nobody: assisting it is a no-op, not a deselect.
        assert_eq!(run(&mut world, r#"AssistUnit("target")"#), Some(VICTIM));

        assert_eq!(run(&mut world, r#"AssistUnit("nosuchunit")"#), Some(VICTIM));
        assert_eq!(run(&mut world, r#"TargetUnit("nosuchunit")"#), Some(VICTIM));

        // TargetLastEnemy with nothing remembered: also a no-op.
        assert_eq!(run(&mut world, "TargetLastEnemy()"), Some(VICTIM));

        world.resource_mut::<scan::LastEnemy>().0 = Some(VICTIM);
        set(&mut world, None);
        assert_eq!(run(&mut world, "TargetLastEnemy()"), Some(VICTIM));

        // A stale memory (the unit despawned, so its guid is in no index) leaves the target alone.
        world.resource_mut::<scan::LastEnemy>().0 = Some(GONE);
        assert_eq!(run(&mut world, "TargetLastEnemy()"), Some(VICTIM));
        set(&mut world, None);
        assert_eq!(run(&mut world, "TargetLastEnemy()"), None);
    }
}
