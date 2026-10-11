//! The template registry `0x6ee500`: a name registers once, matched case-insensitively
//! (`SStrHash 0x64b3f0`, `SStrCmpI 0x64a4c0`), and a second registration is refused with
//! `"Virtual object named %s already exists"` (`0x871188`, severity 1), unless the entry was
//! registered under a non-zero script-execution context and the new one comes under context 0
//! (`0x6ee594`-`0x6ee5a2`), which replaces it. An addon loads under context 0 when secure and under
//! its own name otherwise (`AddOn_Load`, `0x51f37e`-`0x51f387`); the UI's own files load under the
//! resting context, 0.

use crate::framexml;
use crate::loader::load;
use crate::script::{AddOnInfo, UiScript};
use crate::status::LogWrite;

/// A frame template `name` whose width is `width`, the probe of which definition is registered.
fn template(name: &str, width: u32) -> String {
    format!(
        r#"<Ui><Frame name="{name}" virtual="true">
            <Size><AbsDimension x="{width}" y="5"/></Size>
        </Frame></Ui>"#
    )
}

fn script() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s
}

/// Load `xml` as one of the UI's own documents, returning its load-status lines.
fn load_doc(s: &UiScript, xml: &str) -> Vec<String> {
    let report = load(s, &framexml::parse(xml).unwrap(), &|_| None);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    report.status.lines().to_vec()
}

/// The width a frame built from `inherits` gets: which definition the lookup answers.
fn width_through(s: &UiScript, inherits: &str) -> f64 {
    s.eval::<f64>(&format!(
        "return CreateFrame('Frame', nil, nil, '{inherits}'):GetWidth()"
    ))
    .unwrap()
}

const REFUSED: &str = "Virtual object named DupTemplate already exists";

#[test]
fn a_second_template_of_a_registered_name_is_refused_and_the_first_kept() {
    let s = script();
    assert!(load_doc(&s, &template("DupTemplate", 10)).is_empty());
    let lines = load_doc(&s, &template("DupTemplate", 20));
    assert_eq!(lines, vec![REFUSED.to_string()]);
    assert_eq!(width_through(&s, "DupTemplate"), 10.0);
}

/// Within one document too: the registrar is called per top-level element (`0x6ee1bd`).
#[test]
fn a_repeat_within_one_document_is_refused_as_well() {
    let s = script();
    let xml = r#"<Ui>
        <Frame name="DupTemplate" virtual="true"><Size><AbsDimension x="10" y="5"/></Size></Frame>
        <Frame name="DupTemplate" virtual="true"><Size><AbsDimension x="20" y="5"/></Size></Frame>
    </Ui>"#;
    assert_eq!(load_doc(&s, xml), vec![REFUSED.to_string()]);
    assert_eq!(width_through(&s, "DupTemplate"), 10.0);
}

/// The hash upper-cases and the compare folds case, so a name differing only in case is the same
/// name; the message names the refused spelling, the registrar's own argument (`0x6ee5a9`).
#[test]
fn a_name_differing_only_in_case_is_the_same_name() {
    let s = script();
    load_doc(&s, &template("DupTemplate", 10));
    let lines = load_doc(&s, &template("DUPTEMPLATE", 20));
    assert_eq!(
        lines,
        vec!["Virtual object named DUPTEMPLATE already exists".to_string()]
    );
    assert_eq!(width_through(&s, "duptemplate"), 10.0);
    assert_eq!(width_through(&s, "DUPTEMPLATE"), 10.0);
}

/// Severity 1 is above a trace, so the refused document's record opens under its banner in
/// `FrameXML.log` with `FrameXML_Debug` off (`0x6ee207`).
#[test]
fn the_refusal_puts_the_document_in_the_load_log() {
    let s = script();
    load_doc(&s, &template("DupTemplate", 10));
    let report = load(
        &s,
        &framexml::parse(&template("DupTemplate", 20)).unwrap(),
        &|_| None,
    );
    let mut parent = crate::status::Status::default();
    report
        .status
        .close_into(&mut parent, false, "++ Loading file X".into());
    assert_eq!(parent.lines(), ["++ Loading file X", REFUSED]);
}

/// `"-- Added virtual frame %s"` (`0x87116c`) at severity 0, only while `FrameXML_Debug` is on
/// (`0x6ee6c6`), and not for a refused one, which returns before it (`0x6ee5be`).
#[test]
fn a_registration_is_traced_under_framexml_debug() {
    let s = script();
    assert!(load_doc(&s, &template("QuietTemplate", 10)).is_empty());
    s.run("FrameXML_Debug(1)").unwrap();
    let lines = load_doc(&s, &template("TracedTemplate", 10));
    assert_eq!(
        lines,
        vec!["-- Added virtual frame TracedTemplate".to_string()]
    );
    let lines = load_doc(&s, &template("TracedTemplate", 20));
    assert_eq!(
        lines,
        vec!["Virtual object named TracedTemplate already exists".to_string()]
    );
}

