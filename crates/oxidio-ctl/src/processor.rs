//! Command processor — processes commands and broadcasts state updates.
//!
//! The `CommandProcessor` receives commands from any frontend client via the
//! control channel, executes them against the shared `Player`, and broadcasts
//! state updates to all subscribers.

use std::path::{ Path, PathBuf };
use std::sync::{ Arc, Mutex };
use std::time::{ Duration, Instant };

use tokio::sync::{ broadcast, mpsc };

use oxidio_core::player::PlaybackState;
use oxidio_core::library::LibraryScanner;
use oxidio_core::{ Player, Playlist, RepeatMode };

use oxidio_protocol::{
    AppCommand, BrowserEntry, BrowserSnapshot, PlaybackStateValue,
    RepeatModeValue, SettingsSnapshot, StateSnapshot, StateUpdate,
    TrackEntry, TrackInfo,
};


/// Application settings managed by the processor.
#[derive( Debug, Clone, serde::Serialize, serde::Deserialize )]
#[serde( default )]
pub struct ProcessorSettings {
    pub discord_enabled: bool,
    pub smtc_enabled: bool,
    pub web_enabled: bool,
    pub web_port: u16,
    pub web_bind: String,
}


impl Default for ProcessorSettings {
    fn default() -> Self {
        Self {
            discord_enabled: true,
            smtc_enabled: true,
            web_enabled: false,
            web_port: 8384,
            web_bind: "127.0.0.1".to_string(),
        }
    }
}


impl ProcessorSettings {
    /// Returns the path to the settings file.
    fn settings_path() -> Option<PathBuf> {
        dirs::config_dir().map( |p| p.join( "oxidio" ).join( "settings.json" ) )
    }


    /// Loads settings from disk, or returns defaults if not found.
    pub fn load() -> Self {
        let path = match Self::settings_path() {
            Some( p ) => p,
            None => return Self::default(),
        };

        if !path.exists() {
            return Self::default();
        }

        match std::fs::read_to_string( &path ) {
            Ok( contents ) => {
                serde_json::from_str( &contents ).unwrap_or_default()
            }
            Err( e ) => {
                tracing::warn!( "Failed to read settings: {}", e );
                Self::default()
            }
        }
    }


    /// Saves settings to disk.
    pub fn save( &self ) {
        let path = match Self::settings_path() {
            Some( p ) => p,
            None => return,
        };

        if let Some( parent ) = path.parent() {
            if !parent.exists() {
                if let Err( e ) = std::fs::create_dir_all( parent ) {
                    tracing::warn!( "Failed to create settings directory: {}", e );
                    return;
                }
            }
        }

        match serde_json::to_string_pretty( self ) {
            Ok( json ) => {
                if let Err( e ) = std::fs::write( &path, json ) {
                    tracing::warn!( "Failed to save settings: {}", e );
                }
            }
            Err( e ) => {
                tracing::warn!( "Failed to serialize settings: {}", e );
            }
        }
    }


    /// Converts to a protocol snapshot.
    pub fn to_snapshot( &self ) -> SettingsSnapshot {
        SettingsSnapshot {
            discord_enabled: self.discord_enabled,
            smtc_enabled: self.smtc_enabled,
            web_enabled: self.web_enabled,
            web_port: self.web_port,
            web_bind: self.web_bind.clone(),
        }
    }
}


/// Lightweight browser state for web clients.
struct BrowserState {
    current_dir: PathBuf,
    entries: Vec<BrowserEntryInternal>,
    selected: usize,
}


#[derive( Debug, Clone )]
struct BrowserEntryInternal {
    path: PathBuf,
    name: String,
    is_dir: bool,
    is_audio: bool,
}


/// Audio file extensions for browser highlighting.
const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "flac", "ogg", "wav", "m4a", "aac", "opus", "wma", "aiff", "alac",
];


