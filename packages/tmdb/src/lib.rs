//! TMDB metadata provider.
//!
//! Accepts either a v3 API key or a v4 read access token (starts with "eyJ").
//! Attribution required by TMDB is shown in Kinora's admin pages.

use kinora_module_sdk::{host, json, module, reply, Request, Value};
use serde::{Deserialize, Serialize};

const API: &str = "https://api.themoviedb.org/3";
const IMAGES: &str = "https://image.tmdb.org/t/p";
/// A season is asked for once and then answers every episode in it.
const SEASON_CACHE_SECONDS: u32 = 600;

/// What Kinora stores for a title (the `Metadata` shape of the module ABI).
#[derive(Debug, Default, PartialEq, Serialize)]
pub struct Metadata {
    pub provider_id: String,
    pub title: Option<String>,
    pub original_title: Option<String>,
    pub year: Option<u16>,
    pub overview: Option<String>,
    pub tagline: Option<String>,
    pub rating: Option<f32>,
    pub official_rating: Option<String>,
    pub genres: Vec<String>,
    pub runtime_minutes: Option<u32>,
    pub poster_url: Option<String>,
    pub backdrop_url: Option<String>,
    pub thumb_url: Option<String>,
    pub logo_url: Option<String>,
    pub release_date: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    Movie,
    Series,
}

#[derive(Deserialize)]
struct Search {
    kind: MediaKind,
    title: String,
    year: Option<u16>,
}

#[derive(Deserialize)]
struct Details {
    kind: MediaKind,
    provider_id: String,
}

#[derive(Deserialize)]
struct Episode {
    series_provider_id: String,
    season: u32,
    episode: u32,
}

struct Tmdb {
    key: String,
    language: String,
}

impl Tmdb {
    fn from(req: &Request) -> Option<Self> {
        Some(Tmdb {
            key: req.setting("api_key")?.to_string(),
            language: req.setting("language").unwrap_or("en-US").into(),
        })
    }

    fn get(&self, path: &str, query: &[(&str, String)], cache_seconds: u32) -> Result<Option<Value>, String> {
        let mut params: Vec<(&str, String)> = vec![("language", self.language.clone())];
        params.extend_from_slice(query);
        let token = self.key.starts_with("eyJ");
        if !token {
            params.push(("api_key", self.key.clone()));
        }
        let query = params.iter().map(|(k, v)| format!("{k}={}", encode(v))).collect::<Vec<_>>().join("&");
        let auth = format!("Bearer {}", self.key);
        let headers: &[(&str, &str)] = if token { &[("authorization", &auth)] } else { &[] };
        let resp = host::http_get(&format!("{API}{path}?{query}"), headers, cache_seconds)?;
        match resp.status {
            404 => Ok(None),
            401 => Err("TMDB rejected the API key".into()),
            s if !(200..300).contains(&s) => Err(format!("TMDB returned {s}")),
            _ => resp.json().map(Some),
        }
    }

    fn search(&self, a: Search) -> Result<Option<Metadata>, String> {
        let (path, year_param) = match a.kind {
            MediaKind::Movie => ("/search/movie", "year"),
            MediaKind::Series => ("/search/tv", "first_air_date_year"),
        };
        let mut q = vec![("query", a.title), ("include_adult", "false".to_string())];
        if let Some(y) = a.year {
            q.push((year_param, y.to_string()));
        }
        let mut results = self.get(path, &q, 0)?;
        // Retry without the year: release years in file names are often off by one.
        let empty = |r: &Option<Value>| {
            r.as_ref().and_then(|v| v.get("results")).and_then(Value::as_array).map(|a| a.is_empty()).unwrap_or(true)
        };
        if a.year.is_some() && empty(&results) {
            q.pop();
            results = self.get(path, &q, 0)?;
        }
        Ok(results
            .as_ref()
            .and_then(|v| v.get("results"))
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .map(|first| to_metadata(first, Some(a.kind))))
    }

    fn details(&self, a: Details) -> Result<Option<Metadata>, String> {
        let path = match a.kind {
            MediaKind::Movie => format!("/movie/{}", encode(&a.provider_id)),
            MediaKind::Series => format!("/tv/{}", encode(&a.provider_id)),
        };
        // Title logos in the metadata language (its first two letters), English, or language-neutral.
        let lang = self.language.split('-').next().unwrap_or("en").to_string();
        let query =
            [("append_to_response", "images".to_string()), ("include_image_language", format!("{lang},en,null"))];
        Ok(self.get(&path, &query, 0)?.map(|v| to_metadata(&v, Some(a.kind))))
    }

