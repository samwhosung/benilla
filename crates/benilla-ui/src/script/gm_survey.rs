//! The GM survey globals of the reference (`0x48cc00`..`0x48cf85`). The survey opens on a ticket
//! status the 1.12 servers never send, so its window never shows; an addon can still call these.
//!
//! The questions come from three tables the app pushes ([`GmSurveyTables`]): the language's survey
//! id (`0x496440`), the survey's ten question ids (`0x496470`) and each question's text
//! (`0x4964b0`). The answers live in a local buffer (`0xb4b45c` ratings, `0xb4e380` comments,
//! `0xb4e3a8` the overall comment) that `GMSurveyAnswerSubmit` and `GMSurveyCommentSubmit` fill
//! (`0x496570`, `0x4965e0`) and `GMSurveySubmit` sends (`0x32a`, `0x496630`) and clears
//! (`0x496510`).

use mlua::{Lua, Value};

use super::binding_abi::{number_arg, string_arg};
use super::lottery::{queue, UnservedSend};
use super::Model;

/// The survey tables as the client joins them, pushed by the app; empty without the DBCs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GmSurveyTables {
    /// `GMSurveyCurrentSurvey`'s survey id for the language (`0x496440`), 0 for none.
    pub id: u32,
    /// The survey row's question ids by slot (`0x496470`), 0 for none.
    pub questions: [u32; 10],
    /// `(question id, text)` (`0x4964b0`).
    pub texts: Vec<(u32, String)>,
}

/// One answered question as `GMSurveySubmit` sends it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SurveyAnswerSend {
    pub question: u32,
    pub rating: u8,
    pub comment: String,
}

/// The survey buffer: the tables and what has been answered since the last submit.
#[derive(Debug, Default)]
pub(crate) struct SurveyState {
    tables: GmSurveyTables,
    ratings: [u8; 10],
    comments: [Option<String>; 10],
    /// The overall comment (`0xb4e3a8`).
    comment: Option<String>,
}

impl SurveyState {
    /// `0x496510`: every answer and the overall comment.
    fn clear(&mut self) {
        self.ratings = [0; 10];
        self.comments = Default::default();
        self.comment = None;
    }
}

impl super::UiScript {
    /// Push the survey tables, once the app has read them.
    pub fn set_gm_survey_tables(&mut self, tables: GmSurveyTables) {
        self.model_mut().gm_survey.tables = tables;
    }
}

/// A stored comment: a null or empty string stores nothing (`0x4965b4`, `0x49660c`).
fn kept(comment: String) -> Option<String> {
    Some(comment).filter(|c| !c.is_empty())
}