impl BrowserState {
    fn new( path: PathBuf ) -> Self {
        let mut state = Self {
            current_dir: path,
            entries: Vec::new(),
            selected: 0,
        };
        state.refresh();
        state
    }


    fn refresh( &mut self ) {
        self.entries.clear();
        self.selected = 0;

        if let Some( parent ) = self.current_dir.parent() {
            self.entries.push( BrowserEntryInternal {
                path: parent.to_path_buf(),
                name: "..".to_string(),
                is_dir: true,
                is_audio: false,
            });
        }

        let mut dirs = Vec::new();
        let mut files = Vec::new();

        if let Ok( read_dir ) = std::fs::read_dir( &self.current_dir ) {
            for entry in read_dir.flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();

                if name.starts_with( '.' ) {
                    continue;
                }

                let is_dir = path.is_dir();
                let is_audio = !is_dir && Self::is_audio_file( &path );

                let browser_entry = BrowserEntryInternal { path, name, is_dir, is_audio };

                if is_dir {
                    dirs.push( browser_entry );
                } else if is_audio {
                    files.push( browser_entry );
                }
            }
        }

        dirs.sort_by( |a, b| a.name.to_lowercase().cmp( &b.name.to_lowercase() ) );
        files.sort_by( |a, b| a.name.to_lowercase().cmp( &b.name.to_lowercase() ) );

        self.entries.extend( dirs );
        self.entries.extend( files );
    }


    fn navigate_to( &mut self, path: &std::path::Path ) {
        let canonical = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.current_dir.join( path )
        };

        if canonical.is_dir() {
            self.current_dir = canonical;
            self.refresh();
        }
    }


    fn to_snapshot( &self ) -> BrowserSnapshot {
        BrowserSnapshot {
            current_dir: self.current_dir.to_string_lossy().to_string(),
            entries: self.entries.iter().map( |e| BrowserEntry {
                name: e.name.clone(),
                path: e.path.to_string_lossy().to_string(),
                is_dir: e.is_dir,
                is_audio: e.is_audio,
            }).collect(),
            selected_index: self.selected,
        }
    }


    fn is_audio_file( path: &std::path::Path ) -> bool {
        path.extension()
            .and_then( |e| e.to_str() )
            .map( |e| AUDIO_EXTENSIONS.contains( &e.to_lowercase().as_str() ) )
            .unwrap_or( false )
    }
}


/// Processes commands from the control channel and broadcasts state updates.
///
/// Accepts a shared `Arc<Player>` so that the TUI can also read from the
/// same player instance for rendering.
pub struct CommandProcessor {
    player: Arc<Player>,
    settings: ProcessorSettings,
    browser: BrowserState,

    // Channels
    command_rx: mpsc::Receiver<AppCommand>,
    broadcast_tx: broadcast::Sender<StateUpdate>,

    // State tracking for change detection
    last_playback_state: PlaybackState,
    last_track_path: Option<PathBuf>,
    last_volume: f32,
    last_shuffle: bool,
    last_repeat: RepeatMode,
    last_position_broadcast: Instant,
    last_vis_broadcast: Instant,

    // Current view mode (tracked for connected clients)
    view_mode: String,

    // Shared cover art path (read by web server's /api/cover endpoint)
    cover_art_path: Arc<Mutex<Option<PathBuf>>>,
}


impl CommandProcessor {
    /// Creates a new command processor with a shared player.
    ///
    /// @param player - Shared player instance (also held by the TUI)
    /// @param settings - Application settings
    /// @param start_path - Starting directory for the file browser
    /// @param browse - Whether to start in browse mode
    /// @param command_rx - Receiver for commands from frontends
    /// @param broadcast_tx - Sender for broadcasting state updates
    pub fn new(
        player: Arc<Player>,
        settings: ProcessorSettings,
        start_path: PathBuf,
        browse: bool,
        command_rx: mpsc::Receiver<AppCommand>,
        broadcast_tx: broadcast::Sender<StateUpdate>,
    ) -> Self {
        let browser = BrowserState::new( start_path );

        let volume = player.volume();
        let shuffle = player.playlist().read().unwrap().shuffle();
        let repeat = player.playlist().read().unwrap().repeat();

        let view_mode = if browse {
            "browser".to_string()
        } else {
            "playlist".to_string()
        };

        Self {
            player,
            settings,
            browser,
            command_rx,
            broadcast_tx,
            last_playback_state: PlaybackState::Stopped,
            last_track_path: None,
            last_volume: volume,
            last_shuffle: shuffle,
            last_repeat: repeat,
            last_position_broadcast: Instant::now(),
            last_vis_broadcast: Instant::now(),
            view_mode,
            cover_art_path: Arc::new( Mutex::new( None ) ),
        }
    }


