//! The script memory watch, the head of the in-game interface's per-frame callback (`0x495590`,
//! `0x495599`-`0x495656`). While a world is entered (`[0xb4b424]`) and `scriptMemory` (KB) is
//! above zero, a Lua state counted over it is collected in full (`FrameScriptCollectGarbage(1)`,
//! `0x4955d5`) and counted again (`lua_getgccount 0x6f43f0`); still over, the first such frame
//! signals `MEMORY_EXHAUSTED` with the limit in MB (`0x49563d`), whose stock popup offers only
//! `QUIT_NOW` (`StaticPopup.lua`, `UIParent.lua`), and arms a deadline 15 s out (`0x49560e`). A
//! frame still over at the deadline quits the client (`0x401560(0)` at `0x495602`), as
//! `ForceQuit` does. A frame back within the limit clears the deadline and signals
//! `MEMORY_RECOVERED` (`0x495656`), which hides the popup. The count is the whole state's, stock
//! interface and every addon alike: the one reader of the CVar's record (`0x882654`) besides the
//! glue's two verbs is this one.
//!
//! Deviation: the count is benilla-lua's collector estimate ([`mlua::Lua::used_memory`]), which
//! sizes objects as a 64-bit 5.1 build lays them out, not 5.0's `nblocks` (`[G+0x28]`) of the
//! 32-bit client's allocations, so the same interface reads heavier here. A string is 32 bytes and
//! its length against 17 and its length (`0x6f9da1`), a table head 64 against 32 (`0x6fa500`), a
//! Lua closure 40 and 8 an upvalue against 36 and 4 (`0x6f9ebf`), a C closure 40 and 16 an upvalue
//! against 16 and 16 (`0x6f9e7c`), and the string table 8 bytes a slot, at most half full,
//! against 4 a bucket (`0x6f9c62`); an array slot's 16 bytes (`0x6fa579`) and a hash node's 40
//! (`0x6fa613`) are the same. The stock interface counts some 13 MB here, of which those
//! differences make some 3 MB.

use benilla_ui::script::{ScriptValue, UiScript};
use bevy::prelude::*;

use crate::ui_script::LeavingWorldArmed;

/// The CVar the watch reads, `[0x882654]`'s record: the limit in KB, as its integer (`+0x28`).
const CVAR_SCRIPT_MEMORY: &str = "scriptMemory";
/// From the first frame over to the quit (`add eax,0x3a98`, `0x49560e`).
const GRACE_MS: u32 = 15_000;

/// The deadline word (`0xb4e3e8`, 0 unarmed). Only the watch writes it, so it outlives a reload
/// and a logout: a world entered with it armed and past quits on its first frame over.
#[derive(Resource, Default, Debug, PartialEq, Eq)]
pub(crate) struct ScriptMemoryWatch {
    deadline: u32,
}

/// What one frame of the watch does past its collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MemoryVerdict {
    /// `MEMORY_EXHAUSTED` with the limit in MB, `%d` of `limit >> 10` (`0x495624`-`0x49563d`).
    Exhausted(i32),
    /// `MEMORY_RECOVERED` (`0x495656`).
    Recovered,
    /// `0x401560(0)`: the deadline passed with the state still over.
    Quit,
}

/// The two Lua state calls the watch makes.
pub(crate) trait ScriptHeap {
    /// `lua_getgccount` (`0x6f43f0`): the counted bytes `>> 10`.
    fn count_kb(&mut self) -> i32;
    /// `FrameScriptCollectGarbage(1)` (`0x703bc0`): a full collection.
    fn collect(&mut self);
}

impl ScriptHeap for UiScript {
    fn count_kb(&mut self) -> i32 {
        i32::try_from(self.lua().used_memory() >> 10).unwrap_or(i32::MAX)
    }

    fn collect(&mut self) {
        let _ = self.lua().gc_collect();
    }
}

impl ScriptMemoryWatch {
    /// One frame in the world with the limit `limit_kb` at `now` on the millisecond clock. Both
    /// compares are signed (`jle`), so a state exactly at the limit is within it, and a limit of
    /// 0 or below turns the watch off without touching the deadline (`0x4955ba`).
    pub(crate) fn poll(
        &mut self,
        limit_kb: i32,
        now: u32,
        heap: &mut impl ScriptHeap,
    ) -> Option<MemoryVerdict> {
        if limit_kb <= 0 {
            return None;
        }
        if heap.count_kb() > limit_kb {
            heap.collect();
        }
        if heap.count_kb() <= limit_kb {
            if self.deadline == 0 {
                return None;
            }
            self.deadline = 0;
            return Some(MemoryVerdict::Recovered);
        }
        if self.deadline != 0 {
            // `sub eax,[0xb4e3e8]; js`: the deadline reached on the wrapping clock.
            return ((now.wrapping_sub(self.deadline) as i32) >= 0).then_some(MemoryVerdict::Quit);
        }
        // A sum that wraps to 0 is stored as 1, which reads as armed (`0x495618`).
        self.deadline = now.wrapping_add(GRACE_MS).max(1);
        // `cdq; and edx,0x3ff; add; sar 0xa`: the limit is positive here, so a plain shift.
        Some(MemoryVerdict::Exhausted(limit_kb >> 10))
    }
}

