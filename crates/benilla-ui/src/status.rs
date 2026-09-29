//! The reference's load status (`CStatus`, `Status.cpp`, entries `{text, severity}`): each level of
//! a UI load (the add-on, its `.toc`, each XML document) reports into a record of its own, which is
//! merged into its caller's when it closes and drained, line by line, to `Logs\FrameXML.log`
//! (`0x842ecc`) at the load's end.

/// A trace, printed only under `FrameXML_Debug` (`"-- Creating %s named %s"`, `0x6ee2bc`).
pub const TRACE: u8 = 0;
/// A file-level failure: a file that did not open or did not parse (`0x6edad4`, `0x704c15`).
pub const FAILURE: u8 = 2;

/// One level's record: its lines in order and the running maximum severity (`+0x10`, which the
/// append `0x41a000` and the prepend `0x419ed0` raise, never a count).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    lines: Vec<String>,
    max: u8,
}

impl Status {
    /// Append a line (`0x41a000`).
    pub fn report(&mut self, severity: u8, text: impl Into<String>) {
        self.lines.push(text.into());
        self.max = self.max.max(severity);
    }

    /// Merge `other` in after this record's own lines, as `0x41a100` re-reports each entry.
    pub fn merge(&mut self, other: Status) {
        self.lines.extend(other.lines);
        self.max = self.max.max(other.max);
    }

    /// Close one level into its caller: its banner goes first (a prepend at severity 0) when
    /// `debug` is on or the level reported anything above a trace (`jg` on `FrameXML_Debug`, else
    /// `jle` on the maximum, `0x6eddb6`/`0x6eddbb`, `0x6ee207`, `0x51f44f`), then the merge.
    pub fn close_into(mut self, parent: &mut Status, debug: bool, banner: String) {
        if debug || self.max > TRACE {
            self.lines.insert(0, banner);
        }
        parent.merge(self);
    }

    /// The lines, in the order the drain writes them.
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// The lines, leaving the record empty.
    pub fn take_lines(&mut self) -> Vec<String> {
        self.max = TRACE;
        std::mem::take(&mut self.lines)
    }
}

/// A path as the reference reports it: install-relative and `\`-separated.
pub fn install_path(path: &str) -> String {
    path.replace('/', "\\")
}

/// Whether the load-one-file routine (`0x6ede10`) runs `path` as a chunk: the text from its last
/// `.` equals `.lua` (`0x8710c8`) under `_strnicmp` (`0x6edeea`-`0x6edf05`).
pub fn runs_as_lua(path: &str) -> bool {
    path.rfind('.')
        .is_some_and(|i| path[i..].eq_ignore_ascii_case(".lua"))
}

/// A file that did not open: `"Error loading %s"` (`0x872e50`) from the chunk arm (`0x704bc0`),
/// else `"Couldn't open %s"` (`0x846ff4`) from the XML arm (`0x6edaa0`).
pub fn missing(path: &str, lua: bool) -> String {
    if lua {
        format!("Error loading {}", install_path(path))
    } else {
        format!("Couldn't open {}", install_path(path))
    }
}

/// A document that opened and did not parse (`0x846fd8`, `0x6edb11`).
pub fn unparsed(path: &str) -> String {
    format!("Couldn't parse XML in {}", install_path(path))
}

/// An XML document's banner (`0x87100c`, `0x6ee21c`).
pub fn file_banner(path: &str) -> String {
    format!("++ Loading file {}", install_path(path))
}

/// A `.toc`'s banner (`0x870fec`, `0x6eddc8`).
pub fn toc_banner(path: &str) -> String {
    format!("** Loading table of contents {}", install_path(path))
}

/// An add-on's banner (`0x8539fc`, `0x51f464`).
pub fn addon_banner(name: &str) -> String {
    format!("Loading add-on {name}")
}

/// Where an add-on's `.toc` is, as its banner names it: `Interface\AddOns\<name>\<name>.toc`.
pub fn addon_toc_path(name: &str) -> String {
    format!("Interface\\AddOns\\{name}\\{name}.toc")
}

/// One drain of a load's record into `Logs\FrameXML.log`, in the order the loads ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LogWrite {
    /// A UI load's drain (`UI_Init`, `0x490187`-`0x4901b6`): the log opens in mode 0 (`0x48ff06`),
    /// created anew (`CREATE_ALWAYS`, `0x65a1c0`) at its first line (`0x65a930`), so a load that
    /// reported nothing leaves the file as it was.
    Rewrite(Vec<String>),
    /// A runtime `LoadAddOn`'s drain (`0x46aac0`): mode 4 (`0x48ea4e`), opened (`OPEN_ALWAYS`)
    /// and appended to at its first line.
    Append(Vec<String>),
}

/// A VM's two load records and the drains they have made. `LoadAddOn` keeps its own record
/// (`ds:0xb4e3dc`), refcounted (`ds:0xb4e3e0`) so a nested call writes one block, and drained when
/// the count returns to zero (`0x48eab4`), even inside a UI load, whose own drain comes last.
#[derive(Debug, Default)]
pub(crate) struct LoadLog {
    pub(crate) ui: Status,
    pub(crate) demand: Status,
    pub(crate) demand_depth: u32,
    pub(crate) writes: Vec<LogWrite>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The install's own log shape (the reference's `Logs\FrameXML.log`, `FrameXML_Debug` 0): the
    /// add-on banner first, then the toc's, then the miss, and a later document with a message
    /// under its own banner; a clean document says nothing.
    #[test]
    fn banners_frame_only_what_reported_something() {
        let mut root = Status::default();
        let mut addon = Status::default();
        let mut toc = Status::default();
        toc.report(FAILURE, missing("Interface/AddOns/A/Missing.xml", false));
        let mut clean = Status::default();
        clean.report(TRACE, "-- Creating Frame named X");
        clean.close_into(&mut toc, false, file_banner("Interface/AddOns/A/Clean.xml"));
        let mut broken = Status::default();
        broken.report(FAILURE, missing("Interface/AddOns/A/b.lua", true));
        broken.close_into(&mut toc, false, file_banner("Interface/AddOns/A/Core.xml"));
        toc.close_into(&mut addon, false, toc_banner(&addon_toc_path("A")));
        addon.close_into(&mut root, false, addon_banner("A"));
        let mut quiet = Status::default();
        Status::default().close_into(&mut quiet, false, toc_banner(&addon_toc_path("B")));
        quiet.close_into(&mut root, false, addon_banner("B"));
        assert_eq!(
            root.lines(),
            [
                "Loading add-on A",
                "** Loading table of contents Interface\\AddOns\\A\\A.toc",
                "Couldn't open Interface\\AddOns\\A\\Missing.xml",
                "-- Creating Frame named X",
                "++ Loading file Interface\\AddOns\\A\\Core.xml",
                "Error loading Interface\\AddOns\\A\\b.lua",
            ]
        );
    }

    /// Under `FrameXML_Debug` every level prints its banner, reported or not.
    #[test]
    fn debug_prints_every_banner() {
        let mut root = Status::default();
        Status::default().close_into(&mut root, true, file_banner("x.xml"));
        assert_eq!(root.lines(), ["++ Loading file x.xml"]);
    }

    #[test]
    fn the_chunk_arm_is_the_lua_extension_alone() {
        assert!(runs_as_lua("Interface/AddOns/A/core.LUA"));
        assert!(!runs_as_lua("Interface/AddOns/A/core.lua.xml"));
        assert!(!runs_as_lua("Interface/AddOns/A/core"));
    }
}