    /// Returns a cloneable reference to the shared cover art path.
    ///
    /// Used by the web server to serve album art via `/api/cover`.
    pub fn cover_art_path( &self ) -> Arc<Mutex<Option<PathBuf>>> {
        Arc::clone( &self.cover_art_path )
    }


    /// Runs the command processing loop.
    ///
    /// Blocks until the channel is closed or a Quit command is received.
    pub async fn run( &mut self ) {
        let mut tick_interval = tokio::time::interval( Duration::from_millis( 100 ) );

        loop {
            tokio::select! {
                Some( cmd ) = self.command_rx.recv() => {
                    if matches!( cmd, AppCommand::Quit ) {
                        self.save_session();
                        break;
                    }
                    self.handle_command( cmd );
                }

                _ = tick_interval.tick() => {
                    self.tick();
                }
            }
        }
    }


    /// Periodic update — handles auto-advance and broadcasts state changes.
    fn tick( &mut self ) {
        // Auto-advance to next track when current ends
        if self.player.track_ended() {
            match self.player.play_next() {
                Ok( true ) => {
                    tracing::info!( "Auto-advanced to next track" );
                }
                Ok( false ) => {}
                Err( e ) => {
                    tracing::warn!( "Auto-advance error: {}", e );
                    let _ = self.broadcast_tx.send( StateUpdate::StatusMessage {
                        message: format!( "Auto-advance error: {}", e ),
                    });
                }
            }
        }

        self.broadcast_changes();
    }


    /// Detects state changes and broadcasts incremental updates.
    fn broadcast_changes( &mut self ) {
        let current_state = self.player.state();
        let current_track = self.player.current_track();
        let current_volume = self.player.volume();

        let playlist_arc = self.player.playlist();
        let playlist = playlist_arc.read().unwrap();
        let current_shuffle = playlist.shuffle();
        let current_repeat = playlist.repeat();
        drop( playlist );

        // Playback state changed
        if current_state != self.last_playback_state {
            let _ = self.broadcast_tx.send( StateUpdate::PlaybackStateChanged {
                state: playback_state_to_value( current_state ),
            });
            self.last_playback_state = current_state;
        }

        // Track changed
        if current_track != self.last_track_path {
            let track_info = current_track.as_ref().map( |path| {
                self.build_track_info( path )
            });
            let duration = self.player.duration().map( |d| d.as_secs_f64() );

            let _ = self.broadcast_tx.send( StateUpdate::TrackChanged {
                track: track_info,
                duration_secs: duration,
            });
            self.last_track_path = current_track;
        }

        // Volume changed
        if ( current_volume - self.last_volume ).abs() > 0.001 {
            let _ = self.broadcast_tx.send( StateUpdate::VolumeChanged {
                level: current_volume,
            });
            self.last_volume = current_volume;
        }

        // Shuffle/repeat changed
        if current_shuffle != self.last_shuffle || current_repeat != self.last_repeat {
            let _ = self.broadcast_tx.send( StateUpdate::ModeChanged {
                shuffle: current_shuffle,
                repeat_mode: repeat_mode_to_value( current_repeat ),
            });
            self.last_shuffle = current_shuffle;
            self.last_repeat = current_repeat;
        }

        // Position update (throttled to ~2Hz)
        let now = Instant::now();
        if current_state == PlaybackState::Playing
            && now.duration_since( self.last_position_broadcast ) >= Duration::from_millis( 500 )
        {
            let position = self.player.position();
            let _ = self.broadcast_tx.send( StateUpdate::Position {
                secs: position.as_secs_f64(),
            });
            self.last_position_broadcast = now;
        }

        // Visualizer data (throttled to ~10Hz)
        if current_state == PlaybackState::Playing
            && now.duration_since( self.last_vis_broadcast ) >= Duration::from_millis( 100 )
        {
            if let Some( vis ) = self.player.vis_data() {
                let _ = self.broadcast_tx.send( StateUpdate::VisualizerData {
                    bars: vis.to_vec(),
                });
                self.last_vis_broadcast = now;
            }
        }
    }


