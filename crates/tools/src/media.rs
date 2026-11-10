//! Pictures, audio, video and Spotify on Markon's own screen. Pure helpers live here so
//! they can be tested without the network; the tool runs are in `lib.rs`.
use crate::cards::CardKind;
use serde_json::Value;

/// Largest picture Markon downloads.
pub const MAX_IMAGE: usize = 8 * 1024 * 1024;

/// The video id of a YouTube link, or `None` for anything else.
pub fn youtube_id(url: &str) -> Option<String> {
    let u = url.trim();
    let rest = u
        .strip_prefix("https://")
        .or_else(|| u.strip_prefix("http://"))?;
    let rest = rest.strip_prefix("www.").unwrap_or(rest);
    let rest = rest.strip_prefix("m.").unwrap_or(rest);
    let id = if let Some(r) = rest.strip_prefix("youtu.be/") {
        r.split(['?', '&', '#', '/']).next()?
    } else {
        let r = rest.strip_prefix("youtube.com/")?;
        if let Some(q) = r.strip_prefix("watch?") {
            q.split('&').find_map(|kv| kv.strip_prefix("v="))?
        } else {
            let r = ["shorts/", "embed/", "live/"]
                .iter()
                .find_map(|p| r.strip_prefix(p))?;
            r.split(['?', '&', '#', '/']).next()?
        }
    };
    let ok = id.len() == 11
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    ok.then(|| id.to_string())
}

/// A Spotify link or URI as a URI the Spotify app can play.
pub fn spotify_uri(s: &str) -> Option<String> {
    const KINDS: [&str; 5] = ["track", "album", "playlist", "artist", "episode"];
    let s = s.trim();
    let (kind, id) = if let Some(r) = s.strip_prefix("spotify:") {
        r.split_once(':')?
    } else {
        let r = s
            .strip_prefix("https://open.spotify.com/")
            .or_else(|| s.strip_prefix("http://open.spotify.com/"))?;
        // Localised links: open.spotify.com/intl-de/track/...
        let r = match r.split_once('/') {
            Some((first, rest)) if first.starts_with("intl-") => rest,
            _ => r,
        };
        let (kind, rest) = r.split_once('/')?;
        (kind, rest.split(['?', '#', '/']).next()?)
    };
    let ok = KINDS.contains(&kind)
        && (16..=32).contains(&id.len())
        && id.chars().all(|c| c.is_ascii_alphanumeric());
    ok.then(|| format!("spotify:{kind}:{id}"))
}

/// Audio or video, judged by the file extension of a path or URL.
pub fn media_kind(source: &str) -> Option<CardKind> {
    let path = source.split(['?', '#']).next()?;
    let ext = path.rsplit_once('.')?.1.to_ascii_lowercase();
    match ext.as_str() {
        "mp3" | "m4a" | "aac" | "wav" | "flac" | "ogg" | "oga" | "opus" | "aiff" | "aif" => {
            Some(CardKind::Audio)
        }
        "mp4" | "m4v" | "mov" | "webm" => Some(CardKind::Video),
        _ => None,
    }
}

/// The file extension for picture bytes, judged by their content (never by what a server claims).
pub fn image_ext(bytes: &[u8]) -> Option<&'static str> {
    match bytes {
        [0xFF, 0xD8, 0xFF, ..] => Some("jpg"),
        [0x89, b'P', b'N', b'G', ..] => Some("png"),
        [b'G', b'I', b'F', b'8', ..] => Some("gif"),
        [
            b'R',
            b'I',
            b'F',
            b'F',
            _,
            _,
            _,
            _,
            b'W',
            b'E',
            b'B',
            b'P',
            ..,
        ] => Some("webp"),
        _ => None,
    }
}

/// Wikimedia Commons picture search: free, no account, and every result carries its licence.
pub fn commons_search_url(query: &str) -> String {
    let q: String = query
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' => c.to_string(),
            ' ' => "+".into(),
            c => {
                let mut b = [0u8; 4];
                c.encode_utf8(&mut b)
                    .bytes()
                    .map(|x| format!("%{x:02X}"))
                    .collect()
            }
        })
        .collect();
    format!(
        "https://commons.wikimedia.org/w/api.php?action=query&format=json&generator=search\
         &gsrnamespace=6&gsrlimit=10&gsrsearch={q}+filetype:bitmap&prop=imageinfo\
         &iiprop=url|extmetadata|size&iiurlwidth=1024"
    )
}

pub struct Found {
    pub url: String,
    pub page: String,
    pub title: String,
    pub credit: String,
}

