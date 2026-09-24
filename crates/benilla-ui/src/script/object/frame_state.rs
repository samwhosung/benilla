//! Frame method-table cluster: Show/Hide/visibility, identity/hierarchy (`GetName`/`GetParent`/
//! `SetParent`), strata/level/scale/alpha, the Backdrop mechanism (`SetBackdrop`/`SetBackdropColor`/
//! `SetBackdropBorderColor`), and mouse-enable. Split out of [`super`] purely for size — the shared
//! id/handle plumbing, `CreateFrame`, and the method-table wiring stay there; this module's
//! [`install`] just populates its share of the one shared method table.

use mlua::{Lua, Table, Value};

use crate::script::region_map::{set_shared, Side};

use crate::order::Strata;
use crate::script::{event, Backdrop, Insets, Model};
use crate::widget::FrameKind;

use super::{decode_id, draw_layer_from_str, frame_handle_of, frame_wrapper, strata_from_str};

/// Populate `m`'s visibility/hierarchy/strata/backdrop/mouse methods (see the module doc).
pub(super) fn install(lua: &Lua, m: &Table) -> mlua::Result<()> {
    // Show / Hide / visibility. **`SetShown` is not here and must not come back**: the branchless
    // setter belongs to a later expansion — no 1.12 method table registers it, and neither the
    // stock chain nor either addon corpus calls it (decision 2142's census). A frame is shown or
    // hidden by the two verbs the era has.
    m.set(
        "Show",
        lua.create_function(|lua, this: Table| set_shown(lua, &this, true))?,
    )?;
    m.set(
        "Hide",
        lua.create_function(|lua, this: Table| set_shown(lua, &this, false))?,
    )?;
    m.set(
        "IsShown",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(crate::script::binding_abi::flag(
                model.arena.frame(h).map(|f| f.shown).unwrap_or(false),
            ))
        })?,
    )?;
    m.set(
        "IsVisible",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(crate::script::binding_abi::flag(
                model
                    .arena
                    .frame(h)
                    .map(|f| f.effective_visible)
                    .unwrap_or(false),
            ))
        })?,
    )?;
    // Identity / hierarchy
    set_shared(lua, m, Side::Frame, "GetName", |lua, this: Table| {
        let h = frame_handle_of(lua, &this)?;
        let name = {
            let model = lua.app_data_ref::<Model>().expect("model");
            model.arena.frame(h).and_then(|f| f.name.clone())
        };
        match name {
            Some(n) => Ok(Value::String(lua.create_string(&n)?)),
            None => Ok(Value::Nil),
        }
    })?;
    // GetID/SetID — the app-meaning-free numeric label (XML `id=`, the client's `+0xb4`): a
    // dropdown row's list position, a tab index. Default 0 (see `Frame::wow_id`).
    m.set(
        "GetID",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(model.arena.frame(h).map(|f| f.wow_id).unwrap_or(0))
        })?,
    )?;
    m.set(
        "SetID",
        lua.create_function(|lua, (this, id): (Table, i64)| {
            let h = frame_handle_of(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            if let Some(f) = model.arena.frame_mut(h) {
                f.wow_id = id;
            }
            Ok(())
        })?,
    )?;
    // ── GetObjectType / IsObjectType, frame side (`0x7a11d0`/`0x7a1290`) ─────────────────────────
    //
    // The region side landed first because a FontString found it; `_Nameplates` is why this half
    // exists too — `_Nameplates.lua:164` tests `Nameplate:GetObjectType() ~= "Button"` and `:191`
    // tests `== "StatusBar"`, on FRAMES, in the same file whose `Region:GetObjectType()` calls the
    // region verbs answer.
    //
    // The chains are a hardcoded straight-line list per class in the binary, not a runtime parent
    // walk, so they are a table here too. Three of them are things a reasonable person invents
    // wrongly:
    //
    //  · **`ScrollingMessageFrame` derives from `Frame`, NOT from `MessageFrame`.** The name says
    //    otherwise and the roster is explicit (`0x787940`).
    //  · **`SimpleHTML` is spelled with a capitalised HTML.** Our enum variant is `SimpleHtml`, so
    //    anything derived from the variant name — `format!("{:?}")` and friends — would hand
    //    addons `"SimpleHtml"`, and `GetObjectType` is compared with `==` (`IsObjectType`'s
    //    case-folding would hide it; the getter's would not).
    //  · **There is no `Cooldown` type.** 1.12.1's census finds 23 type-name globals and none is
    //    that: the reference builds its cooldown as a `Model` playing `UI-Cooldown-Indicator.mdx`
    //    (`CooldownFrameTemplate`), and since decision 2019 so does this engine — the Era-shaped
    //    `FrameKind::Cooldown` that modelled the mechanism first-class (0137 phase 4) is gone, so
    //    `CreateFrame("Cooldown")` is the reference's own unknown-type refusal, never an announced
    //    Era type to a Lua ecosystem that branches on presence (the superset 1189 took back out).
    fn type_chain(kind: FrameKind) -> &'static [&'static str] {
        match kind {
            FrameKind::Frame => &["Frame", "Region"],
            FrameKind::WorldFrame => &["Frame", "Region"],
            FrameKind::Button => &["Button", "Frame", "Region"],
            FrameKind::CheckButton => &["CheckButton", "Button", "Frame", "Region"],
            // `CLootButton::IsObjectType 0x495af0` prepends its own name to the base's three
            // (`"Button"` `[0x879954]`, `"Frame"` `[0x878560]`, `"Region"` `[0x878870]`) — read
            // off the function, not assumed from the class hierarchy.
            FrameKind::LootButton => &["LootButton", "Button", "Frame", "Region"],
            FrameKind::EditBox => &["EditBox", "Frame", "Region"],
            FrameKind::StatusBar => &["StatusBar", "Frame", "Region"],
            FrameKind::Slider => &["Slider", "Frame", "Region"],
            FrameKind::ScrollFrame => &["ScrollFrame", "Frame", "Region"],
            FrameKind::Model => &["Model", "Frame", "Region"],
            // 4 deep: `PlayerModel` derives from `Model`, and `DressUpModel` (1969) and
            // `TabardModel` (1977) derive from it in turn, for the roster's maximum depth of 5
            // (the reference's full 23-class roster).
            FrameKind::PlayerModel => &["PlayerModel", "Model", "Frame", "Region"],
            FrameKind::DressUpModel => &["DressUpModel", "PlayerModel", "Model", "Frame", "Region"],
            FrameKind::TabardModel => &["TabardModel", "PlayerModel", "Model", "Frame", "Region"],
            FrameKind::MessageFrame => &["MessageFrame", "Frame", "Region"],
            FrameKind::ScrollingMessageFrame => &["ScrollingMessageFrame", "Frame", "Region"],
            FrameKind::ColorSelect => &["ColorSelect", "Frame", "Region"],
            FrameKind::SimpleHtml => &["SimpleHTML", "Frame", "Region"],
            FrameKind::MovieFrame => &["MovieFrame", "Frame", "Region"],
            FrameKind::GameTooltip => &["GameTooltip", "Frame", "Region"],
            FrameKind::Minimap => &["Minimap", "Frame", "Region"],
        }
    }
    fn chain_of(lua: &Lua, this: &Table) -> mlua::Result<&'static [&'static str]> {
        let h = frame_handle_of(lua, this)?;
        let model = lua.app_data_ref::<Model>().expect("model");
        Ok(model
            .arena
            .frame(h)
            .map_or(&["Frame", "Region"][..], |f| type_chain(f.kind)))
    }

    set_shared(lua, m, Side::Frame, "GetObjectType", |lua, this: Table| {
        Ok(Value::String(lua.create_string(chain_of(lua, &this)?[0])?))
    })?;

    // Same four traps as the region twin (see `script::region`): case-insensitive whole-string,
    // number 1 on a hit and nil on a miss, exactly one value either way, and a non-string
    // non-number argument raises the reference's own `Usage:` text.
    set_shared(
        lua,
        m,
        Side::Frame,
        "IsObjectType",
        |lua, (this, want): (Table, Value)| {
            let chain = chain_of(lua, &this)?;
            let want = match &want {
                Value::String(s) => s.to_str()?.to_string(),
                Value::Number(n) => n.to_string(),
                Value::Integer(i) => i.to_string(),
                _ => {
                    let h = frame_handle_of(lua, &this)?;
                    let model = lua.app_data_ref::<Model>().expect("model");
                    let who = model
                        .arena
                        .frame(h)
                        .and_then(|f| f.name.clone())
                        .unwrap_or_else(|| "<unnamed>".to_string());
                    return Err(mlua::Error::runtime(format!(
                        "Usage: {who}:IsObjectType(\"TYPE\")"
                    )));
                }
            };
            Ok(if chain.iter().any(|t| want.eq_ignore_ascii_case(t)) {
                Value::Number(1.0)
            } else {
                Value::Nil
            })
        },
    )?;

    // ── GetFrameType / IsFrameType: the FRAME-side spellings of the pair above ──
    //
    // 1.12 registers the type-identity pair **twice, under two names**. The Region script
    // (`CScriptRegion`) publishes `GetObjectType`/`IsObjectType` — that is the pair above, and its
    // `IsObjectType` binding is `0x7a1290`. The *frame* script (`CSimpleFrameScript.cpp`,
    // `__FILE__` `0x879504`) publishes `GetFrameType 0x773640` and `IsFrameType 0x773700` as well.
    // `GetObjectType`/`IsObjectType` are what LATER
    // clients kept; `GetFrameType`/`IsFrameType` are 1.12's own, and we shipped only the first
    // pair. That is 1189's superset argument inverted — not an extra name we invented, a real one
    // we were missing — and it cost the world map:
    //
    //     Cartographer 2.02, LookNFeel.lua:368, inside OnEnable:
    //         if v:GetFrameType() == "Model" and not v:GetName() then self.playerModel = v end
    //
    // With the verb nil that line raised, AceAddon swallowed it, `self.playerModel` stayed nil,
    // and the next `Cartographer_ChangeZone` → `SetAlpha` put a red script error on screen from
    // `LookNFeel.lua:737` every time the map opened. FuBar's own `FuBar.lua:576`
    // (`type(frame:GetFrameType()) ~= "string"`) is the same call.
    //
    // Both delegate to the same `type_chain` the `Object` pair reads: `GetFrameType` is
    // `call [edx+0x1c]` — the identical per-class type-name slot `GetObjectType` reads — then
    // `lua_pushstring`, one value, extra arguments ignored. `IsFrameType` walks the chain through
    // `[eax+0x18]`, the same case-insensitive whole-string compare, and answers the NUMBER 1 or
    // nil.
    //
    // **One edge is INFERRED and named rather than guessed silently:** the one description of
    // `IsFrameType`'s *absent-argument* branch reads "pushes the frame's own typename (`call
    // [eax+4]` GetName)", which is self-contradictory — a typename and a name are different slots —
    // so the branch is not settled. We take the sibling's behaviour (the `Usage:` raise) pending a
    // byte read. No caller in the 219-addon corpus, the director's AddOns, or the shipped FrameXML
    // ever omits the argument: `JIM_toolbox/Config2/Pulse_Config.lua:118` is the corpus's only
    // `IsFrameType` site and it passes `"Slider"`.
    m.set(
        "GetFrameType",
        lua.create_function(|lua, this: Table| {
            Ok(Value::String(lua.create_string(chain_of(lua, &this)?[0])?))
        })?,
    )?;
    m.set(
        "IsFrameType",
        lua.create_function(|lua, (this, want): (Table, Value)| {
            let chain = chain_of(lua, &this)?;
            let want = match &want {
                Value::String(s) => s.to_str()?.to_string(),
                Value::Number(n) => n.to_string(),
                Value::Integer(i) => i.to_string(),
                _ => {
                    let h = frame_handle_of(lua, &this)?;
                    let model = lua.app_data_ref::<Model>().expect("model");
                    let who = model
                        .arena
                        .frame(h)
                        .and_then(|f| f.name.clone())
                        .unwrap_or_else(|| "<unnamed>".to_string());
                    return Err(mlua::Error::runtime(format!(
                        "Usage: {who}:IsFrameType(\"TYPE\")"
                    )));
                }
            };
            Ok(if chain.iter().any(|t| want.eq_ignore_ascii_case(t)) {
                Value::Number(1.0)
            } else {
                Value::Nil
            })
        })?,
    )?;

    set_shared(lua, m, Side::Frame, "GetParent", |lua, this: Table| {
        let h = frame_handle_of(lua, &this)?;
        let parent_id = {
            let mut model = lua.app_data_mut::<Model>().expect("model");
            model
                .arena
                .frame(h)
                .and_then(|f| f.parent)
                .map(|p| model.frame_id(p))
        };
        match parent_id {
            Some(pid) => Ok(Value::Table(frame_wrapper(lua, pid)?)),
            None => Ok(Value::Nil),
        }
    })?;
    // ── The four structure queries: GetChildren / GetNumChildren / GetRegions / GetNumRegions ──
    //
    // Registered bindings in 1.12 (`GetNumRegions 0x773e60`, `GetRegions 0x773f60`,
    // `GetNumChildren 0x774080`, `GetChildren 0x774180`): each marshals and delegates, with no
    // fidelity math of its own).
    //
    // **Why these four are worth more than their size.** They are how an addon walks a frame it did
    // not build, which is the whole basis of the "reskin the default UI" genre — and that genre is
    // the top of the 1.12 popularity list. `pfUI.api.StripTextures` is the canonical shape:
    //
    //     for _, v in ipairs({ frame:GetRegions() }) do
    //       if v.SetTexture then ... v:SetTexture(nil) end
    //     end
    //
    // With `GetRegions` absent that is `attempt to call method 'GetRegions' (a nil value)`, raised
    // out of a library file every one of pfUI, pfQuest, pfQuest-turtle and ShaguDPS embeds — four
    // separate top-20 addons dying on one missing verb, plus Questie on `GetChildren`.
    //
    // **Two lists, one walk each, in the arena's own order.** `Frame::children` is insertion order,
    // which our own arena documents as the client's `+0x300` child-list order; `Frame::regions` is
    // creation order. Neither is re-sorted here — a structure query reports the structure, and the
    // draw order is `crate::order`'s separate concern.
    //
    // **A detached region is not in the list.** `Region:SetParent(nil)` sets `Region::detached`
    // rather than removing the entry, because the owner's `Vec` is what `WidgetArena::destroy`
    // frees from — but the client's re-link virtual `0x77fd10` with a null parent genuinely
    // **unlinks from the old parent's draw layer and region list**, so a detached region must be
    // invisible to both region verbs. Our flag is a representation choice; the list these
    // report is the client's.
    //
    // **The count and the list come from ONE function** ([`regions_of`] / [`children_of`]), never
    // two walks with the same filter written twice. `GetNumRegions` disagreeing with
    // `#{GetRegions()}` is a bug an addon would hit as an off-by-one deep inside a loop it did not
    // write, and the only way to make it impossible is to have one of them BE the other.
    //
    // **The order and inclusion rules below — one of them a correction:**
    //
    //  · **The order is link order, oldest first, and no reversal.** `CSimpleFrame` embeds a punned
    //    `TSList` header whose ctor sets `tail = &header`, `head = (&header)|1`, and BOTH linkers
    //    append at the TAIL (`0x76a750` for regions, `0x76aa20` for children — one caller each).
    //    Our insertion-order `Vec` is the same order.
    //  · **Hidden children and regions are returned and counted** — a VERIFIED negative: the
    //    counting loops never load the visibility word at all.
    //  · **The TITLE REGION IS NOT RETURNED**, and that one corrected us. Both creation paths
    //    (`0x773910`, `0x769b79`) dispatch a vtable slot that is a bare `[this+0x9c] = parent` and
    //    never reach the linker `0x77fd10`, so a title region is not in the list this walks —
    //    corroborated by `Hide`/`Show` carrying an explicit extra `[frame+0xa8]` case *because* the
    //    list walk misses it. Ours is in `Frame::regions` (so the arena can free it), so
    //    [`regions_of`] filters it out.
    //  · **A Button's label (`+0x338`) and its four state textures ARE in the list** — they link
    //    through `0x77fd10` like any region, which is what ours do too.
    //  · **The return shape**: N separate values, never a table; a frame with none returns ZERO
    //    values (no `lua_newtable` in either body), and `GetNum*` returns one number — `0` when
    //    empty, never nil.
    //
    // The one thing still out of reach is not these bindings': Questie reads
    // `({Minimap:GetChildren()})[9]` for the player arrow; here is why that index works —
    // `CMinimap`'s ctor `0x4edbc0` builds NINE engine-owned `Model` children before the XML
    // `<Frames>` descent, the ninth (`[Minimap+0x338]`) being the arrow `SetPlayerFacing 0x4eb8e0`
    // writes. We create none of them, so index 9 is nil here until the minimap grows its engine
    // children.
    m.set(
        "GetChildren",
        lua.create_function(|lua, this: Table| {
            let ids = children_of(lua, &this)?;
            let mut out = Vec::with_capacity(ids.len());
            for id in ids {
                out.push(Value::Table(frame_wrapper(lua, id)?));
            }
            Ok(mlua::MultiValue::from_vec(out))
        })?,
    )?;
    m.set(
        "GetNumChildren",
        lua.create_function(|lua, this: Table| Ok(children_of(lua, &this)?.len()))?,
    )?;
    m.set(
        "GetRegions",
        lua.create_function(|lua, this: Table| {
            let ids = regions_of(lua, &this)?;
            let mut out = Vec::with_capacity(ids.len());
            for id in ids {
                out.push(Value::Table(crate::script::region::region_wrapper(
                    lua, id,
                )?));
            }
            Ok(mlua::MultiValue::from_vec(out))
        })?,
    )?;
    m.set(
        "GetNumRegions",
        lua.create_function(|lua, this: Table| Ok(regions_of(lua, &this)?.len()))?,
    )?;
    // SetParent — the runtime reparent. The binding half
    // (`0x7a1550`): the parent argument is a frame table, a NAME string (`0x76c760`), or an
    // explicit nil — an ABSENT argument is NOT the nil path (`0x6f3400` returns −1) and raises
    // like an unresolvable name; the cycle guard is the binding's own inline ancestor walk and
    // RAISES (`0x87cb14`), it does not silently no-op. The arena half is
    // `reparent_begin`/`reparent_finish`, split so `OnHide` fires between them the way the
    // reference's hide→show round-trip does — `fire_visibility_changes` reads each frame's live
    // state for direction, so hide and show must be fired from their own phase, never one
    // combined list.
    set_shared(
        lua,
        m,
        Side::Frame,
        "SetParent",
        |lua, args: mlua::MultiValue| {
            let mut it = args.into_iter();
            let this = match it.next() {
                Some(Value::Table(t)) => t,
                _ => return Err(mlua::Error::runtime("expected a frame")),
            };
            let h = frame_handle_of(lua, &this)?;
            let who = {
                let model = lua.app_data_ref::<Model>().expect("model");
                model
                    .arena
                    .frame(h)
                    .and_then(|f| f.name.clone())
                    .unwrap_or_else(|| "<unnamed>".to_string())
            };
            let parent_arg = it.next();
            // `_G[name]` is read with no guard alive (`object::NamedTarget`), and **without**
            // `$parent` expansion: `0x7a1550` calls `0x76c760` directly, and only the layout
            // vtable's `0x76c700` runs the token.
            let named = match &parent_arg {
                Some(Value::String(s)) => Some(match s.to_str() {
                    Ok(n) => super::prefetch_named_target(lua, n.as_ref(), None),
                    Err(_) => super::NamedTarget::unreadable(),
                }),
                _ => None,
            };
            let new_parent = {
                let model = lua.app_data_ref::<Model>().expect("model");
                match &parent_arg {
                    // Explicit nil reparents to the screen root (the strata/level RESET path).
                    Some(Value::Nil) => None,
                    Some(Value::Table(t)) => Some(
                        decode_id(t)
                            .ok()
                            .and_then(|id| model.id_to_frame.get(&id).copied())
                            .ok_or_else(|| {
                                mlua::Error::runtime(format!(
                                    "{who}:SetParent(): Couldn't find region named ''"
                                ))
                            })?,
                    ),
                    Some(Value::String(_)) => {
                        // `_G[name]` + the **Frame** tag check — the reference's `0x76c760`
                        // (`0x7a1550`'s NAME-string path) type-guards against `[0xcf0c10]`, the
                        // narrow Frame id, not `SetPoint`'s root `[0xcf0c3c]`. So a global naming
                        // a REGION fails here exactly as an absent one does.
                        let nt = named.as_ref().expect("a String argument is prefetched");
                        let hit = super::resolve_named_target(&model, nt)
                            .and_then(|id| model.id_to_frame.get(&id).copied());
                        Some(hit.ok_or_else(|| {
                            mlua::Error::runtime(format!(
                                "{who}:SetParent(): Couldn't find region named '{}'",
                                nt.name
                            ))
                        })?)
                    }
                    // Absent argument, or any other type: the reference never reaches the
                    // reparent slot (`0x87cb48` raise).
                    _ => {
                        return Err(mlua::Error::runtime(format!(
                            "{who}:SetParent(): Couldn't find region named ''"
                        )))
                    }
                }
            };
            // The cycle guard raises — `"%s:SetParent(): Would create a loop parenting to %s"`
            // (`0x7a177f`'s inline `+0x9c` walk, `newParent == self` included).
            if let Some(np) = new_parent {
                let model = lua.app_data_ref::<Model>().expect("model");
                if np == h || model.arena.is_ancestor(h, np) {
                    let pname = model
                        .arena
                        .frame(np)
                        .and_then(|f| f.name.clone())
                        .unwrap_or_else(|| "<unnamed>".to_string());
                    return Err(mlua::Error::runtime(format!(
                        "{who}:SetParent(): Would create a loop parenting to {pname}"
                    )));
                }
            }
            let hidden = {
                let mut model = lua.app_data_mut::<Model>().expect("model");
                model.arena.reparent_begin(h, new_parent)
            };
            // None = the verified total no-op (same parent, `0x76ab20`): nothing runs, not even
            // the layout touch.
            let Some(hidden) = hidden else { return Ok(()) };
            let was_visible = !hidden.is_empty();
            event::fire_visibility_changes(lua, hidden);
            let shown = {
                let mut model = lua.app_data_mut::<Model>().expect("model");
                let shown = model.arena.reparent_finish(h, new_parent, was_visible);
                // A real reparent moves the subtree's effective scale — a layout-gate input,
                // and a measure-key input the subtree's FontStrings cannot name one by one.
                //
                // **The layout half NAMES its nodes**. This was the conservative
                // touch, on the reading that a reparent is human-rate; a pooled widget's recycle
                // is a reparent, and `WOW_LAYOUT_DERIVE_TRACE` put every sampled give-up in a
                // quest-accept spike on this one line — 58 whole-graph derivations in one frame,
                // 128 ms of a 161 ms hitch. `touch_layout_reparent`'s doc has why the subtree is
                // the whole of what moved. The measure sweep keeps the wide form: a measure key
                // is not a layout input and its sweep is a different cost.
                model.touch_layout_reparent(h);
                model.touch_measure_all();
                shown
            };
            event::fire_visibility_changes(lua, shown);
            Ok(())
        },
    )?;
    // Strata / level / scale / alpha
    m.set(
        "SetFrameStrata",
        lua.create_function(|lua, (this, strata): (Table, String)| {
            let h = frame_handle_of(lua, &this)?;
            let s = strata_from_str(&strata)
                .ok_or_else(|| mlua::Error::runtime(format!("unknown frameStrata '{strata}'")))?;
            lua.app_data_mut::<Model>()
                .expect("model")
                .arena
                .set_frame_strata(h, s);
            Ok(())
        })?,
    )?;
    m.set(
        "GetFrameStrata",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            let s = model.arena.frame(h).map(|f| f.strata).unwrap_or_default();
            Ok(strata_name(s).to_string())
        })?,
    )?;
    m.set(
        "SetFrameLevel",
        lua.create_function(|lua, (this, level): (Table, i64)| {
            let h = frame_handle_of(lua, &this)?;
            let lvl = level.clamp(0, i64::from(u16::MAX)) as u16;
            // **A script level change carries no children** — the binding `0x774560` calls
            // `set_frame_level 0x76a4f0` with `propagate=0`.
            // Only the toplevel raise shifts a subtree. Stock `BonusActionButtonTemplate` is written
            // for this: it raises the button +2 and then its cooldown +2 by hand, landing the sweep
            // one level over the button — carrying the children made it three, over the
            // cooldown-count text an addon hangs at button + 2.
            lua.app_data_mut::<Model>()
                .expect("model")
                .arena
                .set_frame_level(h, lvl, false);
            Ok(())
        })?,
    )?;
    m.set(
        "GetFrameLevel",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(i64::from(
                model.arena.frame(h).map(|f| f.level).unwrap_or(0),
            ))
        })?,
    )?;
    m.set(
        "SetScale",
        lua.create_function(|lua, (this, scale): (Table, f32)| {
            let h = frame_handle_of(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            // Effective-scale changes ride the propagation's own eps gate; the own-scale compare
            // is the cheap superset (same own scale => no effective change is possible).
            let changed = model.arena.frame(h).is_some_and(|f| f.scale != scale);
            model.arena.set_scale(h, scale);
            if changed {
                model.touch_layout();
                // The owner's effective scale is in every descendant FontString's measure key.
                model.touch_measure_all();
            }
            Ok(())
        })?,
    )?;
    m.set(
        "GetScale",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(model.arena.frame(h).map(|f| f.scale).unwrap_or(1.0))
        })?,
    )?;
    m.set(
        "SetAlpha",
        lua.create_function(|lua, (this, alpha): (Table, f32)| {
            let h = frame_handle_of(lua, &this)?;
            // The 1.12 API clamps to 0..1 — ref Lua leans on it (fade code that passes a 0..255
            // alpha stays fully opaque until the value falls below 1/255).
            lua.app_data_mut::<Model>()
                .expect("model")
                .arena
                .set_alpha(h, alpha.clamp(0.0, 1.0));
            Ok(())
        })?,
    )?;
    m.set(
        "GetAlpha",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(model.arena.frame(h).map(|f| f.alpha).unwrap_or(1.0))
        })?,
    )?;
    // Backdrop (`0x7776e0` → `0x76a5d0`): SetBackdrop(table|nil) installs (or, with nil, tears
    // down) the frame's tiled bg + 8-piece border plate. The two color setters tint the bg / all 8
    // border pieces (never the reverse — `0x77f410`/`0x77f440`). The Lua-table SetBackdrop defaults
    // both colors to WHITE
    // (the ctor), so a caller must SetBackdropColor after to tint (the tooltip's OnLoad does).
    m.set(
        "SetBackdrop",
        lua.create_function(|lua, (this, arg): (Table, Value)| {
            let h = frame_handle_of(lua, &this)?;
            let bd = match arg {
                Value::Nil => None,
                Value::Table(t) => Some(backdrop_from_table(&t)?),
                other => {
                    return Err(mlua::Error::runtime(format!(
                        "SetBackdrop: expected a table or nil, got {}",
                        other.type_name()
                    )))
                }
            };
            let mut model = lua.app_data_mut::<Model>().expect("model");
            match bd {
                Some(b) => {
                    model.backdrops.insert(h, b);
                }
                None => {
                    model.backdrops.remove(&h);
                }
            }
            Ok(())
        })?,
    )?;
    // `GetBackdrop()` (`0x777370`) — five things
    // a plausible implementation gets wrong, so each is spelled out with its reason:
    //
    // 1. It is a **reconstruction from the struct**, never the caller's table. The reference stores
    //    no Lua reference anywhere: `SetBackdrop` reads six keys into a fresh 0x68-byte struct at
    //    `frame+0x1ac` and drops the table, so `0x777426`–`0x7776b9` re-push every key from those
    //    fields. Handing back a stored clone would leak keys the reader never accepted and would
    //    keep stale values a later `SetBackdropColor` changed.
    // 2. **No backdrop ⇒ ZERO Lua values, not `nil`** — the early bail is `xor eax,eax; ret`, which
    //    for a *return* path really is "no values" (contrast `binding_abi`'s note: the same two
    //    bytes after a `luaL_error` are unreachable boilerplate). Observable through the count,
    //    and it is the shape our `GetTitleRegion` will *not* have when it lands — that one pushes
    //    nil, i.e. one value. The client cannot distinguish "never set" from `SetBackdrop(nil)`.
    // 3. **A partial `SetBackdrop` omits nothing on the way out.** Every `SetBackdrop` allocates a
    //    fresh struct (`0x777801`, ctor `0x77e5f0`), so a key the caller left out is a *ctor
    //    default* here, not an absent key and not the previous backdrop's value: `bgFile`/`edgeFile`
    //    `""`, `tileSize` 0, `edgeSize` **32**. Our `backdrop_from_table` already builds on
    //    `Backdrop::default()`, so this falls out — but only because `None` maps to `""` below
    //    rather than to nil.
    // 4. **`tile` is the NUMBER `1`, or the key is ABSENT — never `true`/`false`.** The push is
    //    `0x3ff00000` (the double 1.0) on true and `lua_pushnil` on false, and `lua_settable` with a
    //    nil value creates no key (and *erases* one from a recycled table, which is why the nil is
    //    written rather than skipped). An addon reading `if backdrop.tile then` sees the same truth
    //    either way; one that round-trips the table into `SetBackdrop` is why the number matters,
    //    since `tile` there goes through a coercer that takes numbers.
    // 5. The undocumented **in-place form** (`0x77740e`): `lua_type(L,2) == LUA_TTABLE` skips
    //    `lua_newtable` and fills arg 2, reusing an existing `insets` subtable rather than replacing
    //    it. Implemented — it is four lines, and an addon caching one table across frames would
    //    otherwise silently get a new one each call. A non-table arg 2 is ignored, not an error.
    //
    // No `bgColor`/`edgeColor`/`alpha` key exists: the two colors live in the struct but the reader
    // never pushes them (`GetBackdropColor`/`GetBackdropBorderColor` are their only accessors).
    m.set(
        "GetBackdrop",
        lua.create_function(|lua, (this, target): (Table, Value)| {
            let h = frame_handle_of(lua, &this)?;
            // Copied out before a single Lua write: filling a *caller-supplied* table can run a
            // `__newindex` metamethod, which can re-enter us and would panic on the app-data borrow.
            let bd = {
                let model = lua.app_data_ref::<Model>().expect("model");
                model.backdrops.get(&h).cloned()
            };
            let Some(bd) = bd else {
                return Ok(mlua::MultiValue::new()); // trap 2 — zero values, not nil
            };
            let t = match target {
                Value::Table(t) => t,
                _ => lua.create_table()?,
            };
            t.set("bgFile", bd.bg_file.as_deref().unwrap_or(""))?;
            t.set("edgeFile", bd.edge_file.as_deref().unwrap_or(""))?;
            t.set(
                "tile",
                if bd.tile {
                    Value::Number(1.0) // trap 4
                } else {
                    Value::Nil // trap 4 — erases the key from a recycled table
                },
            )?;
            t.set("tileSize", f64::from(bd.tile_size))?;
            t.set("edgeSize", f64::from(bd.edge_size))?;
            let insets = match t.get::<Value>("insets") {
                Ok(Value::Table(existing)) => existing, // trap 5 — reuse, don't replace
                _ => {
                    let fresh = lua.create_table()?;
                    t.set("insets", &fresh)?;
                    fresh
                }
            };
            insets.set("left", f64::from(bd.insets.left))?;
            insets.set("right", f64::from(bd.insets.right))?;
            insets.set("top", f64::from(bd.insets.top))?;
            insets.set("bottom", f64::from(bd.insets.bottom))?;
            Ok(mlua::MultiValue::from_vec(vec![Value::Table(t)]))
        })?,
    )?;
    m.set(
        "SetBackdropColor",
        lua.create_function(|lua, (this, args): (Table, mlua::MultiValue)| {
            let color = backdrop_color(lua, args);
            let h = frame_handle_of(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            if let Some(bd) = model.backdrops.get_mut(&h) {
                bd.bg_color = color;
            }
            Ok(())
        })?,
    )?;
    m.set(
        "SetBackdropBorderColor",
        lua.create_function(|lua, (this, args): (Table, mlua::MultiValue)| {
            let color = backdrop_color(lua, args);
            let h = frame_handle_of(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            if let Some(bd) = model.backdrops.get_mut(&h) {
                bd.border_color = color;
            }
            Ok(())
        })?,
    )?;
    m.set(
        "GetBackdropColor",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            match model.backdrops.get(&h) {
                Some(bd) => Ok((
                    bd.bg_color[0],
                    bd.bg_color[1],
                    bd.bg_color[2],
                    bd.bg_color[3],
                )),
                None => Ok((1.0, 1.0, 1.0, 1.0)),
            }
        })?,
    )?;
    m.set(
        "GetBackdropBorderColor",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            match model.backdrops.get(&h) {
                Some(bd) => Ok((
                    bd.border_color[0],
                    bd.border_color[1],
                    bd.border_color[2],
                    bd.border_color[3],
                )),
                None => Ok((1.0, 1.0, 1.0, 1.0)),
            }
        })?,
    )?;
    // Mouse interaction (EnableMouse gates hit-testing; keyboard focus is out of scope)
    m.set(
        "EnableMouse",
        lua.create_function(|lua, (this, enable): (Table, bool)| {
            let h = frame_handle_of(lua, &this)?;
            lua.app_data_mut::<Model>()
                .expect("model")
                .arena
                .set_mouse_enabled(h, enable);
            Ok(())
        })?,
    )?;
    m.set(
        "IsMouseEnabled",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(crate::script::binding_abi::flag(
                model.arena.is_mouse_enabled(h),
            ))
        })?,
    )?;
    // EnableKeyboard(flag) / IsKeyboardEnabled() — `0x776f90` / `0x776ff0`, real Frame entries,
    // beside the mouse pair above.
    //
    // 8 corpus addons call this and were RAISING on it (`ColorPickerPlus:258`,
    // `Dewdrop-2.0.lua:2021-2022` — which is in ~65 addons — `AckisRecipeList/ARLFrame.lua:1650`).
    // The flag round-trips; **key delivery is not gated on it**, exactly as the wheel's flag
    // shipped in 1198, because the machinery it would gate does not exist here yet.
    //
    // Why that is the right half rather than a silent stub, which this codebase has been bitten by
    // three times (1203/1205/1211): the majority of the corpus calls pass **false**, and for those
    // our behaviour is already the reference's — we deliver no keys to arbitrary frames either. The
    // `true` callers want delivery we do not do, but they did not get it before this landed; they
    // got a raise that killed the enclosing function. And being enabled genuinely is separable
    // from having a handler (`0x76af00` never touches the handler slots), so the flag is a real
    // piece of the model, not a placeholder for one.
    m.set(
        "EnableKeyboard",
        lua.create_function(|lua, (this, enable): (Table, bool)| {
            let h = frame_handle_of(lua, &this)?;
            lua.app_data_mut::<Model>()
                .expect("model")
                .arena
                .set_keyboard_enabled(h, enable);
            Ok(())
        })?,
    )?;
    m.set(
        "IsKeyboardEnabled",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(crate::script::binding_abi::flag(
                model.arena.is_keyboard_enabled(h),
            ))
        })?,
    )?;
    // `EnableMouseWheel(flag)` / `IsMouseWheelEnabled()` — the wheel's own gate, a separate flag
    // from `EnableMouse` in the reference and separate here.
    //
    // The flag is real and round-trips. **The dispatch is NOT gated on it yet, deliberately.**
    // Our wheel dispatch keys off "does this frame carry an `OnMouseWheel` handler", walking up to
    // the nearest ancestor that does — more permissive than the reference, which also requires the
    // frame to be wheel-enabled so a scroll region can hand the wheel to the window behind it
    // without tearing its handler out.
    //
    // Gating it today would break our own UI: 44 `OnMouseWheel` sites across 14 shipped files and
    // **not one of them declares `enableMouseWheel`**, because the loader has never read that
    // attribute. The condition to flip it is concrete rather than someday — teach the loader the
    // attribute, declare it on those 44 sites, then gate. Until then this is a disclosed superset
    // (1189's argument, pointed the other way), and the two corpus addons that stopped on the
    // missing *method* are unblocked either way.
    // EnableDrawLayer(layer) / DisableDrawLayer(layer) — Frame method table `0x878ec0`, entries
    // `0x7755b0` / `0x775680`, between `IsToplevel` and `Show`. See [`crate::widget::Frame`]'s
    // `disabled_layers` for what was read and why the mask lives on the frame.
    //
    // An unknown layer name is a NO-OP here rather than a raise. That is the honest state, not a
    // decision: `draw_layer_from_str` is the same parser `SetDrawLayer` and `CreateTexture` use,
    // and what the reference does with an unparseable name in THIS pair has not been read out of
    // the binary. Both corpus callers (pfUI, MoveAnything) pass literals from the five-name set.
    for (name, disable) in [("EnableDrawLayer", false), ("DisableDrawLayer", true)] {
        m.set(
            name,
            lua.create_function(move |lua, (this, layer): (Table, Value)| {
                let Some(l) = layer
                    .as_string()
                    .and_then(|s| s.to_str().ok().and_then(|s| draw_layer_from_str(&s)))
                else {
                    return Ok(());
                };
                let h = frame_handle_of(lua, &this)?;
                let mut model = lua.app_data_mut::<Model>().expect("model");
                if let Some(frame) = model.arena.frame_mut(h) {
                    let bit = 1u8 << l.index();
                    if disable {
                        frame.disabled_layers |= bit;
                    } else {
                        frame.disabled_layers &= !bit;
                    }
                }
                Ok(())
            })?,
        )?;
    }
    m.set(
        "EnableMouseWheel",
        lua.create_function(|lua, (this, enable): (Table, bool)| {
            let h = frame_handle_of(lua, &this)?;
            lua.app_data_mut::<Model>()
                .expect("model")
                .arena
                .set_mouse_wheel_enabled(h, enable);
            Ok(())
        })?,
    )?;
    m.set(
        "IsMouseWheelEnabled",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(crate::script::binding_abi::flag(
                model.arena.is_mouse_wheel_enabled(h),
            ))
        })?,
    )?;
    // Clamp-to-screen (`0x776c00`/`0x776cb0`, geometry flags bit4): the layout resolve
    // keeps the frame's assembled rect inside the window, size preserved. GameTooltip frames
    // default true by construction (widget::Frame::clamped_to_screen).
    m.set(
        "SetClampedToScreen",
        lua.create_function(|lua, (this, clamp): (Table, bool)| {
            let h = frame_handle_of(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            let changed = model.arena.is_clamped_to_screen(h) != clamp;
            model.arena.set_clamped_to_screen(h, clamp);
            if changed {
                model.touch_layout();
            }
            Ok(())
        })?,
    )?;
    m.set(
        "IsClampedToScreen",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(crate::script::binding_abi::flag(
                model.arena.is_clamped_to_screen(h),
            ))
        })?,
    )?;
    // Hit-rect insets — the MOUSE rect only (widget::Frame::hit_rect_insets): the hit test shrinks
    // the resolved rect by these four before testing the cursor, and nothing else reads them, so a
    // frame's geometry/draw/anchor answers are unchanged. The ref sets them wherever a button's
    // frame is larger than its art (the micro buttons' 18 px empty header).
    m.set(
        "SetHitRectInsets",
        lua.create_function(
            |lua, (this, left, right, top, bottom): (Table, f32, f32, f32, f32)| {
                let h = frame_handle_of(lua, &this)?;
                lua.app_data_mut::<Model>()
                    .expect("model")
                    .arena
                    .set_hit_rect_insets(h, [left, right, top, bottom]);
                Ok(())
            },
        )?,
    )?;
    m.set(
        "GetHitRectInsets",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            let i = model.arena.hit_rect_insets(h);
            Ok((i[0], i[1], i[2], i[3]))
        })?,
    )?;
    Ok(())
}