    /// Handles a single command from any frontend.
    fn handle_command( &mut self, cmd: AppCommand ) {
        match cmd {
            // Playback
            AppCommand::Play => {
                self.play_selected();
            }
            AppCommand::Pause => {
                let _ = self.player.pause();
            }
            AppCommand::Resume => {
                let _ = self.player.resume();
            }
            AppCommand::TogglePlayback => {
                match self.player.state() {
                    PlaybackState::Playing => { let _ = self.player.pause(); }
                    PlaybackState::Paused => { let _ = self.player.resume(); }
                    PlaybackState::Stopped => { self.play_selected(); }
                }
            }
            AppCommand::Stop => {
                let _ = self.player.stop();
            }
            AppCommand::Next => {
                self.play_next();
            }
            AppCommand::Previous => {
                self.play_previous();
            }
            AppCommand::Seek { position_secs } => {
                let _ = self.player.seek( Duration::from_secs_f64( position_secs ) );
            }
            AppCommand::SetVolume { level } => {
                self.player.set_volume( level.clamp( 0.0, 1.5 ) );
            }
            AppCommand::VolumeUp => {
                let vol = ( self.player.volume() + 0.05 ).min( 1.5 );
                self.player.set_volume( vol );
            }
            AppCommand::VolumeDown => {
                let vol = ( self.player.volume() - 0.05 ).max( 0.0 );
                self.player.set_volume( vol );
            }

            // Playlist
            AppCommand::PlayTrack { index } => {
                let track = {
                    let playlist_arc = self.player.playlist();
                    let mut playlist = playlist_arc.write().unwrap();
                    playlist.jump_to( index ).cloned()
                };
                if let Some( path ) = track {
                    let _ = self.player.play( path );
                }
                self.broadcast_playlist();
            }
            AppCommand::AddPath { path } => {
                let path = PathBuf::from( &path );
                if path.is_dir() {
                    let mut scanner = LibraryScanner::new();
                    scanner.add_root( path );
                    if let Ok( tracks ) = scanner.scan() {
                        let playlist_arc = self.player.playlist();
                        let mut playlist = playlist_arc.write().unwrap();
                        playlist.add_many( tracks.into_iter().map( |t| t.path ) );
                    }
                } else {
                    let playlist_arc = self.player.playlist();
                    let mut playlist = playlist_arc.write().unwrap();
                    playlist.add( path );
                }
                self.broadcast_playlist();
            }
            AppCommand::RemoveTrack { index } => {
                let playlist_arc = self.player.playlist();
                let mut playlist = playlist_arc.write().unwrap();
                playlist.remove( index );
                drop( playlist );
                self.broadcast_playlist();
            }
            AppCommand::ClearPlaylist => {
                let _ = self.player.stop();
                let playlist_arc = self.player.playlist();
                let mut playlist = playlist_arc.write().unwrap();
                playlist.clear();
                drop( playlist );
                self.broadcast_playlist();
            }
            AppCommand::ToggleShuffle => {
                let playlist_arc = self.player.playlist();
                let mut playlist = playlist_arc.write().unwrap();
                let new_val = !playlist.shuffle();
                playlist.set_shuffle( new_val );
            }
            AppCommand::SetRepeat { mode } => {
                let repeat = match mode {
                    RepeatModeValue::Off => RepeatMode::Off,
                    RepeatModeValue::One => RepeatMode::One,
                    RepeatModeValue::All => RepeatMode::All,
                };
                let playlist_arc = self.player.playlist();
                let mut playlist = playlist_arc.write().unwrap();
                playlist.set_repeat( repeat );
            }
            AppCommand::CycleRepeat => {
                let playlist_arc = self.player.playlist();
                let mut playlist = playlist_arc.write().unwrap();
                let next = match playlist.repeat() {
                    RepeatMode::Off => RepeatMode::One,
                    RepeatMode::One => RepeatMode::All,
                    RepeatMode::All => RepeatMode::Off,
                };
                playlist.set_repeat( next );
            }
            AppCommand::MoveTrack { from, to } => {
                let playlist_arc = self.player.playlist();
                let mut playlist = playlist_arc.write().unwrap();
                playlist.move_track( from, to );
                drop( playlist );
                self.broadcast_playlist();
            }
            AppCommand::Dedup => {
                let playlist_arc = self.player.playlist();
                let mut playlist = playlist_arc.write().unwrap();
                let removed = playlist.dedup();
                drop( playlist );
                if removed > 0 {
                    self.broadcast_playlist();
                    let _ = self.broadcast_tx.send( StateUpdate::StatusMessage {
                        message: format!( "Removed {} duplicate(s)", removed ),
                    });
                }
            }
            AppCommand::SavePlaylist { name } => {
                if let Some( dir ) = Playlist::ensure_playlist_dir() {
                    let path = dir.join( format!( "{}.m3u", name ) );
                    let playlist_arc = self.player.playlist();
                    let playlist = playlist_arc.read().unwrap();
                    match playlist.save( &path ) {
                        Ok(()) => {
                            let _ = self.broadcast_tx.send( StateUpdate::StatusMessage {
                                message: format!( "Saved playlist: {}", name ),
                            });
                        }
                        Err( e ) => {
                            let _ = self.broadcast_tx.send( StateUpdate::StatusMessage {
                                message: format!( "Save error: {}", e ),
                            });
                        }
                    }
                }
            }
            AppCommand::LoadPlaylist { name } => {
                if let Some( dir ) = Playlist::playlist_dir() {
                    let path = dir.join( format!( "{}.m3u", name ) );
                    match Playlist::load( &path ) {
                        Ok( loaded ) => {
                            let _ = self.player.stop();
                            let playlist_arc = self.player.playlist();
                            let mut playlist = playlist_arc.write().unwrap();
                            *playlist = loaded;
                            drop( playlist );
                            self.broadcast_playlist();
                            let _ = self.broadcast_tx.send( StateUpdate::StatusMessage {
                                message: format!( "Loaded playlist: {}", name ),
                            });
                        }
                        Err( e ) => {
                            let _ = self.broadcast_tx.send( StateUpdate::StatusMessage {
                                message: format!( "Load error: {}", e ),
                            });
                        }
                    }
                }
            }

            // Browser
            AppCommand::BrowseTo { path } => {
                self.browser.navigate_to( &PathBuf::from( &path ) );
                let _ = self.broadcast_tx.send( StateUpdate::BrowserChanged {
                    browser: self.browser.to_snapshot(),
                });
            }
            AppCommand::BrowseUp => {
                if let Some( parent ) = self.browser.current_dir.parent() {
                    let parent = parent.to_path_buf();
                    self.browser.navigate_to( &parent );
                    let _ = self.broadcast_tx.send( StateUpdate::BrowserChanged {
                        browser: self.browser.to_snapshot(),
                    });
                }
            }
            AppCommand::BrowseHome => {
                if let Some( home ) = dirs::home_dir() {
                    self.browser.navigate_to( &home );
                    let _ = self.broadcast_tx.send( StateUpdate::BrowserChanged {
                        browser: self.browser.to_snapshot(),
                    });
                }
            }
            AppCommand::BrowseOpen { index } => {
                if let Some( entry ) = self.browser.entries.get( index ).cloned() {
                    if entry.is_dir {
                        self.browser.navigate_to( &entry.path );
                        let _ = self.broadcast_tx.send( StateUpdate::BrowserChanged {
                            browser: self.browser.to_snapshot(),
                        });
                    }
                }
            }
            AppCommand::BrowseAddToPlaylist { index } => {
                if let Some( entry ) = self.browser.entries.get( index ).cloned() {
                    let path_str = entry.path.to_string_lossy().to_string();
                    self.handle_command( AppCommand::AddPath { path: path_str } );
                }
            }

            // Playlist management
            AppCommand::ListPlaylists => {
                if let Some( dir ) = Playlist::playlist_dir() {
                    let mut names = Vec::new();
                    if let Ok( entries ) = std::fs::read_dir( &dir ) {
                        for entry in entries.flatten() {
                            let path = entry.path();
                            if path.extension().and_then( |e| e.to_str() ) == Some( "m3u" ) {
                                if let Some( name ) = path.file_stem().and_then( |s| s.to_str() ) {
                                    if name != "_last" {
                                        names.push( name.to_string() );
                                    }
                                }
                            }
                        }
                    }
                    names.sort();
                    let msg = if names.is_empty() {
                        "No saved playlists".to_string()
                    } else {
                        format!( "Playlists: {}", names.join( ", " ) )
                    };
                    let _ = self.broadcast_tx.send( StateUpdate::StatusMessage { message: msg } );
                }
            }
            AppCommand::DeletePlaylist { name } => {
                if let Some( dir ) = Playlist::playlist_dir() {
                    let path = dir.join( format!( "{}.m3u", name ) );
                    if path.exists() {
                        match std::fs::remove_file( &path ) {
                            Ok(()) => {
                                let _ = self.broadcast_tx.send( StateUpdate::StatusMessage {
                                    message: format!( "Deleted playlist: {}", name ),
                                });
                            }
                            Err( e ) => {
                                let _ = self.broadcast_tx.send( StateUpdate::StatusMessage {
                                    message: format!( "Delete error: {}", e ),
                                });
                            }
                        }
                    } else {
                        let _ = self.broadcast_tx.send( StateUpdate::StatusMessage {
                            message: format!( "Playlist not found: {}", name ),
                        });
                    }
                }
            }

            // Settings
            AppCommand::ToggleSetting { key } => {
                match key.as_str() {
                    "discord_enabled" => {
                        self.settings.discord_enabled = !self.settings.discord_enabled;
                    }
                    "smtc_enabled" => {
                        self.settings.smtc_enabled = !self.settings.smtc_enabled;
                    }
                    "web_enabled" => {
                        self.settings.web_enabled = !self.settings.web_enabled;
                    }
                    _ => {
                        tracing::warn!( "Unknown setting key: {}", key );
                        return;
                    }
                }
                self.settings.save();
                let _ = self.broadcast_tx.send( StateUpdate::SettingsChanged {
                    settings: self.settings.to_snapshot(),
                });
            }

            // View mode
            AppCommand::SetView { view } => {
                self.view_mode = view;
            }

            // Full state request
            AppCommand::RequestFullState => {
                let snapshot = self.build_full_snapshot();
                let _ = self.broadcast_tx.send( StateUpdate::FullState { state: snapshot } );
            }

            // Quit is handled in run() before reaching here
            AppCommand::Quit => {}
        }
    }