/// Register the survey globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GMSurveyQuestion(index) (`0x48cc00`): `is-number`, then for 1 to 10 the text of the question
    // in slot `index - 1` (`0x4964b0`): an id above 0 whose row exists, else nil. The usage text
    // carries the exe's own `Get`.
    g.set(
        "GMSurveyQuestion",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: GMSurveyGetQuestion(index)")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let tables = &model.gm_survey.tables;
            let text = usize::try_from(index - 1)
                .ok()
                .and_then(|slot| tables.questions.get(slot))
                .filter(|&&id| id > 0)
                .and_then(|id| tables.texts.iter().find(|t| t.0 == *id))
                .map(|t| lua.create_string(&t.1))
                .transpose()?;
            Ok(text.map_or(Value::Nil, Value::String))
        })?,
    )?;

    // GMSurveyAnswerSubmit(question, rank, comment) (`0x48cd30`): the first two `is-number`, the
    // comment a string or number, else the usage text; the question 1 to 10, then the rank's low
    // byte 0 to 5 (`cmp bl,5; jbe`). The rating byte and the comment go to the slot (`0x496570`),
    // whether or not the survey names a question there.
    g.set(
        "GMSurveyAnswerSubmit",
        lua.create_function(|lua, (question, rank, comment): (Value, Value, Value)| {
            let usage = "Usage: GMSurveyAnswerSubmit(question, rank, comment)";
            let question = number_arg(lua, question, usage)?;
            let rank = number_arg(lua, rank, usage)?;
            let comment = string_arg(lua, comment, usage)?;
            if !(1..=10).contains(&question) {
                return Err(mlua::Error::runtime("Question limited from 1 to 10"));
            }
            if (rank as u8) > 5 {
                return Err(mlua::Error::runtime("Ranks limited from 0 to 5"));
            }
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let slot = (question - 1) as usize;
            model.gm_survey.ratings[slot] = rank as u8;
            model.gm_survey.comments[slot] = kept(comment);
            Ok(())
        })?,
    )?;

    // GMSurveyCommentSubmit(comment) (`0x48cef0`): a string or number, else the usage text; the
    // overall comment replaces the last (`0x4965e0`).
    g.set(
        "GMSurveyCommentSubmit",
        lua.create_function(|lua, comment: Value| {
            let comment = string_arg(lua, comment, "Usage: GMSurveyCommentSubmit(comment)")?;
            lua.app_data_mut::<Model>()
                .expect("model app_data")
                .gm_survey
                .comment = kept(comment);
            Ok(())
        })?,
    )?;

    // GMSurveySubmit() (`0x48cf80` over `0x496630`): sends `0x32a` with the survey id, each slot's
    // question id, rating and comment until a slot names no question (`0x49668b`), and the
    // overall comment; then clears the buffer (`0x496510`).
    g.set(
        "GMSurveySubmit",
        lua.create_function(|lua, ()| {
            let send = {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                let s = &mut model.gm_survey;
                let answers = s
                    .tables
                    .questions
                    .iter()
                    .take_while(|&&q| q != 0)
                    .enumerate()
                    .map(|(slot, &question)| SurveyAnswerSend {
                        question,
                        rating: s.ratings[slot],
                        comment: s.comments[slot].clone().unwrap_or_default(),
                    })
                    .collect();
                let send = UnservedSend::GmSurveySubmit {
                    survey: s.tables.id,
                    answers,
                    comment: s.comment.clone().unwrap_or_default(),
                };
                s.clear();
                send
            };
            queue(lua, send);
            Ok(())
        })?,
    )?;

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

    /// The shipped survey: id 6, questions 28 to 32 in the first five slots.
    fn shipped() -> crate::script::GmSurveyTables {
        crate::script::GmSurveyTables {
            id: 6,
            questions: [28, 29, 30, 31, 32, 0, 0, 0, 0, 0],
            texts: vec![
                (28, "Did the GM help?".into()),
                (29, "Easy to follow?".into()),
                (30, "Respectful?".into()),
                (31, "Response time?".into()),
                (32, "Overall?".into()),
                (33, "Recommend WoW?".into()),
            ],
        }
    }

    /// `0x4964b0`: a slot with a question id above 0 whose row exists answers its text; slots
    /// past the survey's five, an id with no row and indexes outside 1 to 10 answer nil.
    #[test]
    fn the_survey_question_is_the_text_of_the_slots_question() {
        let mut s = UiScript::new().unwrap();
        s.set_gm_survey_tables(shipped());
        for (i, want) in [
            (1, "Did the GM help?"),
            (5, "Overall?"),
            (2, "Easy to follow?"),
        ] {
            assert_eq!(
                s.eval::<String>(&format!("return GMSurveyQuestion({i})"))
                    .unwrap(),
                want
            );
        }
        // The sixth question row is not in any slot.
        for i in ["6", "10", "11", "0", "-1"] {
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
        // A slot naming a question the table lacks.
        let mut t = shipped();
        t.questions[0] = 99;
        s.set_gm_survey_tables(t);
        assert!(s.eval::<bool>("return GMSurveyQuestion(1) == nil").unwrap());
    }

    /// The answers wait in the buffer, `GMSurveySubmit` sends one per slot with a question and
    /// the zero slot ends the list, and the buffer is empty afterwards.
    #[test]
    fn a_survey_submit_sends_the_buffer_once_and_clears_it() {
        use crate::script::SurveyAnswerSend as A;
        use crate::script::UnservedSend::GmSurveySubmit;
        let answer = |question, rating, comment: &str| A {
            question,
            rating,
            comment: comment.into(),
        };
        let mut s = UiScript::new().unwrap();
        s.set_gm_survey_tables(shipped());
        s.run("GMSurveyAnswerSubmit(1, 5, 'great') GMSurveyAnswerSubmit(3, 2, '') GMSurveyAnswerSubmit(7, 4, 'no slot')")
            .unwrap();
        s.run("GMSurveyCommentSubmit('overall')").unwrap();
        assert!(
            s.take_unserved_sends().is_empty(),
            "answering sends nothing"
        );
        s.run("GMSurveySubmit()").unwrap();
        assert_eq!(
            s.take_unserved_sends(),
            [GmSurveySubmit {
                survey: 6,
                answers: vec![
                    answer(28, 5, "great"),
                    answer(29, 0, ""),
                    answer(30, 2, ""),
                    answer(31, 0, ""),
                    answer(32, 0, ""),
                ],
                comment: "overall".into(),
            }]
        );
        // Cleared: the next submit carries the questions with no answers, and no comment.
        s.run("GMSurveySubmit()").unwrap();
        let sent = s.take_unserved_sends();
        let [GmSurveySubmit {
            answers, comment, ..
        }] = sent.as_slice()
        else {
            panic!("{sent:?}");
        };
        assert!(answers
            .iter()
            .all(|a| a.rating == 0 && a.comment.is_empty()));
        assert_eq!(comment, "");
        // A later answer replaces the first; an empty comment clears a stored one.
        s.run("GMSurveyAnswerSubmit(1, 1, 'a') GMSurveyAnswerSubmit(1, 3, '')")
            .unwrap();
        s.run("GMSurveyCommentSubmit('x') GMSurveyCommentSubmit('') GMSurveySubmit()")
            .unwrap();
        let sent = s.take_unserved_sends();
        let [GmSurveySubmit {
            answers, comment, ..
        }] = sent.as_slice()
        else {
            panic!("{sent:?}");
        };
        assert_eq!((answers[0].rating, answers[0].comment.as_str()), (3, ""));
        assert_eq!(comment, "");
        // Without the tables the survey is id 0 with no questions.
        let mut s = UiScript::new().unwrap();
        s.run("GMSurveySubmit()").unwrap();
        assert_eq!(
            s.take_unserved_sends(),
            [GmSurveySubmit {
                survey: 0,
                answers: vec![],
                comment: String::new()
            }]
        );
    }
}
