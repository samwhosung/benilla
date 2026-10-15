//! The sends of the lottery and GM survey verbs, byte for byte from the instructions that build
//! them: each data store holds the opcode (`PutUInt32`, `0x418190`) and then the payload below.

use benilla_protocol::messages::{self, opcode, SurveyAnswer};

fn hx(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

#[test]
fn the_opcodes_are_the_ones_the_verbs_push() {
    assert_eq!(
        opcode::CMSG_LOTTERY_QUERY_OBSOLETE,
        0x334,
        "push 0x334 at 0x4c4229"
    );
    assert_eq!(
        opcode::CMSG_BUY_LOTTERY_TICKET_OBSOLETE,
        0x336,
        "push 0x336 at 0x4c4365 and 0x4c4427"
    );
    assert_eq!(
        opcode::CMSG_GMSURVEY_SUBMIT,
        0x32a,
        "push 0x32a at 0x49663a"
    );
}

#[test]
fn lottery_bodies_golden() {
    // `GetLastLotteryNumbers`: the opcode alone (0x4c4229..0x4c4256).
    assert_eq!(messages::lottery_query(), Vec::<u8>::new());
    // `BuyRandomPicks(5)`: the count (0x4c444e), then 0 (0x4c4457).
    assert_eq!(messages::buy_lottery_ticket(5, 0), hx("0500000000000000"));
    // `SubmitNumbers(1, 2, 31, 4)`: 0 (0x4c4378), then the mask, bit n - 1 per pick (0x4c4385).
    let mask = 1u32 | 1 << 1 | 1 << 30 | 1 << 3;
    assert_eq!(mask, 0x4000_000b);
    assert_eq!(
        messages::buy_lottery_ticket(0, mask),
        hx("000000000b000040")
    );
}

#[test]
fn gm_survey_submit_body_golden() {
    // The tables empty: survey id 0 (0x49666a), the first question id 0 (0x496686) ends the loop
    // (0x49668b), then the overall comment, a lone NUL (0x4966d1 pushes the empty string).
    assert_eq!(
        messages::gm_survey_submit(0, &[], ""),
        hx(concat!("00000000", "00000000", "00"))
    );
    // The same with a comment: NUL-terminated (PutCString writes length + 1, 0x418453).
    assert_eq!(
        messages::gm_survey_submit(0, &[], "ok"),
        hx(concat!("00000000", "00000000", "6f6b00"))
    );
    // One answer, then the zero id that ends the loop.
    let one = [SurveyAnswer {
        question: 3,
        rating: 5,
        comment: "x",
    }];
    assert_eq!(
        messages::gm_survey_submit(7, &one, "c"),
        hx(concat!(
            "07000000", "03000000", "05", "7800", "00000000", "6300"
        ))
    );
    // Ten answers fill every slot: no terminator.
    let ten: Vec<SurveyAnswer<'_>> = (1..=10)
        .map(|question| SurveyAnswer {
            question,
            rating: 0,
            comment: "",
        })
        .collect();
    let body = messages::gm_survey_submit(1, &ten, "");
    assert_eq!(body.len(), 4 + 10 * (4 + 1 + 1) + 1);
    assert_eq!(
        &body[body.len() - 7..],
        hx(concat!("0a000000", "00", "00", "00")).as_slice()
    );

    // The shipped survey (6; questions 28..=32; two answered): the id, five (id, rating, comment)
    // triples, the zero id of the sixth slot (0x49668b), then the overall comment.
    let answers: Vec<SurveyAnswer<'_>> = [
        (28, 5, "a"),
        (29, 0, ""),
        (30, 0, ""),
        (31, 2, "b"),
        (32, 0, ""),
    ]
    .into_iter()
    .map(|(question, rating, comment)| SurveyAnswer {
        question,
        rating,
        comment,
    })
    .collect();
    assert_eq!(
        messages::gm_survey_submit(6, &answers, "z"),
        hx(concat!(
            "06000000", "1c000000", "05", "6100", "1d000000", "00", "00", "1e000000", "00", "00",
            "1f000000", "02", "6200", "20000000", "00", "00", "00000000", "7a00"
        ))
    );
}

/// `BuySkillTier`'s send: `0x21f` (pushed at `0x4d3fbc`) and the skill line id (`0x4d3fe0`).
#[test]
fn skill_buy_step_body_golden() {
    assert_eq!(opcode::CMSG_SKILL_BUY_STEP, 0x21f);
    assert_eq!(messages::skill_buy_step(493), hx("ed010000"));
}

/// `LFGQuery`'s send: `0x1ff` (pushed at `0x4e97f9`), the criterion word and the class.
#[test]
fn lfg_query_body_golden() {
    assert_eq!(opcode::MSG_LOOKING_FOR_GROUP, 0x1ff);
    assert_eq!(
        messages::looking_for_group_query(0, 0),
        hx("0000000000000000")
    );
    assert_eq!(
        messages::looking_for_group_query(0x0102_0304, 9),
        hx("0403020109000000")
    );
}
