# Spotify Lyrics Overlay

A lightweight Windows desktop overlay that reads the active Spotify session through Windows Media Control and looks up synced lyrics on LRCLIB. It does not use Spotify's Web API and does not require Spotify Premium.

## What is scaffolded

- Tauri v2 transparent, borderless, always-on-top window.
- Windows GSMTC listener, LRCLIB lookup, in-memory lyric cache, LRC parser, and a 150 ms sync ticker.
- Stream-chat inspired lyrics panel with active-line highlighting and smooth scroll.
- System tray menu, click-through toggle, and global `Ctrl + Shift + L` shortcut.
- Settings stored in the user's app config directory as `config.json`.

On each new track, the app sends its artist and title, plus album and duration when available, to LRCLIB to find matching lyrics.

The project has not been built or run yet. On Windows, install the Tauri prerequisites and [a supported Node.js version](https://vite.dev/guide/), then run `npm install` followed by `npm run tauri dev` from this directory when ready to build and launch it.
