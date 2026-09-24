//! **`GetChildren` / `GetNumChildren` / `GetRegions` / `GetNumRegions`** — the four structure
//! queries, and the properties an addon walking a frame it did not build depends on.
//!
//! The consumer that motivated them is `pfUI.api.StripTextures`, embedded in four separate top-20
//! addons (pfUI, pfQuest, pfQuest-turtle, ShaguDPS):
//!
//! ```lua
//! for _, v in ipairs({ frame:GetRegions() }) do
//!   if v.SetTexture then ... end
//! end
//! ```
//!
//! so the shape under test is the one that idiom needs: **multiple return values**, in the
//! structure's own order, each a usable widget object.
//!
//! Every claim here is confirmed against the reference (`0x773f60`/`0x774180`): the lists are
//! `[frame+0x300]` and `[frame+0x1b8]`, both linkers APPEND AT THE TAIL so the values come back
//! oldest-first with no reversal, hidden nodes
//! are returned and counted, a detached region and the title region are both absent, a Button's
//! label and state textures are present, and an empty frame returns zero values while `GetNum*`
//! returns the number `0`.

use super::common::script;

/// The whole contract in one document: both lists, their order, the counts agreeing with them, the
/// empty case, and — the one that is a behaviour rather than a plumbing detail — that a region
/// detached by `SetParent(nil)` leaves both region verbs.
#[test]
fn the_structure_queries_report_the_structure() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        host = CreateFrame("Frame", "SQHost", UIParent)
        -- children, in creation order; one of them hidden, because a structure query is not a
        -- visibility query and an addon reskinning a frame must still see what is hidden.
        a = CreateFrame("Frame",  "SQChildA", host)
        b = CreateFrame("Button", "SQChildB", host)
        b:Hide()
        c = CreateFrame("Frame",  "SQChildC", host)
        -- regions, in creation order, across two draw layers: a BACKGROUND texture declared FIRST
        -- and an OVERLAY fontstring declared second, so a draw-ordered walk and a creation-ordered
        -- one are distinguishable.
        t1 = host:CreateTexture("SQTexA", "BACKGROUND")
        fs = host:CreateFontString("SQFontB", "OVERLAY")
        t2 = host:CreateTexture("SQTexC", "BACKGROUND")
    "#,
    )
    .unwrap();

    // ── The lists, by name and IN ORDER. Named rather than counted, so a walk that returned the
    //    right number of the wrong things (or the right things reversed) cannot pass.
    assert_eq!(
        s.eval::<String>(
            "local n = '' for _, v in ipairs({ SQHost:GetChildren() }) do \
             n = n .. v:GetName() .. ' ' end return n"
        )
        .unwrap(),
        "SQChildA SQChildB SQChildC ",
        "children come back in the arena's insertion order — the client's +0x300 child-list order"
    );
    assert_eq!(
        s.eval::<String>(
            "local n = '' for _, v in ipairs({ SQHost:GetRegions() }) do \
             n = n .. v:GetName() .. ' ' end return n"
        )
        .unwrap(),
        "SQTexA SQFontB SQTexC ",
        "regions come back in CREATION order, not draw order — the OVERLAY fontstring stays second"
    );

    // ── The counts are the same walk. A count that disagrees with the list is an off-by-one deep
    //    inside a loop the addon did not write, so the two verbs share one function by construction
    //    and this pins it.
    assert_eq!(
        s.eval::<(usize, usize)>(
            "return SQHost:GetNumChildren(), table.getn({ SQHost:GetChildren() })"
        )
        .unwrap(),
        (3, 3)
    );
    assert_eq!(
        s.eval::<(usize, usize)>(
            "return SQHost:GetNumRegions(), table.getn({ SQHost:GetRegions() })"
        )
        .unwrap(),
        (3, 3)
    );

    // ── The objects are USABLE, not opaque handles: `StripTextures` feature-tests `v.SetTexture`
    //    to tell a texture from a fontstring, then calls it. Both halves have to hold.
    assert_eq!(
        s.eval::<String>(
            "local out = '' for _, v in ipairs({ SQHost:GetRegions() }) do \
             if v.SetTexture then v:SetTexture(nil) out = out .. 'T' else out = out .. 'F' end \
             end return out"
        )
        .unwrap(),
        "TFT",
        "a texture answers SetTexture and a fontstring does not — the StripTextures branch"
    );

    // ── A childless, regionless frame yields NOTHING, and counts zero. `ipairs` over it must run
    //    zero times rather than once over a nil.
    assert_eq!(
        s.eval::<(usize, usize, usize)>(
            "local e = CreateFrame(\"Frame\", \"SQEmpty\", UIParent) \
             return e:GetNumChildren(), e:GetNumRegions(), table.getn({ e:GetRegions() })"
        )
        .unwrap(),
        (0, 0, 0)
    );

    // ── DETACHED REGIONS LEAVE BOTH VERBS. `Region:SetParent(nil)` unlinks from the parent's draw
    //    layer AND its region list in the client (`0x77fd10`). We keep the entry so the arena can
    //    still free the slot — a representation choice that must not be observable here, or a
    //    StripTextures-shaped walk would "strip" a region that is already off the screen.
    s.run("SQFontB:SetParent(nil)").unwrap();
    assert_eq!(
        s.eval::<String>(
            "local n = '' for _, v in ipairs({ SQHost:GetRegions() }) do \
             n = n .. v:GetName() .. ' ' end return n"
        )
        .unwrap(),
        "SQTexA SQTexC ",
        "a detached region is gone from the list"
    );
    assert_eq!(
        s.eval::<usize>("return SQHost:GetNumRegions()").unwrap(),
        2,
        "...and from the count, which is the same walk"
    );

    // ── THE TITLE REGION IS NOT IN THE LIST, and this is the assertion that corrected us. Its two
    //    creation paths in the client dispatch a vtable slot that is a bare `[this+0x9c] = parent`
    //    and never reach the region linker, so it was never in the list `GetRegions` walks —
    //    corroborated by `Hide`/`Show` carrying an explicit extra `[frame+0xa8]` case *because* the
    //    walk misses it (`0x768060`). Ours lives in `Frame::regions` so the arena can still free
    //    it, which makes that a representation detail this walk must not leak.
    s.run("SQHost:CreateTitleRegion()").unwrap();
    assert_eq!(
        s.eval::<usize>("return SQHost:GetNumRegions()").unwrap(),
        2,
        "a title region does not join the region list"
    );
    assert_eq!(
        s.eval::<String>(
            "local n = '' for _, v in ipairs({ SQHost:GetRegions() }) do \
             n = n .. v:GetName() .. ' ' end return n"
        )
        .unwrap(),
        "SQTexA SQTexC ",
        "...and does not appear in it"
    );

    // ── A child reparented away leaves its old parent's child list too (the frame twin of the
    //    above, and the case an addon hits when it re-hosts a default-UI frame).
    s.run("SQChildB:SetParent(UIParent)").unwrap();
    assert_eq!(
        s.eval::<String>(
            "local n = '' for _, v in ipairs({ SQHost:GetChildren() }) do \
             n = n .. v:GetName() .. ' ' end return n"
        )
        .unwrap(),
        "SQChildA SQChildC ",
        "a reparented child is gone from its old parent's list"
    );
}
