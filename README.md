# Oxidio

A lightweight Terminal User Interface (TUI) music player written in Rust.

![License](https://img.shields.io/badge/license-MIT-blue.svg)

## Features

- **Audio Playback** - Play, pause, stop, seek, volume control, next/previous track
- **Multiple Formats** - MP3, FLAC, OGG, WAV, M4A/AAC, OPUS, WMA, AIFF, ALAC
- **Visualizers** - Bars, spectrum analyzer, waveform, and level meter
- **Playlist Management** - Shuffle, repeat modes (off/one/all), reordering, save/load
- **File Browser** - Navigate local and network (SMB/UNC) paths
- **Session Persistence** - Remembers playlist, position, volume, and settings
- **Platform Integration**
  - Windows: System Media Transport Controls (lock screen, media keys)
  - Discord Rich Presence

## Installation

### From Source

**Prerequisites:**
- Rust 1.70+
- Linux: ALSA development headers (`libasound2-dev` on Debian/Ubuntu)

```bash
git clone https://github.com/pcnate/oxidio.git
cd oxidio
cargo build --release
```

The binary will be at `target/release/oxidio`.

### Windows Installer

Download the latest installer from [Releases](https://github.com/pcnate/oxidio/releases).

## Usage

```bash
# Launch with file browser
oxidio --browse

# Open a directory
oxidio --path /path/to/music

# Play specific files
oxidio track1.mp3 track2.flac

# Play all audio in a directory
oxidio /path/to/music/
```

## Keyboard Shortcuts

### Playback

| Key | Action |
|-----|--------|
| `Space` | Play / Pause |
| `s` | Stop |
| `n` | Next track |
| `p` | Previous track |
| `<` / `>` | Seek backward / forward |
| `+` / `-` | Volume up / down |

### Navigation

| Key | Action |
|-----|--------|
| `↑` / `k` | Move up |
| `↓` / `j` | Move down |
| `g` | Go to start |
| `G` | Go to end |
| `Tab` | Next view |
| `Shift+Tab` | Previous view |

### Playlist

| Key | Action |
|-----|--------|
| `S` | Toggle shuffle |
| `r` | Cycle repeat mode |
| `e` | Edit mode |
| `d` | Delete track (edit mode) |
| `J` / `K` | Reorder tracks (edit mode) |
| `c` | Clear playlist |

### Views

| Key | Action |
|-----|--------|
| `v` | Visualizer |
| `i` | Track info |
| `m` | Cycle visualizer style |
| `?` | Help |
| `/` | Command mode |
| `Esc` | Exit current mode |
| `q` | Quit |

## Configuration

Settings are stored at:
- Linux: `~/.config/oxidio/settings.json`
- Windows: `%APPDATA%\oxidio\settings.json`

```json
{
  "discord_enabled": true,
  "smtc_enabled": true
}
```

## Building

### Native Build

```bash
cargo build --release
```

### Cross-Compile for Windows (from Linux)

```bash
cargo build --release --target x86_64-pc-windows-gnu
```

### Docker Build

```bash
docker-compose up build-all
# Outputs to ./dist/
```

## License

MIT
