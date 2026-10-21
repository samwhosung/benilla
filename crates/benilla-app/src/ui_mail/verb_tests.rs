//! The mail verbs inside their calls, through the whole client's call host: each packet goes out
//! before the verb returns, as the reference's bindings send in their own bodies, so a chunk's
//! packets leave in call order and its next line reads what the verb did.

use benilla_protocol::messages::{mail_message_type, MailAttachment, MailListEntry};
use benilla_protocol::SessionEvent;
use benilla_ui::script::{MailInboxRow, MailState, StationeryView, UiScript, UnitState};
use bevy::prelude::*;
use crossbeam_channel::Receiver;

use super::{MailOpen, MailPending, CHECKED_READ};
use crate::net::{ClientCommand, NetCommands};
use crate::script_calls::with_hosted_vm;

const MAILBOX: u64 = 0xF110_0000_0000_0B0C;
/// The sender of every player mail here.
const BOB: u64 = 0x42;

/// Commands as their debug text, `ClientCommand` having no `PartialEq`.
fn text(commands: impl IntoIterator<Item = ClientCommand>) -> Vec<String> {
    commands.into_iter().map(|c| format!("{c:?}")).collect()
}

/// A wire row: a player's letter from Bob, unread, with text id `text_id` and 29 days left.
fn entry(message_id: u32, text_id: u32) -> MailListEntry {
    MailListEntry {
        message_id,
        message_type: mail_message_type::NORMAL,
        sender_guid: Some(BOB),
        sender_id: None,
        subject: "s".into(),
        item_text_id: text_id,
        package: 0,
        stationery: 41,
        item: None,
        money: 0,
        cod: 0,
        checked: 0,
        expire_days: 29.0,
        mail_template_id: 0,
    }
}

fn linen() -> MailAttachment {
    MailAttachment {
        entry: 2589,
        perm_enchant: 0,
        random_prop_id: 0,
        suffix_factor: 0,
        count: 1,
        charges: 0,
        durability: 0,
        durability_max: 0,
    }
}

/// The VM's row for `e`, as the feed resolves it.
fn row_of(e: &MailListEntry) -> MailInboxRow {
    MailInboxRow {
        mail_id: e.message_id,
        item_text_id: e.item_text_id,
        stationery: e.stationery,
        message_type: e.message_type,
        mail_template_id: e.mail_template_id,
        money: e.money,
        cod: e.cod,
        days_left: e.expire_days,
        item_id: e.item.as_ref().map_or(0, |i| i.entry),
        item_count: e.item.as_ref().map_or(0, |i| u32::from(i.count)),
        was_read: e.checked & CHECKED_READ != 0,
        can_reply: true,
        subject: e.subject.clone(),
        ..Default::default()
    }
}

/// The whole client, headless, with a mailbox open on `mails` and the VM holding their rows,
/// recording every mail event its handlers hear.
struct Mailbox {
    app: App,
    rx: Receiver<ClientCommand>,
}

impl Mailbox {
    fn new(mails: Vec<MailListEntry>) -> Self {
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        let (tx, rx) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx));
        let world = app.world_mut();
        let rows = mails.iter().map(row_of).collect();
        {
            let mut mail = world.resource_mut::<MailOpen>();
            mail.click(MAILBOX);
            mail.mails = mails;
        }
        let mut script = UiScript::new().expect("a VM");
        script
            .run(
                r#"HEARD = {}
                local f = CreateFrame("Frame")
                f:RegisterEvent("MAIL_INBOX_UPDATE")
                f:SetScript("OnEvent", function() table.insert(HEARD, event) end)"#,
            )
            .expect("the recorder installs");
        script.set_unit("player", Some(UnitState::default()));
        script.set_money(1_000);
        script.set_mail(Some(MailState {
            inbox: rows,
            open: true,
        }));
        world.insert_non_send_resource(script);
        Self { app, rx }
    }

    /// Run `lua` with the host lent, as every entry that runs Lua lends it.
    fn run(&mut self, lua: &str) {
        with_hosted_vm(self.app.world_mut(), |s| s.expect("a VM").run(lua))
            .expect("the script runs");
    }

    fn eval<T: benilla_ui::mlua::FromLuaMulti>(&mut self, lua: &str) -> T {
        with_hosted_vm(self.app.world_mut(), |s| s.expect("a VM").eval::<T>(lua))
            .expect("the chunk runs")
    }

    /// One frame's mail feed, its memos kept from the last.
    fn feed(&mut self) {
        self.app
            .world_mut()
            .run_system_cached(super::feed_mail)
            .expect("the feed runs");
    }

    fn script(&mut self) -> Mut<'_, UiScript> {
        self.app.world_mut().non_send_resource_mut::<UiScript>()
    }

    /// What went on the wire, the name and item queries aside.
    fn sent(&self) -> Vec<String> {
        text(self.rx.try_iter().filter(|c| {
            !matches!(
                c,
                ClientCommand::ItemQuery { .. } | ClientCommand::NameQuery { .. }
            )
        }))
    }
}

