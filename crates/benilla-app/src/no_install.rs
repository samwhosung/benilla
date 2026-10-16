//! The boot with no WoW install: fatal. Fonts, art and the stock interface all come from the
//! install, so a client without one is a window of textless buttons. The reference ships inside its
//! install; its archive mount (`0x403740`, `0x403b00`) logs a missing base archive and goes on, and
//! ends the process with a fatal error dialog (`SErrDisplayAppFatal2`, `0x403b6f`) only for one
//! that is present and fails its checks.
//!
//! Deviation: a boot with no install at all ends at a fatal error dialog, where the reference logs
//! each missing archive and goes on (`0x403b00`), because nothing past this point can draw a word.
//!
//! [`require`] is the decision, pure; [`fail`] is the exit, which shows a native dialog only to a
//! person: an [unattended](crate::run_mode::unattended) run prints the same message and exits.

use std::path::PathBuf;

use bevy::app::AppExit;

/// Why the client cannot start: the message a person is shown and a log carries.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct NoInstall {
    title: &'static str,
    message: String,
}

impl NoInstall {
    pub(crate) fn message(&self) -> &str {
        &self.message
    }

    /// A `Data` folder whose archives will not open, a corrupt one or none the chain knows: as fatal
    /// as no folder, the case the reference's `0x403b6f` covers for a base archive.
    pub(crate) fn unreadable(data_dir: &std::path::Path, err: &anyhow::Error) -> Self {
        Self {
            title: "benilla: the WoW install cannot be read",
            message: format!(
                "benilla found a World of Warcraft install at\n  {}\nbut cannot read its archives:\n  \
                 {err:#}\n\nCheck that this is an unmodified 1.12.1 Data folder.",
                data_dir.display()
            ),
        }
    }
}

/// The install the boot goes on with, or the fatal error: `found` is [`benilla_formats::wow_data`]
/// and `looked_in` its [`benilla_formats::candidates`], named in order in the message.
pub(crate) fn require(found: Option<PathBuf>, looked_in: &[PathBuf]) -> Result<PathBuf, NoInstall> {
    found.ok_or_else(|| NoInstall {
        title: "benilla: no WoW install found",
        message: message(looked_in),
    })
}

/// What to tell the player: the folders searched and the two ways to supply an install.
fn message(looked_in: &[PathBuf]) -> String {
    let mut out =
        String::from("benilla needs a World of Warcraft 1.12.1 install and found none.\n");
    if looked_in.is_empty() {
        // `$WOW_DATA` set and empty ends the search with nothing in it.
        out.push_str("\nWOW_DATA is set and empty, which means no install.\n");
    } else {
        out.push_str("\nIt looked in:\n");
        for dir in looked_in {
            out.push_str(&format!("  {}\n", dir.display()));
        }
    }
    out.push_str(
        "\nPut the install's Data folder at Data/ or WoW/Data/ beside the benilla binary, \
         or point WOW_DATA at it.",
    );
    out
}

/// Report `err` and end the boot with a failing exit: stderr always, the dialog too unless the run
/// is `unattended`, so an instrumented run never waits on a click.
pub(crate) fn fail(err: &NoInstall, unattended: bool) -> AppExit {
    eprintln!("benilla: {}", err.message());
    if !unattended {
        // Linux shows it through `zenity` and logs nothing when that is missing, which stderr covers.
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Error)
            .set_title(err.title)
            .set_description(err.message())
            .set_buttons(rfd::MessageButtons::Ok)
            .show();
    }
    AppExit::error()
}

/// [`require`] on the real resolver.
pub(crate) fn resolve() -> Result<PathBuf, NoInstall> {
    require(benilla_formats::wow_data(), &benilla_formats::candidates())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn dirs() -> Vec<PathBuf> {
        [
            "/opt/wow/Data",
            "/games/benilla/Data",
            "/games/benilla/WoW/Data",
        ]
        .map(PathBuf::from)
        .to_vec()
    }

    /// With no install the boot is fatal, and the message names every folder searched, in order,
    /// and both ways to supply one.
    #[test]
    fn no_install_is_fatal_and_names_every_folder_searched() {
        let looked_in = dirs();
        let err = require(None, &looked_in).expect_err("no install must be fatal");
        let msg = err.message();
        let mut from = 0;
        for dir in &looked_in {
            let at = msg[from..]
                .find(&dir.display().to_string())
                .unwrap_or_else(|| panic!("{dir:?} not named in order in:\n{msg}"));
            from += at;
        }
        assert!(
            msg.contains("Data/ or WoW/Data/ beside the benilla binary"),
            "{msg}"
        );
        assert!(msg.contains("WOW_DATA"), "{msg}");
    }

    /// `WOW_DATA=` (set, empty) searches nowhere, and the message says why instead of listing none.
    #[test]
    fn an_empty_wow_data_is_named_as_the_reason() {
        let err = require(None, &[]).expect_err("no install must be fatal");
        assert!(
            err.message().contains("WOW_DATA is set and empty"),
            "{}",
            err.message()
        );
        assert!(!err.message().contains("looked in"));
    }

    /// An install found goes on, whatever else was searched.
    #[test]
    fn a_found_install_boots() {
        let found = Path::new("/games/benilla/Data").to_path_buf();
        assert_eq!(require(Some(found.clone()), &dirs()), Ok(found));
    }

    /// The unattended exit is a failing one: `AppExit::error()` is what the launcher returns as the
    /// process status. (The attended branch shows a dialog, so no test takes it.)
    #[test]
    fn the_unattended_exit_fails() {
        let err = require(None, &dirs()).unwrap_err();
        assert_ne!(fail(&err, true), AppExit::Success);
    }

    /// A folder whose archives will not open names the folder and the error, under its own title.
    #[test]
    fn an_unreadable_install_names_the_folder_and_the_error() {
        let err = NoInstall::unreadable(
            Path::new("/games/benilla/Data"),
            &anyhow::anyhow!("no known vanilla MPQs found in /games/benilla/Data"),
        );
        assert!(err
            .message()
            .contains("/games/benilla/Data\nbut cannot read"));
        assert!(err.message().contains("no known vanilla MPQs"));
        assert_ne!(err.title, "benilla: no WoW install found");
    }
}
