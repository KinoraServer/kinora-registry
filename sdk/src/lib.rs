//! The Kinora module SDK.
//!
//! A module is a WebAssembly file the Kinora server runs in a sandbox. The
//! server calls it with a JSON [`Request`] (an operation name, the module's
//! settings and the operation's arguments) and gets JSON back. The module can
//! only reach the outside world through the functions in [`host`], and only
//! the ones its `package.json` asks permission for.
//!
//! ```ignore
//! use kinora_module_sdk::{module, Request, Value};
//!
//! fn handle(req: Request) -> Result<Value, String> {
//!     match req.op.as_str() {
//!         "event" => Ok(Value::Null),
//!         other => Err(format!("unknown operation {other}")),
//!     }
//! }
//! module!(handle);
//! ```
//!
//! The wire format (module ABI 1) is described in `docs/modules.md`. On any
//! target other than wasm32 the host functions are served by [`testing`], so
//! modules are unit tested with plain `cargo test`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
pub use serde_json::{self, json, Value};

/// The module ABI this SDK speaks. Goes in `package.json` as `module.abi`.
pub const ABI: u32 = 1;

/// Module settings as saved by the admin: every value is a string
/// (`"true"` / `"false"` for switches).
pub type Settings = BTreeMap<String, String>;

/// One call from the server.
#[derive(Debug, Clone, Deserialize)]
pub struct Request {
    /// Which operation, e.g. `metadata.search`. See `docs/modules.md`.
    pub op: String,
    #[serde(default)]
    pub settings: Settings,
    #[serde(default)]
    pub args: Value,
}

impl Request {
    /// The arguments as a typed struct.
    pub fn args<T: serde::de::DeserializeOwned>(&self) -> Result<T, String> {
        serde_json::from_value(self.args.clone()).map_err(|e| format!("bad arguments for {}: {e}", self.op))
    }

    /// A text setting, trimmed. `None` when it is missing or empty.
    pub fn setting(&self, key: &str) -> Option<&str> {
        self.settings.get(key).map(|v| v.trim()).filter(|v| !v.is_empty())
    }

    /// A switch setting; `default` when it was never saved.
    pub fn flag(&self, key: &str, default: bool) -> bool {
        self.settings.get(key).map(|v| v == "true").unwrap_or(default)
    }
}

/// Turn any serialisable value into a response.
pub fn reply<T: Serialize>(value: T) -> Result<Value, String> {
    serde_json::to_value(value).map_err(|e| e.to_string())
}

/// Functions the server offers to modules. Each needs a permission in the
/// module's `package.json`, except [`log`].
pub mod host {
    use super::*;

    #[derive(Debug, Clone, Default, Serialize, Deserialize)]
    pub struct HttpRequest {
        /// `GET` or `POST`.
        pub method: String,
        /// Must be `https://` and its host must be listed in `permissions.http`.
        pub url: String,
        #[serde(default)]
        pub headers: Vec<(String, String)>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub body: Option<String>,
        /// For GET: the server may answer from its memory for this many
        /// seconds instead of asking again. 0 turns that off.
        #[serde(default)]
        pub cache_seconds: u32,
    }

    #[derive(Debug, Clone, Default, Serialize, Deserialize)]
    pub struct HttpResponse {
        pub status: u16,
        pub body: String,
    }

    impl HttpResponse {
        pub fn json(&self) -> Result<Value, String> {
            serde_json::from_str(&self.body).map_err(|e| format!("response is not JSON: {e}"))
        }
    }

    fn call<T: serde::de::DeserializeOwned>(request: Value) -> Result<T, String> {
        let answer = crate::raw_host_call(&request.to_string());
        let mut answer: Value =
            serde_json::from_str(&answer).map_err(|e| format!("bad answer from the server: {e}"))?;
        if let Some(error) = answer.get("error").and_then(Value::as_str) {
            return Err(error.to_string());
        }
        serde_json::from_value(answer.get_mut("ok").map(Value::take).unwrap_or(Value::Null)).map_err(|e| e.to_string())
    }

    /// Write a line to the server log. `level` is `debug`, `info`, `warn` or `error`.
    pub fn log(level: &str, message: &str) {
        let _ = call::<Value>(json!({ "fn": "log", "level": level, "message": message }));
    }

