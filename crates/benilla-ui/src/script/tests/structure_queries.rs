//! The structure queries `GetChildren`, `GetNumChildren`, `GetRegions` and `GetNumRegions`
//! (`0x773f60`/`0x774180`): they walk `[frame+0x300]` and `[frame+0x1b8]`, whose linkers append at
//! the tail, so values come back oldest first as multiple returns, hidden ones included.

use super::common::script;

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

    // ── The lists, by name and in order.
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

    // ── The counts are the same walk.
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

    // ── Usable objects: pfUI's `StripTextures` tests `v.SetTexture`, then calls it.
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

    // ── An empty frame returns no values and counts zero.
    assert_eq!(
        s.eval::<(usize, usize, usize)>(
            "local e = CreateFrame(\"Frame\", \"SQEmpty\", UIParent) \
             return e:GetNumChildren(), e:GetNumRegions(), table.getn({ e:GetRegions() })"
        )
        .unwrap(),
        (0, 0, 0)
    );

    // ── A detached region leaves both verbs: `Region:SetParent(nil)` unlinks it from the draw
    //    layer and the region list (`0x77fd10`); our arena keeps the entry, unseen here.
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

    // ── The title region is not in the list: its creation sets `[this+0x9c] = parent` without
    //    the region linker, and `Hide`/`Show` handle `[frame+0xa8]` separately (`0x768060`).
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

    // ── A reparented child leaves its old parent's list.
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