    /// Plays the track at the current playlist index.
    fn play_selected( &self ) {
        let track = {
            let playlist_arc = self.player.playlist();
            let playlist = playlist_arc.read().unwrap();
            playlist.current().cloned()
                .or_else( || {
                    if !playlist.is_empty() {
                        Some( playlist.tracks()[ 0 ].clone() )
                    } else {
                        None
                    }
                })
        };

        if let Some( path ) = track {
            {
                let playlist_arc = self.player.playlist();
                let mut playlist = playlist_arc.write().unwrap();
                if playlist.current_index().is_none() && !playlist.is_empty() {
                    playlist.jump_to( 0 );
                }
            }
            let _ = self.player.play( path );
        }
    }


    /// Advances to the next track.
    fn play_next( &self ) {
        match self.player.play_next() {
            Ok( true ) => {
                self.broadcast_playlist();
            }
            Ok( false ) => {
                let _ = self.broadcast_tx.send( StateUpdate::StatusMessage {
                    message: "End of playlist".to_string(),
                });
            }
            Err( e ) => {
                let _ = self.broadcast_tx.send( StateUpdate::StatusMessage {
                    message: format!( "Next track error: {}", e ),
                });
            }
        }
    }


    /// Goes to the previous track.
    fn play_previous( &self ) {
        match self.player.play_previous() {
            Ok( true ) => {
                self.broadcast_playlist();
            }
            Ok( false ) => {}
            Err( e ) => {
                let _ = self.broadcast_tx.send( StateUpdate::StatusMessage {
                    message: format!( "Previous track error: {}", e ),
                });
            }
        }
    }


