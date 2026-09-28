#!/usr/bin/env bash
# Addon-API coverage: how much of the 1.12.1 client's global surface benilla's UI VM presents.
#
#   scripts/api-coverage.sh              # the report
#   scripts/api-coverage.sh --missing    # + every engine global we do not have
#   scripts/api-coverage.sh --beyond     # + every global we have that 1.12 does not
#
# The 1.12 side is `reference/1.12-globals.tsv`, the reference client's in-world `_G`
# (scripts/gen-reference-globals.py); ours is a real `UiScript::new()` dumped through `pairs(_G)`.
# Never quote the percentage alone: a missing global is an unbuilt feature or a hole in a shipped
# one, and a global beyond 1.12 sends a feature-detecting addon down a path we cannot honour.
set -u
cd "$(dirname "$0")/.."

REF="reference/1.12-globals.tsv"
[ -f "$REF" ] || { echo "no $REF — run scripts/gen-reference-globals.py" >&2; exit 1; }

show_missing=0
show_beyond=0
for a in "$@"; do
  case "$a" in
    --missing) show_missing=1 ;;
    --beyond)  show_beyond=1 ;;
    *) echo "usage: $0 [--missing] [--beyond]" >&2; exit 2 ;;
  esac
done

ours=$(mktemp) || exit 1
trap 'rm -f "$ours"' EXIT
echo "asking benilla's VM what it exposes..." >&2
if ! cargo run -q -p benilla-ui --example dump_globals >"$ours" 2>/dev/null; then
  echo "dump_globals failed — build the workspace first" >&2
  exit 1
fi

awk -F'\t' -v ref="$REF" -v show_missing="$show_missing" -v show_beyond="$show_beyond" '
  # ── the reference side ──────────────────────────────────────────────────────────────────────
  FNR == NR {
    if ($0 ~ /^#/) next
    origin[$1] = $3
    n_ref[$3]++
    if ($2 == "function") n_ref_fn[$3]++
    next
  }
  # ── our side ───────────────────────────────────────────────────────────────────────────────
  {
    ours[$1] = $2
    n_ours++
    o = ($1 in origin) ? origin[$1] : "beyond"
    if (o == "engine" || o == "lua") have++
    else if (o == "framexml") transcribable[$1] = 1
    else {
      # Categorize the superset. Only the last bucket is an API-target question; the first is
      # benilla being benilla, and the second is our Lua runtime being 5.1 where 1.12 is 5.0.
      if ($1 ~ /^Benilla/)                                    bridge[$1] = 1
      else if ($1 ~ /^(_G|_VERSION|coroutine|print|select)$/)  lua51[$1] = 1
      else                                                     api[$1] = 1
    }
  }
  END {
    surface = n_ref["engine"] + n_ref["lua"]
    missing = surface - have
    pct = surface ? sprintf("%.0f%%", 100 * have / surface) : "-"
    printf "\nthe 1.12.1 surface — %s (%d names: the running client'\''s _G, plus what the\n                       shipped UI defines behind a LoadOnDemand window)\n", ref, \
      n_ref["engine"] + n_ref["framexml"] + n_ref["lua"]
    printf "  engine    %5d functions, %5d other   <- benilla implements these in Rust\n", \
      n_ref_fn["engine"], n_ref["engine"] - n_ref_fn["engine"]
    printf "  framexml  %5d functions, %5d other   <- the stock FrameXML defines these\n", \
      n_ref_fn["framexml"], n_ref["framexml"] - n_ref_fn["framexml"]
    printf "  lua       %5d functions, %5d other   <- mlua provides these\n", \
      n_ref_fn["lua"], n_ref["lua"] - n_ref_fn["lua"]

    printf "\nbenilla'\''s VM — %d globals (UiScript::new(), asked at runtime)\n\n", n_ours
    printf "1.12 globals: %d   we have: %d (%s)   missing: %d   beyond-1.12: %d (listed)\n", \
      surface, have, pct, missing, length(api)
    print  "  ^ never quote this without the split: unbuilt-feature vs missing-verb vs superset."

    printf "\nbeyond 1.12, by kind — every one of these is a deliberate exception or a bug:\n"
    printf "  %3d  benilla host bridge      Benilla*, called only by our own FrameXML\n", length(bridge)
    printf "  %3d  Lua 5.1 past 1.12'\''s 5.0  ", length(lua51); dump(lua51, "")
    printf "  %3d  WoW API past 1.12        the phase-5 list\n", length(api)
    printf "  %3d  ours in Rust that 1.12 defines in FrameXML\n", length(transcribable)
    dump(transcribable, "       ")

    if (show_beyond || length(api) <= 30) {
      printf "\nWoW API beyond 1.12 (%d):\n", length(api)
      dump(api, "  ")
    }
    if (show_missing) {
      printf "\nmissing engine globals (%d):\n", missing
      for (k in origin)
        if ((origin[k] == "engine" || origin[k] == "lua") && !(k in ours)) miss[k] = 1
      dump(miss, "  ")
    }
    print ""
  }
  # Print a set as wrapped, sorted, space-separated names.
  function dump(set, indent,   k, sorted, i, n, line) {
    n = 0
    for (k in set) sorted[++n] = k
    asort_names(sorted, n)
    line = indent
    for (i = 1; i <= n; i++) {
      if (length(line) + length(sorted[i]) + 1 > 96) { print line; line = indent }
      line = line sorted[i] " "
    }
    if (line != indent) print line
  }
  # Insertion sort — `asort` is a gawk extension and macOS ships BSD awk.
  function asort_names(a, n,   i, j, t) {
    for (i = 2; i <= n; i++) {
      t = a[i]
      for (j = i - 1; j >= 1 && a[j] > t; j--) a[j + 1] = a[j]
      a[j + 1] = t
    }
  }
' "$REF" "$ours"
