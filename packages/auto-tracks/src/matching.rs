//! Track matching.
//!
//! Stream indexes differ between files, so a choice is remembered by its
//! language, title, codec and flags, never by index.
//!
//! Matching rules:
//! - The language must match. If no track in the new file has it, there is no
//!   opinion and the client falls back to the file's default track.
//! - Subtitles also prefer the same forced and SDH flags, then the same title
//!   ("Signs & Songs" vs "Full subtitles" are both English), then the codec.
//! - Audio prefers the same title, codec and channel count.
//! - Commentary tracks are only picked if the viewer picked commentary.
//! - Ties go to the track that comes first in the file.

use serde::{Deserialize, Serialize};

/// An audio stream of a file, as Kinora describes it (extra fields are ignored).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AudioStream {
    pub index: u32,
    #[serde(default)]
    pub codec: String,
    #[serde(default)]
    pub channels: u32,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
}

/// A subtitle stream of a file, as Kinora describes it (extra fields are ignored).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SubtitleStream {
    pub index: u32,
    #[serde(default)]
    pub codec: String,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub is_forced: bool,
    #[serde(default)]
    pub is_hearing_impaired: bool,
}

/// An audio track as the viewer picked it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioRef {
    pub language: Option<String>,
    pub title: Option<String>,
    pub codec: String,
    pub channels: u32,
}

impl From<&AudioStream> for AudioRef {
    fn from(a: &AudioStream) -> Self {
        Self { language: a.language.clone(), title: a.title.clone(), codec: a.codec.clone(), channels: a.channels }
    }
}

/// A subtitle choice: off, or a track described by its properties.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SubtitleRef {
    Off,
    Track { language: Option<String>, title: Option<String>, codec: String, forced: bool, hearing_impaired: bool },
}

impl From<Option<&SubtitleStream>> for SubtitleRef {
    fn from(s: Option<&SubtitleStream>) -> Self {
        match s {
            None => SubtitleRef::Off,
            Some(s) => SubtitleRef::Track {
                language: s.language.clone(),
                title: s.title.clone(),
                codec: s.codec.clone(),
                forced: s.is_forced,
                hearing_impaired: is_hearing_impaired(s),
            },
        }
    }
}

/// What a viewer last picked for a show (or season, or library).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TrackChoice {
    #[serde(default)]
    pub audio: Option<AudioRef>,
    #[serde(default)]
    pub subtitle: Option<SubtitleRef>,
}

/// The tracks to start with. `None` means "no opinion, use the file's default".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TrackPick {
    pub audio_index: Option<u32>,
    /// `Some(None)` turns subtitles off; `None` leaves them to the client.
    pub subtitle_index: Option<Option<u32>>,
}

pub fn apply(choice: &TrackChoice, audio: &[AudioStream], subtitles: &[SubtitleStream]) -> TrackPick {
    TrackPick {
        audio_index: choice.audio.as_ref().and_then(|r| pick_audio(audio, r)),
        subtitle_index: choice.subtitle.as_ref().and_then(|r| pick_subtitle(subtitles, r)),
    }
}

/// The audio stream in a new file closest to what the viewer picked before.
pub fn pick_audio(streams: &[AudioStream], want: &AudioRef) -> Option<u32> {
    let want_lang = want.language.as_deref().map(normalize_language);
    let want_commentary = is_commentary(want.title.as_deref());
    best(streams.iter().filter(|a| a.language.as_deref().map(normalize_language) == want_lang).map(|a| {
        let mut score = 0;
        if same_title(a.title.as_deref(), want.title.as_deref()) {
            score += 4;
        }
        if a.codec == want.codec {
            score += 2;
        }
        if a.channels == want.channels {
            score += 2;
        }
        if is_commentary(a.title.as_deref()) != want_commentary {
            score -= 20;
        }
        (a.index, score)
    }))
}

/// The subtitle stream in a new file closest to what the viewer picked before.
/// `Some(None)` means subtitles off; `None` means no opinion.
pub fn pick_subtitle(streams: &[SubtitleStream], want: &SubtitleRef) -> Option<Option<u32>> {
    let SubtitleRef::Track { language, title, codec, forced, hearing_impaired } = want else {
        return Some(None);
    };
    let want_lang = language.as_deref().map(normalize_language);
    let want_commentary = is_commentary(title.as_deref());
    best(streams.iter().filter(|s| s.language.as_deref().map(normalize_language) == want_lang).map(|s| {
        let mut score = 0;
        // A forced track only covers the foreign bits; never swap it for full subtitles or back.
        if s.is_forced == *forced {
            score += 8;
        }
        if same_title(s.title.as_deref(), title.as_deref()) {
            score += 6;
        } else if similar_title(s.title.as_deref(), title.as_deref()) {
            score += 3;
        }
        if is_hearing_impaired(s) == *hearing_impaired {
            score += 4;
        }
        if &s.codec == codec {
            score += 1;
        }
        if is_commentary(s.title.as_deref()) != want_commentary {
            score -= 20;
        }
        (s.index, score)
    }))
    .map(Some)
}

