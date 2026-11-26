use crate::lyrics::{fetch_lyrics, LyricLine, LyricResult};
use crate::LyricTick;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};
use tokio::sync::{Mutex, RwLock};

#[derive(Clone)]
struct LiveTrack {
    key: String,
    title: String,
    artist: String,
    source: String,
    duration_ms: Option<u64>,
    position_at_sample_ms: u64,
    sampled_at: Instant,
    is_playing: bool,
    playback_rate: f64,
    lines: Vec<LyricLine>,
    lyrics_state: String,
}

pub struct PlaybackRuntime {
    #[cfg(target_os = "windows")]
    selected_session: RwLock<Option<usize>>,
    #[cfg(target_os = "windows")]
    sources: RwLock<HashMap<usize, String>>,
    #[cfg(target_os = "windows")]
    latest_models: RwLock<HashMap<usize, gsmtc::SessionModel>>,
    now_playing: RwLock<Option<LiveTrack>>,
    lyric_cache: Mutex<HashMap<String, Option<LyricResult>>>,
    fetching: Mutex<std::collections::HashSet<String>>,
    client: reqwest::Client,
    offset_ms: AtomicI64,
}

impl PlaybackRuntime {
    pub fn new(offset_ms: i64) -> Self {
        let client = reqwest::Client::builder()
            .user_agent("SpotifyLyricsOverlay/0.1.0")
            .timeout(Duration::from_secs(12))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            #[cfg(target_os = "windows")]
            selected_session: RwLock::new(None),
            #[cfg(target_os = "windows")]
            sources: RwLock::new(HashMap::new()),
            #[cfg(target_os = "windows")]
            latest_models: RwLock::new(HashMap::new()),
            now_playing: RwLock::new(None),
            lyric_cache: Mutex::new(HashMap::new()),
            fetching: Mutex::new(std::collections::HashSet::new()),
            client,
            offset_ms: AtomicI64::new(offset_ms),
        }
    }

    pub fn set_offset_ms(&self, value: i64) {
        self.offset_ms.store(value, Ordering::Relaxed);
    }
}

pub fn start(app: AppHandle, runtime: Arc<PlaybackRuntime>) {
    #[cfg(target_os = "windows")]
    start_gsmtc_listener(runtime.clone());

    tauri::async_runtime::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(150));
        loop {
            ticker.tick().await;
            let tick = make_tick(&runtime).await;
            let _ = app.emit("lyric-tick", tick);
        }
    });
}

async fn make_tick(runtime: &PlaybackRuntime) -> LyricTick {
    let current = runtime.now_playing.read().await.clone();
    let Some(track) = current else {
        return LyricTick::empty();
    };

    let elapsed_ms = if track.is_playing {
        (track.sampled_at.elapsed().as_millis() as f64 * track.playback_rate.max(0.0)) as u64
    } else {
        0
    };
    let raw_position = track.position_at_sample_ms.saturating_add(elapsed_ms) as i64;
    let configured_offset = runtime.offset_ms.load(Ordering::Relaxed);
    let position_ms = raw_position.saturating_add(configured_offset).max(0) as u64;
    let active_index = if track.lyrics_state == "synced" && !track.lines.is_empty() {
        track
            .lines
            .partition_point(|line| line.timestamp_ms <= position_ms)
            .checked_sub(1)
    } else {
        None
    };
    LyricTick {
        title: Some(track.title),
        artist: Some(track.artist),
        source: Some(track.source),
        position_ms,
        duration_ms: track.duration_ms,
        is_playing: track.is_playing,
        active_index,
        lines: track.lines,
        lyrics_state: track.lyrics_state,
    }
}

#[cfg(target_os = "windows")]
fn start_gsmtc_listener(runtime: Arc<PlaybackRuntime>) {
    use gsmtc::{ManagerEvent, SessionUpdateEvent};

    tauri::async_runtime::spawn(async move {
        let mut events = match gsmtc::SessionManager::create().await {
            Ok(events) => events,
            Err(error) => {
                eprintln!("Could not connect to Windows media sessions: {error}");
                return;
            }
        };

        while let Some(event) = events.recv().await {
            match event {
                ManagerEvent::SessionCreated {
                    session_id,
                    mut rx,
                    source,
                } => {
                    runtime.sources.write().await.insert(session_id, source);
                    if *runtime.selected_session.read().await == Some(session_id) {
                        activate_session(runtime.clone(), Some(session_id)).await;
                    }

                    let runtime_for_session = runtime.clone();
                    tauri::async_runtime::spawn(async move {
                        while let Some(update) = rx.recv().await {
                            let model = match update {
                                SessionUpdateEvent::Model(model)
                                | SessionUpdateEvent::Media(model, _) => model,
                            };
                            consume_model(runtime_for_session.clone(), session_id, model).await;
                        }
                    });
                }
                ManagerEvent::SessionRemoved { session_id } => {
                    runtime.sources.write().await.remove(&session_id);
                    runtime.latest_models.write().await.remove(&session_id);
                    if *runtime.selected_session.read().await == Some(session_id) {
                        activate_session(runtime.clone(), None).await;
                    }
                }
                ManagerEvent::CurrentSessionChanged { session_id } => {
                    activate_session(runtime.clone(), session_id).await;
                }
            }
        }
    });
}