    /// Permission: `http` (a list of host names).
    pub fn http(request: &HttpRequest) -> Result<HttpResponse, String> {
        let mut v = serde_json::to_value(request).map_err(|e| e.to_string())?;
        v["fn"] = json!("http");
        call(v)
    }

    pub fn http_get(url: &str, headers: &[(&str, &str)], cache_seconds: u32) -> Result<HttpResponse, String> {
        http(&HttpRequest {
            method: "GET".into(),
            url: url.into(),
            headers: headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            body: None,
            cache_seconds,
        })
    }

    /// Read a value the module stored. `user` keeps one value per viewer (and
    /// it is deleted with the viewer); `None` is shared. Permission: `storage`.
    pub fn kv_get(user: Option<&str>, key: &str) -> Result<Option<String>, String> {
        call(json!({ "fn": "kv_get", "user": user, "key": key }))
    }

    pub fn kv_set(user: Option<&str>, key: &str, value: &str) -> Result<(), String> {
        call::<Value>(json!({ "fn": "kv_set", "user": user, "key": key, "value": value })).map(|_| ())
    }

    pub fn kv_delete(user: Option<&str>, key: &str) -> Result<(), String> {
        call::<Value>(json!({ "fn": "kv_delete", "user": user, "key": key })).map(|_| ())
    }

    /// Delete every key that starts with `prefix` ("" for all of them).
    pub fn kv_clear(user: Option<&str>, prefix: &str) -> Result<(), String> {
        call::<Value>(json!({ "fn": "kv_clear", "user": user, "prefix": prefix })).map(|_| ())
    }

    /// Add a line to Admin > Activity. Permission: `activity`.
    pub fn activity_record(event: &str, user_id: Option<&str>, payload: &Value) -> Result<(), String> {
        call::<Value>(json!({ "fn": "activity_record", "event": event, "user_id": user_id, "payload": payload }))
            .map(|_| ())
    }
}

// ---------------------------------------------------------------------------
// Wire format (ABI 1). The server writes the request into memory it got from
// `kn_alloc`, calls `kn_call(ptr, len)` and reads the answer from the returned
// `ptr << 32 | len`. Host calls work the same way in the other direction.
// ---------------------------------------------------------------------------

#[doc(hidden)]
pub fn __alloc(len: u32) -> u32 {
    (give(vec![0u8; len as usize]) >> 32) as u32
}

#[cfg(target_arch = "wasm32")]
unsafe fn take(ptr: u32, len: u32) -> Vec<u8> {
    // Handed out by `give` as a boxed slice of exactly this length.
    Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr as *mut u8, len as usize)).into_vec()
}

fn give(bytes: Vec<u8>) -> u64 {
    let bytes = Box::leak(bytes.into_boxed_slice());
    ((bytes.as_mut_ptr() as u64) << 32) | bytes.len() as u64
}

/// Run one request through a module's handler. Used by [`module!`] and by tests.
pub fn dispatch(request: &[u8], handler: fn(Request) -> Result<Value, String>) -> Vec<u8> {
    let result = serde_json::from_slice::<Request>(request).map_err(|e| format!("bad request: {e}")).and_then(handler);
    match result {
        Ok(v) => json!({ "ok": v }),
        Err(e) => json!({ "error": e }),
    }
    .to_string()
    .into_bytes()
}

#[doc(hidden)]
pub fn __call(ptr: u32, len: u32, handler: fn(Request) -> Result<Value, String>) -> u64 {
    #[cfg(target_arch = "wasm32")]
    let request = unsafe { take(ptr, len) };
    #[cfg(not(target_arch = "wasm32"))]
    let request: Vec<u8> = {
        let _ = (ptr, len);
        Vec::new()
    };
    give(dispatch(&request, handler))
}

#[cfg(target_arch = "wasm32")]
fn raw_host_call(request: &str) -> String {
    #[link(wasm_import_module = "kinora")]
    extern "C" {
        fn host_call(ptr: u32, len: u32) -> u64;
    }
    let packed = unsafe { host_call(request.as_ptr() as u32, request.len() as u32) };
    let bytes = unsafe { take((packed >> 32) as u32, packed as u32) };
    String::from_utf8(bytes).unwrap_or_default()
}

