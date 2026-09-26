#!/usr/bin/env bash
# The clean-run gate: boot the real client, log out, re-enter, walk the realm list, check the log.
#
# Needs a server and opens a window for about a minute; gates.sh points at it but does not run it.
# Logs in as the checkout's `.probe-identity`, else the shell's WOW_USER/WOW_PASS/WOW_CHAR
# (scripts/probe-identity.sh). WOW_HOST is the server (default localhost), WOW_DATA the install
# watched for writes (default the tree's WoW link), WOW_BG passes through to the client, and
# WOW_SMOKE_KEEP=1 keeps the logs.
#
#   scripts/smoke.sh
set -uo pipefail

# Test $PWD's checkout when it is one of this repo, else this file's (the same rule as gates.sh).
root="$(git -C "$PWD" rev-parse --show-toplevel 2>/dev/null || true)"
if [ -z "$root" ] || [ ! -f "$root/scripts/smoke.sh" ]; then
    root="$(cd "$(dirname "$0")/.." && pwd)"
fi
cd "$root" || exit 1
echo "smoke: $root"

# Resolve the account first, then unset every inherited WOW_* the case below does not keep, so
# each leg runs on the switches it names; WOW_BG is kept because it only changes window stacking.
. "$root/scripts/probe-identity.sh"
probe_identity smoke "$root" || exit 1
user="$PROBE_USER"
pass="$PROBE_PASS"
char="$PROBE_CHAR"
inherited=""
for v in $(env | sed -n 's/^\(WOW_[A-Za-z0-9_]*\)=.*/\1/p'); do
    case "$v" in WOW_DATA | WOW_HOST | WOW_BG | WOW_SMOKE_KEEP) continue ;; esac
    unset "$v"
    # The account is in $user/$pass/$char now and each leg passes what it needs by name (the realm
    # leg refuses a WOW_CHAR), so it leaves the environment either way. It was only "ignored"
    # when the checkout's declared identity took its place.
    [ -n "$PROBE_DECLARED" ] || case "$v" in WOW_USER | WOW_PASS | WOW_CHAR) continue ;; esac
    inherited="$inherited $v"
done
[ -n "$inherited" ] &&
    echo "smoke: ignoring inherited env —$inherited (each leg names its own; a gate is not shell-dependent)"

# Check the server before the build: a refused connection would read as a client bug in the log.
probe_server_or_skip smoke || exit 0

log="$(mktemp "${TMPDIR:-/tmp}/benilla-smoke.XXXXXX")"
stamp="$(mktemp "${TMPDIR:-/tmp}/benilla-smoke-stamp.XXXXXX")"
before="$(mktemp "${TMPDIR:-/tmp}/benilla-smoke-before.XXXXXX")"
# One EXIT trap, so an early `fail` strands no temp file; WOW_SMOKE_KEEP spares the log.
trap 'rm -f "$stamp" "$before"; [ -n "${WOW_SMOKE_KEEP:-}" ] || rm -f "$log"' EXIT

# The install is read-only: the run must add, remove and modify nothing in it. The sorted file
# list catches additions and removals, `-newer` against the stamp catches edits in place; plain
# POSIX `find` because `stat`'s format flag differs between BSD (-f) and GNU (-c).
install_root=""
if [ -n "${WOW_DATA:-}" ]; then
    install_root="$WOW_DATA"
elif [ -d "$root/WoW" ]; then
    install_root="$root/WoW"
fi
# A running reference client (wow.exe) writes its own WTF files into a shared install, and this
# watch cannot tell those from benilla's; note it so the verdict can say so.
reference_client_before=""
if pgrep -f '[w]ow\.exe' >/dev/null 2>&1; then
    reference_client_before=1
fi
if [ -n "$install_root" ]; then
    find -L "$install_root" -type f 2>/dev/null | sort >"$before"
    echo "smoke: install read-only watch on $install_root ($(wc -l <"$before" | tr -d ' ') files)"
    [ -n "$reference_client_before" ] && \
        echo "smoke: NOTE — the reference client (wow.exe) is running; the install watch cannot attribute"
fi

# Build before the timed run: the timeout bounds the client, and a cold build takes minutes.
if ! cargo build -q -p benilla >"$log" 2>&1; then
    echo "SMOKE FAILED: the client did not build"
    sed -E 's/\x1b\[[0-9;]*m//g' "$log" | tail -30
    exit 1
fi

