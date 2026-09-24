# `reference/`: verified facts about the 1.12.1 client, vendored

Generated tables describing the reference client, committed so benilla can measure itself against
them on any machine without re-deriving them. Data, not documentation: each has a generator beside
it in `scripts/`.

Nothing here is Blizzard content. A catalogue of names is not the thing named, the same way a
comment may quote a byte address and `docs/MAP.md` may name a file.

## `1.12-globals.tsv`

The 1.12.1 client's entire global namespace: 21,555 names, `name<TAB>type<TAB>origin`.

Where it comes from: a dump of `_G` inside the running reference client, which is the surface an
addon sees (FrameXML-defined globals as well as engine ones), unioned with every global the shipped
UI defines. The union matters because the twelve `Blizzard_*` addons are LoadOnDemand: a live dump
only carries their globals if their windows were opened first, so the 1,986 rows a dump cannot see
carry type `lod`. The table answers "is this a 1.12 name?", not "was this name live in one session".

The `origin` column says whose job it is to provide the name, attributed by its definition site in
the complete 1.12 shipped-UI corpus:

| origin | count | meaning |
|---|---|---|
| `engine` | 1,104 | the C client provides it; benilla implements these in Rust |
| `framexml` | 20,427 | the shipped UI defines it; the stock files run off the player's own install (`ui_script::reference_ui`) |
| `lua` | 24 | Lua 5.0's own runtime |

The `framexml` bulk is UI objects, `ContainerFrame1Item16IconTexture` and its 11,000 siblings,
which materialize from the XML and are nobody's to implement. The number that matters for the API
surface is the function split: 1,100 engine functions, 1,075 FrameXML functions. A name defined in
shipped Lua is FrameXML's, and hardcoding it in Rust is a category error even when it works.

Regenerating: `scripts/gen-reference-globals.py`, whose header carries the method and the traps. It
needs a 1.12 install, its FrameXML extracted to a folder, and the maintainer's runtime capture of
the reference client's `_G`, which is not in this repo, so it is a manual regeneration. The table is
stable: it describes a client that shipped in 2006.

Reading it: `scripts/api-coverage.sh` asks a real `UiScript::new()` what benilla exposes and reports
`have / missing / beyond-1.12`. `crates/benilla-ui/src/script/tests/reference_surface.rs` is the
gate: it fails if benilla grows a global that 1.12 does not have and nobody wrote down why.

## `1.12-shapes.tsv` and `1.12-events.tsv`

The binding-shape table (what each Lua verb takes and returns, gated by `ui_script/shape_gate.rs`)
and the event table with the arguments each producer pushes (gated by
`ui_script/event_shape_gate.rs`). Each file's header carries its column contract.

Regenerating: `1.12-shapes.tsv` is vendored as the reference analysis writes it.
`scripts/gen-reference-events.py` derives `1.12-events.tsv` from two more vendored tables,
`1.12-event-catalog.tsv` (event id to name, off the client's name-pointer array) and
`1.12-event-firesites.tsv` (every fire site with the format string it pushes), so it runs from any
clone and reproduces the committed file byte for byte.

## `1.12-message-catalog.tsv`

The client's message registry (`0xb4b498`: id, key, kind, sound, chat type).
`scripts/gen-message-catalog.py` re-shapes it into `crates/benilla-ui/src/messages/catalog.rs`,
the table `CGGameUI::DisplayError` indexes.

## `1.12-verb-events.tsv`

Every event the reference fires from inside a Lua verb, keyed by the verb: the verb's own body, or
a helper that only registered verbs call (`0x4d8c90` is the trainer's). The two tables above say
what an event carries and whether something fires it; this one says who. A stock file that calls a
verb for its side effect of an event repaints nothing when the verb fires nothing.

Regenerating: `scripts/gen-reference-verb-events.py`, whose header carries the rule, the two
families it drops (glue-space verbs, the unit-field bridge) and why getters are excluded (a getter's
fire is a cache-miss re-query, a state event benilla's feeds fire off the packet). A fire site is
attributed to a function by the verified function extents plus the disassembly's padding
boundaries, and a site past its candidate's extent is left out rather than mis-filed. The fire
sites and the shapes are the vendored tables above; the function names, their extents and the
disassembly are the maintainer's, so this one is a manual regeneration.

Reading it: `crates/benilla-app/src/ui_script/verb_event_gate.rs` is the gate. For every pair whose
verb benilla registers, the module that registers the verb fires the event, or the pair is declared
there: `ELSEWHERE` (a feed fires it on the state the verb changes, and the named file is checked
too) or `GAP` (nothing does, with the reason).