/// SDH / closed captions: the ffprobe flag, or a title that says so.
pub fn is_hearing_impaired(s: &SubtitleStream) -> bool {
    s.is_hearing_impaired
        || s.title.as_deref().map(words).is_some_and(|w| {
            w.iter().any(|w| matches!(w.as_str(), "sdh" | "cc" | "hi")) || w.windows(2).any(|p| p[0] == "hearing")
        })
}

fn is_commentary(title: Option<&str>) -> bool {
    title.is_some_and(|t| t.to_lowercase().contains("commentary"))
}

/// Highest score wins; on a tie the first one does.
fn best(scored: impl Iterator<Item = (u32, i32)>) -> Option<u32> {
    let mut top: Option<(u32, i32)> = None;
    for (index, score) in scored {
        if top.is_none_or(|(_, s)| score > s) {
            top = Some((index, score));
        }
    }
    top.map(|(i, _)| i)
}

fn same_title(a: Option<&str>, b: Option<&str>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => words(a) == words(b),
        (None, None) => true,
        _ => false,
    }
}

/// Titles that share a meaningful word: "Signs & Songs [Kantai]" vs "Signs/Songs".
fn similar_title(a: Option<&str>, b: Option<&str>) -> bool {
    const NOISE: &[&str] = &["english", "eng", "en", "subs", "subtitles", "subtitle", "sub", "track", "the", "and"];
    let (Some(a), Some(b)) = (a, b) else { return false };
    let b = words(b);
    words(a).iter().any(|w| w.len() > 1 && !NOISE.contains(&w.as_str()) && b.contains(w))
}

fn words(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_lowercase).collect()
}