    /// Broadcasts current playlist state.
    fn broadcast_playlist( &self ) {
        let playlist_arc = self.player.playlist();
        let playlist = playlist_arc.read().unwrap();
        let entries = build_track_entries( &playlist );
        let index = playlist.current_index();
        drop( playlist );

        let _ = self.broadcast_tx.send( StateUpdate::PlaylistChanged {
            playlist: entries,
            index,
        });
    }


    /// Builds a full state snapshot.
    fn build_full_snapshot( &self ) -> StateSnapshot {
        let playlist_arc = self.player.playlist();
        let playlist = playlist_arc.read().unwrap();
        let entries = build_track_entries( &playlist );
        let playlist_index = playlist.current_index();
        let shuffle = playlist.shuffle();
        let repeat = playlist.repeat();
        drop( playlist );

        let current_track = self.player.current_track().map( |path| {
            self.build_track_info( &path )
        });

        StateSnapshot {
            playback_state: playback_state_to_value( self.player.state() ),
            current_track,
            position_secs: self.player.position().as_secs_f64(),
            duration_secs: self.player.duration().map( |d| d.as_secs_f64() ),
            volume: self.player.volume(),
            playlist: entries,
            playlist_index,
            shuffle,
            repeat_mode: repeat_mode_to_value( repeat ),
            view_mode: self.view_mode.clone(),
            visualizer_data: self.player.vis_data().map( |v| v.to_vec() ),
            browser: Some( self.browser.to_snapshot() ),
            settings: self.settings.to_snapshot(),
        }
    }


