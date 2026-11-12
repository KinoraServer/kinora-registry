# Writing a module

A module is server logic that an admin installs at runtime. It is one WebAssembly file that
Kinora runs in a sandbox: it has no file system, no network and no clock of its own, and
reaches the outside only through the host functions its `package.json` asks permission for.

Modules here are written in Rust with the small SDK in [`sdk/`](../sdk). Anything that
compiles to core WebAssembly and follows the [wire format](#wire-format-module-abi-1) works.

- [A module in 20 lines](#a-module-in-20-lines)
- [`package.json`](#packagejson)
- [Operations](#operations)
- [Host functions](#host-functions)
- [Limits](#limits)
- [Testing](#testing)
- [Wire format (module ABI 1)](#wire-format-module-abi-1)

## A module in 20 lines

`packages/activity-log/` is the smallest real one. A module is a handler from a request to a
JSON answer:

```rust
use kinora_module_sdk::{host, module, Request, Value};

pub fn handle(req: Request) -> Result<Value, String> {
    match req.op.as_str() {
        "event" => {
            let event = &req.args["event"];
            let kind = event.get("type").and_then(Value::as_str).unwrap_or("event");
            host::activity_record(kind, None, event)?;
            Ok(Value::Null)
        }
        other => Err(format!("unknown operation {other}")),
    }
}

module!(handle);
```

Its `Cargo.toml`:

```toml
[package]
name = "kinora-activity-log"
version = "1.0.0"
edition.workspace = true
license.workspace = true
publish.workspace = true

[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
kinora-module-sdk.workspace = true
serde.workspace = true
serde_json.workspace = true
```

Add the folder to `members` in the root `Cargo.toml`. Keep dependencies few and free of
anything that needs an operating system: the target is `wasm32-unknown-unknown`.

## `package.json`

The general fields are in [format.md](format.md#the-source-packagejson). A module adds:

```json
"module": {
  "abi": 1,
  "crate": "kinora-tmdb",
  "provides": ["metadata_provider"],
  "permissions": { "http": ["api.themoviedb.org"] },
  "settings": [
    { "key": "api_key", "label": "API key", "kind": "secret", "help": "Free at themoviedb.org." }
  ]
}
```

| Field         | What                                                                             |
| ------------- | -------------------------------------------------------------------------------- |
| `abi`         | The wire format the module speaks. `1`.                                          |
| `crate`       | The Cargo package name. Its version must equal the package's `version`.          |
| `provides`    | The extension points it fills: `metadata_provider`, `track_selection`, `event_subscriber` |
| `permissions` | What it may use: `http` (a list of host names), `storage`, `activity` (`true`)   |
| `settings`    | Fields Kinora draws in the module's card. `kind` is `text`, `secret` or `bool`; `key` is lowercase with underscores; `default` and `help` are optional |

Admins see the permissions before they install, so ask only for what the module uses. `http`
hosts are exact DNS names (no wildcards, no IP addresses).

A module can't add pages or scripts to the web app. Its settings form is the `settings` list.

## Operations

Every call carries `op`, the saved `settings` (all values are strings; switches are `"true"`
or `"false"`) and `args`. Answer with JSON, or an error string.

### `metadata_provider`

| `op`               | `args`                                             | Answer                                        |
| ------------------ | -------------------------------------------------- | --------------------------------------------- |
| `metadata.ready`   | none                                               | `{"ready": true}` or `{"ready": false, "reason": "…"}` |
| `metadata.search`  | `kind` (`movie` or `series`), `title`, `year`      | a metadata object, or `null` for no match     |
| `metadata.details` | `kind`, `provider_id`                              | a metadata object, or `null`                  |
| `metadata.episode` | `series_provider_id`, `season`, `episode`          | a metadata object, or `null`                  |

`metadata.ready` is asked before a scan, so a module that still needs an API key can say so
instead of failing every lookup. The metadata object:

```json
{
  "provider_id": "603",
  "title": "The Matrix",
  "original_title": null,
  "year": 1999,
  "overview": "…",
  "tagline": "…",
  "rating": 8.2,
  "official_rating": "R",
  "genres": ["Action", "Science Fiction"],
  "runtime_minutes": 136,
  "poster_url": "https://…",
  "backdrop_url": "https://…",
  "thumb_url": null,
  "logo_url": null,
  "release_date": "1999-03-31"
}
```

Only `provider_id` and `genres` must be there; the rest may be `null`. `rating` is 0 to 10.
Image URLs are absolute; the server downloads and caches the images itself, so the image host
does not need to be in `permissions.http`.

### `track_selection`

| `op`              | `args`                                                              | Answer |
| ----------------- | ------------------------------------------------------------------- | ------ |
| `tracks.pick`     | `user_id`, `scopes`, `audio`, `subtitles`                           | `{"from": "series", "audio_index": 1, "subtitle_index": 3}`, or `null` for no opinion |
| `tracks.remember` | `user_id`, `item_id`, `scopes`, `audio`, `subtitles`, and the viewer's choice as `audio_index` and `subtitle_index` (`null` for off) | `null` |
| `tracks.forget`   | `user_id`                                                           | `null` |

`scopes` lists what the item belongs to, most specific first, e.g.
`[{"scope": "season", "id": "…"}, {"scope": "series", "id": "…"}, {"scope": "library", "id": "…"}]`.
`audio` and `subtitles` are the file's streams (index, language, codec, title and so on).
In the answer, leave `subtitle_index` out for "no opinion" and send `null` for "subtitles off".
`packages/auto-tracks/` shows the full argument structs.

### `event_subscriber`

| `op`    | `args`                                   | Answer |
| ------- | ---------------------------------------- | ------ |
| `event` | `event`: an object with a `type` and its fields | `null` |

Event types are `playback_started`, `playback_stopped`, `library_scan_started`,
`library_scan_finished`, `item_added` and `user_created`. Ignore types you don't know: new ones get added.

## Host functions

All in `kinora_module_sdk::host`. A call without its permission returns an error.

| Function                                         | Permission | What                                                     |
| ------------------------------------------------ | ---------- | -------------------------------------------------------- |
| `log(level, message)`                            | none       | A line in the server log                                 |
| `http(&HttpRequest)`, `http_get(url, headers, cache_seconds)` | `http` | A `GET` or `POST` over `https://` to a listed host. Redirects are not followed. With `cache_seconds`, the server may answer a repeated GET from memory |
| `kv_get`, `kv_set`, `kv_delete`, `kv_clear`      | `storage`  | The module's own key/value data in Kinora's database. Pass a user id to keep one value per viewer (deleted with the viewer), or `None` for shared |
| `activity_record(event, user_id, payload)`       | `activity` | A line in Admin → Activity                               |

A module sees only its own storage. Uninstalling keeps it unless the admin ticks "Also delete
its settings and saved data".

## Limits

- Every call runs in a fresh instance: nothing in memory survives between calls. Keep state
  with `kv_*`.
- At most 64 MB of memory, a cap on how much it may compute per call, and at most 200 host
  calls per call. Going over fails that one call.
- The file must import nothing except `kinora.host_call`. That rules out WASI and
  `wasm-bindgen`.

## Testing

On any target other than wasm32 the host functions are served by `kinora_module_sdk::testing`,
so a module is tested with plain `cargo test`:

```rust
#[test]
fn records_events() {
    testing::reset();
    testing::call(handle, "event", &[], json!({ "event": { "type": "user_created" } })).unwrap();
    assert_eq!(testing::activity().len(), 1);
}
```

`testing::on_http` answers the module's HTTP requests, `testing::http_requests` lists what it
asked for, and `testing::call` takes the settings as `&[("key", "value")]`. Storage is kept in
memory per test thread.

Then build the registry and install the module in a real server, as the
[README](../README.md#working-on-packages) shows.

## Wire format (module ABI 1)

The SDK does all of this; it matters only for a module in another language.

The module exports:

| Export                          | What                                                              |
| ------------------------------- | ----------------------------------------------------------------- |
| `memory`                        | Its linear memory                                                 |
| `kn_alloc(len: i32) -> i32`     | Returns a pointer to `len` writable bytes                         |
| `kn_call(ptr: i32, len: i32) -> i64` | Handles the request at `ptr`; returns `ptr << 32 \| len` of the answer |

and may import one function:

| Import                                        | What                                             |
| --------------------------------------------- | ------------------------------------------------ |
| `kinora.host_call(ptr: i32, len: i32) -> i64` | Runs a host function; returns `ptr << 32 \| len` of its answer, written to memory from `kn_alloc` |

Everything is UTF-8 JSON. A request is `{"op": "…", "settings": {…}, "args": …}`. An answer,
in both directions, is `{"ok": …}` or `{"error": "…"}`. A host call is an object with `"fn"`
and that function's fields, e.g. `{"fn": "kv_get", "user": null, "key": "k"}`.
