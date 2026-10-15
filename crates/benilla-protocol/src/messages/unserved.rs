//! The send bodies of verbs for systems no 1.12 server runs: the lottery window (`0x4c4220`,
//! `0x4c4290`, `0x4c4400`) and the GM survey (`0x496630`). Each body is the payload after the
//! opcode, which `ClientServices::Send` (`0x5ab630`) takes from the data store's first dword.

/// `CMSG_LOTTERY_QUERY_OBSOLETE`: `GetLastLotteryNumbers`' data store holds the opcode (`0x334`,
/// pushed at `0x4c4229`) and nothing else.
pub fn lottery_query() -> Vec<u8> {
    Vec::new()
}

/// `CMSG_BUY_LOTTERY_TICKET_OBSOLETE`: two `PutUInt32` (`0x418190`) after the opcode. `BuyRandomPicks`
/// writes the count then 0 (`0x4c444e`, `0x4c4457`); `SubmitNumbers` writes 0 then the pick mask
/// (`0x4c4378`, `0x4c4385`).
pub fn buy_lottery_ticket(first: u32, second: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(8);
    body.extend_from_slice(&first.to_le_bytes());
    body.extend_from_slice(&second.to_le_bytes());
    body
}

/// One answered survey question: its id, the rating byte and the comment.
pub struct SurveyAnswer<'a> {
    pub question: u32,
    pub rating: u8,
    pub comment: &'a str,
}

/// `CMSG_GMSURVEY_SUBMIT` (`0x496630`): the survey id (`PutUInt32`, `0x49666a`), then per question
/// slot, up to ten, its id (`0x496686`); a zero id ends the loop (`0x49668b`), else the rating
/// byte (`PutInt8`, `0x49669b`) and the comment as a NUL-terminated string (`PutCString`,
/// `0x4966b6`, which writes the length plus one). The overall comment closes the body
/// (`0x4966d6`). The loop writes the zero id that ends it, so fewer than ten answers end in 0.
pub fn gm_survey_submit(survey: u32, answers: &[SurveyAnswer<'_>], comment: &str) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&survey.to_le_bytes());
    for slot in 0..10 {
        let question = answers.get(slot).map_or(0, |a| a.question);
        body.extend_from_slice(&question.to_le_bytes());
        if question == 0 {
            break;
        }
        let a = &answers[slot];
        body.push(a.rating);
        body.extend_from_slice(a.comment.as_bytes());
        body.push(0);
    }
    body.extend_from_slice(comment.as_bytes());
    body.push(0);
    body
}
