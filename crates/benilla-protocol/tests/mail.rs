//! The mail wire: the send verbs, the inbox page with its three sender shapes, the send result's
//! tails, the letter-body fetch, and the arrival notices.

use benilla_protocol::events::{decode, SessionEvent};
use benilla_protocol::messages::{
    self, mail_action, mail_error, mail_message_type, MailAttachment,
};
use benilla_protocol::ServerPacket;

fn hx(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

const MAILBOX: u64 = 0x00F1_1000_0000_0042;

#[test]
fn mail_send_bodies_golden() {
    // CMSG_GET_MAIL_LIST: one full mailbox guid.
    assert_eq!(
        messages::get_mail_list(MAILBOX),
        hx("420000000010f100"),
        "CMSG_GET_MAIL_LIST body"
    );

    assert_eq!(
        messages::send_mail(
            MAILBOX,
            "Bob",
            "Hello",
            "Here is your item.",
            41,
            0,
            0x0001_0000_0000_0099,
            12_345,
            500,
        ),
        hx(concat!(
            "420000000010f100",                       // mailbox
            "426f6200",                               // "Bob\0"
            "48656c6c6f00",                           // "Hello\0"
            "4865726520697320796f7572206974656d2e00", // "Here is your item.\0"
            "29000000",                               // stationery 41
            "00000000",                               // package 0
            "9900000000000100",                       // item guid
            "39300000",                               // money 12345
            "f4010000",                               // cod 500
            "0000000000000000",                       // the 9-byte zero tail: u64 0
            "00",                                     // + u8 0
        )),
        "CMSG_SEND_MAIL body"
    );

    // The five `u64 mailbox, u32 mailId` CMSGs share one shape.
    let golden = hx("420000000010f1004d000000"); // mailbox + mailId 77
    assert_eq!(messages::mail_take_money(MAILBOX, 77), golden);
    assert_eq!(messages::mail_take_item(MAILBOX, 77), golden);
    assert_eq!(messages::mail_mark_as_read(MAILBOX, 77), golden);
    assert_eq!(messages::mail_return_to_sender(MAILBOX, 77), golden);
    assert_eq!(messages::mail_delete(MAILBOX, 77), golden);

    // CMSG_MAIL_CREATE_TEXT_ITEM: the shared shape, then the mail's `u32` template id
    // (`0x4afb4a`; vmangos `MailCreateTextItem::ReadFromWorldPacket`).
    assert_eq!(
        messages::mail_create_text_item(MAILBOX, 77, 0),
        hx("420000000010f1004d00000000000000"),
        "CMSG_MAIL_CREATE_TEXT_ITEM body"
    );
    assert_eq!(
        messages::mail_create_text_item(MAILBOX, 77, 87),
        hx("420000000010f1004d00000057000000"),
        "CMSG_MAIL_CREATE_TEXT_ITEM body, a template letter"
    );

    // CMSG_ITEM_TEXT_QUERY: u32 textId, u32 mailId, u32 unk(0).
    assert_eq!(
        messages::item_text_query(1234, 77),
        hx("d20400004d00000000000000"),
        "CMSG_ITEM_TEXT_QUERY body"
    );
}

/// Append the item block every `SMSG_MAIL_LIST_RESULT` row carries, zeroed without an attachment:
/// four `u32`, a `u8` stack count, then three `u32`.
fn push_item_block(
    body: &mut Vec<u8>,
    entry: u32,
    perm_enchant: u32,
    random_prop_id: u32,
    suffix_factor: u32,
    count: u8,
    charges: u32,
    durability_max: u32,
    durability: u32,
) {
    body.extend_from_slice(&entry.to_le_bytes());
    body.extend_from_slice(&perm_enchant.to_le_bytes());
    body.extend_from_slice(&random_prop_id.to_le_bytes());
    body.extend_from_slice(&suffix_factor.to_le_bytes());
    body.push(count);
    body.extend_from_slice(&charges.to_le_bytes());
    body.extend_from_slice(&durability_max.to_le_bytes());
    body.extend_from_slice(&durability.to_le_bytes());
}

#[test]
fn mail_list_result_wire() {
    // SMSG_MAIL_LIST_RESULT: `u8` count, then rows whose sender field depends on the message type
    // and whose item block is always present.
    let mut body = vec![3u8]; // count

    // MAIL_NORMAL: the sender is a player guid.
    body.extend_from_slice(&1u32.to_le_bytes()); // messageId
    body.push(mail_message_type::NORMAL);
    body.extend_from_slice(&0x0000_0001_0000_00AAu64.to_le_bytes()); // sender guid
    body.extend_from_slice(b"Old Friend\0");
    body.extend_from_slice(&555u32.to_le_bytes()); // itemTextId
    body.extend_from_slice(&0u32.to_le_bytes()); // package (dropped)
    body.extend_from_slice(&1u32.to_le_bytes()); // stationery
    push_item_block(&mut body, 6948, 0, 0, 0, 3, 0, 100, 80);
    body.extend_from_slice(&10_000u32.to_le_bytes()); // money
    body.extend_from_slice(&2_500u32.to_le_bytes()); // COD
    body.extend_from_slice(&0x9u32.to_le_bytes()); // checked: READ|COD_PAYMENT
    body.extend_from_slice(&2.5f32.to_le_bytes()); // expire_days
    body.extend_from_slice(&0u32.to_le_bytes()); // mailTemplateId

    // MAIL_AUCTION: the sender is a `u32` id; no item, so an all-zero item block.
    body.extend_from_slice(&2u32.to_le_bytes());
    body.push(mail_message_type::AUCTION);
    body.extend_from_slice(&42u32.to_le_bytes()); // sender id (auction id)
    body.extend_from_slice(b"Auction won\0");
    body.extend_from_slice(&0u32.to_le_bytes()); // itemTextId
    body.extend_from_slice(&0u32.to_le_bytes()); // package
    body.extend_from_slice(&0u32.to_le_bytes()); // stationery
    push_item_block(&mut body, 0, 0, 0, 0, 0, 0, 0, 0);
    body.extend_from_slice(&500u32.to_le_bytes()); // money
    body.extend_from_slice(&0u32.to_le_bytes()); // COD
    body.extend_from_slice(&0x1u32.to_le_bytes()); // checked: READ
    body.extend_from_slice(&1.0f32.to_le_bytes()); // expire_days
    body.extend_from_slice(&0u32.to_le_bytes());

    // MAIL_ITEM: no sender bytes at all.
    body.extend_from_slice(&3u32.to_le_bytes());
    body.push(mail_message_type::ITEM);
    body.extend_from_slice(b"Welcome\0");
    body.extend_from_slice(&0u32.to_le_bytes()); // itemTextId
    body.extend_from_slice(&0u32.to_le_bytes()); // package
    body.extend_from_slice(&41u32.to_le_bytes()); // stationery
    push_item_block(&mut body, 0, 0, 0, 0, 0, 0, 0, 0);
    body.extend_from_slice(&0u32.to_le_bytes()); // money
    body.extend_from_slice(&0u32.to_le_bytes()); // COD
    body.extend_from_slice(&0u32.to_le_bytes()); // checked
    body.extend_from_slice(&30.0f32.to_le_bytes()); // expire_days
    body.extend_from_slice(&8383u32.to_le_bytes()); // mailTemplateId

    let packet = messages::parse_server(messages::opcode::SMSG_MAIL_LIST_RESULT, &body).unwrap();
    match &packet {
        ServerPacket::MailList { mails } => {
            assert_eq!(mails.len(), 3);

            let m = &mails[0];
            assert_eq!(m.message_id, 1);
            assert_eq!(m.message_type, mail_message_type::NORMAL);
            assert_eq!(m.sender_guid, Some(0x0000_0001_0000_00AA));
            assert_eq!(m.sender_id, None);
            assert_eq!(m.subject, "Old Friend");
            assert_eq!(m.item_text_id, 555);
            assert_eq!(m.stationery, 1);
            assert_eq!(
                m.item,
                Some(MailAttachment {
                    entry: 6948,
                    perm_enchant: 0,
                    random_prop_id: 0,
                    suffix_factor: 0,
                    count: 3,
                    charges: 0,
                    durability_max: 100,
                    durability: 80,
                })
            );
            assert_eq!(m.money, 10_000);
            assert_eq!(m.cod, 2_500);
            assert_eq!(m.checked, 0x9);
            assert_eq!(m.expire_days, 2.5);
            assert_eq!(m.mail_template_id, 0);

            let m = &mails[1];
            assert_eq!(m.message_id, 2);
            assert_eq!(m.message_type, mail_message_type::AUCTION);
            assert_eq!(m.sender_guid, None);
            assert_eq!(m.sender_id, Some(42));
            assert_eq!(m.subject, "Auction won");
            assert_eq!(m.item, None, "all-zero item block folds to None");
            assert_eq!(m.money, 500);
            assert_eq!(m.checked, 0x1);
            assert_eq!(m.expire_days, 1.0);

            let m = &mails[2];
            assert_eq!(m.message_id, 3);
            assert_eq!(m.message_type, mail_message_type::ITEM);
            assert_eq!(m.sender_guid, None);
            assert_eq!(m.sender_id, None, "MAIL_ITEM reads no sender bytes at all");
            assert_eq!(m.subject, "Welcome");
            assert_eq!(m.item, None);
            assert_eq!(m.mail_template_id, 8383);
        }
        other => panic!("mail list, got {}", other.name()),
    }

    match decode(packet).pop().unwrap() {
        SessionEvent::MailList { mails } => assert_eq!(mails.len(), 3),
        other => panic!("mail list event, got {other:?}"),
    }
}

#[test]
fn send_mail_result_wire() {
    // Action SEND, error OK: no tail.
    let mut plain = 1u32.to_le_bytes().to_vec();
    plain.extend_from_slice(&mail_action::SEND.to_le_bytes());
    plain.extend_from_slice(&mail_error::OK.to_le_bytes());
    match messages::parse_server(messages::opcode::SMSG_SEND_MAIL_RESULT, &plain).unwrap() {
        ServerPacket::SendMailResult {
            mail_id,
            action,
            error,
            equip_error,
            item,
        } => {
            assert_eq!(
                (mail_id, action, error),
                (1, mail_action::SEND, mail_error::OK)
            );
            assert_eq!(equip_error, None);
            assert_eq!(item, None);
        }
        other => panic!("send mail result (plain), got {}", other.name()),
    }
    let packet = messages::parse_server(messages::opcode::SMSG_SEND_MAIL_RESULT, &plain).unwrap();
    match decode(packet).pop().unwrap() {
        SessionEvent::SendMailResult {
            mail_id,
            action,
            error,
            equip_error,
            item,
        } => {
            assert_eq!(
                (mail_id, action, error),
                (1, mail_action::SEND, mail_error::OK)
            );
            assert_eq!((equip_error, item), (None, None));
        }
        other => panic!("send mail result event, got {other:?}"),
    }

    // Equip-error shape: error == MAIL_ERR_EQUIP_ERROR carries one trailing u32.
    let mut equip = 2u32.to_le_bytes().to_vec();
    equip.extend_from_slice(&mail_action::ITEM_TAKEN.to_le_bytes());
    equip.extend_from_slice(&mail_error::EQUIP_ERROR.to_le_bytes());
    equip.extend_from_slice(&1234u32.to_le_bytes()); // the equip-error code
    match messages::parse_server(messages::opcode::SMSG_SEND_MAIL_RESULT, &equip).unwrap() {
        ServerPacket::SendMailResult {
            mail_id,
            error,
            equip_error,
            item,
            ..
        } => {
            assert_eq!((mail_id, error), (2, mail_error::EQUIP_ERROR));
            assert_eq!(equip_error, Some(1234));
            assert_eq!(item, None);
        }
        other => panic!("send mail result (equip error), got {}", other.name()),
    }

    // Item-taken shape: action == MAIL_ITEM_TAKEN and error == OK carries entry + count.
    let mut taken = 3u32.to_le_bytes().to_vec();
    taken.extend_from_slice(&mail_action::ITEM_TAKEN.to_le_bytes());
    taken.extend_from_slice(&mail_error::OK.to_le_bytes());
    taken.extend_from_slice(&6948u32.to_le_bytes()); // item entry
    taken.extend_from_slice(&3u32.to_le_bytes()); // item count
    match messages::parse_server(messages::opcode::SMSG_SEND_MAIL_RESULT, &taken).unwrap() {
        ServerPacket::SendMailResult {
            mail_id,
            action,
            error,
            equip_error,
            item,
        } => {
            assert_eq!(
                (mail_id, action, error),
                (3, mail_action::ITEM_TAKEN, mail_error::OK)
            );
            assert_eq!(equip_error, None);
            assert_eq!(item, Some((6948, 3)));
        }
        other => panic!("send mail result (item taken), got {}", other.name()),
    }
}

#[test]
fn item_text_query_response_wire() {
    // SMSG_ITEM_TEXT_QUERY_RESPONSE: u32 textId, cstr text.
    let mut body = 1234u32.to_le_bytes().to_vec();
    body.extend_from_slice(b"This is the letter body.\0");
    match messages::parse_server(messages::opcode::SMSG_ITEM_TEXT_QUERY_RESPONSE, &body).unwrap() {
        ServerPacket::ItemTextQueryResponse { text_id, text } => {
            assert_eq!(text_id, 1234);
            assert_eq!(text, "This is the letter body.");
        }
        other => panic!("item text query response, got {}", other.name()),
    }
    let packet =
        messages::parse_server(messages::opcode::SMSG_ITEM_TEXT_QUERY_RESPONSE, &body).unwrap();
    match decode(packet).pop().unwrap() {
        SessionEvent::MailItemText { text_id, text } => {
            assert_eq!(text_id, 1234);
            assert_eq!(text, "This is the letter body.");
        }
        other => panic!("mail item text event, got {other:?}"),
    }
}

#[test]
fn received_mail_wire() {
    // SMSG_RECEIVED_MAIL: one `f32` delay, which the reference reads as a float; vmangos writes
    // `uint32(0)`, the same bits as 0.0.
    let vmangos_body = 0u32.to_le_bytes();
    match messages::parse_server(messages::opcode::SMSG_RECEIVED_MAIL, &vmangos_body).unwrap() {
        ServerPacket::ReceivedMail { seconds } => assert_eq!(seconds, 0.0),
        other => panic!("received mail, got {}", other.name()),
    }
    for seconds in [0.0f32, 120.0f32] {
        let body = seconds.to_le_bytes();
        let packet = messages::parse_server(messages::opcode::SMSG_RECEIVED_MAIL, &body).unwrap();
        match decode(packet).pop().unwrap() {
            SessionEvent::ReceivedMail { seconds: got } => assert_eq!(got, seconds),
            other => panic!("received mail event, got {other:?}"),
        }
    }
}

#[test]
fn query_next_mail_time_wire() {
    // MSG_QUERY_NEXT_MAIL_TIME's reply: one `f32`, 0.0 for unread mail waiting, -86400.0 for none.
    for seconds in [0.0f32, -86400.0f32] {
        let body = seconds.to_le_bytes();
        match messages::parse_server(messages::opcode::MSG_QUERY_NEXT_MAIL_TIME, &body).unwrap() {
            ServerPacket::NextMailTime { seconds: got } => assert_eq!(got, seconds),
            other => panic!("query next mail time, got {}", other.name()),
        }
    }
}
