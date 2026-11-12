//! Activity log: writes server events (playback, scans, new users) to
//! Admin > Activity.

use kinora_module_sdk::{host, module, Request, Value};

/// The viewer an event is about, when there is one.
fn user_of(event: &Value) -> Option<&str> {
    match event.get("type").and_then(Value::as_str) {
        Some("playback_started" | "playback_stopped") => event.get("user_id").and_then(Value::as_str),
        _ => None,
    }
}

pub fn handle(req: Request) -> Result<Value, String> {
    match req.op.as_str() {
        "event" => {
            let event = &req.args["event"];
            let kind = event.get("type").and_then(Value::as_str).unwrap_or("event");
            host::activity_record(kind, user_of(event), event)?;
            Ok(Value::Null)
        }
        other => Err(format!("unknown operation {other}")),
    }
}

module!(handle);

#[cfg(test)]
mod tests {
    use super::*;
    use kinora_module_sdk::{json, testing};

    #[test]
    fn records_events_with_their_viewer() {
        testing::reset();
        let event = json!({"type": "playback_started", "user_id": "u1", "item_id": "i1", "method": "direct_play"});
        testing::call(handle, "event", &[], json!({ "event": event })).unwrap();
        let event = json!({"type": "library_scan_started", "library_id": "l1"});
        testing::call(handle, "event", &[], json!({ "event": event })).unwrap();

        let log = testing::activity();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0]["event"], "playback_started");
        assert_eq!(log[0]["user_id"], "u1");
        assert_eq!(log[0]["payload"]["item_id"], "i1");
        assert_eq!(log[1]["event"], "library_scan_started");
        assert!(log[1]["user_id"].is_null());
    }
}