/// The best search hit: the highest-ranked picture at least 300 px wide.
pub fn parse_commons(v: &Value) -> Option<Found> {
    let mut hits: Vec<&Value> = v["query"]["pages"].as_object()?.values().collect();
    hits.sort_by_key(|p| p["index"].as_i64().unwrap_or(i64::MAX));
    hits.into_iter().find_map(|p| {
        let info = p["imageinfo"].get(0)?;
        if info["width"].as_u64().unwrap_or(0) < 300 {
            return None;
        }
        let url = info["thumburl"].as_str().or(info["url"].as_str())?;
        let meta = &info["extmetadata"];
        let plain =
            |k: &str| crate::native::html_to_text(meta[k]["value"].as_str().unwrap_or("")).1;
        let credit = [plain("Artist"), plain("LicenseShortName")]
            .into_iter()
            .filter(|s| !s.trim().is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
        let title = p["title"].as_str().unwrap_or("Picture");
        let title = title.strip_prefix("File:").unwrap_or(title);
        let title = title.rsplit_once('.').map_or(title, |(t, _)| t);
        Some(Found {
            url: url.to_string(),
            page: info["descriptionurl"].as_str().unwrap_or(url).to_string(),
            title: title.to_string(),
            credit: credit.trim().to_string(),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_youtube_links_in_every_usual_form() {
        let id = Some("dQw4w9WgXcQ".to_string());
        assert_eq!(
            youtube_id("https://www.youtube.com/watch?v=dQw4w9WgXcQ"),
            id
        );
        assert_eq!(
            youtube_id("https://youtube.com/watch?feature=x&v=dQw4w9WgXcQ&t=3"),
            id
        );
        assert_eq!(youtube_id("https://youtu.be/dQw4w9WgXcQ?si=abc"), id);
        assert_eq!(youtube_id("https://www.youtube.com/shorts/dQw4w9WgXcQ"), id);
        assert_eq!(youtube_id("https://m.youtube.com/watch?v=dQw4w9WgXcQ"), id);
        assert_eq!(youtube_id("https://www.youtube.com/watch?v=short"), None);
        assert_eq!(
            youtube_id("https://evil.com/youtube.com/watch?v=dQw4w9WgXcQ"),
            None
        );
        assert_eq!(
            youtube_id("https://youtube.com.evil.com/watch?v=dQw4w9WgXcQ"),
            None
        );
    }

    #[test]
    fn reads_spotify_links_and_uris() {
        let t = Some("spotify:track:4uLU6hMCjMI75M1A2tKUQC".to_string());
        assert_eq!(
            spotify_uri("https://open.spotify.com/track/4uLU6hMCjMI75M1A2tKUQC?si=x"),
            t
        );
        assert_eq!(
            spotify_uri("https://open.spotify.com/intl-de/track/4uLU6hMCjMI75M1A2tKUQC"),
            t
        );
        assert_eq!(spotify_uri("spotify:track:4uLU6hMCjMI75M1A2tKUQC"), t);
        assert_eq!(
            spotify_uri("https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M").as_deref(),
            Some("spotify:playlist:37i9dQZF1DXcBWIGoYBM5M")
        );
        assert_eq!(
            spotify_uri("spotify:track:x\" & do shell script \"rm"),
            None,
            "no script injection"
        );
        assert_eq!(spotify_uri("https://open.spotify.com/user/someone"), None);
    }

    #[test]
    fn tells_audio_from_video() {
        assert_eq!(
            media_kind("/Users/me/Music/song.MP3"),
            Some(CardKind::Audio)
        );
        assert_eq!(
            media_kind("https://x.org/a/clip.mp4?token=1"),
            Some(CardKind::Video)
        );
        assert_eq!(media_kind("/Users/me/notes.txt"), None);
        assert_eq!(media_kind("/Users/me/noext"), None);
    }

    #[test]
    fn trusts_picture_bytes_not_server_claims() {
        assert_eq!(image_ext(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("jpg"));
        assert_eq!(image_ext(b"\x89PNG\r\n"), Some("png"));
        assert_eq!(image_ext(b"RIFF\0\0\0\0WEBPVP8 "), Some("webp"));
        assert_eq!(image_ext(b"<html><script>"), None);
        assert_eq!(image_ext(b"<svg onload=x>"), None, "SVG can carry script");
    }

    #[test]
    fn picks_the_best_ranked_picture_from_commons() {
        let v = json!({"query": {"pages": {
            "1": {"index": 2, "title": "File:Big cat.jpg", "imageinfo": [{"width": 2000,
                "thumburl": "https://upload.wikimedia.org/big.jpg", "descriptionurl": "https://commons.wikimedia.org/wiki/File:Big_cat.jpg",
                "extmetadata": {"Artist": {"value": "<a href=\"x\">Ana</a>"}, "LicenseShortName": {"value": "CC BY-SA 4.0"}}}]},
            "2": {"index": 1, "title": "File:Tiny.png", "imageinfo": [{"width": 120, "url": "https://upload.wikimedia.org/tiny.png"}]}
        }}});
        let f = parse_commons(&v).unwrap();
        assert_eq!(
            f.url, "https://upload.wikimedia.org/big.jpg",
            "the tiny one is skipped"
        );
        assert_eq!(f.title, "Big cat");
        assert_eq!(f.credit, "Ana · CC BY-SA 4.0");
        assert!(commons_search_url("funny cat").contains("gsrsearch=funny+cat+filetype:bitmap"));
        assert!(
            commons_search_url("a&b").contains("a%26b"),
            "the query cannot add parameters"
        );
    }
}