/// Each mail verb sends inside its call, so a roll on the chunk's next line follows it on the wire
/// (`0x4aeb0b`, `0x4af85c`, `0x4af9d2`, `0x4afc4d`, `0x4afd79`, `0x4afb08`, `0x4aea6c`).
#[test]
fn each_mail_verb_is_on_the_wire_before_the_next_line() {
    let mut paid = entry(1, 0);
    (paid.money, paid.item) = (50, Some(linen()));
    let mut h = Mailbox::new(vec![paid, entry(2, 9)]);
    let roll = text([ClientCommand::RandomRoll { min: 1, max: 100 }]);
    let verb = |lua: &str, h: &mut Mailbox| {
        h.script().mail_request_resolved();
        h.run(&format!("{lua} RandomRoll(1, 100)"));
        let sent = h.sent();
        assert_eq!(sent.len(), 2, "{lua}: {sent:?}");
        assert_eq!(sent[1..], roll[..], "{lua}");
        sent[0].clone()
    };
    let take = |kind: &str, id: u32| format!("{kind} {{ mailbox: {MAILBOX}, mail_id: {id} }}");
    assert_eq!(
        verb("CheckInbox()", &mut h),
        format!("GetMailList {{ mailbox: {MAILBOX} }}")
    );
    assert_eq!(verb("TakeInboxMoney(1)", &mut h), take("MailTakeMoney", 1));
    assert_eq!(verb("TakeInboxItem(1)", &mut h), take("MailTakeItem", 1));
    assert_eq!(
        verb("ReturnInboxItem(1)", &mut h),
        take("MailReturnToSender", 1)
    );
    assert_eq!(
        verb("TakeInboxTextItem(2)", &mut h),
        format!("MailCreateTextItem {{ mailbox: {MAILBOX}, mail_id: 2, template_id: 0 }}")
    );
    assert_eq!(verb("DeleteInboxItem(2)", &mut h), take("MailDelete", 2));
    h.script().set_mail_stationeries(vec![StationeryView {
        id: 41,
        name: "Default Stationery".into(),
        icon: "Interface\\Icons\\INV_Letter_15".into(),
        cost: Some(0),
        texture: "STATIONERYTEST".into(),
    }]);
    let sent = verb(
        "SelectStationery(1) SendMail('Jaina', 'Hi', 'body')",
        &mut h,
    );
    assert!(sent.starts_with("SendMail {"), "{sent}");
}

/// `TakeInboxTextItem` sends the template id after the mail id (`0x4afb4a`), which a template
/// letter needs to be copied.
#[test]
fn a_template_letter_is_copied_with_its_template_id() {
    let mut template = entry(3, 0);
    template.mail_template_id = 87;
    let mut h = Mailbox::new(vec![template]);
    h.run("TakeInboxTextItem(1)");
    assert_eq!(
        h.sent(),
        [format!(
            "MailCreateTextItem {{ mailbox: {MAILBOX}, mail_id: 3, template_id: 87 }}"
        )]
    );
}

