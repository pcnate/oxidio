//! Shared protocol types for Oxidio frontends.
//!
//! This crate defines the command and state types used for communication
//! between the control channel and its clients (TUI, web, CLI oneshots).
//! All types are serializable for WebSocket/IPC transport.

use serde::{ Deserialize, Serialize };


/// Commands that can be issued from any frontend client.
#[derive( Debug, Clone, Serialize, Deserialize )]
#[serde( tag = "cmd", rename_all = "snake_case" )]
pub enum AppCommand {
    // Playback
    Play,
    Pause,
    Resume,
    TogglePlayback,
    Stop,
    Next,
    Previous,
    Seek { position_secs: f64 },
    SetVolume { level: f32 },
    VolumeUp,
    VolumeDown,

    // Playlist
    PlayTrack { index: usize },
    AddPath { path: String },
    RemoveTrack { index: usize },
    ClearPlaylist,
    ToggleShuffle,
    SetRepeat { mode: RepeatModeValue },
    CycleRepeat,
    MoveTrack { from: usize, to: usize },
    Dedup,
    SavePlaylist { name: String },
    LoadPlaylist { name: String },

    // Browser navigation
    BrowseTo { path: String },
    BrowseUp,
    BrowseHome,
    BrowseOpen { index: usize },
    BrowseAddToPlaylist { index: usize },

    // Playlist management
    ListPlaylists,
    DeletePlaylist { name: String },

    // Settings
    ToggleSetting { key: String },

    // View
    SetView { view: String },

    // State requests
    RequestFullState,

    // Application
    Quit,
}


/// Repeat mode values for serialization.
#[derive( Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize )]
#[serde( rename_all = "snake_case" )]
pub enum RepeatModeValue {
    Off,
    One,
    All,
}


/// Playback state values.
#[derive( Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize )]
#[serde( rename_all = "snake_case" )]
pub enum PlaybackStateValue {
    Stopped,
    Playing,
    Paused,
}


/// Information about a single track.
#[derive( Debug, Clone, Serialize, Deserialize )]
pub struct TrackInfo {
    pub path: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_number: Option<u32>,
    pub genre: Option<String>,
    pub year: Option<u32>,
    pub codec: Option<String>,
    pub bitrate: Option<u32>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u32>,
    pub duration_secs: Option<f64>,
    #[serde( default )]
    pub has_cover_art: bool,
}


/// A track entry in the playlist (lightweight, for list display).
#[derive( Debug, Clone, Serialize, Deserialize )]
pub struct TrackEntry {
    pub index: usize,
    pub path: String,
    pub display_name: String,
}


/// A file browser entry.
#[derive( Debug, Clone, Serialize, Deserialize )]
pub struct BrowserEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub is_audio: bool,
}


/// Snapshot of the file browser state.
#[derive( Debug, Clone, Serialize, Deserialize )]
pub struct BrowserSnapshot {
    pub current_dir: String,
    pub entries: Vec<BrowserEntry>,
    pub selected_index: usize,
}


/// Snapshot of application settings (for display in frontends).
#[derive( Debug, Clone, Serialize, Deserialize )]
pub struct SettingsSnapshot {
    pub discord_enabled: bool,
    pub smtc_enabled: bool,
    pub web_enabled: bool,
    pub web_port: u16,
    pub web_bind: String,
}


/// Full application state snapshot sent to clients on connect.
#[derive( Debug, Clone, Serialize, Deserialize )]
pub struct StateSnapshot {
    pub playback_state: PlaybackStateValue,
    pub current_track: Option<TrackInfo>,
    pub position_secs: f64,
    pub duration_secs: Option<f64>,
    pub volume: f32,
    pub playlist: Vec<TrackEntry>,
    pub playlist_index: Option<usize>,
    pub shuffle: bool,
    pub repeat_mode: RepeatModeValue,
    pub view_mode: String,
    pub visualizer_data: Option<Vec<f32>>,
    pub browser: Option<BrowserSnapshot>,
    pub settings: SettingsSnapshot,
}


/// Incremental state updates broadcast to all clients.
///
/// Avoids sending full snapshots on every tick.
#[derive( Debug, Clone, Serialize, Deserialize )]
#[serde( tag = "type", rename_all = "snake_case" )]
pub enum StateUpdate {
    /// Full state snapshot (sent on connect and on request).
    FullState {
        #[serde( flatten )]
        state: StateSnapshot,
    },

    /// Playback position update (throttled to ~2Hz).
    Position {
        secs: f64,
    },

    /// Visualizer frequency data (sent at ~10Hz when active).
    VisualizerData {
        bars: Vec<f32>,
    },

    /// Playback state changed (playing/paused/stopped).
    PlaybackStateChanged {
        state: PlaybackStateValue,
    },

    /// Current track changed.
    TrackChanged {
        track: Option<TrackInfo>,
        duration_secs: Option<f64>,
    },

    /// Playlist contents changed.
    PlaylistChanged {
        playlist: Vec<TrackEntry>,
        index: Option<usize>,
    },

    /// Volume level changed.
    VolumeChanged {
        level: f32,
    },

    /// Shuffle/repeat mode changed.
    ModeChanged {
        shuffle: bool,
        repeat_mode: RepeatModeValue,
    },

    /// Settings changed.
    SettingsChanged {
        settings: SettingsSnapshot,
    },

    /// File browser state changed.
    BrowserChanged {
        browser: BrowserSnapshot,
    },

    /// Status message (transient notification).
    StatusMessage {
        message: String,
    },
}
