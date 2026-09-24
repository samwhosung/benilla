//! `AddOn_CanLoad` (`0x51e780`) as a pure function: the one load law every surface consults.
//! The first check that fires decides, in the reference's order: missing, the visiting guard (a
//! cycle is loadable), enabled, banned, corrupt, version, dependencies, demand.
//!
//! The version gate is an exact `==` against a hard-coded 11200 (`0x51d7d0`), so 11201 is out of
//! date and a missing `## Interface` compares as 0. With `checkAddonVersion` off the reason is
//! reset to none, so force-load falls through to the dependency and demand checks.
//!
//! The reason renders once (`0x51e930`): the addon's own token, else `DEP_` and the deepest failing
//! dependency's token, else nil. `BANNED`, `CORRUPT` and `INSECURE` read the server's
//! `SMSG_ADDON_INFO` signature state, which is not modelled; they are never produced.

/// The `## Interface` this client implements, the reference's hard-coded `0x2bc0`.
pub const CLIENT_INTERFACE: u32 = 11200;

/// One addon as the gate reads it; every addon registry lowers into this.
pub struct GateRow<'a> {
    pub name: &'a str,
    /// This character's enable state (`AddOns.txt`; an addon nobody disabled is enabled).
    pub enabled: bool,
    /// `## Interface` as the client parses it ([`crate::toc::Toc::interface_version`]):
    /// the leading integer, `0` when absent.
    pub interface: u32,
    pub load_on_demand: bool,
    /// Loaded this session. A loaded dependency satisfies the recursion outright (`0x51e8ba`),
    /// which otherwise passes on the caller's `demandOnly` (`0x51e790`, `0x51e8c9`): in game, an
    /// ordinary dependency enabled but not loaded reports `DEP_NOT_DEMAND_LOADED`.
    pub loaded: bool,
    /// `## Dependencies` / `## RequiredDeps` / any `## Dep*` (one list in the reference).
    pub dependencies: Vec<&'a str>,
}

/// The arbiter's answer: loadable, or the two out-params.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Loadable,
    Refused {
        /// The addon's own reason token (`DISABLED`, `INTERFACE_VERSION`, …).
        reason: Option<&'static str>,
        /// A failing dependency's raw token, rendered as `DEP_<token>`.
        dep: Option<&'static str>,
    },
}

impl Verdict {
    pub fn loadable(self) -> bool {
        self == Verdict::Loadable
    }

    /// The formatter (`0x51e930`) with its callers' nil guard, so `LOADABLE` never renders.
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
/// `demand_only` is the in-game query (`dl=1`), where an unloaded addon that is not LoadOnDemand
/// reports `NOT_DEMAND_LOADED`; `version_check` is the live `checkAddonVersion`, read per query.
pub fn can_load(rows: &[GateRow], index: usize, demand_only: bool, version_check: bool) -> Verdict {
    let mut visiting = vec![false; rows.len()];
    walk(rows, index, demand_only, version_check, &mut visiting)
}

/// One level of the arbiter, its checks in the reference's order.
fn walk(
    rows: &[GateRow],
    i: usize,
    demand_only: bool,
    version_check: bool,
    visiting: &mut [bool],
) -> Verdict {
    // Check 2: the visiting guard; a cycle resolves as loadable.
    if visiting[i] {
        return Verdict::Loadable;
    }
    let row = &rows[i];
    // Check 3: enabled.
    if !row.enabled {
        return Verdict::Refused {
            reason: Some("DISABLED"),
            dep: None,
        };
    }
    // Checks 4 and 5, banned and corrupt: no signature state, never produced.
    // Check 6: the version gate, exact `==`; with the check off it falls through (`0x51e876`).
    if row.interface != CLIENT_INTERFACE && version_check {
        return Verdict::Refused {
            reason: Some("INTERFACE_VERSION"),
            dep: None,
        };
    }
    // Check 7: the dependencies, recursively; a shared out-param makes the deepest failure's raw
    // token the one `DEP_` wraps (`0x51e8ce`).
    visiting[i] = true;
    for dep in &row.dependencies {
        let found = rows.iter().position(|r| r.name.eq_ignore_ascii_case(dep));
        let verdict = match found {
            // Check 1, in the recursion: a name not in the registry is `MISSING`.
            None => Verdict::Refused {
                reason: Some("MISSING"),
                dep: None,
            },
            // A loaded dependency satisfies the walk before any recursion (`0x51e8ba`).
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
    // Check 8: the demand gate, in game only (`dl=1`).
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

    /// Enabled comes before every later check, so the glue shows a disabled row grey, never red.
    #[test]
    fn a_disabled_addon_reports_only_disabled() {
        let mut a = row("A", vec!["Ghost"]);
        a.enabled = false;
        a.interface = 0; // out of date too
        let rows = vec![a];
        assert_eq!(
            can_load(&rows, 0, false, true).token().as_deref(),
            Some("DISABLED")
        );
    }

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
        // A dependency not installed fails the recursion's check 1.
        let rows = vec![row("A", vec!["Ghost"])];
        assert_eq!(
            can_load(&rows, 0, false, true).token().as_deref(),
            Some("DEP_MISSING")
        );
    }

    #[test]
    fn a_cycle_is_loadable_to_the_query() {
        let a = row("Ping", vec!["Pong"]);
        let b = row("Pong", vec!["Ping"]);
        let rows = vec![a, b];
        assert_eq!(can_load(&rows, 0, false, true), Verdict::Loadable);
    }

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
