//! Bindings that read a text argument through `lua_tostring` (`0x6f3690`) convert a number in
//! place with Lua's `%.14g` (`luaV_tostring` `0x6f7c80`, format `0x871960`), never Rust's own
//! formatting.

use super::common::script;

/// `DoEmote`'s target (`0x49fdc5`), `SetLookingForGroup`'s comment (`0x4e9741`) and `SetModel`'s
/// path (`0x76d9ec`).
#[test]
fn a_number_argument_is_lua_number_text() {
    let mut s = script();
    s.run(
        r#"
        DoEmote("WAVE", 1/3)
        SetLookingForGroup(0, 0, 0, "", 0, 0, 1e20)
        local _, _, _, comment = GetLookingForGroup()
        assert(comment == "1e+020", comment)
        NumModel = CreateFrame("Model", "NumModel")
        NumModel:SetModel(2/3)
        assert(NumModel:GetModel() == "0.66666666666667", NumModel:GetModel())
    "#,
    )
    .unwrap();
    let emotes = s.take_emote_requests();
    assert_eq!(emotes[0].target.as_deref(), Some("0.33333333333333"));
}
