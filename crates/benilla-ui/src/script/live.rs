//! The live-read seam: a getter asks the lent host a question of its own family's type and reads
//! the answer as of the place it runs, as the reference's getters read the live object and caches
//! at the call (`UnitHealth`, `0x5174d0`). A family adds its question here as a type
//! ([`LiveQuery`]) and its answer on the host's half, each in a file of its own; the host's trait
//! carries one hook for every family ([`super::CallHost::read_live`]).

use std::any::Any;

use mlua::Lua;

/// A question a getter asks the lent host, answered as of the place it runs.
pub trait LiveQuery: Any {
    /// What the host hands back.
    type Answer: 'static;
}

/// One question on its way to the host, with the slot its answer goes in.
pub struct LiveAsk<'a> {
    query: &'a dyn Any,
    answer: &'a mut dyn Any,
}

impl LiveAsk<'_> {
    /// Answer with `f` when the question is a `Q`, `None` declining it, so the getter reads its
    /// snapshot; whether it was a `Q`.
    pub fn answer<Q: LiveQuery>(&mut self, f: impl FnOnce(&Q) -> Option<Q::Answer>) -> bool {
        let Some(query) = self.query.downcast_ref::<Q>() else {
            return false;
        };
        if let Some(slot) = self.answer.downcast_mut::<Option<Q::Answer>>() {
            *slot = f(query);
        }
        true
    }
}

/// `query`'s answer through the lent host, as of the place the getter runs, or through the reads
/// a call's apply lends while it runs the VM's own steps ([`super::UiScript::reading`]): `None`
/// with neither to ask (a glue VM, or a call's apply that lends none), from a host no family of
/// which answers it, and when its family declines, where the getter reads its snapshot. The
/// answer runs no Lua.
pub fn ask<Q: LiveQuery>(lua: &Lua, query: &Q) -> Option<Q::Answer> {
    let mut answer: Option<Q::Answer> = None;
    let asked = super::host::read_host(lua, |h| {
        h.read_live(&mut LiveAsk {
            query,
            answer: &mut answer,
        })
    });
    // A host's apply calling into the VM with the host out lends its own reads instead
    // ([`super::UiScript::reading`]).
    if asked.is_none() {
        super::host::read_reader(lua, |r| {
            r.read_live(&mut LiveAsk {
                query,
                answer: &mut answer,
            })
        });
    }
    answer
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::{CallHost, CallTail, ScriptCall, UiScript};

    struct Doubled(u32);
    impl LiveQuery for Doubled {
        type Answer = u32;
    }

    struct Unasked;
    impl LiveQuery for Unasked {
        type Answer = ();
    }

    /// Answers [`Doubled`] off its own state, as of each ask.
    struct Host(u32);

    impl CallHost for Host {
        fn apply(&mut self, _: &mut UiScript, _: ScriptCall) -> Option<CallTail> {
            None
        }

        fn read_live(&mut self, ask: &mut LiveAsk<'_>) -> bool {
            let base = self.0;
            ask.answer(|q: &Doubled| (q.0 > 0).then_some(q.0 * 2 + base))
        }
    }

    #[test]
    fn a_question_reads_the_lent_host() {
        let mut s = UiScript::new().unwrap();
        let lua = s.lua().clone();
        assert_eq!(ask(&lua, &Doubled(3)), None, "no host lent");
        let mut host = Host(1);
        s.hosted(&mut host, |_| {
            assert_eq!(ask(&lua, &Doubled(3)), Some(7));
            assert_eq!(ask(&lua, &Doubled(0)), None, "declined");
            assert_eq!(ask(&lua, &Unasked), None, "no family answers it");
        });
    }
}
