//! `ConsoleExec` (`0x515f10`), which runs its line through `ConsoleCommandExecute` (`0x63ce00`)
//! inside the call (`0x515f43`), and the parse rules the host's own executor shares.
//!
//! The commands that write a CVar run here, on the VM's mirror of the registry, so their effect is
//! what the chunk's next `GetCVar` reads, as in the reference: `ConsoleVar.cpp`'s `set`
//! (`0x63d500`), `cvar_reset` (`0x63d590`) and `cvar_default` (`0x63d640`), and a CVar's own
//! command with a value (`0x63dde0`). Their writes queue to the host like `SetCVar`'s, and what
//! they print goes with the change callbacks' lines to the host's log. Every other line (a bare
//! CVar name, `cvarlist`, the host's registered commands, an unknown word) goes to the host's
//! executor, which reads it with the same [`console_word`].

use mlua::{Lua, Value};

use super::Model;

/// `0x850d18`, verbatim.
const USAGE_CONSOLE_EXEC: &str = "Usage: ConsoleExec(\"console_command\")";

/// The parse buffer the command word is copied into (`0xc4ee08`): `0x3f` bytes of text
/// (`0x63f893`).
const WORD_BYTES: usize = 0x3f;

/// `ConsoleCommandExecute`'s split: leading spaces skipped (`0x63ce2b`, `0x20` only), then
/// `0x63f880` copies whole characters up to a space or `0x3f` bytes into the word, and skips the
/// spaces after it. A tab neither ends the word nor is skipped. A word cut at the buffer's end
/// leaves its remainder at the head of the rest.
pub fn console_word(line: &str) -> (&str, &str) {
    let line = line.trim_start_matches(' ');
    let mut end = 0;
    for (i, c) in line.char_indices() {
        if c == ' ' || i + c.len_utf8() > WORD_BYTES {
            break;
        }
        end = i + c.len_utf8();
    }
    (&line[..end], line[end..].trim_start_matches(' '))
}

/// The delimiters the console's commands tokenize with (`0x80e1d4`).
const CONSOLE_DELIMS: &[u8] = b" ,;\t\"\r\n";

/// The name buffer of `set`, `cvar_reset` and `cvar_default` (`0x63d510`, `0x63d59d`, `0x63d64d`).
const NAME_BYTES: usize = 0x40;

/// `set`'s value buffer (`0x63d529`).
const VALUE_BYTES: usize = 0x100;

/// Storm's `SStrTokenize` (`0x64ae50`): the next token of `s` into a buffer of `size` bytes, and
/// `s` advanced past it. With `"` among the delimiters (`0x64ae8d`):
///
/// - leading delimiters are skipped, but a `"` among them opens a quoted span and is consumed
///   (`0x64aebf`);
/// - outside a span a delimiter ends the token and is consumed (`0x64af37`, `0x64af66`), and a `"`
///   after the token's first byte ends it and is left (`0x64af09`-`0x64af12`);
/// - inside a span delimiters are kept, and a `"` closes the span, is consumed and ends the token
///   (`0x64af14`-`0x64af2a`);
/// - at `size - 1` bytes storing stops, and the rest of the token is skipped to its end
///   (`0x64af4e`).
///
/// Empty input is the empty token.
fn s_str_tokenize(s: &mut &[u8], size: usize, delims: &[u8]) -> Vec<u8> {
    let quotes = delims.contains(&b'"');
    let mut i = 0;
    let mut in_quote = false;
    while let Some(&c) = s.get(i) {
        if !delims.contains(&c) {
            break;
        }
        i += 1;
        if quotes && c == b'"' {
            in_quote = true;
            break;
        }
    }
    let mut token = Vec::new();
    while let Some(&c) = s.get(i) {
        if quotes && c == b'"' {
            if !token.is_empty() && !in_quote {
                break;
            }
            i += 1;
            in_quote = !in_quote;
            if !in_quote {
                break;
            }
            continue;
        }
        if !in_quote && delims.contains(&c) {
            i += 1;
            break;
        }
        if token.len() + 1 < size {
            token.push(c);
        }
        i += 1;
    }
    *s = &s[i..];
    token
}