echo "smoke: running the logout/re-login round trip (~45 s, opens a window)…"
# WOW_NOSOUND: the client opens no audio device (`sound/mod.rs`), so the run is silent.
WOW_UNATTENDED=1 WOW_NOSOUND=1 WOW_USER="$user" WOW_PASS="$pass" WOW_CHAR="$char" WOW_LOGOUT_SMOKE=1 \
    timeout 180 cargo run -q -p benilla >"$log" 2>&1
code=$?

# Strip ANSI once: the tracing level is colour-wrapped, and the checks below match plain text.
plain="$(sed -E 's/\x1b\[[0-9;]*m//g' "$log")"

fail() {
    echo "SMOKE FAILED: $1"
    [ -n "${WOW_SMOKE_KEEP:-}" ] && echo "  log: $log"
    printf '%s\n' "$plain" | tail -30
    exit 1
}

# Before the timeout check, which it usually explains: a WOW_CHAR not on the account leaves the
# client waiting at the roster.
printf '%s' "$plain" | grep -q "not on this account" &&
    fail "$char is not on $user — the client sat at the roster (make it, or rig it: WOW_RIG=…)"
[ $code -eq 124 ] && fail "the client did not exit within the timeout"
printf '%s' "$plain" | grep -q "logout-smoke: empty roster" &&
    fail "$user has no characters — the re-entry leg cannot run (make one, or rig it: WOW_RIG=…)"
printf '%s' "$plain" | grep -q "logout-smoke: done" ||
    fail "the round trip never completed (no 'logout-smoke: done')"

# The re-entered character must be drivable: the leg lists what would block movement (a /logout
# root must end with its session), and anything but `none` fails.
sup="$(printf '%s\n' "$plain" | sed -n 's/.*logout-smoke: re-entered.*suppressors: \(.*\), done.*/\1/p' | tail -1)"
printf '  %-24s %s\n' "re-entry suppressors" "${sup:-<unreported>}"
[ "$sup" = "none" ] ||
    fail "the re-entered character could not be driven — suppressors: ${sup:-<unreported>} \
(everything the ended session granted its mover dies with it)"

# vmangos skips the logout root when resting, on a taxi or at account security >= `InstantLogout`
# (`MiscHandler.cpp`, `CMSG_LOGOUT_REQUEST`); probe accounts are GM, so report whether the check
# above met a real root. `player::wire_in::session_end_tests` forces the rooted case.
if printf '%s\n' "$plain" | grep -q "mover mode Root granted"; then
    printf '  %-24s %s\n' "rooted logout" "yes — the drivable check above is a real pass"
else
    printf '  %-24s %s\n' "rooted logout" \
        "no (instant logout: GM probe / resting), so the check above did not run"
fi

errors="$(printf '%s\n' "$plain" | grep -cE ' ERROR ')"
panics="$(printf '%s\n' "$plain" | grep -cE 'panicked at')"
[ "$panics" -ne 0 ] && fail "$panics panic(s)"
[ "$errors" -ne 0 ] && fail "$errors ERROR line(s)"

# A marker counts once per edge it fires on. Each world entry builds a world VM beside the
# character screen's glue VM (the reference's `0x490bd0`/`0x48fbf0` pair), so per-VM markers count
# twice per login; half that means an entry adopted the glue VM and its spent `VmMemo`s.
sessions=2            # world entries in this walk
vms=$((sessions * 2)) # …and the Lua states they cost: a glue VM and a world VM each
for marker in "Fonts.xml loaded"; do
    n="$(printf '%s\n' "$plain" | grep -cF "$marker")"
    printf '  %-24s %s\n' "$marker" "$n"
    [ "$n" -eq "$vms" ] ||
        fail "'$marker' happened $n time(s), expected $vms — one per VM built, two per login; \
$sessions would mean the entry adopted the character screen's VM and inherited its spent VmMemos"
done
# The in-game UI and the keybinding table (`seed_bindings_for_vm`, run on the world-entry edge)
# build once per login; one count for two logins means the second reused the first's frame tree.
for marker in "UIParent.xml loaded" "commands registered"; do
    n="$(printf '%s\n' "$plain" | grep -cF "$marker")"
    printf '  %-24s %s\n' "$marker" "$n"
    [ "$n" -eq "$sessions" ] ||
        fail "'$marker' happened $n time(s), expected $sessions — once per login: the UI rebuilt per login and the keybinding table seeded on the entry edge"
done

# The shutdown tail runs once per session: at the /logout, and at the exit, which closes the window
# as a player does. Counted off the saved-variables line, written only when the UI loaded.
writes="$(printf '%s\n' "$plain" | grep -cF "saved variables: wrote")"
printf '  %-24s %s\n' "shutdown writes" "$writes"
[ "$writes" -eq "$sessions" ] ||
    fail "the shutdown tail wrote $writes time(s), expected $sessions — a session ended without \
saving (the quit root must be observed in \`Last\`, after PostUpdate's exit_on_all_closed)"

