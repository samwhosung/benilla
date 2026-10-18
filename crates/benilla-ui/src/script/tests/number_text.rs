//! Bindings that read a text argument through `lua_tostring` (`0x6f3690`) convert a number in
//! place with Lua's `%.14g` (`luaV_tostring` `0x6f7c80`, format `0x871960`), never Rust's own
//! formatting.

use super::common::script;
use crate::script::Model;
use crate::widget::KindState;

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

/// `ReplaceIconTexture` (`0x76ee0d`) stores its texture through `lua_tostring` too.
#[test]
fn replace_icon_texture_stores_a_number_as_lua_number_text() {
    let s = script();
    s.run(
        r#"
        IconModel = CreateFrame("Model", "IconModel")
        IconModel:SetModel("Interface\\Buttons\\A.mdx")
        IconModel:ReplaceIconTexture(1/3)
    "#,
    )
    .unwrap();
    let lua = s.lua();
    let model = lua.app_data_ref::<Model>().expect("model");
    let h = model.arena.lookup("IconModel").expect("frame");
    let Some(KindState::Model(m)) = model.arena.frame(h).map(|f| &f.kind_state) else {
        panic!("a Model pane");
    };
    assert_eq!(m.icon.as_deref(), Some("0.33333333333333"));
}
