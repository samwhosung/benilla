//! The sends of the lottery and GM survey verbs ([`messages::lottery_query`] and kin).

use anyhow::Result;

use crate::messages::{self, opcode, SurveyAnswer};

use super::WorldWriter;

impl WorldWriter {
    /// `CMSG_LOTTERY_QUERY_OBSOLETE`, empty: `GetLastLotteryNumbers`.
    pub fn lottery_query(&mut self) -> Result<()> {
        self.send(
            opcode::CMSG_LOTTERY_QUERY_OBSOLETE,
            &messages::lottery_query(),
        )
    }

    /// `CMSG_BUY_LOTTERY_TICKET_OBSOLETE`: `BuyRandomPicks` or `SubmitNumbers`.
    pub fn buy_lottery_ticket(&mut self, first: u32, second: u32) -> Result<()> {
        self.send(
            opcode::CMSG_BUY_LOTTERY_TICKET_OBSOLETE,
            &messages::buy_lottery_ticket(first, second),
        )
    }

    /// `CMSG_GMSURVEY_SUBMIT`: `GMSurveySubmit`.
    pub fn gm_survey_submit(
        &mut self,
        survey: u32,
        answers: &[SurveyAnswer<'_>],
        comment: &str,
    ) -> Result<()> {
        self.send(
            opcode::CMSG_GMSURVEY_SUBMIT,
            &messages::gm_survey_submit(survey, answers, comment),
        )
    }
}