/// The next console token of `rest`, in a buffer of `size` bytes.
fn console_token(rest: &mut &[u8], size: usize) -> String {
    String::from_utf8_lossy(&s_str_tokenize(rest, size, CONSOLE_DELIMS)).into_owned()
}

/// `set`'s two arguments (`0x63d51d`, `0x63d539`): the name's token, then the value's, so a value
/// is one word or one quoted span, and none is the empty string.
pub fn set_args(rest: &str) -> (String, String) {
    let mut rest = rest.as_bytes();
    let name = console_token(&mut rest, NAME_BYTES);
    (name, console_token(&mut rest, VALUE_BYTES))
}

/// `cvar_reset`'s and `cvar_default`'s one argument, the CVar's name's token (`0x63d5aa`,
/// `0x63d65a`).
pub fn cvar_name_arg(rest: &str) -> String {
    console_token(&mut rest.as_bytes(), NAME_BYTES)
}

/// The line `set` and the reset pair print for a name no row has. Deviation: the reference's `set`
/// registers an unknown name as a new category-5 record (`0x63d569`-`0x63d582`); benilla refuses
/// it, because nothing would read the row and `config.toml` keeps an unknown key anyway.
pub fn no_such_cvar(name: &str) -> String {
    format!("console: no CVar named '{name}'")
}

/// `ConsoleExec` is in the registrar both states call (`0x51bb20`, from the glue registrar
/// `0x46abb0` and the in-game `0x51bb60`).
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    lua.globals().set(
        "ConsoleExec",
        lua.create_function(|lua, line: Value| {
            // `lua_isstring(1)` (`0x515f18`): a string or a number runs, an empty one too; anything
            // else raises the usage line (`0x850d18`) and runs nothing. The executor reads a C
            // string, so an embedded NUL ends the line.
            let line = super::binding_abi::string_arg(lua, line, USAGE_CONSOLE_EXEC)?;
            let line = line.split('\0').next().unwrap_or("");
            let (word, rest) = console_word(line);
            // `reloadUI` is `ReloadUiCommand` (`0x4035f0`), which asks `CGGameUI::Reload`'s gate,
            // tag 10 (`0x4035fa` → `0x491380`), inside the call: refused, it does nothing.
            if word.eq_ignore_ascii_case("reloadui")
                && !super::protected::gate(lua, super::protected::Tag::RELOAD)
            {
                return Ok(());
            }
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            if !run_cvar_command(&mut model, word, rest) {
                model.console_lines.push(line.to_string());
            }
            Ok(())
        })?,
    )?;
    Ok(())
}

