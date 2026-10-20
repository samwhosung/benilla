//! A tooltip line's text as the host prepares it ahead of a hover: literal pieces and
//! `GlobalStrings.lua` templates by key, each template read when the tooltip renders, as the
//! builders read `_G` at every call through `FrameScript_GetText 0x703bf0`. A string rewritten
//! after the host prepared the line reaches the next render.

use crate::strings::{fill, plural, Arg};

/// An argument of a template's `%` hole, owned so a prepared line outlives its source.
#[derive(Clone, Debug, PartialEq)]
pub enum KeyedArg {
    S(String),
    D(i64),
    F(f64),
}

impl KeyedArg {
    fn as_arg(&self) -> Arg<'_> {
        match self {
            KeyedArg::S(s) => Arg::S(s),
            KeyedArg::D(n) => Arg::D(*n),
            KeyedArg::F(x) => Arg::F(*x),
        }
    }
}

/// One piece of a prepared text.
#[derive(Clone, Debug, PartialEq)]
pub enum TextPart {
    Text(String),
    /// A template by key: filled with `args` (`SStrPrintf 0x64a7f0`), or, with `args` `None`,
    /// copied as it is (`SStrCopy 0x64a5a0`). `or` stands in for a missing or empty template.
    Keyed {
        key: String,
        /// `GetText`'s plural ordinal ([`plural`]).
        ordinal: Option<u32>,
        args: Option<Vec<KeyedArg>>,
        or: String,
    },
}

/// A prepared text: its pieces in order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TooltipText(pub Vec<TextPart>);

impl TooltipText {
    /// The template under `key` filled with `args`; nothing when the key is missing.
    pub fn keyed(key: impl Into<String>, args: Vec<KeyedArg>) -> Self {
        Self(vec![TextPart::Keyed {
            key: key.into(),
            ordinal: None,
            args: Some(args),
            or: String::new(),
        }])
    }

    /// The string under `key` as it is, `%` signs and all; nothing when the key is missing.
    pub fn copied(key: impl Into<String>) -> Self {
        Self(vec![TextPart::Keyed {
            key: key.into(),
            ordinal: None,
            args: None,
            or: String::new(),
        }])
    }

    /// This text followed by literal `text`.
    pub fn then(mut self, text: impl Into<String>) -> Self {
        self.0.push(TextPart::Text(text.into()));
        self
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The text with each template read through `get` now.
    pub fn text(&self, get: &dyn Fn(&str) -> Option<String>) -> String {
        let mut out = String::new();
        for part in &self.0 {
            match part {
                TextPart::Text(t) => out.push_str(t),
                TextPart::Keyed {
                    key,
                    ordinal,
                    args,
                    or,
                } => {
                    let template = match ordinal {
                        Some(_) => plural(key, *ordinal, get),
                        None => get(key).filter(|s| !s.is_empty()),
                    };
                    match (template, args) {
                        (Some(t), Some(args)) => {
                            let args: Vec<Arg<'_>> = args.iter().map(KeyedArg::as_arg).collect();
                            out.push_str(&fill(&t, &args));
                        }
                        (Some(t), None) => out.push_str(&t),
                        (None, _) => out.push_str(or),
                    }
                }
            }
        }
        out
    }

    /// [`Self::text`] read from the VM's globals, `None` when it comes out empty: the line core
    /// adds no line with no text (`0x5302a9`).
    pub(crate) fn line(&self, lua: &mlua::Lua) -> Option<String> {
        Some(self.text(&|key| crate::strings::global(lua, key))).filter(|t| !t.is_empty())
    }
}

impl From<&str> for TooltipText {
    fn from(text: &str) -> Self {
        Self(vec![TextPart::Text(text.to_string())])
    }
}

impl From<String> for TooltipText {
    fn from(text: String) -> Self {
        Self(vec![TextPart::Text(text)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A template is read at each call, so a rewrite between two reads shows; a missing key
    /// leaves its stand-in, a copied string keeps its `%` signs, and an ordinal other than 1 takes
    /// the `_P1` twin.
    #[test]
    fn a_prepared_text_reads_its_templates_at_each_render() {
        let cost = TooltipText::keyed("MANA_COST", vec![KeyedArg::D(30)]);
        assert_eq!(
            cost.text(&|k| (k == "MANA_COST").then(|| "%d Mana".into())),
            "30 Mana"
        );
        assert_eq!(
            cost.text(&|k| (k == "MANA_COST").then(|| "Mana: %d".into())),
            "Mana: 30"
        );
        assert_eq!(cost.text(&|_| None), "");
        let use_all = TooltipText::copied("SPELL_USE_ALL_MANA");
        assert_eq!(
            use_all.text(&|_| Some("Uses 100% mana".into())),
            "Uses 100% mana"
        );
        let reagents = TooltipText::copied("SPELL_REAGENTS").then("Rune");
        assert_eq!(reagents.text(&|_| None), "Rune");
        let secs = TooltipText(vec![TextPart::Keyed {
            key: "INT_SPELL_DURATION_SEC".into(),
            ordinal: Some(30),
            args: Some(vec![KeyedArg::D(30)]),
            or: "$d".into(),
        }]);
        let get = |k: &str| match k {
            "INT_SPELL_DURATION_SEC" => Some("%d sec".to_string()),
            "INT_SPELL_DURATION_SEC_P1" => Some("%d secs".to_string()),
            _ => None,
        };
        assert_eq!(secs.text(&get), "30 secs");
        assert_eq!(secs.text(&|_| None), "$d");
    }
}
