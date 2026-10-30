//! Schema-driven encoding vs. upstream linejs' generated `LINEStruct`
//! writers (golden hex produced by linejs + node `thrift`).

use line_rs::thrift::{encode_struct, write_message, Protocol, Schema};
use line_rs::tv;

fn encode(method: &str, args_struct: &str, args: line_rs::Value) -> String {
    let fields = encode_struct(Schema::line(), args_struct, &args).unwrap();
    hex::encode(write_message(method, &fields, Protocol::Compact).unwrap())
}

#[test]
fn send_message_args() {
    let args = tv!({"seq": 7, "message": {
        "to": "u0123456789abcdef0123456789abcdef", "createdTime": 0, "deliveredTime": 0, "hasContent": false,
        "contentType": "IMAGE", "contentMetadata": {"a": "b"}, "sessionId": 0, "text": "hi",
        "relatedMessageId": "123", "messageRelationType": "REPLY", "relatedMessageServiceCode": "TALK"
    }});
    assert_eq!(
        encode("sendMessage", "sendMessage_args", args),
        "8221000b73656e644d657373616765150e1c282175303132333435363738396162636465663031323334353637383961626364656636\
001600480268694215023b0188016101621300280331323315062502000000"
    );
}

#[test]
fn sync_args() {
    let args = tv!({"request": {"lastRevision": 5, "lastGlobalRevision": 0, "lastIndividualRevision": 0, "count": 100}});
    assert_eq!(encode("sync", "sync_args", args), "8221000473796e631c160a15c80116001600000000");
}

#[test]
fn square_send_message_args() {
    let args = tv!({"request": {"reqSeq": 1, "squareChatMid": "m1", "squareMessage": {
        "message": {"to": "m1", "text": "x", "contentType": "NONE"}, "squareMessageRevision": 0
    }}});
    assert_eq!(
        encode("sendMessage", "SquareService_sendMessage_args", args),
        "8221000b73656e644d6573736167651c150218026d311c1c28026d31880178550000360000000000"
    );
}

#[test]
fn get_contacts_v3_args() {
    let args = tv!({"request": {"targetUsers": [{"targetUserMid": "u1"}, {"targetUserMid": "u2"}], "syncReason": "AUTO_REPAIR"}});
    assert_eq!(
        encode("getContactsV3", "getContactsV3_args", args),
        "8221000d676574436f6e746163747356331c192c18027531001802753200150a000000"
    );
}