/// Parse a Lua `SetBackdrop` table into a [`Backdrop`] (`0x7776e0`'s key reads). The keys read —
/// exactly the compiled reader's set — are `bgFile`/`edgeFile` (strings), `tile` (boolean, default
/// false), `tileSize`/`edgeSize` (numbers; a missing number leaves the ctor default: tileSize 0,
/// edgeSize 32), and `insets{left,right,top,bottom}` (numbers, each default 0). A non-string file or
/// non-number size is treated as absent (the client's per-key type gate). Colors stay the ctor
/// white — SetBackdrop never reads a color key; the color setters do that.
fn backdrop_from_table(t: &Table) -> mlua::Result<Backdrop> {
    let mut bd = Backdrop::default();
    if let Ok(Value::String(s)) = t.get::<Value>("bgFile") {
        bd.bg_file = Some(s.to_str()?.to_string());
    }
    if let Ok(Value::String(s)) = t.get::<Value>("edgeFile") {
        bd.edge_file = Some(s.to_str()?.to_string());
    }
    // `tile` — Lua truthiness (nil/false ⇒ false; a missing key reads as Nil).
    bd.tile = !matches!(
        t.get::<Value>("tile").unwrap_or(Value::Nil),
        Value::Nil | Value::Boolean(false)
    );
    if let Ok(Value::Number(n)) = t.get::<Value>("tileSize") {
        bd.tile_size = n as f32;
    } else if let Ok(Value::Integer(n)) = t.get::<Value>("tileSize") {
        bd.tile_size = n as f32;
    }
    if let Ok(Value::Number(n)) = t.get::<Value>("edgeSize") {
        bd.edge_size = n as f32;
    } else if let Ok(Value::Integer(n)) = t.get::<Value>("edgeSize") {
        bd.edge_size = n as f32;
    }
    if let Ok(Value::Table(ins)) = t.get::<Value>("insets") {
        let num = |k: &str| -> f32 {
            match ins.get::<Value>(k) {
                Ok(Value::Number(n)) => n as f32,
                Ok(Value::Integer(n)) => n as f32,
                _ => 0.0,
            }
        };
        bd.insets = Insets {
            left: num("left"),
            right: num("right"),
            top: num("top"),
            bottom: num("bottom"),
        };
    }
    Ok(bd)
}

