//! The getters' live reads that read an object's store raw, past the views lane: a getter fired
//! on an earlier packet's lane reads each store as the frame's last packet left it, where the
//! reference reads the object live inside its packet. The component guard
//! ([`super::guard_tests::HOST_READS_FRAME_FINAL`]) passes `ObjectStore` as versioned by the
//! views, which holds only for the reads through [`crate::ui_unit::views::ViewStores`]; this
//! holds the rest to a list, read off the source of every family the getters lend
//! ([`crate::script_calls::live::LiveReads`]) and every system parameter inside one.

use std::collections::{BTreeMap, BTreeSet};

use crate::test_support::{rel_path, rust_files, src_dir};

/// Each family's read of a store that skips the views, by `(file, parameter, fn, field)`, with what
/// it reads. The list only shrinks: a read goes once it reads through the views, and the rest at
/// the flip.
#[rustfmt::skip]
pub(super) const RAW_STORE_READS: &[(&str, &str, &str, &str, &str)] = &[
    ("death/mod.rs", "DeathView", "body", "self_q", "our store and place, for the death getters"),
    ("death/mod.rs", "DeathView", "state", "objects", "the bag items `self_res` walks for a self-resurrect item"),
    ("spell/inflight.rs", "CastState", "inflight_item_entry", "stores", "the in-flight cast's item (`0x4e53e1`)"),
    ("spell/cast_target.rs", "CastTargeting", "context_for_caster", "stores", "the caster's, the target's and its owner's stores, for the cast context"),
    ("spell/cast_target.rs", "CastTargeting", "context_on", "self_store", "our store, for the cast context"),
    ("spell/cast_target.rs", "CastTargeting", "context_on", "stores", "the target's and its owner's stores, for the cast context"),
    ("spell/targeting/mod.rs", "BindChecks", "caster", "self_q", "our store as the targeting checks' caster"),
    ("spell/targeting/mod.rs", "BindChecks", "caster", "stores", "the targeting checks' caster"),
    ("spell/targeting/mod.rs", "BindChecks", "corpse_facts", "stores", "a corpse the targeting checks test"),
    ("spell/targeting/mod.rs", "BindChecks", "player_caster", "self_q", "our store, for the player caster"),
    ("spell/targeting/mod.rs", "BindChecks", "player_caster", "stores", "slot 17's item, for the player caster"),
    ("spell/targeting/mod.rs", "BindChecks", "player_is_charmed", "self_q", "our charm word"),
    ("spell/targeting/mod.rs", "BindChecks", "unit_binds", "stores", "the target's store and its owner's"),
    ("target/live.rs", "SelectionLive", "object_name", "stores", "the named object's type and entry (`0x517126`)"),
    ("target/relations.rs", "RelationWorld", "me", "self_q", "our store, for the relation functions"),
    ("target/relations.rs", "RelationWorld", "me_guid", "self_q", "our guid with no `SelfGuid`"),
    ("target/relations.rs", "RelationWorld", "unit", "stores", "a unit the relation functions test"),
    ("target/relations.rs", "RelationWorld", "unit_and_store", "stores", "a unit the relation functions test"),
    ("ui_action/state.rs", "ActionWalk", "attack_face", "objects", "the worn weapon an attack button shows"),
    ("ui_action/state.rs", "ActionWalk", "face", "objects", "a button's item, for its face"),
    ("ui_action/state.rs", "ActionWalk", "item_count", "objects", "the bags' count of a button's item"),
    ("ui_action/state.rs", "ActionWalk", "me_live", "objects", "our store, for the action getters"),
    ("ui_item_text/live.rs", "ItemTextLive", "creator", "objects", "the read item's creator"),
    ("ui_item_text/live.rs", "ItemTextLive", "view", "objects", "the read object's title and material"),
    ("ui_loot/mod.rs", "LootSourceObjects", "is_unit", "stores", "whether the loot source is a unit"),
    ("ui_loot/mod.rs", "LootSourceObjects", "player", "stores", "whether the loot source is a player"),
    ("ui_mail/live.rs", "MailLive", "row", "me", "our name, race and sex for a row's subject"),
    ("ui_petition/live.rs", "PetitionLive", "state", "objects", "our guild id"),
    ("ui_quest/live.rs", "QuestGiverLive", "state", "objects", "the quest source's material"),
    ("ui_shapeshift/live.rs", "StanceLive", "form", "objects", "the forms' usable test"),
    ("ui_spellbook/live.rs", "BookLive", "slot", "objects", "the slot's icon and usable test"),
    ("ui_stable/live.rs", "StableLive", "view", "objects", "the live pet's entry"),
    ("ui_tabard/live.rs", "TabardLive", "host", "objects", "our guild id"),
    ("ui_taxi/live.rs", "TaxiLive", "discount", "objects", "the flight master's store and ours, for the discount"),
    ("ui_tooltip/live.rs", "TooltipLive", "with_ctx", "me", "our store, for the spell views"),
    ("ui_tooltip/live.rs", "TooltipLive", "with_ctx", "objects", "our pet and slot 17's item, for the spell views"),
    ("ui_trainer/mod.rs", "PlayerReads", "me", "self_player", "our store, for the rows' re-evaluation"),
    ("ui_trainer/mod.rs", "PlayerReads", "store", "stores", "a unit the rows' re-evaluation reads"),
];