/// Two demand-loaded chain addons, each one XML file declaring `DupTemplate` at its own width;
/// `secure` is each one's `GetAddOnInfo` security.
fn with_addons(rows: &[(&'static str, bool, u32)]) -> UiScript {
    let mut s = script();
    let files: Vec<(String, String)> = rows
        .iter()
        .map(|&(name, _, width)| {
            (
                format!("Interface/AddOns/{name}/{name}.xml"),
                template("DupTemplate", width),
            )
        })
        .collect();
    s.set_addon_chain_reader(Box::new(move |path| {
        files
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, xml)| xml.clone().into_bytes())
    }));
    s.register_addons(
        rows.iter()
            .map(|&(name, secure, _)| AddOnInfo {
                name: name.into(),
                interface: 11200,
                load_on_demand: true,
                chain: true,
                secure,
                enabled: true,
                files: vec![format!("{name}.xml")],
                ..Default::default()
            })
            .collect(),
        None,
        None,
        None,
    );
    s.note_addon_info_reply(&[]);
    s
}

fn load_addon(s: &mut UiScript, name: &str) -> Vec<String> {
    assert_eq!(
        s.eval::<Option<i64>>(&format!("return LoadAddOn('{name}')"))
            .unwrap(),
        Some(1)
    );
    s.take_load_log_writes()
        .into_iter()
        .flat_map(|w| match w {
            LogWrite::Append(lines) | LogWrite::Rewrite(lines) => lines,
        })
        .collect()
}

fn refused_in(lines: &[String]) -> bool {
    lines.iter().any(|l| l == REFUSED)
}

/// The UI's own templates register under context 0, which nothing replaces.
#[test]
fn an_addon_cannot_replace_a_template_of_the_ui_s_own() {
    let mut s = with_addons(&[("Probe", false, 20), ("Blizzard_Probe", true, 30)]);
    load_doc(&s, &template("DupTemplate", 10));
    assert!(refused_in(&load_addon(&mut s, "Probe")));
    assert!(refused_in(&load_addon(&mut s, "Blizzard_Probe")));
    assert_eq!(width_through(&s, "DupTemplate"), 10.0);
}

/// Two insecure addons: both contexts are non-zero, so the second is refused.
#[test]
fn of_two_insecure_addons_the_first_keeps_the_name() {
    let mut s = with_addons(&[("ProbeA", false, 10), ("ProbeB", false, 20)]);
    assert!(!refused_in(&load_addon(&mut s, "ProbeA")));
    assert!(refused_in(&load_addon(&mut s, "ProbeB")));
    assert_eq!(width_through(&s, "DupTemplate"), 10.0);
}

/// The one replacing case: an entry from a non-zero context, a registration under context 0.
#[test]
fn a_secure_addon_replaces_an_insecure_addons_template() {
    let mut s = with_addons(&[("Probe", false, 10), ("Blizzard_Probe", true, 20)]);
    assert!(!refused_in(&load_addon(&mut s, "Probe")));
    assert!(!refused_in(&load_addon(&mut s, "Blizzard_Probe")));
    assert_eq!(width_through(&s, "DupTemplate"), 20.0);
}

/// And never the reverse: a secure addon's entry is context 0.
#[test]
fn an_insecure_addon_cannot_replace_a_secure_addons_template() {
    let mut s = with_addons(&[("Blizzard_Probe", true, 10), ("Probe", false, 20)]);
    assert!(!refused_in(&load_addon(&mut s, "Blizzard_Probe")));
    assert!(refused_in(&load_addon(&mut s, "Probe")));
    assert_eq!(width_through(&s, "DupTemplate"), 10.0);
}

/// The context is the loading addon's, set and restored around its load (`0x51f36d`, `0x51f584`):
/// a document loaded after an insecure addon is back at context 0, so it replaces that addon's.
#[test]
fn the_context_is_restored_after_an_addon_loads() {
    let mut s = with_addons(&[("Probe", false, 10)]);
    load_addon(&mut s, "Probe");
    let lines = load_doc(&s, &template("DupTemplate", 20));
    assert!(lines.is_empty(), "{lines:?}");
    assert_eq!(width_through(&s, "DupTemplate"), 20.0);
}

/// A top-level `virtual` element with no name, or an empty one (the first-byte test at
/// `0x6ee1a9`), is reported as `"Unnamed virtual node at top level"` (`0x871020`) at severity 1
/// into the document's record (`0x6ee1c4`-`0x6ee1cf`), in document order, and registers nothing.
#[test]
fn an_unnamed_virtual_node_is_reported_into_the_load_log() {
    let s = script();
    let xml = r#"<Ui>
        <Frame virtual="true"/>
        <Frame name="" virtual="true"/>
        <Frame name="NamedTemplate" virtual="true"/>
    </Ui>"#;
    let report = load(&s, &framexml::parse(xml).unwrap(), &|_| None);
    let unnamed = "Unnamed virtual node at top level";
    assert_eq!(report.status.lines(), [unnamed, unnamed]);
    assert_eq!(report.warnings, [unnamed, unnamed]);
    let mut parent = crate::status::Status::default();
    report
        .status
        .close_into(&mut parent, false, "++ Loading file X".into());
    assert_eq!(parent.lines(), ["++ Loading file X", unnamed, unnamed]);
}