/// `GetInboxText` marks an unread mail inside its call (`0x4af18e` → `0x4adcd0`): the packet
/// (`0x4adcfe`), the read bit (`0x4add62`), days left above 3 lowered to 3 on a mail with no COD
/// (`0x4add72`-`0x4add8e`), `MAIL_INBOX_UPDATE` (`0x4add9d`) and the deferred refresh
/// (`0x4adda6`), then the body's ask (`0x4af1bf`). The script's next line reads all of it, and
/// the feed after it, finding the VM's rows as its own, signals nothing more.
#[test]
fn opening_an_unread_mail_marks_it_inside_the_call() {
    let mut h = Mailbox::new(vec![entry(1, 9)]);
    h.feed();
    let _ = h.sent();
    let (read, days, heard): (bool, f64, i64) = h.eval(
        "GetInboxText(1) local _, _, _, _, _, _, days, _, read = GetInboxHeaderInfo(1) \
         return read == 1, days, table.getn(HEARD)",
    );
    assert_eq!((read, days, heard), (true, 3.0, 1));
    assert_eq!(
        h.sent(),
        text([
            ClientCommand::MailMarkAsRead {
                mailbox: MAILBOX,
                mail_id: 1,
            },
            ClientCommand::ItemTextQuery {
                text_id: 9,
                mail_id: 1,
            },
        ])
    );
    let world = h.app.world_mut();
    let held = &world.resource::<MailOpen>().mails[0];
    assert_eq!((held.checked & CHECKED_READ, held.expire_days), (1, 3.0));
    assert!(world.resource_mut::<MailPending>().take_refresh());
    h.feed();
    assert_eq!(h.eval::<i64>("return table.getn(HEARD)"), 1);
}

/// A mail already read is not marked again (`0x4adcee`): no packet, no event and no refresh. Its
/// body is still asked for in the call.
#[test]
fn opening_a_read_mail_only_asks_for_its_body() {
    let mut read = entry(1, 9);
    read.checked = CHECKED_READ;
    let mut h = Mailbox::new(vec![read]);
    h.run("GetInboxText(1)");
    assert_eq!(
        h.sent(),
        text([ClientCommand::ItemTextQuery {
            text_id: 9,
            mail_id: 1,
        }])
    );
    assert_eq!(h.eval::<i64>("return table.getn(HEARD)"), 0);
    let world = h.app.world_mut();
    assert!(!world.resource_mut::<MailPending>().take_refresh());
    assert_eq!(world.resource::<MailOpen>().mails[0].expire_days, 29.0);
}

/// The list handler (`0x4ad1b0`) keeps a row whatever its days left (it never tests `+0x14c`,
/// read at `0x4ad414`), and answers an ignored player's mail at once (`0x4ad47f`): unless returned
/// or a COD payment (`0x4ad496`), `CMSG_MAIL_RETURN_TO_SENDER` when it holds an item or money
/// (`0x4ad4bf`, `0x4ad4c7`), else `CMSG_MAIL_DELETE` (`0x4ad4cf`), each with the mailbox cell as it
/// stands (`0x4ad4e3`), and drops the row (`0x4ad51d`).
#[test]
fn the_list_keeps_expired_mail_and_answers_an_ignored_sender() {
    const IGNORED: u64 = 0x99;
    let mut h = Mailbox::new(Vec::new());
    let from_ignored = |id: u32| MailListEntry {
        sender_guid: Some(IGNORED),
        ..entry(id, 0)
    };
    let mut expired = entry(1, 0);
    expired.expire_days = 0.0;
    let mut overdue = entry(2, 0);
    overdue.expire_days = -1.0;
    let mut with_item = from_ignored(3);
    with_item.item = Some(linen());
    let letter = from_ignored(4);
    let mut returned = from_ignored(5);
    (returned.money, returned.checked) = (20, 0x2);
    let mut paid = from_ignored(6);
    paid.money = 20;
    let world = h.app.world_mut();
    crate::net::handlers::dispatch(
        world,
        vec![
            SessionEvent::IgnoreList {
                guids: vec![IGNORED],
            },
            SessionEvent::MailList {
                mails: vec![expired, overdue, with_item, letter, returned, paid],
            },
        ],
    );
    let kept: Vec<u32> = world
        .resource::<MailOpen>()
        .mails
        .iter()
        .map(|e| e.message_id)
        .collect();
    assert_eq!(kept, [1, 2, 5]);
    assert_eq!(
        h.sent(),
        text([
            ClientCommand::MailReturnToSender {
                mailbox: MAILBOX,
                mail_id: 3,
            },
            ClientCommand::MailDelete {
                mailbox: MAILBOX,
                mail_id: 4,
            },
            ClientCommand::MailReturnToSender {
                mailbox: MAILBOX,
                mail_id: 6,
            },
        ])
    );

    // With the mailbox closed the answer still goes, its cell 0.
    let world = h.app.world_mut();
    world.resource_mut::<MailOpen>().clear();
    crate::net::handlers::dispatch(
        world,
        vec![SessionEvent::MailList {
            mails: vec![from_ignored(7)],
        }],
    );
    assert_eq!(
        h.sent(),
        text([ClientCommand::MailDelete {
            mailbox: 0,
            mail_id: 7,
        }])
    );
}