/// The index of the bracket closing the one at `open`.
fn closing(text: &str, open: usize, lhs: u8, rhs: u8) -> Option<usize> {
    let mut depth = 0usize;
    for (j, &c) in text.as_bytes()[open..].iter().enumerate() {
        if c == lhs {
            depth += 1;
        } else if c == rhs {
            depth -= 1;
            if depth == 0 {
                return Some(open + j);
            }
        }
    }
    None
}

/// `text` with every `//` comment cut, so a word in a comment is never read as code.
fn uncommented(text: &str) -> String {
    text.lines()
        .map(|l| l.find("//").map_or(l, |i| &l[..i]))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `text` with the whitespace around each `.` cut, so a chain split over lines reads as one.
fn dotted(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut after_dot = false;
    for c in text.chars() {
        if c == '.' {
            out.truncate(out.trim_end().len());
            after_dot = true;
            out.push(c);
        } else if after_dot && c.is_whitespace() {
            continue;
        } else {
            after_dot = false;
            out.push(c);
        }
    }
    out
}

/// The identifier starting at `at`.
fn ident_at(text: &str, at: usize) -> &str {
    let rest = &text[at..];
    let end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(rest.len());
    &rest[..end]
}

/// A system parameter's fields, `(name, type)`, off its brace body.
fn fields(body: &str) -> Fields {
    let mut out = Vec::new();
    let (mut depth, mut cur) = (0i32, String::new());
    for c in body.chars() {
        match c {
            '<' | '(' | '[' | '{' => depth += 1,
            '>' | ')' | ']' | '}' => depth -= 1,
            _ => {}
        }
        if c == ',' && depth == 0 {
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    out.push(cur);
    out.into_iter()
        .filter_map(|f| {
            let f: String = f
                .lines()
                .filter(|l| !l.trim_start().starts_with("#["))
                .collect::<Vec<_>>()
                .join(" ");
            let (name, ty) = f.split_once(':')?;
            let name = name.split_whitespace().last()?.to_string();
            Some((name, ty.split_whitespace().collect::<Vec<_>>().join(" ")))
        })
        .collect()
}

/// A system parameter's fields, `(name, type)`.
type Fields = Vec<(String, String)>;

/// Every declaration of each system parameter name, `(file, fields)`.
type Params = BTreeMap<String, Vec<(String, Fields)>>;

/// Every system parameter the crate declares, by name: a derived one's and a macro-built lend
/// set's (`struct Name, NameState {`), with the file it is in and its fields.
fn system_params() -> Params {
    let mut out = Params::new();
    for file in rust_files(&src_dir()) {
        let text = uncommented(&std::fs::read_to_string(&file).expect("readable source"));
        let rel = rel_path(&file);
        for (at, _) in text.match_indices("struct ") {
            if at > 0 && text.as_bytes()[at - 1].is_ascii_alphanumeric() {
                continue;
            }
            let name = ident_at(&text, at + 7);
            let after = text[at + 7 + name.len()..].trim_start();
            let derived = text[..at].rfind("#[derive(").is_some_and(|d| {
                let attrs = &text[d..at];
                attrs.contains("SystemParam") && !attrs.contains(';') && !attrs.contains('}')
            });
            let lend_set = after.starts_with(',');
            if name.is_empty() || !(derived || lend_set) {
                continue;
            }
            let Some(open) = text[at..].find('{').map(|o| at + o) else {
                continue;
            };
            let Some(close) = closing(&text, open, b'{', b'}') else {
                continue;
            };
            out.entry(name.to_string())
                .or_default()
                .push((rel.clone(), fields(&text[open + 1..close])));
        }
    }
    out
}

/// Whether `ty` reads an object's store: the object lookup ([`crate::net::Objects`]) or a query
/// over `ObjectStore`.
fn reads_stores(ty: &str) -> bool {
    let objects = ty.match_indices("Objects").any(|(i, _)| {
        let before = ty[..i].chars().last();
        before.is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'))
            && ty[i + 7..].trim_start().starts_with('<')
    });
    objects || (ty.contains("Query<") && ty.contains("ObjectStore"))
}

/// The bodies of `name`'s impl blocks in `text`.
fn impl_bodies<'a>(text: &'a str, name: &str) -> Vec<&'a str> {
    let mut out = Vec::new();
    for (at, _) in text.match_indices("impl") {
        if at > 0 && text.as_bytes()[at - 1] != b'\n' {
            continue;
        }
        let Some(open) = text[at..].find('{').map(|o| at + o) else {
            continue;
        };
        let mut head = text[at + 4..open].trim();
        if head.starts_with('<') {
            head = closing(head, 0, b'<', b'>').map_or(head, |c| head[c + 1..].trim());
        }
        let ty = head.rsplit(" for ").next().unwrap_or(head);
        let ty = ty.split('<').next().unwrap_or(ty).trim();
        if ty.rsplit("::").next() != Some(name) {
            continue;
        }
        if let Some(close) = closing(text, open, b'{', b'}') {
            out.push(&text[open + 1..close]);
        }
    }
    out
}

/// Every read of a store through a raw field in the getters' families and the parameters inside
/// them, `(file, parameter, fn, field)`. A field's object or entity lookup alone reads no store;
/// a store lookup handed to the views as the live one they fall back to, and the object lookup
/// lent to the views' item walk, are versioned.
fn raw_store_reads() -> BTreeSet<(String, String, String, String)> {
    let params = system_params();
    let mut seen = BTreeSet::new();
    let mut queue = vec!["LiveReads".to_string()];
    let mut raw = Vec::new();
    while let Some(name) = queue.pop() {
        if !seen.insert(name.clone()) {
            continue;
        }
        for (file, fields) in params.get(&name).into_iter().flatten() {
            for (field, ty) in fields {
                for (i, _) in ty.match_indices('<') {
                    let start = ty[..i]
                        .rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                        .map_or(0, |s| s + 1);
                    let inner = &ty[start..i];
                    if params.contains_key(inner) && !seen.contains(inner) {
                        queue.push(inner.to_string());
                    }
                }
                let versioned = name == "ViewStores" || name == "Objects";
                if !versioned && reads_stores(ty) {
                    raw.push((file.clone(), name.clone(), field.clone()));
                }
            }
        }
    }
    let mut out = BTreeSet::new();
    for (file, name, field) in raw {
        let text =
            uncommented(&std::fs::read_to_string(src_dir().join(&file)).expect("readable source"));
        for body in impl_bodies(&text, &name) {
            let body = &dotted(body);
            let needle = format!("self.{field}");
            for (at, _) in body.match_indices(&needle) {
                let end = at + needle.len();
                if body[end..].starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_') {
                    continue;
                }
                let method = body[end..].strip_prefix('.').map(|m| ident_at(m, 0));
                if matches!(method, Some("player" | "entity")) {
                    continue;
                }
                let from = body[..at].rfind([';', '{', '}']).map_or(0, |s| s + 1);
                let to = body[end..].find(';').map_or(body.len(), |s| end + s);
                let statement = &body[from..to];
                let lent = statement.contains(&format!("item_objects(&{needle})"));
                let fallback = method == Some("store")
                    && (statement.contains("by_guid(") || statement.contains(".at("));
                if lent || fallback {
                    continue;
                }
                let func = body[..at]
                    .rmatch_indices("fn ")
                    .map(|(f, _)| ident_at(body, f + 3))
                    .next()
                    .unwrap_or_default();
                out.insert((file.clone(), name.clone(), func.to_string(), field.clone()));
            }
        }
    }
    out
}

