#!/usr/bin/env bash
# Generate docs/MAP.md, the map of what is built, and docs/DEVIATIONS.md, where benilla differs from
# 1.12.1, both from what is on disk.
#
# Regenerated and committed at every land; run it by hand only to look, never edit the output. No
# timestamps, so an identical tree gives an identical file. No `pipefail`/`-e`: a grep that finds
# nothing (a single-file lib, a crate with no bins) must not abort the generator. Bash and POSIX
# awk only (macOS's awk is the floor): no gawk extensions, no new dependencies.
set -u
cd "$(dirname "$0")/.."

# Extract a quoted `key = "value"` field from a Cargo.toml.
field() { grep -m1 "^$2" "$1" 2>/dev/null | sed -E "s/^$2[[:space:]]*=[[:space:]]*\"//; s/\".*$//"; }

{
  echo "# benilla — generated map"
  echo
  echo "> **GENERATED** by \`scripts/genmap.sh\` from the code — do not edit by hand; rerun the script"
  echo "> when what's built changes. *What-is* only: the *what-changed* is in git. No timestamp —"
  echo "> identical code yields an identical map."
  echo

  echo "## Crates"
  echo
  for ct in crates/*/Cargo.toml; do
    name=$(field "$ct" name)
    desc=$(field "$ct" description)
    # Crates here write `bevy.workspace = true`: the trailing class accepts `.`, whitespace and `=`
    # (`bevy = { … }`, `bevy="…"` too) and still rejects a `bevyfoo` crate.
    if grep -qE '^(bevy|bevy_egui|avian3d)[.[:space:]=]' "$ct"; then bevy="Bevy"; else bevy="no Bevy"; fi
    echo "- **$name** ($bevy) — ${desc:-—}"
  done
  echo

  echo "## App subsystems — Bevy plugins in load order (\`crates/benilla-app/src/lib.rs\`)"
  echo
  # Accept bare (`add_plugins(FooPlugin)`) and path-qualified (`add_plugins(foo::FooPlugin)`)
  # registrations; print the plugin type name either way.
  grep -oE 'add_plugins\(([a-z_]+::)*[A-Za-z_]+Plugins?' crates/benilla-app/src/lib.rs \
    | sed -E 's/add_plugins\(//; s/([a-z_]+::)+//' | awk '!seen[$0]++ {print "- " $0}'
  echo

  # `lib.rs` adds each dev group in one line, so their members are read out of `dev.rs`.
  echo "### Behind the \`dev\` feature (\`crates/benilla-app/src/dev.rs\` — compiled out by \`--no-default-features\`)"
  echo
  grep -oE 'add_plugins\(([a-z_]+::)*[A-Za-z_]+Plugins?' crates/benilla-app/src/dev.rs \
    | sed -E 's/add_plugins\(//; s/([a-z_]+::)+//' | awk '!seen[$0]++ {print "- " $0}'
  echo

  echo "## Modules (top-level, per crate)"
  echo
  for lib in crates/*/src/lib.rs crates/*/src/main.rs; do
    [ -f "$lib" ] || continue
    crate=$(echo "$lib" | sed -E 's@crates/([^/]+)/.*@\1@')
    mods=$(grep -E '^[[:space:]]*(pub(\([a-z]+\))? )?mod [a-z_0-9]+;' "$lib" \
      | sed -E 's/.*mod ([a-z_0-9]+);.*/\1/' | sort | tr '\n' ' ')
    [ -n "$mods" ] && echo "- **$crate** ($(basename "$lib")): $mods"
  done
  echo

  echo "## CLI binaries"
  echo
  # Both forms cargo accepts: `src/bin/<name>.rs` and `src/bin/<name>/main.rs`.
  for b in crates/*/src/bin/*.rs crates/*/src/bin/*/main.rs; do
    [ -f "$b" ] || continue
    crate=$(echo "$b" | sed -E 's@crates/([^/]+)/.*@\1@')
    case "$b" in
      */main.rs) name=$(basename "$(dirname "$b")") ;;
      *) name=$(basename "$b" .rs) ;;
    esac
    echo "- \`$name\` (in $crate)"
  done | LC_ALL=C sort
  echo

  # Shaders are embedded and addressed by their owning crate, so the crate is part of the name.
  echo "## WGSL shaders (\`crates/*/src/shaders/\`, embedded)"
  echo
  for s in crates/*/src/shaders/*.wgsl; do
    [ -f "$s" ] || continue
    crate=$(echo "$s" | sed -E 's@crates/([^/]+)/.*@\1@' | tr - _)
    echo "- \`embedded://$crate/shaders/$(basename "$s")\`"
  done
  echo

  echo "## Instruments — \`\$WOW_*\` switches (env-var read sites)"
  echo
  echo "> The headless/dev instrument fleet, discovered from the code: every \`WOW_*\` env var"
  echo "> some \`.rs\` reads, and where it's read — the doc comment at the read site is the"
  # Name no dev keys here: they are listed once, in the debug panel's footer.
  echo "> semantics. (The in-window surfaces — the Ctrl+Shift dev-chord overlays — are plugins"
  echo "> above; this indexes the switches that don't announce themselves.)"
  echo
  # Match any quoted "WOW_*" literal: reads also go through helpers (`knob("WOW_FX_AGE", …)`), and
  # doc comments write the unquoted form. The probe registry `capture/probe_env.rs` is a table,
  # not a read site, so it is left out.
  grep -rHoE '"WOW_[A-Z0-9_]+"' crates --include='*.rs' 2>/dev/null \
    | grep -v '^crates/benilla-app/src/capture/probe_env\.rs:' \
    | sed -E 's@^crates/@@; s/:"/\t/; s/"$//' \
    | awk -F'\t' '{print $2 "\t" $1}' | sort -u \
    | awk -F'\t' '$1 != v { if (v) print line; v = $1; line = "- `" $1 "` — " $2; next }
                  { line = line ", " $2 }
                  END { if (v) print line }'
  echo

  # Every script, with the first sentence of its own header.
  echo "## Scripts (\`scripts/\`)"
  echo
  for f in scripts/*.py scripts/*.sh; do
    [ -f "$f" ] || continue
    # The summary: the header's first non-empty content after the shebang, comment or docstring
    # markers stripped, joined until its first blank line, cut at a sentence end or ~150 chars.
    summary=$(LC_ALL=C awk '
      NR == 1 && /^#!/ { next }
      /^@echo off/ { next }
      { line = $0
        sub(/^[[:space:]]*(r?"""|#|\/\/|rem)[[:space:]]?/, "", line)
        if (line ~ /^[[:space:]]*$/) { if (got) exit; else next }
        if (line ~ /^"""/) exit
        got = 1; out = out (out ? " " : "") line }
      END { print out }' "$f" | sed -E 's/\*\*//g; s/^[A-Za-z0-9_.\/-]+ (—|--) //; s/^([^.]*[.!?])([[:space:]]|$).*/\1/; s/^(.{150}).+/\1…/')
    echo "- \`${f#scripts/}\` — $summary"
  done
  echo

} > docs/MAP.md