/// Run `word` if it is one of the commands that write a CVar; `false` hands the line on.
fn run_cvar_command(model: &mut Model, word: &str, rest: &str) -> bool {
    match word.to_ascii_lowercase().as_str() {
        "set" => {
            let (name, value) = set_args(rest);
            if !model.cvars.contains_key(&name.to_ascii_lowercase()) {
                model.cvar_console.push(no_such_cvar(&name));
            } else {
                super::cvars::write_cvar(model, &name, value);
            }
        }
        "cvar_reset" | "cvar_default" => {
            let name = cvar_name_arg(rest);
            if name.is_empty() {
                model
                    .cvar_console
                    .push("usage: cvar_default <cvar>".to_string());
                return true;
            }
            match model.cvars.get(&name.to_ascii_lowercase()) {
                Some(slot) => {
                    let default = slot.default.clone();
                    super::cvars::write_cvar(model, &name, default);
                }
                None => model.cvar_console.push(no_such_cvar(&name)),
            }
        }
        // A CVar's own command (`0x63dde0`): the rest past its spaces, untrimmed, is the value;
        // with none it prints the value, which is the host's.
        key => {
            let value = rest.trim_start_matches(' ');
            if value.is_empty() || !model.cvars.contains_key(key) {
                return false;
            }
            super::cvars::write_cvar(model, word, value.to_string());
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::{console_word, s_str_tokenize, set_args, CONSOLE_DELIMS};
    use crate::script::UiScript;

    /// `0x64ae50` with the console's delimiters (`0x80e1d4`), token by token.
    #[test]
    fn the_tokenizer_reads_words_quoted_spans_and_cuts() {
        let tokens = |line: &str, size: usize| {
            let mut s = line.as_bytes();
            let mut out = Vec::new();
            for _ in 0..4 {
                out.push(String::from_utf8(s_str_tokenize(&mut s, size, CONSOLE_DELIMS)).unwrap());
            }
            out
        };
        assert_eq!(tokens("a b", 16), ["a", "b", "", ""]);
        assert_eq!(tokens(" ,;a,,b", 16), ["a", "b", "", ""]);
        assert_eq!(tokens("\"a b\" c", 16), ["a b", "c", "", ""]);
        assert_eq!(
            tokens("\"a,;b\"c", 16),
            ["a,;b", "c", "", ""],
            "a closing quote ends it"
        );
        assert_eq!(
            tokens("ab\"c d\"", 16),
            ["ab", "c d", "", ""],
            "a quote after text is left"
        );
        assert_eq!(tokens("\"open span", 16), ["open span", "", "", ""]);
        assert_eq!(
            tokens("abcdef gh", 4),
            ["abc", "gh", "", ""],
            "cut, then skipped to the end"
        );
        assert_eq!(tokens("", 16), ["", "", "", ""]);
    }

    /// `set` tokenizes its name into 0x40 bytes and its value into 0x100 (`0x63d500`), so the
    /// value is one word or one quoted span, and a missing one is the empty string.
    #[test]
    fn set_takes_one_token_for_its_value() {
        let s = UiScript::new().unwrap();
        s.register_cvars([("myKnob", "z")]);
        let set = |s: &UiScript, line: &str| {
            s.eval::<String>(&format!("ConsoleExec({line:?}) return GetCVar('myKnob')"))
                .unwrap()
        };
        assert_eq!(set(&s, "set myKnob a b"), "a");
        assert_eq!(set(&s, "set myKnob \"a b\""), "a b");
        assert_eq!(set(&s, "set myKnob 1,2"), "1");
        assert_eq!(set(&s, "set myKnob ab\"c\""), "ab");
        assert_eq!(set(&s, "set myKnob ,;  v"), "v");
        assert_eq!(
            set(&s, "set myKnob"),
            "",
            "no value stores the empty string"
        );
        assert_eq!(
            set(&s, &format!("set myKnob {}", "x".repeat(300))),
            "x".repeat(255)
        );
        // A CVar's own command takes the rest as it is.
        assert_eq!(set(&s, "myKnob a b, c "), "a b, c ");
        let long = format!("{}myKnob", "n".repeat(64));
        assert_eq!(
            set_args(&format!("{long} v")),
            ("n".repeat(63), "v".to_string())
        );
    }

    /// `0x63ce2b` skips spaces only; `0x63f880` ends the word at a space or after `0x3f` bytes.
    #[test]
    fn the_word_ends_at_a_space_only() {
        assert_eq!(console_word("  farclip   400 "), ("farclip", "400 "));
        assert_eq!(console_word("farclip\t400"), ("farclip\t400", ""));
        assert_eq!(console_word("\tfarclip 400"), ("\tfarclip", "400"));
        assert_eq!(console_word(""), ("", ""));
        let long = "a".repeat(70);
        assert_eq!(console_word(&long), (&long[..63], &long[63..]));
    }

    #[test]
    fn console_exec_writes_a_registered_cvar_and_queues_anything_else() {
        let mut s = UiScript::new().unwrap();
        s.register_cvars([("uiScale", "1")]);
        s.run("ConsoleExec('uiscale 0.8') ConsoleExec('reloadui') ConsoleExec('uiScale')")
            .unwrap();
        assert_eq!(
            s.eval::<String>("return GetCVar('uiScale')").unwrap(),
            "0.8",
            "the CVar store is the same one SetCVar writes, matched case-insensitively"
        );
        // A bare CVar name, whose value the reference prints, goes to the command lane.
        assert_eq!(
            s.take_console_lines(),
            vec!["reloadui".to_string(), "uiScale".to_string()]
        );
        assert_eq!(
            s.take_cvar_changes(),
            vec![("uiScale".to_string(), "0.8".to_string())],
            "a bare name writes nothing"
        );
    }

    /// `ConsoleCommandExecute` runs inside the call (`0x515f43`), so every command that writes a
    /// CVar is read back by the chunk's next statement: `set`, the reset pair and a CVar's own
    /// command alike.
    #[test]
    fn console_exec_runs_the_cvar_commands_inside_the_call() {
        let mut s = UiScript::new().unwrap();
        s.register_cvars([("farclip", "350"), ("MusicVolume", "0.4")]);
        let read = |s: &UiScript, chunk: &str| {
            s.eval::<String>(&format!("{chunk} return GetCVar('farclip')"))
                .unwrap()
        };
        assert_eq!(read(&s, "ConsoleExec('set farclip 400')"), "400");
        assert_eq!(read(&s, "ConsoleExec('cvar_default farclip')"), "350");
        assert_eq!(read(&s, "ConsoleExec('SET FARCLIP 500')"), "500");
        assert_eq!(read(&s, "ConsoleExec('cvar_reset farclip')"), "350");
        assert_eq!(read(&s, "ConsoleExec('farclip 450')"), "450");
        assert_eq!(
            s.take_cvar_changes(),
            [
                ("farclip", "400"),
                ("farclip", "350"),
                ("farclip", "500"),
                ("farclip", "350"),
                ("farclip", "450"),
            ]
            .map(|(n, v)| (n.to_string(), v.to_string())),
            "each write reaches the host like a SetCVar's"
        );
        assert!(
            s.take_console_lines().is_empty(),
            "nothing left for the host"
        );
        s.run("ConsoleExec('set nosuch 1') ConsoleExec('cvar_default nosuch')")
            .unwrap();
        assert_eq!(
            s.take_cvar_console_lines(),
            [
                "console: no CVar named 'nosuch'",
                "console: no CVar named 'nosuch'"
            ]
        );
    }

    /// `lua_isstring(1)` (`0x515f18`) passes a string or a number, an empty string included,
    /// and `luaL_error` (`0x515f27`) raises the usage line for anything else, running nothing.
    #[test]
    fn console_exec_takes_a_string_or_a_number_and_raises_its_usage_otherwise() {
        let mut s = UiScript::new().unwrap();
        for call in [
            "ConsoleExec()",
            "ConsoleExec(nil)",
            "ConsoleExec({})",
            "ConsoleExec(true)",
        ] {
            let err = s.run(call).unwrap_err().to_string();
            assert!(
                err.contains("Usage: ConsoleExec(\"console_command\")"),
                "{call}: {err}"
            );
        }
        assert!(s.take_console_lines().is_empty(), "nothing ran");
        s.run("ConsoleExec(5) ConsoleExec('')").unwrap();
        assert_eq!(
            s.take_console_lines(),
            vec!["5".to_string(), String::new()],
            "a number is its text, and an empty line still runs"
        );
    }

    /// `reloadUI` is `ReloadUiCommand` (`0x4035f0`), which asks the gate's tag 10 (`0x4035fa`): a
    /// macro's line outside an input event is ignored, a key's body or the client's own passes.
    #[test]
    fn console_exec_reload_ui_asks_the_gate() {
        let mut s = UiScript::new().unwrap();
        s.run(r#"RunScript("ConsoleExec('reloadUI')")"#).unwrap();
        assert!(s.take_console_lines().is_empty());
        s.run_binding(r#"RunScript("ConsoleExec('ReloadUI')")"#)
            .unwrap();
        assert_eq!(s.take_console_lines(), vec!["ReloadUI".to_string()]);
        s.run("ConsoleExec('reloadui')").unwrap();
        assert_eq!(s.take_console_lines(), vec!["reloadui".to_string()]);
    }
}