/// A profanity list masking the one word `darn`.
fn darn_filter() -> crate::text_filter::TextFilter {
    crate::text_filter::TextFilter::new(
        &[benilla_formats::FilterPattern {
            id: 1,
            pattern: r"\<darn\>".into(),
        }],
        &[],
    )
}

/// The list handler gives a row with no positive stationery id the first usable paper's
/// (`0x4ad551`-`0x4ad56f`), and masks each kept row's subject (`0x4ad5b2`, `dl` 0) in list order.
#[test]
fn the_list_defaults_the_paper_and_masks_the_subject() {
    let mut h = Mailbox::new(Vec::new());
    let world = h.app.world_mut();
    world.insert_resource(super::Stationery(
        benilla_formats::StationeryCatalog::from_rows(vec![
            benilla_formats::StationeryRow {
                id: 42,
                item: 9312,
                texture: "FREE".into(),
                flags: 1,
            },
            benilla_formats::StationeryRow {
                id: 41,
                item: 9311,
                texture: "STATIONERYTEST".into(),
                flags: 1,
            },
        ]),
    ));
    {
        let mut items = world.resource_mut::<crate::items::Items>();
        // The cheapest paper is not the lowest id, so the sort, not the id order, picks it.
        for (entry, price) in [(9311, 25), (9312, 0)] {
            let mut t = crate::items::test_template("Paper");
            t.buy_price = price;
            items.insert_template(entry, Some(t));
        }
    }
    world.insert_resource(darn_filter());
    let mut none = entry(1, 0);
    none.stationery = 0;
    none.subject = "darn it".into();
    let mut negative = entry(2, 0);
    negative.stationery = u32::MAX;
    let mut gm = entry(3, 0);
    gm.stationery = 61;
    crate::net::handlers::dispatch(
        world,
        vec![SessionEvent::MailList {
            mails: vec![none, negative, gm],
        }],
    );
    let mail = world.resource::<MailOpen>();
    let rows: Vec<(u32, &str)> = mail
        .mails
        .iter()
        .map(|e| (e.stationery, e.subject.as_str()))
        .collect();
    assert_eq!(rows, [(42, "!@#$ it"), (42, "s"), (61, "s")]);
}

