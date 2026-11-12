//! Auto audio & subtitles.
//!
//! When a viewer changes the audio or subtitle track, Kinora calls
//! `tracks.remember` and this module saves a description of the pair for that
//! viewer, keyed by the season, the show and the library. When playback
//! starts, `tracks.pick` looks the choice up (season if the "per season"
//! setting is on, then show, then library if the "library fallback" setting is
//! on) and finds the closest tracks in the new file. Because this runs on the
//! server, every client gets the same picks.

pub mod matching;

use kinora_module_sdk::{host, module, reply, Request, Value};
use serde::{Deserialize, Serialize};

use matching::{AudioRef, AudioStream, SubtitleRef, SubtitleStream, TrackChoice};

const PER_SEASON: &str = "per_season";
const LIBRARY_FALLBACK: &str = "library_fallback";

/// Where an item sits, most specific first: `season`, `series`, `library`.
#[derive(Debug, Deserialize)]
struct Scope {
    scope: String,
    id: String,
}

fn key(scope: &Scope) -> String {
    format!("choice:{}:{}", scope.scope, scope.id)
}

#[derive(Debug, Deserialize)]
struct Pick {
    user_id: String,
    scopes: Vec<Scope>,
    #[serde(default)]
    audio: Vec<AudioStream>,
    #[serde(default)]
    subtitles: Vec<SubtitleStream>,
}

/// The answer to `tracks.pick`.
#[derive(Debug, Serialize)]
struct Picked {
    /// Which remembered choice was used.
    from: String,
    audio_index: Option<u32>,
    /// Missing: no opinion, the client picks. `null`: subtitles off.
    #[serde(skip_serializing_if = "Option::is_none")]
    subtitle_index: Option<Option<u32>>,
}

fn pick(req: &Request, a: Pick) -> Result<Option<Picked>, String> {
    let per_season = req.flag(PER_SEASON, false);
    let library_fallback = req.flag(LIBRARY_FALLBACK, true);
    for scope in &a.scopes {
        if (scope.scope == "season" && !per_season) || (scope.scope == "library" && !library_fallback) {
            continue;
        }
        let saved = host::kv_get(Some(&a.user_id), &key(scope))?;
        let Some(choice) = saved.and_then(|c| serde_json::from_str::<TrackChoice>(&c).ok()) else { continue };
        let picked = matching::apply(&choice, &a.audio, &a.subtitles);
        if picked.audio_index.is_none() && picked.subtitle_index.is_none() {
            // Nothing in this file matches (e.g. a show without the library's usual language). Keep looking.
            continue;
        }
        return Ok(Some(Picked {
            from: scope.scope.clone(),
            audio_index: picked.audio_index,
            subtitle_index: picked.subtitle_index,
        }));
    }
    Ok(None)
}

#[derive(Debug, Deserialize)]
struct Remember {
    user_id: String,
    scopes: Vec<Scope>,
    #[serde(default)]
    audio: Vec<AudioStream>,
    #[serde(default)]
    subtitles: Vec<SubtitleStream>,
    /// The audio stream playing now.
    audio_index: Option<u32>,
    /// The subtitle stream showing now, or null for off.
    subtitle_index: Option<u32>,
}

