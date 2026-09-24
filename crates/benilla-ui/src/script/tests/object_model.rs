//! The frame object model (`0x701bd0`).

use super::common::script;

#[test]
fn create_frame_publishes_global_and_names_resolve() {
    let s = script();
    s.run(
        r#"
        local p = CreateFrame("Frame", "ParentF")
        local c = CreateFrame("Frame", "ChildF", p)
        assert(ParentF == p, "named frame auto-publishes to _G")
        assert(getglobal("ParentF") == p, "getglobal resolves the published name")
        assert(p:GetName() == "ParentF")
        assert(c:GetParent() == p, "GetParent returns the parent wrapper")
        assert(c:GetParent():GetName() == "ParentF")
    "#,
    )
    .unwrap();
}

#[test]
fn wrapper_identity_is_stable_across_lookups() {
    let s = script();
    let same: bool = s
        .eval(
            r#"
        local p = CreateFrame("Frame", "IdF")
        local c = CreateFrame("Frame", nil, p)
        -- GetParent twice, and the _G publish, must all be the SAME table (cached wrapper).
        return (c:GetParent() == c:GetParent()) and (c:GetParent() == p) and (IdF == p)
    "#,
        )
        .unwrap();
    assert!(same, "wrapper identity must be cached/stable");
}

#[test]
fn non_overwriting_named_publish() {
    let s = script();
    let ok: bool = s
        .eval(
            r#"
        local a = CreateFrame("Frame", "Dup")
        local b = CreateFrame("Frame", "Dup")   -- same name; must NOT overwrite _G
        return (Dup == a) and (a ~= b)
    "#,
        )
        .unwrap();
    assert!(ok);
}
