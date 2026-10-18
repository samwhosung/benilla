# `reference/`: benilla-re's tables, vendored

Byte-for-byte copies of the [benilla-re](https://github.com/samwhosung/benilla-re) tables benilla
reads, under their benilla-re names, taken at the commit `PIN` holds (40 hex digits). benilla-re
documents every column (`tables/README.md`) and rebuilds each table from the client's exe; nothing
here is edited by hand, and nothing here is Blizzard content.

The pin moves when a change needs a row benilla-re has after it, and with that change: to a
commit on benilla-re's main (the script refuses any other), then the sync, then the tests that
read the table. Update: put the new benilla-re commit in `PIN`, then

```
scripts/sync-reference.py <benilla-re checkout>            # copy the tables at the PIN
scripts/sync-reference.py <benilla-re checkout> --check    # fail when a copy differs
```

`scripts/gates.sh` runs the check when it finds a benilla-re checkout (`BENILLA_RE`, `../benilla-re`
beside the repo or beside the main checkout) and skips with a line otherwise; it fails instead
where the gates require the install data. `cargo xtask vendored <benilla> --strict` in benilla-re
makes the same comparison from its side. To vendor another table, add its name to `TABLES` in the
script.

| file | read by |
|---|---|
| `bindings.tsv` | `ui_script/shape_gate.rs`, `script/tests/widget_surface.rs`, `script/tests/glue_surface.rs` |
| `events.tsv` | `script/glue.rs` (the in-game event table, compiled in), `ui_script/event_shape_gate.rs` |
| `glue_events.tsv` | `script/glue.rs` (the glue event table, compiled in), `script/tests/glue_surface.rs` |
| `event_args.tsv` | `ui_script/event_shape_gate.rs` |
| `verb_events.tsv` | `ui_script/verb_event_gate.rs` |
| `glue_surface.tsv` | `script/tests/glue_surface.rs`, `ui_script/glue.rs` |
| `lua_globals.tsv` | `script/tests/reference_surface.rs`, `ui_script/surface_gate.rs`, `ui_script/layer_tests.rs`, `ui_script/reference_ui.rs`, `tests/message_keys.rs`, `examples/addon_harness.rs`, `scripts/api-coverage.sh` |
| `messages.tsv` | `scripts/gen-message-catalog.py`, which writes `messages/catalog.rs`; the gates check it regenerates byte for byte |
| `chat_types.tsv` | `script/chat_types.rs` (its `DEFAULTS` test) |
| `chat_groups.tsv` | `script/chat_window.rs` (its `MESSAGE_GROUPS` test) |
| `cvars.tsv` | `cvars/listing.rs` (the registry's list order, compiled in), `cvars/reference_table.rs` (the CVar table's reference column) |