# The read-only verdict names each changed file, the whole lead to whatever wrote it.
if [ -n "$install_root" ]; then
    after="$(mktemp "${TMPDIR:-/tmp}/benilla-smoke-after.XXXXXX")"
    find -L "$install_root" -type f 2>/dev/null | sort >"$after"
    appeared="$(comm -13 "$before" "$after")"
    vanished="$(comm -23 "$before" "$after")"
    touched="$(find -L "$install_root" -type f -newer "$stamp" 2>/dev/null)"
    rm -f "$after"
    if [ -n "$appeared" ] || [ -n "$vanished" ] || [ -n "$touched" ]; then
        if [ -n "$reference_client_before" ] || pgrep -f '[w]ow\.exe' >/dev/null 2>&1; then
            echo "SMOKE INCONCLUSIVE: the install changed, but the REFERENCE CLIENT was running."
            echo "  wow.exe writes its own WTF while it runs, and this watch is a whole-tree mtime"
            echo "  diff — it cannot tell those writes from benilla's. This is NOT evidence that"
            echo "  benilla wrote to the install, and it is not evidence that it did not."
            echo "  Close the reference client and re-run to get a real reading."
        else
            echo "SMOKE FAILED: the run WROTE TO THE INSTALL — benilla never does."
            echo "  Every file benilla persists goes through \`crate::local_state\` into"
            echo "  \`benilla-config/\` beside the binary."
        fi
        [ -n "$appeared" ] && { echo "  added:"; printf '%s\n' "$appeared" | sed 's/^/    /'; }
        [ -n "$vanished" ] && { echo "  removed:"; printf '%s\n' "$vanished" | sed 's/^/    /'; }
        [ -n "$touched" ] && { echo "  modified:"; printf '%s\n' "$touched" | sed 's/^/    /'; }
        exit 1
    fi
    echo "  install untouched         $(wc -l <"$before" | tr -d ' ') files"
else
    echo "  install                   not watched (no WoW link and no WOW_DATA — the read-only rule went unmeasured)"
fi

# ── The realm-list leg ───────────────────────────────────────────────────────────────────────
# The realm list is a dialog over character select that the IO thread serves from the character
# park; this drives Change Realm, Okay, Change Realm, Cancel, Okay against the real server and
# fails anything but a completed walk.
echo "smoke: running the realm-list boundary walk (~12 s, opens a window)…"
rlog="$(mktemp "${TMPDIR:-/tmp}/benilla-smoke-realm.XXXXXX")"
# No WOW_CHAR: the walk drives character select, and `realm_select::smoke` refuses a seated body.
WOW_UNATTENDED=1 WOW_NOSOUND=1 WOW_USER="$user" WOW_PASS="$pass" WOW_REALM_SMOKE=1 \
    timeout 120 cargo run -q -p benilla >"$rlog" 2>&1
rcode=$?
rplain="$(sed -E 's/\x1b\[[0-9;]*m//g' "$rlog")"
[ -n "${WOW_SMOKE_KEEP:-}" ] || rm -f "$rlog"
realm_fail() {
    echo "SMOKE FAILED (realm leg): $1"
    [ -n "${WOW_SMOKE_KEEP:-}" ] && echo "  log: $rlog"
    printf '%s\n' "$rplain" | tail -30
    exit 1
}
[ $rcode -eq 124 ] && realm_fail "the realm walk did not finish within the timeout"
printf '%s' "$rplain" | grep -q "realm-smoke: FAILED" &&
    realm_fail "$(printf '%s\n' "$rplain" | sed -n 's/.*realm-smoke: FAILED — //p' | tail -1)"
printf '%s' "$rplain" | grep -q "realm-smoke: done" ||
    realm_fail "the walk never completed (no 'realm-smoke: done')"
rp="$(printf '%s\n' "$rplain" | grep -cE 'panicked at')"
[ "$rp" -ne 0 ] && realm_fail "$rp panic(s)"
printf '  %-24s %s\n' "realm walk" \
    "$(printf '%s\n' "$rplain" | sed -n 's/.*realm-smoke: done — //p' | tail -1)"

echo "SMOKE GREEN — ${sessions} logins + the realm walk, 0 errors, 0 panics$([ -n "$install_root" ] && echo ', install untouched')"
[ -n "${WOW_SMOKE_KEEP:-}" ] && echo "log: $log"
exit 0
