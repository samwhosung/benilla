//! **The one load law — `AddOn_CanLoad 0x51e780`, as a pure function**.
//!
//! Every question of the form "can this addon load, and if not, why" has exactly one answer in
//! the reference: a single arbiter whose checks run in a fixed order, whose version gate and
//! CVar read re-run on every query, and whose two out-params (`reasonOut`, `depReasonOut`)
//! render through one formatter. benilla had three partial copies of it — the API verbs'
//! `not_loadable`, `LoadAddOn`'s inline checks, and the startup walk's disabled set — and the
//! screens were about to become a fourth. This module is the one copy they all consult.
//!
//! The check order: **missing → visiting-guard (returns loadable — a cycle resolves
//! optimistically) → enabled → banned → corrupt → the version gate → required-deps recursion →
//! demand gate**. The first check that fires decides.
//!
//! ## The version gate, byte for byte
//!
//! `cmp [rec+0x1c], 11200` — an **exact `==` against a hard-coded immediate** (`0x51d7d0` is
//! `mov eax,0x2bc0; ret`, one caller image-wide). `## Interface: 11201` is as out of date as
//! `10000`, and a manifest with **no** `## Interface` line compares as `0` — out of date, not
//! "unknown". On mismatch the reason `7` is written and then, when the check is OFF
//! (`checkAddonVersion`, the *Load out of date AddOns* checkbox inverted), **actively reset to
//! `0`** — an out-of-date addon under force-load is byte-indistinguishable from an up-to-date
//! one; there is no "loadable but flagged" state. Force-load is a fall-through, not a success:
//! the dependency and demand checks still run.
//!
//! ## The reason rendering (`0x51e930`)
//!
//! `reason != 0` → its token; `reason == 0 && depReason != 0` → `DEP_<token>`, applied
//! **exactly once at any nesting depth** (the recursion passes the caller's `depReasonOut` as
//! the callee's *both* out-params, so the deepest failure's raw token lands in the parent's dep
//! slot); both zero → nil (all three callers guard it — `LOADABLE` is unreachable as a reason).
//! `BANNED`/`CORRUPT`/`INSECURE` gate on the server's `SMSG_ADDON_INFO` signature state, which
//! benilla does not model (nothing a player installs is Blizzard-signed) — the arms exist in
//! the table for honesty and are never produced.

/// The `## Interface` this client implements — the reference's own hard-coded `0x2bc0`.
pub const CLIENT_INTERFACE: u32 = 11200;

/// One addon as the gate reads it — the adapter shape every registry (the VM's `AddOnInfo`
/// rows, the app's discovered manifests, the glue screen's folder view) lowers into.
pub struct GateRow<'a> {
    pub name: &'a str,
    /// This character's enable state (`AddOns.txt`; an addon nobody disabled is enabled).
    pub enabled: bool,
    /// `## Interface` as the client parses it ([`crate::toc::Toc::interface_version`]):
    /// the leading integer, `0` when absent.
    pub interface: u32,
    pub load_on_demand: bool,
    /// Loaded this session. A loaded dependency satisfies the recursion outright: the short-circuit
    /// lives inside `AddOn_CanLoad` itself (`0x51e8ba call AddOn_IsLoaded; jne` skips the
    /// recursion), and the recursion propagates the caller's own `demandOnly` (`0x51e790` spill →
    /// `0x51e8c9` reload). The reference's concrete answers match this implementation exactly: a
    /// LoadOnDemand addon over a loaded ordinary dep is loadable/nil; over an enabled-but-UNLOADED
    /// ordinary dep it reports `DEP_NOT_DEMAND_LOADED`.
    pub loaded: bool,
    /// `## Dependencies` / `## RequiredDeps` / any `## Dep*` (one list in the reference).
    pub dependencies: Vec<&'a str>,
}

/// The arbiter's answer: loadable, or the two out-params.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Loadable,
    Refused {
        /// The addon's own reason token (`DISABLED`, `INTERFACE_VERSION`, …), when the failure
        /// is its own.
        reason: Option<&'static str>,
        /// The dependency's raw token when the failure is a dep's — rendered as `DEP_<token>`.
        dep: Option<&'static str>,
    },
}

