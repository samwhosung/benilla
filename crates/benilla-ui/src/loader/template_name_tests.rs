//! `name=` and `id=` belong to the node they are written on: `PreLoadXML 0x769770` reads them from
//! the instance before `LoadXML` walks its templates (`0x76985c`), and the region factories read
//! `name` once, outside that walk (`0x6f2751`, `0x6f27e1`).

use crate::framexml;
use crate::loader::load;
use crate::script::UiScript;

const DOC: &str = r#"<Ui>
    <Button name="TmpCloseTemplate" id="3" virtual="true">
        <Size><AbsDimension x="32" y="32"/></Size>
    </Button>
    <Texture name="TmpTexTemplate" virtual="true">
        <Color r="1" g="0" b="0" a="1"/>
    </Texture>
    <Frame name="Holder">
        <Size><AbsDimension x="64" y="64"/></Size>
        <Anchors><Anchor point="CENTER"/></Anchors>
        <Layers>
            <Layer level="ARTWORK">
                <Texture inherits="TmpTexTemplate"/>
            </Layer>
        </Layers>
        <Frames>
            <Button inherits="TmpCloseTemplate"/>
            <Button name="$parentClose" inherits="TmpCloseTemplate"/>
        </Frames>
    </Frame>
</Ui>"#;

fn loaded() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    let report = load(&s, &framexml::parse(DOC).unwrap(), &|_| None);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    s
}

/// Stock `UIOptionsFrame.xml:1477`'s unnamed `<Button inherits="UIPanelCloseButton">` is the
/// shape: in 1.12.1 the template's name is no global and the button answers no name.
#[test]
fn an_unnamed_instance_takes_neither_its_templates_name_nor_its_id() {
    let s = loaded();
    assert!(
        s.eval::<bool>("return TmpCloseTemplate == nil").unwrap(),
        "the unnamed instance published the template's name as a global"
    );
    assert!(s
        .eval::<bool>("return (select(1, Holder:GetChildren())):GetName() == nil")
        .unwrap());
    assert_eq!(
        s.eval::<i64>("return (select(1, Holder:GetChildren())):GetID()")
            .unwrap(),
        0
    );
}

#[test]
fn a_named_instance_keeps_its_own_name_and_takes_no_template_id() {
    let s = loaded();
    assert_eq!(
        s.eval::<String>("return HolderClose:GetName()").unwrap(),
        "HolderClose"
    );
    assert_eq!(s.eval::<i64>("return HolderClose:GetID()").unwrap(), 0);
}

#[test]
fn an_unnamed_region_takes_no_name_from_its_template() {
    let s = loaded();
    assert!(s.eval::<bool>("return TmpTexTemplate == nil").unwrap());
    assert!(s
        .eval::<bool>("return (select(1, Holder:GetRegions())):GetName() == nil")
        .unwrap());
}

/// `CreateFrame` builds its node with the `name` it was given (`0x70622d`), nil included.
#[test]
fn create_frame_with_no_name_stays_unnamed_under_a_named_template() {
    let s = loaded();
    assert!(s
        .eval::<bool>(
            "local b = CreateFrame('Button', nil, Holder, 'TmpCloseTemplate') \
             return b:GetName() == nil and TmpCloseTemplate == nil"
        )
        .unwrap());
}
