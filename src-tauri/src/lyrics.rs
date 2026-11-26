use serde::Deserialize;
use std::time::Duration;

#[derive(Clone, Debug, serde::Serialize)]
pub struct LyricLine {
    pub timestamp_ms: u64,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct LyricResult {
    pub lines: Vec<LyricLine>,
    pub state: String,
}

#[derive(Deserialize)]
struct LrclibRecord {
    #[serde(rename = "syncedLyrics")]
    synced_lyrics: Option<String>,
    #[serde(rename = "plainLyrics")]
    plain_lyrics: Option<String>,
}

pub async fn fetch_lyrics(
    client: &reqwest::Client,
    artist: &str,
    track: &str,
    album: Option<&str>,
    duration_ms: Option<u64>,
) -> Result<Option<LyricResult>, String> {
    if artist.trim().is_empty() || track.trim().is_empty() {
        return Ok(None);
    }
    let mut query = vec![("artist_name", artist), ("track_name", track)];
    if let Some(album) = album.filter(|value| !value.trim().is_empty()) {
        query.push(("album_name", album));
    }
    let duration_seconds = duration_ms.map(|value| value.saturating_add(500) / 1000);
    let duration_string = duration_seconds
        .filter(|value| (1..=3600).contains(value))
        .map(|value| value.to_string());

    let mut request = client.get("https://lrclib.net/api/get").query(&query);
    if let Some(duration) = duration_string.as_deref() {
        request = request.query(&[("duration", duration)]);
    }

    let response = request
        .timeout(Duration::from_secs(12))
        .send()
        .await
        .map_err(|error| format!("LRCLIB request failed: {error}"))?;

    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("later");
        return Err(format!("LRCLIB rate limit reached; retry after {retry_after}"));
    }
    let response = response
        .error_for_status()
        .map_err(|error| format!("LRCLIB returned an error: {error}"))?;
    let record: LrclibRecord = response
        .json()
        .await
        .map_err(|error| format!("Could not read LRCLIB response: {error}"))?;

    if let Some(synced) = record.synced_lyrics.filter(|value| !value.trim().is_empty()) {
        let lines = parse_lrc(&synced);
        if !lines.is_empty() {
            return Ok(Some(LyricResult {
                lines,
                state: "synced".into(),
            }));
        }
    }

    let plain_lines = record
        .plain_lyrics
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|text| LyricLine {
            timestamp_ms: 0,
            text: text.to_owned(),
        })
        .collect::<Vec<_>>();
    if !plain_lines.is_empty() {
        return Ok(Some(LyricResult {
            lines: plain_lines,
            state: "plain".into(),
        }));
    }

    Ok(None)
}

pub fn parse_lrc(input: &str) -> Vec<LyricLine> {
    let mut lines = Vec::new();
    let mut offset_ms = 0_i64;

    for raw_line in input.lines() {
        let line = raw_line.trim_start_matches('\u{feff}').trim();
        if let Some(value) = line.strip_prefix("[offset:").and_then(|rest| rest.strip_suffix(']')) {
            offset_ms = value.trim().parse().unwrap_or(0);
            continue;
        }

        let mut remainder = line;
        let mut timestamps = Vec::new();
        while let Some(rest) = remainder.strip_prefix('[') {
            let Some(close) = rest.find(']') else { break };
            let tag = &rest[..close];
            if let Some(timestamp) = parse_timestamp(tag) {
                timestamps.push(timestamp);
            } else if tag.contains(':') {
                // A metadata tag before the timestamp is harmless; keep scanning.
            } else {
                break;
            }
            remainder = &rest[close + 1..];
        }

        let text = remainder.trim().to_owned();
        if text.is_empty() || timestamps.is_empty() {
            continue;
        }
        for timestamp in timestamps {
            let adjusted = (timestamp as i64 + offset_ms).max(0) as u64;
            lines.push(LyricLine {
                timestamp_ms: adjusted,
                text: text.clone(),
            });
        }
    }

    lines.sort_by_key(|line| line.timestamp_ms);
    lines
}

fn parse_timestamp(value: &str) -> Option<u64> {
    let (minutes, seconds) = value.split_once(':')?;
    let minutes = minutes.parse::<u64>().ok()?;
    let (whole_seconds, fraction) = seconds.split_once('.').unwrap_or((seconds, ""));
    let whole_seconds = whole_seconds.parse::<u64>().ok()?;
    if whole_seconds >= 60 {
        return None;
    }
    let fraction_ms = match fraction.len() {
        0 => 0,
        1 => fraction.parse::<u64>().ok()? * 100,
        2 => fraction.parse::<u64>().ok()? * 10,
        _ => fraction.chars().take(3).collect::<String>().parse::<u64>().ok()?,
    };
    Some(minutes.saturating_mul(60_000) + whole_seconds * 1000 + fraction_ms)
}