/// Every getter family's store read that skips the views has its row in [`RAW_STORE_READS`], and
/// every row names such a read, so a new raw read is counted as it lands.
#[test]
fn every_raw_store_read_in_the_getters_is_listed() {
    let found = raw_store_reads();
    assert!(
        found.iter().any(|(_, name, ..)| name == "TooltipLive"),
        "the scan reaches the getters' families: {found:?}"
    );
    let listed: BTreeSet<(String, String, String, String)> = RAW_STORE_READS
        .iter()
        .map(|&(f, p, func, field, _)| (f.into(), p.into(), func.into(), field.into()))
        .collect();
    let new: Vec<String> = found
        .difference(&listed)
        .map(|(f, p, func, field)| format!("({f:?}, {p:?}, {func:?}, {field:?}, \"…\"),"))
        .collect();
    let stale: Vec<String> = listed
        .difference(&found)
        .map(|r| format!("{r:?}"))
        .collect();
    assert!(
        new.is_empty(),
        "these getter reads take an object's store raw, past the views, as the frame left it: \
         read it through `ViewStores`, or give it its row in RAW_STORE_READS with what it \
         reads:\n  {}",
        new.join("\n  ")
    );
    assert!(
        stale.is_empty(),
        "these RAW_STORE_READS rows name no raw read: drop them\n  {}",
        stale.join("\n  ")
    );
}
