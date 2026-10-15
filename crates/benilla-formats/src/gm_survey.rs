//! The GM survey's three tables, which the client joins to a questionnaire: `GMSurveyCurrentSurvey`
//! (a language to a survey id, `0x496440`), `GMSurveySurveys` (a survey id to its ten question
//! ids, `0x496470`) and `GMSurveyQuestions` (a question id to its text, `0x4964b0`). The language
//! is the row id: benilla reads enUS, language 0.

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};
use crate::Chain;

const CURRENT: &str = "DBFilesClient\\GMSurveyCurrentSurvey.dbc";
const SURVEYS: &str = "DBFilesClient\\GMSurveySurveys.dbc";
const QUESTIONS: &str = "DBFilesClient\\GMSurveyQuestions.dbc";

/// The survey the client offers in language 0: its id, its ten question slots (0 for none), and
/// the text of each question the slots name that `GMSurveyQuestions.dbc` holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GmSurvey {
    /// `GMSurveyCurrentSurvey`'s survey id; 0 when the language has no row (`0x496440`).
    pub id: u32,
    /// The survey row's question ids in slot order; all 0 when the survey has no row.
    pub questions: [u32; 10],
    /// `(question id, enUS text)` for every question row, in file order.
    pub texts: Vec<(u32, String)>,
}

fn schema(name: &str, strings: usize, ints: usize) -> Schema {
    let mut s = Schema::new(name);
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    for i in 0..ints {
        s.add_field(SchemaField::new(format!("Int{i}"), FieldType::UInt32));
    }
    for i in 0..strings {
        s.add_field(SchemaField::new(format!("Str{i}"), FieldType::String));
    }
    if strings > 0 {
        s.add_field(SchemaField::new("StrFlags", FieldType::UInt32));
    }
    s
}

/// Load the three tables from the patch chain.
pub fn load_gm_survey(chain: &mut Chain) -> Result<GmSurvey> {
    let current = chain
        .read_file(CURRENT)
        .with_context(|| format!("reading {CURRENT}"))?;
    let current = parse(
        &current,
        schema("GMSurveyCurrentSurvey", 0, 1),
        "GMSurveyCurrentSurvey",
    )?;
    // The row whose id is the language, 0.
    let id = current
        .records()
        .iter()
        .find(|r| u32_at(r, 0) == Some(0))
        .and_then(|r| u32_at(r, 1))
        .unwrap_or(0);

    let surveys = chain
        .read_file(SURVEYS)
        .with_context(|| format!("reading {SURVEYS}"))?;
    let surveys = parse(
        &surveys,
        schema("GMSurveySurveys", 0, 10),
        "GMSurveySurveys",
    )?;
    let mut questions = [0u32; 10];
    if let Some(row) = surveys.records().iter().find(|r| u32_at(r, 0) == Some(id)) {
        for (slot, q) in questions.iter_mut().enumerate() {
            *q = u32_at(row, 1 + slot).unwrap_or(0);
        }
    }

    let texts = chain
        .read_file(QUESTIONS)
        .with_context(|| format!("reading {QUESTIONS}"))?;
    let texts = parse(
        &texts,
        schema("GMSurveyQuestions", 8, 0),
        "GMSurveyQuestions",
    )?;
    let texts = texts
        .records()
        .iter()
        .filter_map(|r| Some((u32_at(r, 0)?, str_at(&texts, r, 1)?)))
        .collect();
    Ok(GmSurvey {
        id,
        questions,
        texts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 5875 install (patch.MPQ): every language offers survey 6, which asks questions 28 to 32.
    #[test]
    fn the_shipped_survey_is_six_with_five_questions() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let survey = load_gm_survey(&mut chain).expect("the survey tables");
        assert_eq!(survey.id, 6);
        assert_eq!(survey.questions, [28, 29, 30, 31, 32, 0, 0, 0, 0, 0]);
        let ids: Vec<u32> = survey.texts.iter().map(|t| t.0).collect();
        assert_eq!(ids, [28, 29, 30, 31, 32, 33]);
        assert!(survey.texts[0].1.starts_with("Did the GM provide"));
    }
}