echo "genmap: wrote docs/MAP.md"

# ── docs/DEVIATIONS.md ─────────────────────────────────────────────────────────────────────────────
# Every place benilla writes down that it differs from 1.12.1: the `Deviates` rows of the CVar table,
# and each `Deviation:` comment in the crates (Rust and WGSL `//`, Lua `--`, XML `<!-- -->`). A
# deviation's wording is read from where it lives, so the list cannot drift from the code.
tmp=$(mktemp -d "${TMPDIR:-/tmp}/genmap.XXXXXX") || exit 1
trap 'rm -rf "$tmp"' EXIT

# Test code is left out: a file under `tests/`, named `tests.rs` or `*_tests.rs`, or declared by a
# `#[cfg(test)] mod x;` (listed here, resolved the way rustc does, `#[path]` included), and a
# `#[cfg(test)] mod x { … }` body (skipped in the extractor below, which closes it at the first `}`
# in the module's own column; rustfmt keeps it there).
grep -rlE --include='*.rs' 'cfg\(test\)' crates 2>/dev/null | LC_ALL=C sort | xargs awk '
  FNR == 1 { pend = 0; path = ""; base = FILENAME; sub(/[^\/]*$/, "", base)
             stem = FILENAME; sub(/.*\//, "", stem); sub(/\.rs$/, "", stem)
             own = (stem == "mod" || stem == "lib" || stem == "main") ? base : base stem "/" }
  { t = $0; sub(/^[ \t]+/, "", t) }
  t == "#[cfg(test)]" { pend = 1; path = ""; next }
  pend && t ~ /^#\[path = "/ { path = t; sub(/^#\[path = "/, "", path); sub(/".*/, "", path); next }
  pend && t ~ /^#\[/ { next }
  pend {
    pend = 0
    if (t ~ /^(pub(\([a-z]+\))? )?mod [A-Za-z_0-9]+;/) {
      name = t; sub(/^(pub(\([a-z]+\))? )?mod /, "", name); sub(/;.*/, "", name)
      if (path != "") print base path
      else { print own name ".rs"; print own name "/mod.rs" }
    }
  }' > "$tmp/testmods" 2>/dev/null

# Stage 1: one TSV row per `Deviation:` comment: crate, module, path in the crate, line, sentence.
LC_ALL=C grep -rIlE 'Deviation:' crates 2>/dev/null | LC_ALL=C sort | grep -vE '/tests/|(^|/)tests\.rs$|_tests\.rs$' \
  | xargs env LC_ALL=C awk -v list="$tmp/testmods" '
  function indent_of(s,   t) { t = s; sub(/[^ \t].*$/, "", t); return length(t) }
  function trim(s) { sub(/^[ \t]+/, "", s); sub(/[ \t]+$/, "", s); return s }
  # The first sentence of a joined comment: to the first `.`/`?`/`!` that is followed by an
  # uppercase word or the end, outside backticks and parentheses, and not an abbreviation.
  function first_sentence(t,   len, k, ch, tick, par, nx, rest, pre) {
    len = length(t); tick = 0; par = 0
    for (k = 1; k <= len; k++) {
      ch = substr(t, k, 1)
      if (ch == "`") { tick = !tick; continue }
      if (tick) continue
      if (ch == "(") par++
      else if (ch == ")") { if (par > 0) par-- }
      else if ((ch == "." || ch == "?" || ch == "!") && par == 0) {
        nx = substr(t, k + 1, 1)
        if (nx == "") return substr(t, 1, k)
        if (nx != " ") continue
        rest = substr(t, k + 2); sub(/^ +/, "", rest)
        if (rest != "" && substr(rest, 1, 1) !~ /[A-Z]/) continue
        pre = tolower(substr(t, 1, k - 1))
        if (pre ~ /(^|[^a-z])(vs|cf|ca)$/ || pre ~ /(e\.g|i\.e)$/) continue
        return substr(t, 1, k)
      }
    }
    return t
  }
  # The sentence, and the next one too while it is a fragment (`min(I, 1)`, `returns nothing`).
  function lead(t,   s, rest) {
    s = first_sentence(t)
    while (length(s) < 70 && length(s) < length(t)) {
      rest = substr(t, length(s) + 1); sub(/^ +/, "", rest)
      s = s " " first_sentence(rest)
    }
    return s
  }
  # Where a `Deviation:` label starts in a comment, 0 if none: one inside backticks is a mention.
  function label_at(c,   at, off, rest) {
    off = 0; rest = c
    while ((at = index(rest, "Deviation:")) > 0) {
      if (substr(rest, at - 1, 1) != "`" || at == 1) return off + at
      off += at + 9; rest = substr(rest, at + 10)
    }
    return 0
  }
  function module_of(r,   rest, m) {
    if (r ~ /^src\//) {
      rest = substr(r, 5)
      if (index(rest, "/")) { sub(/\/.*/, "", rest); return rest }
      sub(/\.[^.]*$/, "", rest)
      return (rest == "lib" || rest == "main") ? "(crate root)" : rest
    }
    if (r ~ /^assets\//) { rest = substr(r, 8); m = "assets"; if (index(rest, "/")) { sub(/\/.*/, "", rest); m = m "/" rest } return m }
    sub(/\/.*/, "", r); return r
  }
  function process(f,   n, i, j, s, t, ext, xin, skipping, tindent, pend, p, q, c, kind, code, marker, text, lab, cr, rel, first, cont) {
    n = 0; delete L; delete K; delete C; delete Q; delete XI; delete T
    while ((getline s < f) > 0) L[++n] = s
    close(f)
    ext = f; sub(/.*\./, "", ext)
    if (ext != "rs" && ext != "wgsl" && ext != "xml" && ext != "lua") return
    cr = f; sub(/^crates\//, "", cr); sub(/\/.*/, "", cr)
    rel = f; sub(/^crates\/[^\/]*\//, "", rel)
    xin = 0; skipping = 0; pend = 0
    for (i = 1; i <= n; i++) {
      s = L[i]; t = s; sub(/^[ \t]+/, "", t)
      T[i] = 0
      if (ext == "rs") {
        if (skipping) { T[i] = 1; if (indent_of(s) == tindent && t ~ /^}/) skipping = 0 }
        else if (t == "#[cfg(test)]") pend = 1
        else if (pend) {
          if (t !~ /^#\[/) {
            pend = 0
            if (t ~ /^(pub(\([a-z]+\))? )?mod [A-Za-z_0-9]+ *\{[ \t]*$/) { skipping = 1; tindent = indent_of(s); T[i] = 1 }
          }
        }
      }
      XI[i] = xin; K[i] = ""; C[i] = ""; Q[i] = 0
      if (xin) {
        K[i] = "x"; c = s
        if ((q = index(c, "-->")) > 0) { c = substr(c, 1, q - 1); xin = 0 }
        C[i] = trim(c)
      } else if (ext == "xml" || ext == "lua") {
        p = index(s, "<!--")
        if (p > 0 && ext == "xml") {
          K[i] = "x"; c = substr(s, p + 4); Q[i] = (trim(substr(s, 1, p - 1)) != "")
          if ((q = index(c, "-->")) > 0) c = substr(c, 1, q - 1); else xin = 1
          C[i] = trim(c)
        } else if ((p = index(s, "--")) > 0) {
          K[i] = "--"; c = substr(s, p + 2); sub(/^-+/, "", c); Q[i] = (trim(substr(s, 1, p - 1)) != "")
          C[i] = trim(c)
        }
      } else {
        p = index(s, "//")
        if (p > 0) {
          c = substr(s, p + 2); marker = "//"
          if (substr(c, 1, 1) == "/") { c = substr(c, 2); marker = "///" }
          else if (substr(c, 1, 1) == "!") { c = substr(c, 2); marker = "//!" }
          K[i] = marker; Q[i] = (trim(substr(s, 1, p - 1)) != "")
          C[i] = trim(c)
        }
      }
    }
    for (i = 1; i <= n; i++) {
      if (K[i] == "" || (lab = label_at(C[i])) == 0) continue
      text = trim(substr(C[i], lab + 10))
      # Continue over the comment lines that follow, to its paragraph end: a blank comment line,
      # a list item, a code fence or the next `Deviation:` ends it.
      for (j = i + 1; j <= n; j++) {
        if (K[i] == "x") { if (!XI[j] || K[j] != "x") break }
        else if (K[j] != K[i] || Q[j]) break
        cont = C[j]
        if (cont == "" || cont ~ /^([-*] |```)/ || label_at(cont)) break
        text = text " " cont
      }
      text = trim(lead(trim(text)))
      gsub(/\t/, " ", text)
      # A rustdoc link [`x`] or [`x`](path) reads as `x`.
      while (match(text, /\[`[^`]*`\]\([^)]*\)/)) { q = index(substr(text, RSTART), "]("); text = substr(text, 1, RSTART - 1) substr(text, RSTART + 1, q - 2) substr(text, RSTART + RLENGTH) }
      gsub(/\[`/, "`", text); gsub(/`\]/, "`", text)
      # A plain lowercase first word starts a sentence; an identifier (`pfUI`, `a.b`) is left alone.
      if (match(text, /^[a-z]+/) && (substr(text, RLENGTH + 1, 1) == "" || substr(text, RLENGTH + 1, 1) ~ /^[ ;,-]$/)) text = toupper(substr(text, 1, 1)) substr(text, 2)
      sub(/[.;,:]+$/, "", text)
      if (text == "") text = "(see the comment)"
      if (T[i] || skipfile) { skipped++; continue }
      printf "%s\t%s\t%s\t%d\t%s\n", cr, module_of(rel), rel, i, text
    }
  }
  BEGIN {
    while ((getline s < list) > 0) tests[s] = 1
    close(list)
    for (a = 1; a < ARGC; a++) { skipfile = (ARGV[a] in tests); process(ARGV[a]) }
    if (skipped) print "genmap: " skipped " deviation comment(s) in test code left out" > "/dev/stderr"
    exit
  }' > "$tmp/code.tsv"
LC_ALL=C sort -t "$(printf '\t')" -k1,1 -k2,2 -k3,3 -k4,4n "$tmp/code.tsv" > "$tmp/code.sorted"

# Stage 2: the `Deviates` rows of the CVar table. The whole file is scanned as a token stream
# (comments, strings and char literals skipped), so a call may sit on one line or twelve. A literal
# is read with Rust's `\`-newline continuation and `\"`/`\\` escapes; a constant argument
# (`crate::realmlist::CVAR_REALMLIST`) is resolved to its `const NAME: &str = "…";` in the crates.
# `cvars::tests::the_deviates_rows_are_what_the_generator_reads` pins this reading to the table.
LC_ALL=C awk '
  { src = src $0 "\n" }
  function resolve(e,   id, cmd, line) {
    id = e; sub(/.*::/, "", id)
    if (id !~ /^[A-Za-z_][A-Za-z_0-9]*$/) return "`" e "`"
    cmd = "grep -rhE --include=\"*.rs\" \"const " id ": &str = \\\"\" crates 2>/dev/null"
    cmd | getline line; close(cmd)
    if (line ~ /= "/) { sub(/^[^"]*"/, "", line); sub(/";.*$/, "", line); return line }
    return "`" e "`"
  }
  function str_at(i,   out, c, d) {         # i is on the opening quote; returns the decoded text, sets endi
    out = ""; i++
    while (i <= N) {
      c = substr(src, i, 1)
      if (c == "\"") { endi = i; return out }
      if (c == "\\") {
        d = substr(src, i + 1, 1)
        if (d == "\n") { i += 2; while (substr(src, i, 1) ~ /[ \t\n]/) i++; continue }
        if (d == "\"" || d == "\\" || d == "'"'"'") { out = out d; i += 2; continue }
        if (d == "n") { out = out " "; i += 2; continue }
      }
      out = out c; i++
    }
    endi = N; return out
  }
  function skip_noise(i,   c, d) {            # returns the index past a comment, or i itself
    c = substr(src, i, 1); d = substr(src, i + 1, 1)
    if (c == "/" && d == "/") { while (i <= N && substr(src, i, 1) != "\n") i++; return i }
    if (c == "/" && d == "*") { i += 2; while (i <= N && substr(src, i, 2) != "*/") i++; return i + 2 }
    return i
  }
  function call(i,   depth, c, k, arg, a, isstr, lit, expr, j) {
    # i is just past the opening paren
    depth = 1; k = 1; expr = ""; lit = 0; delete A
    while (i <= N && depth > 0) {
      j = skip_noise(i); if (j != i) { i = j; continue }
      c = substr(src, i, 1)
      if (c == "\"") { A[k] = str_at(i); lit = 1; i = endi + 1; isl[k] = 1; continue }
      if (c == "(" || c == "[" || c == "{") depth++
      if (c == ")" || c == "]" || c == "}") { depth--; if (depth == 0) break }
      if (c == "," && depth == 1) { k++; i++; continue }
      if (c !~ /[ \t\n]/) { if (!isl[k]) A[k] = A[k] c }
      i++
    }
    for (a = 1; a <= 4; a++) {
      arg = A[a]
      if (!isl[a]) arg = resolve(arg)
      gsub(/[\t\n]/, " ", arg)
      printf "%s%s", arg, (a < 4 ? "\t" : "\n")
    }
    delete isl
    return i
  }
  END {
    N = length(src); i = 1; prev = ""
    while (i <= N) {
      j = skip_noise(i); if (j != i) { i = j; continue }
      c = substr(src, i, 1)
      if (c == "\"") { str_at(i); i = endi + 1; continue }
      if (c == "'"'"'") {
        if (substr(src, i + 2, 1) == "'"'"'") i += 3
        else if (substr(src, i + 1, 1) == "\\") { i += 2; while (i <= N && substr(src, i, 1) != "'"'"'") i++; i++ }
        else i++
        continue
      }
      if (c ~ /[A-Za-z_]/) {
        id = c; i++
        while (i <= N && substr(src, i, 1) ~ /[A-Za-z_0-9]/) { id = id substr(src, i, 1); i++ }
        if (id == "deviates" && prev != "fn" && substr(src, i, 1) == "(") i = call(i + 1)
        prev = id; continue
      }
      i++
    }
  }' crates/benilla-app/src/cvars/table.rs 2>/dev/null \
  | LC_ALL=C sort -f -t "$(printf '\t')" -k1,1 > "$tmp/cvars.tsv"

# Markdown-escape outside code spans (an odd backtick count means no spans: every backtick escaped).
MDESC='
  function mdesc(t, cell,   out, k, ch, tick, n) {
    n = gsub(/`/, "`", t); out = ""; tick = 0
    for (k = 1; k <= length(t); k++) {
      ch = substr(t, k, 1)
      if (ch == "`") { if (n % 2) out = out "\\`"; else { tick = !tick; out = out ch } continue }
      if (tick) { out = out ((cell && ch == "|") ? "\\|" : ch); continue }
      if (ch ~ /[\\*_<\[\]&]/ || (cell && ch == "|")) out = out "\\"
      out = out ch
    }
    return out
  }'

{
  echo "# benilla — generated deviations"
  echo
  echo "> **GENERATED** by \`scripts/genmap.sh\` from the code — do not edit by hand; rerun the script"
  echo "> when a deviation is added or reworded. No timestamp — identical code yields an identical file."
  echo
  echo "benilla is the 1.12.1 client reimplemented: what the reference does is what benilla does. Where it"
  echo "differs on purpose, that is a deviation, the maintainer's call alone (\`docs/METHOD.md\`), and it is"
  echo "written down where it lives: a setting's \`Deviates\` row in the CVar table, or a \`Deviation:\` comment"
  echo "at the code, the Lua or the XML that differs. This file gathers them all, regenerated at every land,"
  echo "so the list is read from the code and cannot drift from it. Test code is left out, and so are the"
  echo "CVars that are benilla's own knobs with no 1.12.1 counterpart (\`ours\` rows)."
  echo
  echo "## Settings that ship off the reference default"
  echo
  echo "A setting boots at the reference's default unless it is listed here (\`docs/METHOD.md\`)."
  echo
  echo "| CVar | 1.12.1 | benilla | Why |"
  echo "| --- | --- | --- | --- |"
  LC_ALL=C awk -F '\t' "$MDESC"'
    function code(v) { return "`" (v == "" ? "\"\"" : v) "`" }
    { printf "| %s | %s | %s | %s |\n", code($1), code($3), code($2), mdesc($4, 1) }' "$tmp/cvars.tsv"
  echo
  echo "## Deviations in the code"
  echo
  echo "Each entry is the first sentence of a \`Deviation:\` comment, linking to where it lives."
  echo
  LC_ALL=C awk -F '\t' '{ c[$1]++; if (!($1 in seen)) { seen[$1] = 1; order[++n] = $1 } }
    END { for (i = 1; i <= n; i++) printf "- [%s](#%s): %d\n", order[i], order[i], c[order[i]] }' "$tmp/code.sorted"
  echo
  LC_ALL=C awk -F '\t' "$MDESC"'
    $1 != crate { crate = $1; module = ""; printf "### %s\n\n", crate }
    $2 != module { if (module != "") print ""; module = $2; printf "#### %s\n\n", module }
    { printf "- %s. ([`%s:%d`](../crates/%s/%s#L%d))\n", mdesc($5), $3, $4, $1, $3, $4 }
    END { print "" }' "$tmp/code.sorted"
} > docs/DEVIATIONS.md

echo "genmap: wrote docs/DEVIATIONS.md ($(wc -l < "$tmp/code.sorted" | tr -d ' ') code entries, $(wc -l < "$tmp/cvars.tsv" | tr -d ' ') CVar rows)"