/// `GetInboxText` masks the text it reads (`0x4af203`, `dl` 0) on every call, after the `$`
/// expansion, so each read moves the mask's phase; a template's body is not masked
/// (`0x4af261`-`0x4af26e`).
#[test]
fn get_inbox_text_masks_a_letter_but_not_a_template() {
    let mut letter = entry(1, 9);
    letter.checked = CHECKED_READ;
    let mut template = entry(2, 0);
    template.mail_template_id = 87;
    let mut h = Mailbox::new(vec![letter, template]);
    h.app.world_mut().insert_resource(darn_filter());
    {
        let mut script = h.script();
        let mut state = MailState {
            inbox: Vec::new(),
            open: true,
        };
        for (id, body) in [(1, "darn mail"), (2, "darn template")] {
            let mut e = entry(id, if id == 1 { 9 } else { 0 });
            e.checked = CHECKED_READ;
            e.mail_template_id = if id == 2 { 87 } else { 0 };
            state.inbox.push(MailInboxRow {
                body: Some(body.into()),
                ..row_of(&e)
            });
        }
        script.set_mail(Some(state));
    }
    let bodies: (String, String, String) = h.eval(
        "local a = GetInboxText(1) local b = GetInboxText(1) local c = GetInboxText(2) \
         return a, b, c",
    );
    assert_eq!(
        bodies,
        (
            "!@#$ mail".to_string(),
            "%^&* mail".to_string(),
            "darn template".to_string()
        )
    );
}

/// An auction notice's subject is masked by the list parse that rewrites it, when its item is
/// held then (`0x4ace70`, then `0x4ad5b2`); one whose item is not held shows empty
/// (`0x4acf3d`) until the template's callback rewrites it (`0x4ad010` → `0x4ace40`), unmasked.
#[test]
fn an_auction_subject_is_masked_by_the_list_parse_alone() {
    let mut h = Mailbox::new(Vec::new());
    h.script()
        .run("AUCTION_WON_MAIL_SUBJECT = 'Auction won: %s'")
        .expect("the string sets");
    let world = h.app.world_mut();
    world.insert_resource(darn_filter());
    world
        .resource_mut::<crate::items::Items>()
        .insert_template(2589, Some(crate::items::test_template("darn cloth")));
    let notice = |id: u32, item: u32| MailListEntry {
        message_type: mail_message_type::AUCTION,
        sender_guid: None,
        sender_id: Some(1),
        subject: format!("{item}:0:1"),
        ..entry(id, 0)
    };
    crate::net::handlers::dispatch(
        world,
        vec![SessionEvent::MailList {
            mails: vec![notice(1, 2589), notice(2, 2590)],
        }],
    );
    h.feed();
    // An item not held at the parse leaves the cell empty (`0x4acf3d`) until it lands.
    let waiting: String = h.eval("local _, _, _, b = GetInboxHeaderInfo(2) return b");
    assert_eq!(waiting, "");
    h.app
        .world_mut()
        .resource_mut::<crate::items::Items>()
        .insert_template(2590, Some(crate::items::test_template("darn wool")));
    h.feed();
    let subjects: (String, String) = h.eval(
        "local _, _, _, a = GetInboxHeaderInfo(1) local _, _, _, b = GetInboxHeaderInfo(2) \
         return a, b",
    );
    assert_eq!(
        subjects,
        (
            "Auction won: !@#$ cloth".to_string(),
            "Auction won: darn wool".to_string()
        )
    );
}

/// The read mark's `MAIL_INBOX_UPDATE` (`0x4add9d`) comes before the body is read and masked
/// (`0x4af1bf`, `0x4af203`), so a handler that reads the letter again inside it takes the mask's
/// first phase and the outer call its own masked body.
#[test]
fn a_nested_get_inbox_text_leaves_the_outer_body_masked() {
    let mut h = Mailbox::new(vec![entry(1, 9)]);
    h.app.world_mut().insert_resource(darn_filter());
    {
        let mut script = h.script();
        script.set_mail(Some(MailState {
            inbox: vec![MailInboxRow {
                body: Some("darn mail".into()),
                ..row_of(&entry(1, 9))
            }],
            open: true,
        }));
        script
            .run(
                r#"local f = CreateFrame("Frame")
                f:RegisterEvent("MAIL_INBOX_UPDATE")
                f:SetScript("OnEvent", function()
                    if not INNER then INNER = GetInboxText(1) end
                end)"#,
            )
            .expect("the reader installs");
    }
    let bodies: (String, String) = h.eval("local outer = GetInboxText(1) return outer, INNER");
    assert_eq!(bodies, ("%^&* mail".to_string(), "!@#$ mail".to_string()));
}