impl Verdict {
    pub fn loadable(self) -> bool {
        self == Verdict::Loadable
    }

    /// The formatter (`0x51e930`) plus its callers' nil-guard: the reason token, else
    /// `DEP_<dep>`, else `None` (loadable — `LOADABLE` is unreachable as a reason).
    pub fn token(self) -> Option<String> {
        match self {
            Verdict::Loadable => None,
            Verdict::Refused { reason, dep } => reason
                .map(str::to_owned)
                .or_else(|| dep.map(|d| format!("DEP_{d}"))),
        }
    }
}

/// `AddOn_CanLoad` over a registry: can `rows[index]` load right now?
///
/// `demand_only` is the in-game flavour (`dl=1`): an enabled, up-to-date, **non**-LoadOnDemand
/// addon that has not loaded reports `NOT_DEMAND_LOADED` — a state the glue (`dl=0`) never
/// produces. `version_check` is the live `checkAddonVersion` read; both re-run per query, which
/// is why toggling the checkbox needs no rescan (`0x51e780`).
pub fn can_load(rows: &[GateRow], index: usize, demand_only: bool, version_check: bool) -> Verdict {
    let mut visiting = vec![false; rows.len()];
    walk(rows, index, demand_only, version_check, &mut visiting)
}

/// One level of the arbiter — the checks in the reference's evaluation order.
fn walk(
    rows: &[GateRow],
    i: usize,
    demand_only: bool,
    version_check: bool,
    visiting: &mut [bool],
) -> Verdict {
    // Check 2: the in-progress guard — a cycle resolves optimistically (`[rec+0x2d]` → TRUE).
    if visiting[i] {
        return Verdict::Loadable;
    }
    let row = &rows[i];
    // Check 3: this character's enable state.
    if !row.enabled {
        return Verdict::Refused {
            reason: Some("DISABLED"),
            dep: None,
        };
    }
    // Checks 4/5 (banned / corrupt): no server signature state to read — never produced.
    // Check 6: the version gate — exact ==, then refuse or actively fall through (`0x51e876`).
    if row.interface != CLIENT_INTERFACE && version_check {
        return Verdict::Refused {
            reason: Some("INTERFACE_VERSION"),
            dep: None,
        };
    }
    // Check 7: required dependencies, recursively; the first failure decides, and the deepest
    // failure's raw token is the one `DEP_` wraps (`0x51e8ce`'s shared out-param).
    visiting[i] = true;
    for dep in &row.dependencies {
        let found = rows.iter().position(|r| r.name.eq_ignore_ascii_case(dep));
        let verdict = match found {
            // The recursion's check 1: a name not in the registry is MISSING.
            None => Verdict::Refused {
                reason: Some("MISSING"),
                dep: None,
            },
            // A loaded dependency satisfies the walk outright — `0x51e8ba`, before the
            // recursion (see the row doc).
            Some(d) if rows[d].loaded => Verdict::Loadable,
            Some(d) => walk(rows, d, demand_only, version_check, visiting),
        };
        if let Verdict::Refused { reason, dep } = verdict {
            visiting[i] = false;
            return Verdict::Refused {
                reason: None,
                dep: reason.or(dep),
            };
        }
    }
    visiting[i] = false;
    // Check 8: the demand gate — reachable only from the in-game surface (`dl=1`).
    if demand_only && !row.load_on_demand && !row.loaded {
        return Verdict::Refused {
            reason: Some("NOT_DEMAND_LOADED"),
            dep: None,
        };
    }
    Verdict::Loadable
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row<'a>(name: &'a str, deps: Vec<&'a str>) -> GateRow<'a> {
        GateRow {
            name,
            enabled: true,
            interface: CLIENT_INTERFACE,
            load_on_demand: false,
            loaded: false,
            dependencies: deps,
        }
    }

    /// The check ORDER is the law: a disabled addon reports only DISABLED, whatever else is
    /// wrong with it — the glue colours that row grey, never red (1197's precedence).
    #[test]
    fn a_disabled_addon_reports_only_disabled() {
        let mut a = row("A", vec!["Ghost"]);
        a.enabled = false;
        a.interface = 0; // out of date too — and still invisible behind DISABLED
        let rows = vec![a];
        assert_eq!(
            can_load(&rows, 0, false, true).token().as_deref(),
            Some("DISABLED")
        );
    }

    /// The version gate (`0x51e876`) byte for byte: exact `==` (11201 is out of date), missing
    /// `## Interface` is 0 and out of date, and force-load RESETS the reason — indistinguishable
    /// from up to date.
    #[test]
    fn the_version_gate_is_exact_and_force_load_erases_it() {
        let mut a = row("A", vec![]);
        a.interface = 11201;
        let rows = vec![a];
        assert_eq!(
            can_load(&rows, 0, false, true).token().as_deref(),
            Some("INTERFACE_VERSION")
        );
        assert_eq!(can_load(&rows, 0, false, false), Verdict::Loadable);

        let mut b = row("B", vec![]);
        b.interface = 0; // no ## Interface line
        let rows = vec![b];
        assert_eq!(
            can_load(&rows, 0, false, true).token().as_deref(),
            Some("INTERFACE_VERSION")
        );
    }

    /// `DEP_` is applied exactly once at any depth (`0x51e8ce`) — the deepest failure's raw token
    /// is what the top level wraps.
    #[test]
    fn dep_prefix_applies_once_at_any_depth() {
        let a = row("A", vec!["B"]);
        let b = row("B", vec!["C"]);
        let mut c = row("C", vec![]);
        c.enabled = false;
        let rows = vec![a, b, c];
        assert_eq!(
            can_load(&rows, 0, false, true).token().as_deref(),
            Some("DEP_DISABLED"),
            "not DEP_DEP_DISABLED — the shared out-param collapses the nesting"
        );
        // And a dep that is not installed at all is the recursion's own check 1.
        let rows = vec![row("A", vec!["Ghost"])];
        assert_eq!(
            can_load(&rows, 0, false, true).token().as_deref(),
            Some("DEP_MISSING")
        );
    }

    /// Check 2: a dependency cycle resolves optimistically (the visiting guard returns TRUE),
    /// so neither side reports a failure from the query side.
    #[test]
    fn a_cycle_is_loadable_to_the_query() {
        let a = row("Ping", vec!["Pong"]);
        let b = row("Pong", vec!["Ping"]);
        let rows = vec![a, b];
        assert_eq!(can_load(&rows, 0, false, true), Verdict::Loadable);
    }

    /// Check 8 is in-game only (`dl=1`), runs AFTER the dep loop, and force-load does not skip
    /// it — an out-of-date addon under force-load can still be NOT_DEMAND_LOADED.
    #[test]
    fn the_demand_gate_is_in_game_only_and_survives_force_load() {
        let mut a = row("A", vec![]);
        a.interface = 11507;
        let rows = vec![a];
        assert_eq!(can_load(&rows, 0, false, false), Verdict::Loadable);
        assert_eq!(
            can_load(&rows, 0, true, false).token().as_deref(),
            Some("NOT_DEMAND_LOADED"),
            "force-load falls THROUGH, it does not succeed (`0x51e89d`)"
        );
    }

    /// A LoadOnDemand addon whose required dep already loaded at startup is loadable from the
    /// in-game surface — the loaded short-circuit at `0x51e8ba` (inside `AddOn_CanLoad`,
    /// before the recursion; the reference's own concrete A/B verdict is this assertion).
    #[test]
    fn a_loaded_dependency_satisfies_the_demand_query() {
        let mut a = row("A", vec!["B"]);
        a.load_on_demand = true;
        let mut b = row("B", vec![]);
        b.loaded = true;
        let rows = vec![a, b];
        assert_eq!(can_load(&rows, 0, true, true), Verdict::Loadable);
    }
}