#[cfg(not(target_arch = "wasm32"))]
fn raw_host_call(request: &str) -> String {
    testing::answer(request)
}

/// Export a handler as this module's entry point.
#[macro_export]
macro_rules! module {
    ($handler:path) => {
        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub extern "C" fn kn_alloc(len: u32) -> u32 {
            $crate::__alloc(len)
        }

        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub extern "C" fn kn_call(ptr: u32, len: u32) -> u64 {
            $crate::__call(ptr, len, $handler)
        }
    };
}

/// A stand-in server for unit tests (any target but wasm32). Storage and the
/// activity log are kept in memory per test thread; HTTP is answered by the
/// function given to [`testing::on_http`].
#[cfg(not(target_arch = "wasm32"))]
pub mod testing {
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    use super::host::{HttpRequest, HttpResponse};
    use super::{json, Request, Settings, Value};

    type HttpHandler = Box<dyn Fn(&HttpRequest) -> Result<HttpResponse, String>>;

    #[derive(Default)]
    struct Host {
        http: Option<HttpHandler>,
        kv: BTreeMap<(Option<String>, String), String>,
        activity: Vec<Value>,
        requests: Vec<HttpRequest>,
    }

    thread_local! {
        static HOST: RefCell<Host> = RefCell::new(Host::default());
    }

    /// Forget everything from earlier in this test thread.
    pub fn reset() {
        HOST.with(|h| *h.borrow_mut() = Host::default());
    }

    /// Answer the module's HTTP requests.
    pub fn on_http(handler: impl Fn(&HttpRequest) -> Result<HttpResponse, String> + 'static) {
        HOST.with(|h| h.borrow_mut().http = Some(Box::new(handler)));
    }

    /// Every HTTP request the module made, oldest first.
    pub fn http_requests() -> Vec<HttpRequest> {
        HOST.with(|h| h.borrow().requests.clone())
    }

    /// Every `activity_record` call, oldest first.
    pub fn activity() -> Vec<Value> {
        HOST.with(|h| h.borrow().activity.clone())
    }

    /// Call a handler the way the server does.
    pub fn call(
        handler: fn(Request) -> Result<Value, String>,
        op: &str,
        settings: &[(&str, &str)],
        args: Value,
    ) -> Result<Value, String> {
        let settings: Settings = settings.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let request = json!({ "op": op, "settings": settings, "args": args }).to_string();
        let mut answer: Value = serde_json::from_slice(&super::dispatch(request.as_bytes(), handler)).unwrap();
        match answer.get("error").and_then(Value::as_str) {
            Some(e) => Err(e.to_string()),
            None => Ok(answer["ok"].take()),
        }
    }

    pub(crate) fn answer(request: &str) -> String {
        let v: Value = serde_json::from_str(request).unwrap_or_default();
        let text = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
        let user = v.get("user").and_then(Value::as_str).map(str::to_string);
        let result: Result<Value, String> = HOST.with(|h| {
            let mut h = h.borrow_mut();
            match text("fn").as_str() {
                "log" => Ok(Value::Null),
                "http" => {
                    let req: HttpRequest = serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
                    h.requests.push(req.clone());
                    let handler = h.http.as_ref().ok_or("no HTTP handler set (testing::on_http)")?;
                    handler(&req).map(|r| json!(r))
                }
                "kv_get" => Ok(json!(h.kv.get(&(user, text("key"))))),
                "kv_set" => {
                    h.kv.insert((user, text("key")), text("value"));
                    Ok(Value::Null)
                }
                "kv_delete" => {
                    h.kv.remove(&(user, text("key")));
                    Ok(Value::Null)
                }
                "kv_clear" => {
                    let prefix = text("prefix");
                    h.kv.retain(|(u, k), _| !(*u == user && k.starts_with(&prefix)));
                    Ok(Value::Null)
                }
                "activity_record" => {
                    h.activity.push(v.clone());
                    Ok(Value::Null)
                }
                other => Err(format!("unknown host function {other}")),
            }
        });
        match result {
            Ok(v) => json!({ "ok": v }),
            Err(e) => json!({ "error": e }),
        }
        .to_string()
    }
}