    fn episode(&self, a: Episode) -> Result<Option<Metadata>, String> {
        let path = format!("/tv/{}/season/{}", encode(&a.series_provider_id), a.season);
        let Some(season) = self.get(&path, &[], SEASON_CACHE_SECONDS)? else { return Ok(None) };
        Ok(season
            .get("episodes")
            .and_then(Value::as_array)
            .and_then(|eps| {
                eps.iter().find(|e| e.get("episode_number").and_then(Value::as_u64) == Some(u64::from(a.episode)))
            })
            .map(|e| to_metadata(e, None)))
    }
}

/// Percent-encode a query value or path segment.
fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn image(path: Option<&str>, size: &str) -> Option<String> {
    path.filter(|p| !p.is_empty()).map(|p| format!("{IMAGES}/{size}{p}"))
}

fn s(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).filter(|x| !x.is_empty()).map(str::to_string)
}

fn year_of(date: Option<&str>) -> Option<u16> {
    date.and_then(|d| d.get(0..4)).and_then(|y| y.parse().ok())
}

/// The best title logo from `images.logos` (asked for with `append_to_response=images`):
/// PNG/SVG first (transparent), then the first one. Language filtering is done by TMDB.
fn pick_logo(v: &Value) -> Option<String> {
    let logos = v.get("images")?.get("logos")?.as_array()?;
    let path = |l: &Value| l.get("file_path").and_then(Value::as_str).map(str::to_string);
    let best =
        logos.iter().find(|l| path(l).is_some_and(|p| p.ends_with(".png") || p.ends_with(".svg"))).or(logos.first())?;
    image(path(best).as_deref(), "w500")
}

/// Convert a TMDB movie/tv/episode JSON object into Kinora's metadata shape.
/// `kind` is `None` for an episode.
pub fn to_metadata(v: &Value, kind: Option<MediaKind>) -> Metadata {
    let id = v.get("id").map(|i| i.to_string()).unwrap_or_default();
    let date = s(v, "release_date").or_else(|| s(v, "first_air_date")).or_else(|| s(v, "air_date"));
    let title = s(v, "title").or_else(|| s(v, "name"));
    let original = s(v, "original_title").or_else(|| s(v, "original_name"));
    let runtime = v
        .get("runtime")
        .and_then(Value::as_u64)
        .or_else(|| v.get("episode_run_time").and_then(Value::as_array).and_then(|a| a.first()).and_then(Value::as_u64))
        .map(|n| n as u32)
        .filter(|n| *n > 0);
    let genres = v
        .get("genres")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|g| s(g, "name")).collect())
        .unwrap_or_default();
    let is_episode = kind.is_none();
    Metadata {
        provider_id: id,
        title,
        original_title: original,
        year: year_of(date.as_deref()),
        overview: s(v, "overview"),
        tagline: s(v, "tagline"),
        rating: v.get("vote_average").and_then(Value::as_f64).map(|r| r as f32).filter(|r| *r > 0.0),
        official_rating: None,
        genres,
        runtime_minutes: runtime,
        poster_url: if is_episode { None } else { image(v.get("poster_path").and_then(Value::as_str), "w500") },
        backdrop_url: image(v.get("backdrop_path").and_then(Value::as_str), "w1280"),
        thumb_url: image(v.get("still_path").and_then(Value::as_str), "w780"),
        logo_url: if is_episode { None } else { pick_logo(v) },
        release_date: date,
    }
}

pub fn handle(req: Request) -> Result<Value, String> {
    let tmdb = Tmdb::from(&req);
    if req.op == "metadata.ready" {
        return Ok(match tmdb {
            Some(_) => json!({ "ready": true }),
            None => json!({ "ready": false, "reason": "Add a TMDB API key in the module's settings." }),
        });
    }
    let tmdb = tmdb.ok_or("no TMDB API key is set")?;
    match req.op.as_str() {
        "metadata.search" => reply(tmdb.search(req.args()?)?),
        "metadata.details" => reply(tmdb.details(req.args()?)?),
        "metadata.episode" => reply(tmdb.episode(req.args()?)?),
        other => Err(format!("unknown operation {other}")),
    }
}

module!(handle);

#[cfg(test)]
mod tests {
    use super::*;
    use kinora_module_sdk::host::HttpResponse;
    use kinora_module_sdk::testing;