fn remember(a: Remember) -> Result<(), String> {
    let audio = match a.audio_index {
        Some(i) => Some(AudioRef::from(a.audio.iter().find(|s| s.index == i).ok_or("unknown audio track")?)),
        None => None,
    };
    let subtitle = match a.subtitle_index {
        Some(i) => SubtitleRef::from(Some(a.subtitles.iter().find(|s| s.index == i).ok_or("unknown subtitle track")?)),
        None => SubtitleRef::Off,
    };
    let choice = serde_json::to_string(&TrackChoice { audio, subtitle: Some(subtitle) }).map_err(|e| e.to_string())?;
    for scope in &a.scopes {
        host::kv_set(Some(&a.user_id), &key(scope), &choice)?;
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct Forget {
    user_id: String,
}

pub fn handle(req: Request) -> Result<Value, String> {
    match req.op.as_str() {
        "tracks.pick" => reply(pick(&req, req.args()?)?),
        "tracks.remember" => reply(remember(req.args()?)?),
        "tracks.forget" => {
            let a: Forget = req.args()?;
            reply(host::kv_clear(Some(&a.user_id), "choice:")?)
        }
        other => Err(format!("unknown operation {other}")),
    }
}

module!(handle);

#[cfg(test)]
mod tests {
    use super::*;
    use kinora_module_sdk::{json, testing};

    fn episode(audio: Value, subtitles: Value) -> Value {
        json!({
            "user_id": "u1",
            "scopes": [{"scope": "season", "id": "s1"}, {"scope": "series", "id": "show"}, {"scope": "library", "id": "lib"}],
            "audio": audio,
            "subtitles": subtitles,
        })
    }

    fn tracks() -> (Value, Value) {
        (
            json!([{"index": 1, "codec": "aac", "channels": 2, "language": "eng"},
                   {"index": 2, "codec": "aac", "channels": 2, "language": "jpn"}]),
            json!([{"index": 3, "codec": "ass", "language": "eng", "title": "Signs"},
                   {"index": 4, "codec": "ass", "language": "eng", "title": "Full"}]),
        )
    }

    #[test]
    fn remembers_per_show_and_picks_on_the_next_episode() {
        testing::reset();
        let (audio, subs) = tracks();
        assert!(testing::call(handle, "tracks.pick", &[], episode(audio.clone(), subs.clone())).unwrap().is_null());

        let mut args = episode(audio, subs);
        args["audio_index"] = json!(2);
        args["subtitle_index"] = json!(4);
        testing::call(handle, "tracks.remember", &[], args).unwrap();

        // Next episode: the same tracks in another order.
        let next = episode(
            json!([{"index": 1, "codec": "aac", "channels": 2, "language": "jpn"},
                   {"index": 2, "codec": "aac", "channels": 2, "language": "eng"}]),
            json!([{"index": 5, "codec": "ass", "language": "eng", "title": "Full"}]),
        );
        let picked = testing::call(handle, "tracks.pick", &[], next.clone()).unwrap();
        assert_eq!(picked, json!({"from": "series", "audio_index": 1, "subtitle_index": 5}));
        let picked = testing::call(handle, "tracks.pick", &[(PER_SEASON, "true")], next).unwrap();
        assert_eq!(picked["from"], "season");
    }

    #[test]
    fn subtitles_off_is_remembered_as_null() {
        testing::reset();
        let (audio, subs) = tracks();
        let mut args = episode(audio.clone(), subs.clone());
        args["audio_index"] = json!(1);
        testing::call(handle, "tracks.remember", &[], args).unwrap();
        let picked = testing::call(handle, "tracks.pick", &[], episode(audio, subs)).unwrap();
        assert_eq!(picked["audio_index"], 1);
        assert!(picked.as_object().unwrap().contains_key("subtitle_index"));
        assert!(picked["subtitle_index"].is_null());
    }

    #[test]
    fn library_fallback_can_be_turned_off_and_choices_forgotten() {
        testing::reset();
        let (audio, subs) = tracks();
        let mut args = episode(audio.clone(), subs.clone());
        args["audio_index"] = json!(2);
        args["subtitle_index"] = json!(3);
        testing::call(handle, "tracks.remember", &[], args).unwrap();

        // A different show in the same library.
        let mut other = episode(audio, subs);
        other["scopes"] = json!([{"scope": "series", "id": "other"}, {"scope": "library", "id": "lib"}]);
        let picked = testing::call(handle, "tracks.pick", &[], other.clone()).unwrap();
        assert_eq!(picked["from"], "library");
        assert_eq!(picked["audio_index"], 2);
        assert!(testing::call(handle, "tracks.pick", &[(LIBRARY_FALLBACK, "false")], other.clone()).unwrap().is_null());

        // Another viewer has no choices; forgetting only clears your own.
        let mut theirs = other.clone();
        theirs["user_id"] = json!("u2");
        assert!(testing::call(handle, "tracks.pick", &[], theirs).unwrap().is_null());
        testing::call(handle, "tracks.forget", &[], json!({"user_id": "u1"})).unwrap();
        assert!(testing::call(handle, "tracks.pick", &[], other).unwrap().is_null());
    }

    #[test]
    fn unknown_tracks_are_refused() {
        testing::reset();
        let (audio, subs) = tracks();
        let mut args = episode(audio, subs);
        args["audio_index"] = json!(99);
        assert!(testing::call(handle, "tracks.remember", &[], args).is_err());
    }
}