fn strata_name(s: Strata) -> &'static str {
    match s {
        Strata::World => "WORLD",
        Strata::Background => "BACKGROUND",
        Strata::Low => "LOW",
        Strata::Medium => "MEDIUM",
        Strata::High => "HIGH",
        Strata::Dialog => "DIALOG",
        Strata::Fullscreen => "FULLSCREEN",
        Strata::FullscreenDialog => "FULLSCREEN_DIALOG",
        Strata::Tooltip => "TOOLTIP",
        Strata::Blizzard => "BLIZZARD",
    }
}

fn set_shown(lua: &Lua, this: &Table, shown: bool) -> mlua::Result<()> {
    let h = frame_handle_of(lua, this)?;
    let changed = {
        let mut model = lua.app_data_mut::<Model>().expect("model");
        model.arena.set_shown(h, shown)
    };
    event::fire_visibility_changes(lua, changed);
    Ok(())
}

/// This frame's child frames as stable ids, in the arena's insertion order — the one walk
/// `GetChildren` and `GetNumChildren` share (see their comment on why they must share it).
fn children_of(lua: &Lua, this: &Table) -> mlua::Result<Vec<u32>> {
    let h = frame_handle_of(lua, this)?;
    let mut model = lua.app_data_mut::<Model>().expect("model");
    let Some(children) = model.arena.frame(h).map(|f| f.children.clone()) else {
        return Ok(Vec::new());
    };
    Ok(children.into_iter().map(|c| model.frame_id(c)).collect())
}