    #[test]
    fn movie_json_to_metadata() {
        let v = json!({
            "id": 27205, "title": "Inception", "original_title": "Inception",
            "release_date": "2010-07-15", "overview": "A thief...", "vote_average": 8.4,
            "runtime": 148, "genres": [{"id": 28, "name": "Action"}],
            "poster_path": "/p.jpg", "backdrop_path": "/b.jpg"
        });
        let m = to_metadata(&v, Some(MediaKind::Movie));
        assert_eq!(m.provider_id, "27205");
        assert_eq!(m.year, Some(2010));
        assert_eq!(m.runtime_minutes, Some(148));
        assert_eq!(m.genres, vec!["Action"]);
        assert_eq!(m.poster_url.as_deref(), Some("https://image.tmdb.org/t/p/w500/p.jpg"));
        assert_eq!(m.logo_url, None);
    }

    #[test]
    fn picks_a_transparent_logo() {
        let v = json!({"id": 1, "title": "X", "images": {"logos": [{"file_path": "/a.jpg"}, {"file_path": "/b.png"}]}});
        let m = to_metadata(&v, Some(MediaKind::Movie));
        assert_eq!(m.logo_url.as_deref(), Some("https://image.tmdb.org/t/p/w500/b.png"));
    }

    #[test]
    fn episode_json_uses_still_not_poster() {
        let v =
            json!({"id": 1, "name": "Pilot", "air_date": "2008-01-20", "still_path": "/s.jpg", "episode_number": 1});
        let m = to_metadata(&v, None);
        assert_eq!(m.title.as_deref(), Some("Pilot"));
        assert_eq!(m.thumb_url.as_deref(), Some("https://image.tmdb.org/t/p/w780/s.jpg"));
        assert_eq!(m.poster_url, None);
    }

    #[test]
    fn not_ready_without_a_key() {
        testing::reset();
        let r = testing::call(handle, "metadata.ready", &[("api_key", "  ")], Value::Null).unwrap();
        assert_eq!(r["ready"], false);
        let r = testing::call(handle, "metadata.ready", &[("api_key", "abc")], Value::Null).unwrap();
        assert_eq!(r["ready"], true);
        assert!(testing::call(handle, "metadata.search", &[], json!({"kind": "movie", "title": "X"})).is_err());
    }

    #[test]
    fn search_retries_without_the_year_and_sends_the_key() {
        testing::reset();
        testing::on_http(|req| {
            let body = if req.url.contains("year=") {
                json!({"results": []})
            } else {
                json!({"results": [{"id": 603, "title": "The Matrix", "release_date": "1999-03-31"}]})
            };
            Ok(HttpResponse { status: 200, body: body.to_string() })
        });
        let args = json!({"kind": "movie", "title": "The Matrix & more", "year": 1998});
        let hit = testing::call(handle, "metadata.search", &[("api_key", "k3y")], args).unwrap();
        assert_eq!(hit["provider_id"], "603");
        assert_eq!(hit["year"], 1999);
        let requests = testing::http_requests();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].url.starts_with("https://api.themoviedb.org/3/search/movie?language=en-US&"));
        assert!(requests[0].url.contains("query=The%20Matrix%20%26%20more"));
        assert!(requests[0].url.contains("api_key=k3y"));
        assert!(requests[0].headers.is_empty());
    }

    #[test]
    fn read_access_tokens_go_in_a_header() {
        testing::reset();
        testing::on_http(|_| Ok(HttpResponse { status: 401, body: String::new() }));
        let args = json!({"kind": "series", "provider_id": "1399"});
        let err = testing::call(handle, "metadata.details", &[("api_key", "eyJabc")], args).unwrap_err();
        assert!(err.contains("rejected the API key"));
        let req = &testing::http_requests()[0];
        assert!(!req.url.contains("api_key"));
        assert_eq!(req.headers, vec![("authorization".to_string(), "Bearer eyJabc".to_string())]);
    }

    #[test]
    fn episodes_come_from_the_cached_season() {
        testing::reset();
        testing::on_http(|req| {
            assert!(req.url.contains("/tv/1399/season/2?"));
            assert_eq!(req.cache_seconds, SEASON_CACHE_SECONDS);
            let body = json!({"episodes": [{"id": 1, "name": "One", "episode_number": 1},
                                           {"id": 2, "name": "Two", "episode_number": 2}]});
            Ok(HttpResponse { status: 200, body: body.to_string() })
        });
        let args = json!({"series_provider_id": "1399", "season": 2, "episode": 2});
        let ep = testing::call(handle, "metadata.episode", &[("api_key", "k")], args).unwrap();
        assert_eq!(ep["title"], "Two");
        let args = json!({"series_provider_id": "1399", "season": 2, "episode": 9});
        assert!(testing::call(handle, "metadata.episode", &[("api_key", "k")], args).unwrap().is_null());
    }
}