    /// Builds track info from a path + player metadata.
    fn build_track_info( &self, path: &PathBuf ) -> TrackInfo {
        let metadata = self.player.metadata();
        let duration = self.player.duration();

        // Find and cache cover art for web UI
        let cover_art = find_cover_art( path );
        let has_cover_art = cover_art.is_some();
        if let Ok( mut guard ) = self.cover_art_path.lock() {
            *guard = cover_art;
        }

        TrackInfo {
            path: path.to_string_lossy().to_string(),
            title: metadata.as_ref().and_then( |m| m.title.clone() ),
            artist: metadata.as_ref().and_then( |m| m.artist.clone() ),
            album: metadata.as_ref().and_then( |m| m.album.clone() ),
            album_artist: metadata.as_ref().and_then( |m| m.album_artist.clone() ),
            track_number: metadata.as_ref().and_then( |m| m.track_number ),
            genre: metadata.as_ref().and_then( |m| m.genre.clone() ),
            year: metadata.as_ref().and_then( |m| m.year ),
            codec: metadata.as_ref().and_then( |m| m.codec.clone() ),
            bitrate: metadata.as_ref().and_then( |m| m.bitrate ),
            sample_rate: metadata.as_ref().and_then( |m| m.sample_rate ),
            channels: metadata.as_ref().and_then( |m| m.channels ),
            duration_secs: duration.map( |d| d.as_secs_f64() ),
            has_cover_art,
        }
    }