/// This frame's live regions as stable ids, in creation order — **the title region and detached
/// ones excluded** — the one walk `GetRegions` and `GetNumRegions` share.
///
/// Both exclusions are the client's list, not tidiness:
///
/// - **Detached.** `SetParent(nil)` reaches `0x77fd55` → the removal virtual `0x76a7f0`, which
///   unlinks the node and frees it, and then skips every re-link. We keep the entry only so the
///   arena can still free the slot; reporting it would hand an addon an object the reference never
///   would, and `StripTextures`-shaped code would "strip" a region already off the screen.
/// - **The title region.** Its two creation paths (`0x773910`, `0x769b79`) dispatch a vtable slot
///   that is a bare `[this+0x9c] = parent` and never reach the linker at all, so it was never in
///   this list — corroborated by `Hide`/`Show` needing an explicit extra `[frame+0xa8]` case
///   *because* the walk misses it. Ours is in [`crate::widget::Frame::regions`] so that
///   `WidgetArena::destroy` still frees it, which makes that a representation detail this walk
///   must not leak.
///
/// A Button's label and its four state textures are deliberately NOT excluded: they link through
/// `0x77fd10` like any other region in the reference, and ours go through `create_region` like any
/// other region here.
fn regions_of(lua: &Lua, this: &Table) -> mlua::Result<Vec<u32>> {
    let h = frame_handle_of(lua, this)?;
    let mut model = lua.app_data_mut::<Model>().expect("model");
    let Some((regions, title)) = model
        .arena
        .frame(h)
        .map(|f| (f.regions.clone(), f.title_region))
    else {
        return Ok(Vec::new());
    };
    let live: Vec<_> = regions
        .into_iter()
        .filter(|r| Some(*r) != title)
        .filter(|r| model.arena.region(*r).is_some_and(|reg| !reg.detached))
        .collect();
    Ok(live.into_iter().map(|r| model.region_id(r)).collect())
}

