//! The GM survey globals of the reference (`0x48cc00`..`0x48cf85`). The survey opens on a ticket status the 1.12 servers never send, so its
//! window never shows; an addon can still call these.
//!
//! The client takes the questions from `GMSurveyCurrentSurvey.dbc`, `GMSurveySurveys.dbc` and
//! `GMSurveyQuestions.dbc`, keeps the answers in a local buffer (`0xb4b45c`, `0xb4e380`,
//! `0xb4e3a8`) and `GMSurveySubmit` sends them (`0x32a`) and clears it. Benilla loads none of the
//! three tables and has no buffer to read back, so the questions answer as with the tables empty.
//!
//! Deviation: `GMSurveyAnswerSubmit`, `GMSurveyCommentSubmit` and `GMSurveySubmit` check their
//! arguments as the reference does and then do nothing: the survey wire is not built, and a
//! submission with no survey id would file an empty survey on the server.

use mlua::{Lua, Value};

use super::binding_abi::{number_arg, string_arg};

/// Register the survey globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GMSurveyQuestion(index) (`0x48cc00`): `is-number`, then the index's question text (`0x4964b0`)
    // for 1 to 10; with no survey tables, nil. The usage text carries the exe's own `Get`.
    g.set(
        "GMSurveyQuestion",
        lua.create_function(|lua, index: Value| {
            number_arg(lua, index, "Usage: GMSurveyGetQuestion(index)")?;
            Ok(Value::Nil)
        })?,
    )?;

    // GMSurveyAnswerSubmit(question, rank, comment) (`0x48cd30`): the first two `is-number`, the
    // comment a string or number, else the usage text; the question 1 to 10, then the rank's low
    // byte 0 to 5 (`cmp bl,5; jbe`).
    g.set(
        "GMSurveyAnswerSubmit",
        lua.create_function(|lua, (question, rank, comment): (Value, Value, Value)| {
            let usage = "Usage: GMSurveyAnswerSubmit(question, rank, comment)";
            let question = number_arg(lua, question, usage)?;
            let rank = number_arg(lua, rank, usage)?;
            string_arg(lua, comment, usage)?;
            if !(1..=10).contains(&question) {
                return Err(mlua::Error::runtime("Question limited from 1 to 10"));
            }
            if (rank as u8) > 5 {
                return Err(mlua::Error::runtime("Ranks limited from 0 to 5"));
            }
            Ok(())
        })?,
    )?;

    // GMSurveyCommentSubmit(comment) (`0x48cef0`): a string or number, else the usage text.
    g.set(
        "GMSurveyCommentSubmit",
        lua.create_function(|lua, comment: Value| {
            string_arg(lua, comment, "Usage: GMSurveyCommentSubmit(comment)")?;
            Ok(())
        })?,
    )?;

    // GMSurveySubmit() (`0x48cf80`): the send (module doc) and the buffer's clear.
    g.set("GMSurveySubmit", lua.create_function(|_, ()| Ok(()))?)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::UiScript;

    fn raises(s: &UiScript, call: &str, text: &str) {
        let err = s.run(call).expect_err(call).to_string();
        assert!(err.contains(text), "{call}: {err}");
    }

    #[test]
    fn the_survey_question_is_nil_without_the_survey_tables() {
        let s = UiScript::new().unwrap();
        for i in ["1", "10", "11", "0", "-1", "'3'"] {
            assert_eq!(
                s.arity(&format!("GMSurveyQuestion({i})")).unwrap(),
                1,
                "{i}"
            );
            assert!(
                s.eval::<bool>(&format!("return GMSurveyQuestion({i}) == nil"))
                    .unwrap(),
                "{i}"
            );
        }
        for call in [
            "GMSurveyQuestion()",
            "GMSurveyQuestion(nil)",
            "GMSurveyQuestion('x')",
        ] {
            raises(&s, call, "Usage: GMSurveyGetQuestion(index)");
        }
    }

    /// The answer's checks run in the reference's order: the argument types, the question's
    /// range, then the rank's low byte.
    #[test]
    fn a_survey_answer_checks_types_then_the_question_then_the_rank() {
        let s = UiScript::new().unwrap();
        let usage = "Usage: GMSurveyAnswerSubmit(question, rank, comment)";
        for call in [
            "GMSurveyAnswerSubmit()",
            "GMSurveyAnswerSubmit(1, 2)",
            "GMSurveyAnswerSubmit('x', 2, 'c')",
            "GMSurveyAnswerSubmit(1, nil, 'c')",
            "GMSurveyAnswerSubmit(1, 2, nil)",
            "GMSurveyAnswerSubmit(1, 2, {})",
        ] {
            raises(&s, call, usage);
        }
        for call in [
            "GMSurveyAnswerSubmit(0, 3, 'c')",
            "GMSurveyAnswerSubmit(11, 3, 'c')",
            "GMSurveyAnswerSubmit(-1, 99, 'c')",
        ] {
            raises(&s, call, "Question limited from 1 to 10");
        }
        for call in [
            "GMSurveyAnswerSubmit(1, 6, 'c')",
            "GMSurveyAnswerSubmit(10, -1, 'c')",
            "GMSurveyAnswerSubmit(5, 255, 'c')",
        ] {
            raises(&s, call, "Ranks limited from 0 to 5");
        }
        // The rank is a byte: 256 is 0, in range. A number is a comment too.
        for call in [
            "GMSurveyAnswerSubmit(1, 0, 'c')",
            "GMSurveyAnswerSubmit(10, 5, '')",
            "GMSurveyAnswerSubmit(1, 256, 7)",
            "GMSurveyAnswerSubmit('2', '3', 'c')",
        ] {
            assert_eq!(s.arity(call).unwrap(), 0, "{call}");
        }
    }

    #[test]
    fn a_survey_comment_wants_a_string_or_a_number() {
        let s = UiScript::new().unwrap();
        for call in [
            "GMSurveyCommentSubmit()",
            "GMSurveyCommentSubmit(nil)",
            "GMSurveyCommentSubmit({})",
        ] {
            raises(&s, call, "Usage: GMSurveyCommentSubmit(comment)");
        }
        for call in [
            "GMSurveyCommentSubmit('good')",
            "GMSurveyCommentSubmit(5)",
            "GMSurveySubmit()",
        ] {
            assert_eq!(s.arity(call).unwrap(), 0, "{call}");
        }
    }
}
