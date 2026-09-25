//! Tests for the read-back probe: it reads after the session start, in the corpus's VM.

use std::path::{Path, PathBuf};

use super::probe::{probe, Step};

/// One throwaway AddOns root, cleaned up on drop even if a test panics.
struct Fixtures(PathBuf);

impl Fixtures {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "benilla-probe-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Self(root)
    }

    fn addon(&self, name: &str, body: &str) -> &Self {
        let dir = self.0.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{name}.toc")),
            "## Interface: 11200\nbody.lua\n",
        )
        .unwrap();
        std::fs::write(dir.join("body.lua"), body).unwrap();
        self
    }

    fn root(&self) -> &Path {
        &self.0
    }
}

impl Drop for Fixtures {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A file-scope global and a `PLAYER_LOGIN` global are both readable, and a raise comes back as
/// `ERROR:` without stopping the next eval.
#[test]
fn the_probe_reads_the_vm_after_the_session_start() {
    let fx = Fixtures::new("after");
    fx.addon(
        "Talker",
        r#"
        TalkerFileScope = "loaded"
        local f = CreateFrame("Frame")
        f:RegisterEvent("PLAYER_LOGIN")
        f:SetScript("OnEvent", function() TalkerAtLogin = "logged in" end)
    "#,
    );

    let out = probe(
        fx.root(),
        "Talker",
        &[
            Step::Eval("return TalkerFileScope".to_string()),
            Step::Eval("return TalkerAtLogin".to_string()),
            Step::Eval("return nil + 1".to_string()),
            Step::Eval("return TalkerFileScope".to_string()),
        ],
    )
    .expect("the fixture has a manifest");

    assert!(out.load_errors.is_empty(), "{:?}", out.load_errors);
    assert!(out.session_errors.is_empty(), "{:?}", out.session_errors);
    let answers: Vec<&str> = out.answers.iter().map(|(_, a)| a.as_str()).collect();
    assert_eq!(answers[0], "= loaded", "the file scope ran");
    assert_eq!(
        answers[1], "= logged in",
        "the PLAYER_LOGIN handler ran BEFORE the read — a load-only probe would answer nil"
    );
    assert!(
        answers[2].starts_with("ERROR:"),
        "a raise is reported, not propagated: {}",
        answers[2]
    );
    assert_eq!(
        answers[3], "= loaded",
        "...and the eval after a raise still ran"
    );
}

/// The registry is the whole folder's: a sibling that is never loaded is still installed.
#[test]
fn the_probe_sees_the_whole_folder_installed() {
    let fx = Fixtures::new("registry");
    fx.addon("Asker", "AskerSaw = GetAddOnInfo(\"Sibling\")\n");
    fx.addon("Sibling", "SiblingRan = 1\n");

    let out = probe(
        fx.root(),
        "Asker",
        &[
            Step::Eval("return AskerSaw".to_string()),
            Step::Eval("return GetNumAddOns()".to_string()),
            Step::Eval("return SiblingRan".to_string()),
        ],
    )
    .expect("the fixture has a manifest");

    assert!(out.load_errors.is_empty(), "{:?}", out.load_errors);
    assert_eq!(
        out.answers[0].1, "= Sibling",
        "the sibling must be INSTALLED in the probed VM"
    );
    assert_eq!(
        out.answers[1].1, "= 2",
        "the registry is the folder's, not the selection's"
    );
    // Installed is not loaded: one addon per VM, as in the survey.
    assert_eq!(
        out.answers[2].1, "= nil",
        "an installed sibling must NOT have been run"
    );
}

/// A folder with no manifest is `None`, not an empty outcome.
#[test]
fn a_folder_without_a_manifest_is_refused() {
    let fx = Fixtures::new("nomanifest");
    std::fs::create_dir_all(fx.root().join("Backup")).unwrap();
    assert!(probe(fx.root(), "Backup", &[]).is_none());
    assert!(probe(fx.root(), "NotThereAtAll", &[]).is_none());
}