/// The per-frame watch, ahead of the frame's `ReloadUI` as `0x495590` runs it ahead of the
/// reload's byte (`0x49565b`). Its events fire in place, as `FrameScript_SignalEvent` runs every
/// listener before it returns.
pub(crate) fn watch_script_memory(
    script: Option<NonSendMut<UiScript>>,
    entered: Res<LeavingWorldArmed>,
    mut watch: ResMut<ScriptMemoryWatch>,
    time: Res<Time<Real>>,
    mut exit: MessageWriter<AppExit>,
) {
    let Some(mut script) = script else {
        return;
    };
    if !entered.is_armed() {
        return;
    }
    // A VM with no such record reads 0, the watch off.
    let limit = script.cvar_int(CVAR_SCRIPT_MEMORY).unwrap_or(0);
    let now = crate::ui_billing::now_ms(&time);
    match watch.poll(limit, now, &mut *script) {
        None => {}
        Some(MemoryVerdict::Exhausted(mb)) => {
            warn!("ui: the interface is over its script memory ({mb} MB)");
            script.fire_event("MEMORY_EXHAUSTED", vec![ScriptValue::Int(i64::from(mb))]);
        }
        Some(MemoryVerdict::Recovered) => script.fire_event("MEMORY_RECOVERED", vec![]),
        Some(MemoryVerdict::Quit) => {
            warn!("ui: still over the script memory 15 s on — quitting, as 1.12.1 does");
            exit.write(AppExit::Success);
        }
    }
}

pub(crate) struct UiMemoryPlugin;

impl Plugin for UiMemoryPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ScriptMemoryWatch>()
            .init_resource::<LeavingWorldArmed>()
            .add_systems(
                PreUpdate,
                watch_script_memory
                    .before(crate::ui_script::run_pending_reload)
                    .run_if(crate::ui_script::ingame_ui_up),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A state whose count a collection lowers to what it keeps.
    struct Heap {
        count: i32,
        keeps: i32,
        collections: u32,
    }

    impl ScriptHeap for Heap {
        fn count_kb(&mut self) -> i32 {
            self.count
        }
        fn collect(&mut self) {
            self.count = self.keeps;
            self.collections += 1;
        }
    }

    fn heap(count: i32, keeps: i32) -> Heap {
        Heap {
            count,
            keeps,
            collections: 0,
        }
    }

    #[test]
    fn a_state_over_after_a_collection_warns_once_then_quits_at_15_s() {
        let mut w = ScriptMemoryWatch::default();
        let mut h = heap(3000, 2000);
        assert_eq!(w.poll(1024, 100, &mut h), Some(MemoryVerdict::Exhausted(1)));
        assert_eq!(
            h.collections, 1,
            "over the limit collects before it counts again"
        );
        assert_eq!(
            w.poll(1024, 100 + 14_999, &mut h),
            None,
            "one warning, then the wait"
        );
        assert_eq!(
            w.poll(1024, 100 + 15_000, &mut h),
            Some(MemoryVerdict::Quit)
        );
    }

    #[test]
    fn a_collection_that_brings_the_state_within_says_nothing() {
        let mut w = ScriptMemoryWatch::default();
        let mut h = heap(3000, 900);
        assert_eq!(w.poll(1024, 0, &mut h), None);
        assert_eq!(h.collections, 1);
        let mut at = heap(1024, 1024);
        assert_eq!(w.poll(1024, 0, &mut at), None, "at the limit is within it");
        assert_eq!(at.collections, 0, "nothing over, nothing collected");
    }

    #[test]
    fn coming_back_within_clears_the_deadline_and_recovers() {
        let mut w = ScriptMemoryWatch::default();
        let mut h = heap(3000, 3000);
        assert_eq!(w.poll(2048, 0, &mut h), Some(MemoryVerdict::Exhausted(2)));
        h.keeps = 1000;
        assert_eq!(w.poll(2048, 10, &mut h), Some(MemoryVerdict::Recovered));
        assert_eq!(w.poll(2048, 20, &mut h), None, "recovered once");
        // Over again: a fresh warning and a fresh 15 s.
        h.count = 3000;
        h.keeps = 3000;
        assert_eq!(
            w.poll(2048, 50_000, &mut h),
            Some(MemoryVerdict::Exhausted(2))
        );
        assert_eq!(w.poll(2048, 64_999, &mut h), None);
    }

    #[test]
    fn a_limit_of_zero_or_below_is_off() {
        let mut w = ScriptMemoryWatch::default();
        let mut h = heap(1 << 30, 1 << 30);
        assert_eq!(w.poll(0, 0, &mut h), None);
        assert_eq!(w.poll(-1, 0, &mut h), None);
        assert_eq!(h.collections, 0);
    }

    #[test]
    fn the_deadline_wraps_with_the_clock_and_never_reads_unarmed() {
        let mut w = ScriptMemoryWatch::default();
        let mut h = heap(3000, 3000);
        // `now + 15000` wraps to exactly 0, which is stored as 1.
        let now = 0u32.wrapping_sub(GRACE_MS);
        assert_eq!(w.poll(1024, now, &mut h), Some(MemoryVerdict::Exhausted(1)));
        assert_eq!(w.deadline, 1);
        assert_eq!(w.poll(1024, 0, &mut h), None, "a millisecond short");
        assert_eq!(w.poll(1024, 1, &mut h), Some(MemoryVerdict::Quit));
    }

    #[test]
    fn the_message_carries_the_limit_in_whole_mb() {
        let mut w = ScriptMemoryWatch::default();
        let mut h = heap(1 << 20, 1 << 20);
        assert_eq!(
            w.poll(49_152 + 1023, 0, &mut h),
            Some(MemoryVerdict::Exhausted(48))
        );
    }
}