/// One backdrop colour channel, the reference's own conversion (`SetBackdropColor 0x777d30` /
/// `SetBackdropBorderColor 0x7780d0` — instruction-identical bar the delegate).
///
/// **r/g/b are UNGATED and alpha is not**, and that asymmetry is the whole of this function:
///
///  · r/g/b (`lua` indices 2/3/4) go through a bare `lua_tonumber`, so a `nil` channel is `0.0`
///    and the call completes — no raise. ShaguTweaks `helpers.lua:248` passes `color.r` from a
///    table that does not always have one, and benilla raised on it, killing the addon.
///  · alpha (index 5) is `lua_isnumber`-gated at `0x778227` with `1.0f` staged at `0x778220`, so a
///    **missing or nil** alpha is OPAQUE. Reading the asymmetry the other way — nil alpha as `0.0`
///    like its neighbours — would turn every one of those borders transparent, which is a worse
///    bug than the raise it replaced and would look like a rendering fault.
///
/// Then the reference clamps to `[0,1]` with **NaN landing on 1.0** (the compare's unordered arm
/// takes the high clamp), and quantizes `×255 + 0.5` through `__ftol` — round-half-up, not the
/// bare truncate `SetChatWindowColor 0x4a14f0`'s colour path uses. The field is a packed
/// `0xAARRGGBB`
/// byte quad, so nothing finer survives the store and we quantize on the way in rather than
/// pretend to a precision `GetBackdropColor` could not read back.
fn backdrop_channel(lua: &Lua, v: Option<Value>, gated: bool) -> f32 {
    let x = if gated {
        // `lua_isnumber` — a number or a numeric string; anything else takes the staged default.
        v.and_then(|v| lua.coerce_number(v).ok().flatten())
            .unwrap_or(1.0)
    } else {
        crate::script::binding_abi::coerced_number(lua, v)
    };
    let clamped = if x.is_nan() { 1.0 } else { x.clamp(0.0, 1.0) };
    // `×255 + 0.5` then `__ftol` (truncate toward zero) = round-half-up over `[0,1]`.
    let byte = (clamped * 255.0 + 0.5) as u8;
    f32::from(byte) / 255.0
}

/// The four channels of a backdrop colour setter, in the reference's own gating (see
/// [`backdrop_channel`]). Takes the raw stack so a missing argument and an explicit `nil` reach
/// the same place they do there.
fn backdrop_color(lua: &Lua, args: mlua::MultiValue) -> [f32; 4] {
    let a: Vec<Value> = args.into_iter().collect();
    [
        backdrop_channel(lua, a.first().cloned(), false),
        backdrop_channel(lua, a.get(1).cloned(), false),
        backdrop_channel(lua, a.get(2).cloned(), false),
        backdrop_channel(lua, a.get(3).cloned(), true),
    ]
}