    /// Saves the current session state.
    fn save_session( &self ) {
        let playlist_arc = self.player.playlist();
        let playlist = playlist_arc.read().unwrap();

        if playlist.is_empty() {
            return;
        }

        if let Some( dir ) = Playlist::ensure_playlist_dir() {
            let path = dir.join( "_last.m3u" );
            let _ = playlist.save( &path );

            let session = oxidio_core::SessionState {
                playlist_name: "_last".to_string(),
                track_index: playlist.current_index(),
                shuffle: playlist.shuffle(),
                repeat: playlist.repeat(),
                volume: self.player.volume(),
            };
            let _ = Playlist::save_session( &session );
        }
    }


    /// Gets a reference to the current settings.
    pub fn settings( &self ) -> &ProcessorSettings {
        &self.settings
    }
}


/// Searches for album art image files in the same directory as the track.
///
/// Looks for common cover art filenames (cover, folder, album, front, etc.)
/// with image extensions (jpg, jpeg, png, bmp, gif). Prioritizes exact name
/// matches over generic image files.
///
/// @param track_path - Path to the audio file
///
/// @returns Path to the cover art file, or None if not found
fn find_cover_art( track_path: &Path ) -> Option<PathBuf> {
    let parent = track_path.parent()?;

    let art_names = [
        "cover", "folder", "album", "front", "art", "albumart", "album_art",
    ];
    let extensions = [ "jpg", "jpeg", "png", "bmp", "gif" ];

    let mut found_path: Option<PathBuf> = None;

    match std::fs::read_dir( parent ) {
        Ok( entries ) => {
            for entry in entries.flatten() {
                let path = entry.path();
                let filename = path.file_stem()
                    .and_then( |s| s.to_str() )
                    .map( |s| s.to_lowercase() );
                let ext = path.extension()
                    .and_then( |e| e.to_str() )
                    .map( |e| e.to_lowercase() );

                if let ( Some( name ), Some( ext ) ) = ( filename, ext ) {
                    if extensions.contains( &ext.as_str() ) {
                        if art_names.contains( &name.as_str() ) {
                            found_path = Some( path );
                            break;
                        }
                        if found_path.is_none() {
                            found_path = Some( path );
                        }
                    }
                }
            }
        }
        Err( e ) => {
            tracing::warn!( "Failed to read directory {:?}: {}", parent, e );
        }
    }

    found_path
}


/// Helper: build track entries from a playlist.
fn build_track_entries( playlist: &Playlist ) -> Vec<TrackEntry> {
    playlist.tracks().iter().enumerate().map( |( i, path )| {
        let display_name = path.file_stem()
            .map( |s| s.to_string_lossy().to_string() )
            .unwrap_or_else( || path.to_string_lossy().to_string() );
        TrackEntry {
            index: i,
            path: path.to_string_lossy().to_string(),
            display_name,
        }
    }).collect()
}


/// Convert core PlaybackState to protocol value.
fn playback_state_to_value( state: PlaybackState ) -> PlaybackStateValue {
    match state {
        PlaybackState::Stopped => PlaybackStateValue::Stopped,
        PlaybackState::Playing => PlaybackStateValue::Playing,
        PlaybackState::Paused => PlaybackStateValue::Paused,
    }
}


/// Convert core RepeatMode to protocol value.
fn repeat_mode_to_value( mode: RepeatMode ) -> RepeatModeValue {
    match mode {
        RepeatMode::Off => RepeatModeValue::Off,
        RepeatMode::One => RepeatModeValue::One,
        RepeatMode::All => RepeatModeValue::All,
    }
}