#[cfg(target_os = "windows")]
async fn activate_session(runtime: Arc<PlaybackRuntime>, session_id: Option<usize>) {
    *runtime.selected_session.write().await = session_id;

    let spotify_session = if let Some(id) = session_id {
        runtime
            .sources
            .read()
            .await
            .get(&id)
            .is_some_and(|source| source.to_ascii_lowercase().contains("spotify"))
            .then_some(id)
    } else {
        None
    };

    *runtime.now_playing.write().await = None;
    if let Some(id) = spotify_session {
        let latest_model = { runtime.latest_models.read().await.get(&id).cloned() };
        if let Some(model) = latest_model {
            consume_model(runtime, id, model).await;
        }
    }
}

#[cfg(target_os = "windows")]
async fn consume_model(
    runtime: Arc<PlaybackRuntime>,
    session_id: usize,
    model: gsmtc::SessionModel,
) {
    runtime
        .latest_models
        .write()
        .await
        .insert(session_id, model.clone());

    let selected = *runtime.selected_session.read().await;
    if selected != Some(session_id) || !model.source.to_ascii_lowercase().contains("spotify") {
        return;
    }

    let Some(media) = model.media.as_ref() else {
        return;
    };
    let title = media.title.trim();
    if title.is_empty() {
        return;
    }
    let artist = if media.artist.trim().is_empty() {
        media.subtitle.trim()
    } else {
        media.artist.trim()
    };
    let album = media.album.as_ref().map(|album| album.title.clone());
    let album_key = album
        .as_deref()
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    let key = format!("{}|{}|{}", artist.to_lowercase(), title.to_lowercase(), album_key);
    let playback = model.playback.as_ref();
    let timeline = model.timeline.as_ref();
    let is_playing = playback.is_some_and(|state| state.status == gsmtc::PlaybackStatus::Playing);
    let playback_rate = playback.map(|state| state.rate).unwrap_or(1.0).clamp(0.0, 4.0);

    let now_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    let (position_ms, duration_ms) = timeline.map_or((0, None), |timeline| {
        let start = timeline.start;
        let end = timeline.end;
        let position = timeline.position.saturating_sub(start).max(0) / 10_000;
        let duration = end.saturating_sub(start).max(0) / 10_000;
        let stale_ms = now_unix_ms
            .saturating_sub(timeline.last_updated_at_ms)
            .clamp(0, 10_000) as u64;
        let adjusted_position = (position as u64)
            .saturating_add(if is_playing { stale_ms } else { 0 });
        (
            adjusted_position,
            (duration > 0).then_some(duration as u64),
        )
    });

    let mut should_fetch = false;
    {
        let mut current = runtime.now_playing.write().await;
        if current.as_ref().is_some_and(|track| track.key == key) {
            if let Some(track) = current.as_mut() {
                track.title = title.to_owned();
                track.artist = artist.to_owned();
                track.source = "Spotify".into();
                track.position_at_sample_ms = position_ms;
                track.sampled_at = Instant::now();
                track.duration_ms = duration_ms.or(track.duration_ms);
                track.is_playing = is_playing;
                track.playback_rate = playback_rate;
            }
        } else {
            *current = Some(LiveTrack {
                key: key.clone(),
                title: title.to_owned(),
                artist: artist.to_owned(),
                source: "Spotify".into(),
                duration_ms,
                position_at_sample_ms: position_ms,
                sampled_at: Instant::now(),
                is_playing,
                playback_rate,
                lines: Vec::new(),
                lyrics_state: "loading".into(),
            });
            should_fetch = true;
        }
    }

    if should_fetch {
        begin_lyrics_fetch(
            runtime,
            key,
            artist.to_owned(),
            title.to_owned(),
            album,
            duration_ms,
        )
        .await;
    }
}

#[cfg(target_os = "windows")]
async fn begin_lyrics_fetch(
    runtime: Arc<PlaybackRuntime>,
    key: String,
    artist: String,
    title: String,
    album: Option<String>,
    duration_ms: Option<u64>,
) {
    let cached = runtime.lyric_cache.lock().await.get(&key).cloned();
    if let Some(cached) = cached {
        apply_lyrics_result(&runtime, &key, cached).await;
        return;
    }

    {
        let mut fetching = runtime.fetching.lock().await;
        if !fetching.insert(key.clone()) {
            return;
        }
    }

    tauri::async_runtime::spawn(async move {
        let result = fetch_lyrics(
            &runtime.client,
            &artist,
            &title,
            album.as_deref(),
            duration_ms,
        )
        .await;
        runtime.fetching.lock().await.remove(&key);

        match result {
            Ok(payload) => {
                runtime.lyric_cache.lock().await.insert(key.clone(), payload.clone());
                apply_lyrics_result(&runtime, &key, payload).await;
            }
            Err(error) => {
                eprintln!("{error}");
                let mut current = runtime.now_playing.write().await;
                if let Some(track) = current.as_mut().filter(|track| track.key == key) {
                    track.lines.clear();
                    track.lyrics_state = "error".into();
                }
            }
        }

    });
}

#[cfg(target_os = "windows")]
async fn apply_lyrics_result(
    runtime: &PlaybackRuntime,
    key: &str,
    result: Option<LyricResult>,
) {
    let mut current = runtime.now_playing.write().await;
    let Some(track) = current.as_mut().filter(|track| track.key == key) else {
        return;
    };
    if let Some(result) = result {
        track.lines = result.lines;
        track.lyrics_state = result.state;
    } else {
        track.lines.clear();
        track.lyrics_state = "not-found".into();
    }
}