/// ffprobe reports ISO 639-2 codes, but some files use 639-1 ("en") or the
/// bibliographic variants ("ger" vs "deu"). Map the common ones to one code.
pub fn normalize_language(code: &str) -> String {
    let c = code.trim().to_lowercase();
    let c = c.split(['-', '_']).next().unwrap_or("");
    let mapped = match c {
        "en" => "eng",
        "ja" => "jpn",
        "es" => "spa",
        "fr" | "fre" => "fra",
        "de" | "ger" => "deu",
        "it" => "ita",
        "pt" => "por",
        "ru" => "rus",
        "zh" | "chi" => "zho",
        "ko" => "kor",
        "nl" | "dut" => "nld",
        "sv" => "swe",
        "no" | "nb" | "nob" => "nor",
        "da" => "dan",
        "fi" => "fin",
        "pl" => "pol",
        "tr" => "tur",
        "ar" => "ara",
        "hi" => "hin",
        "he" | "heb" => "heb",
        "cs" | "cze" => "ces",
        "el" | "gre" => "ell",
        "hu" => "hun",
        "th" => "tha",
        "vi" => "vie",
        "id" => "ind",
        "uk" => "ukr",
        "ro" | "rum" => "ron",
        other => other,
    };
    mapped.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn audio(index: u32, lang: &str, codec: &str, channels: u32, title: Option<&str>) -> AudioStream {
        AudioStream { index, codec: codec.into(), channels, language: Some(lang.into()), title: title.map(Into::into) }
    }

    fn sub(index: u32, lang: &str, title: Option<&str>, forced: bool) -> SubtitleStream {
        SubtitleStream {
            index,
            codec: "ass".into(),
            language: Some(lang.into()),
            title: title.map(Into::into),
            is_forced: forced,
            is_hearing_impaired: false,
        }
    }

    /// The layout of the 86 Eighty Six release: English dub first, Japanese
    /// second, three English subtitle tracks told apart only by title.
    fn eighty_six() -> (Vec<AudioStream>, Vec<SubtitleStream>) {
        (
            vec![audio(1, "eng", "aac", 2, Some("English (2.0 AAC)")), audio(2, "jpn", "aac", 2, None)],
            vec![
                sub(3, "eng", Some("Signs & Songs"), false),
                sub(4, "eng", Some("Kantai"), false),
                sub(5, "eng", Some("English subs"), false),
            ],
        )
    }

    #[test]
    fn remembers_japanese_audio_and_full_subtitles_by_title() {
        let (a, s) = eighty_six();
        let choice = TrackChoice { audio: Some(AudioRef::from(&a[1])), subtitle: Some(SubtitleRef::from(Some(&s[2]))) };
        // Next episode: same tracks, different order.
        let next_audio = vec![audio(1, "jpn", "aac", 2, None), audio(2, "eng", "aac", 2, Some("English (2.0 AAC)"))];
        let next_subs = vec![sub(3, "eng", Some("English subs"), false), sub(4, "eng", Some("Signs & Songs"), false)];
        let pick = apply(&choice, &next_audio, &next_subs);
        assert_eq!(pick.audio_index, Some(1));
        assert_eq!(pick.subtitle_index, Some(Some(3)));
    }

    #[test]
    fn subtitles_off_stays_off() {
        let (a, s) = eighty_six();
        let choice = TrackChoice { audio: None, subtitle: Some(SubtitleRef::Off) };
        assert_eq!(apply(&choice, &a, &s).subtitle_index, Some(None));
        assert_eq!(apply(&choice, &a, &s).audio_index, None);
    }

    #[test]
    fn no_opinion_when_the_language_is_missing() {
        let (a, s) = eighty_six();
        let want = AudioRef { language: Some("fra".into()), title: None, codec: "aac".into(), channels: 2 };
        assert_eq!(pick_audio(&a, &want), None);
        let want = SubtitleRef::from(Some(&sub(9, "spa", None, false)));
        assert_eq!(pick_subtitle(&s, &want), None);
    }

    #[test]
    fn forced_and_full_subtitles_are_not_swapped() {
        let subs = vec![sub(3, "eng", Some("Forced"), true), sub(4, "eng", None, false)];
        let full = SubtitleRef::from(Some(&sub(7, "eng", Some("Dialogue"), false)));
        assert_eq!(pick_subtitle(&subs, &full), Some(Some(4)));
        let forced = SubtitleRef::from(Some(&sub(7, "eng", None, true)));
        assert_eq!(pick_subtitle(&subs, &forced), Some(Some(3)));
    }

    #[test]
    fn sdh_is_matched_from_the_flag_or_the_title() {
        let mut flagged = sub(4, "eng", None, false);
        flagged.is_hearing_impaired = true;
        let subs = vec![sub(3, "eng", None, false), flagged];
        let want_sdh = SubtitleRef::from(Some(&sub(9, "eng", Some("English SDH"), false)));
        assert_eq!(pick_subtitle(&subs, &want_sdh), Some(Some(4)));
        let want_plain = SubtitleRef::from(Some(&sub(9, "eng", Some("English"), false)));
        assert_eq!(pick_subtitle(&subs, &want_plain), Some(Some(3)));
    }

    #[test]
    fn similar_titles_beat_unrelated_ones() {
        let subs = vec![sub(3, "eng", Some("Full Dialogue"), false), sub(4, "eng", Some("Signs/Songs [GJM]"), false)];
        let want = SubtitleRef::from(Some(&sub(9, "eng", Some("Signs & Songs"), false)));
        assert_eq!(pick_subtitle(&subs, &want), Some(Some(4)));
    }

    #[test]
    fn audio_prefers_same_codec_and_channels_and_skips_commentary() {
        let streams = vec![
            audio(1, "eng", "aac", 2, Some("Director's Commentary")),
            audio(2, "eng", "aac", 2, Some("Stereo")),
            audio(3, "eng", "eac3", 6, Some("Surround 5.1")),
        ];
        let want = AudioRef { language: Some("eng".into()), title: None, codec: "eac3".into(), channels: 6 };
        assert_eq!(pick_audio(&streams, &want), Some(3));
        let want = AudioRef { language: Some("eng".into()), title: None, codec: "aac".into(), channels: 2 };
        assert_eq!(pick_audio(&streams, &want), Some(2));
        let want = AudioRef::from(&streams[0]);
        assert_eq!(pick_audio(&streams, &want), Some(1));
    }

    #[test]
    fn language_codes_are_normalized() {
        let streams = vec![audio(1, "en", "aac", 2, None), audio(2, "ja", "aac", 2, None)];
        let want = AudioRef { language: Some("jpn".into()), title: None, codec: "aac".into(), channels: 2 };
        assert_eq!(pick_audio(&streams, &want), Some(2));
        assert_eq!(normalize_language("ger"), "deu");
        assert_eq!(normalize_language("pt-BR"), "por");
    }

    #[test]
    fn choice_round_trips_as_json() {
        let (a, s) = eighty_six();
        let choice = TrackChoice { audio: Some(AudioRef::from(&a[1])), subtitle: Some(SubtitleRef::from(Some(&s[0]))) };
        let json = serde_json::to_string(&choice).unwrap();
        assert_eq!(serde_json::from_str::<TrackChoice>(&json).unwrap(), choice);
        let off: TrackChoice = serde_json::from_str(r#"{"subtitle":{"kind":"off"}}"#).unwrap();
        assert_eq!(off.subtitle, Some(SubtitleRef::Off));
    }
}
