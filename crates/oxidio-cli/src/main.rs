//! Oxidio CLI - Terminal UI music player

mod browser;
mod cli;
mod discord;
mod input;
mod media_controls;
mod settings;
mod view;

use std::io;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use crossterm::{
    event::{ self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind },
    terminal::{ disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen },
    ExecutableCommand,
};
use ratatui::{
    layout::Alignment,
    prelude::*,
    widgets::{ Block, Borders, List, ListItem, ListState, Paragraph, Wrap },
};
#[cfg( target_os = "windows" )]
use souvlaki::{ MediaMetadata, MediaPlayback };

use browser::FileBrowser;
use cli::Args;
use input::{ InputBuffer, InputMode };
use media_controls::{ create_media_controls_channel, MediaControlCommand, MediaControlsHandler };
use view::{ ViewMode, VisualizerStyle };

use oxidio_core::{
    command::{ self, RepeatModeArg },
    library::LibraryScanner,
    player::PlaybackState,
    Command, Player, RepeatMode,
};


/// Converts a file path to a file:// URL for SMTC album art.
#[cfg( target_os = "windows" )]
fn path_to_file_url( path: &std::path::Path ) -> Option<String> {
    // Get absolute path
    let abs_path = path.canonicalize().ok()?;
    let path_str = abs_path.to_string_lossy();

    // Remove the \\?\ prefix that canonicalize adds on Windows
    let clean_path = path_str.strip_prefix( r"\\?\" ).unwrap_or( &path_str );

    // Souvlaki on Windows strips "file://" and passes the rest to StorageFile::GetFileFromPathAsync
    // So we need: file://C:/path (NOT file:///C:/path) to get C:/path after stripping
    // Or just pass the raw path and let souvlaki handle it
    let url = format!( "file://{}", clean_path );
    tracing::debug!( "Generated cover URL: {}", url );
    Some( url )
}


/// Application state.
struct App {
    player: Player,
    should_quit: bool,

    // View state
    view_mode: ViewMode,
    playlist_state: ListState,
    browser: FileBrowser,

    // Input state
    input_mode: InputMode,
    input_buffer: InputBuffer,

    // Edit mode
    edit_mode: bool,

    // Visualizer style
    visualizer_style: VisualizerStyle,

    // Volume (0.0 to 1.0)
    volume: f32,

    // Flag to scroll to playing track without changing selection
    scroll_to_playing: bool,

    // Mouse click tracking for double-click detection
    last_click_time: Option<std::time::Instant>,
    last_click_row: Option<u16>,

    // Store playlist area for mouse hit detection
    playlist_area: Option<Rect>,

    // Help view scroll offset
    help_scroll: u16,

    // Status message (shown in status bar)
    status_message: Option<String>,
    status_clear_at: Option<std::time::Instant>,

    // Media controls (SMTC/MPRIS)
    media_controls: Option<MediaControlsHandler>,
    media_controls_rx: mpsc::Receiver<MediaControlCommand>,
    last_smtc_state: Option<PlaybackState>,
    last_smtc_track: Option<PathBuf>,
    /// Force SMTC metadata update on next tick
    force_smtc_update: bool,

    // Discord Rich Presence
    discord: discord::DiscordPresence,
    last_discord_track: Option<PathBuf>,

    // Settings
    settings: settings::Settings,
    settings_selected: usize,
}


impl App {
    /// Creates a new App instance.
    fn new( args: &Args ) -> Result<Self> {
        let player = Player::new()?;

        // Determine starting directory for browser
        let start_path = args.path.clone()
            .or_else( || dirs::home_dir() )
            .unwrap_or_else( || PathBuf::from( "." ) );

        let browser = FileBrowser::new( start_path )?;

        // Determine starting view
        let view_mode = if args.browse {
            ViewMode::Browser
        } else {
            ViewMode::Playlist
        };

        let mut initial_track_index: Option<usize> = None;
        let mut initial_volume: f32 = 1.0;

        // Add any files passed on command line to playlist
        if !args.files.is_empty() {
            let playlist_arc = player.playlist();
            let mut playlist = playlist_arc.write().unwrap();
            for file in &args.files {
                if file.is_dir() {
                    let mut scanner = LibraryScanner::new();
                    scanner.add_root( file.clone() );
                    if let Ok( tracks ) = scanner.scan() {
                        playlist.add_many( tracks.into_iter().map( |t| t.path ) );
                    }
                } else {
                    playlist.add( file.clone() );
                }
            }
        } else {
            // Try to load last session
            if let Some( session ) = oxidio_core::Playlist::load_session() {
                if let Some( dir ) = oxidio_core::Playlist::playlist_dir() {
                    let path = dir.join( format!( "{}.m3u", session.playlist_name ) );
                    if let Ok( loaded ) = oxidio_core::Playlist::load( &path ) {
                        let playlist_arc = player.playlist();
                        let mut playlist = playlist_arc.write().unwrap();
                        *playlist = loaded;
                        playlist.set_shuffle( session.shuffle );
                        playlist.set_repeat( session.repeat );
                        initial_track_index = session.track_index;
                        initial_volume = session.volume;
                        tracing::info!(
                            "Restored session: {}, track {}, shuffle={}, repeat={:?}, volume={}",
                            session.playlist_name,
                            session.track_index.unwrap_or( 0 ),
                            session.shuffle,
                            session.repeat,
                            session.volume
                        );
                    }
                }
            }
        }

        // Initialize media controls (SMTC on Windows, MPRIS on Linux)
        let ( media_controls_tx, media_controls_rx ) = create_media_controls_channel();
        let media_controls = MediaControlsHandler::new( media_controls_tx );

        if media_controls.is_some() {
            tracing::info!( "System media controls initialized" );
        }

        // Apply initial volume to player
        player.set_volume( initial_volume );

        let mut playlist_state = ListState::default();
        if initial_track_index.is_some() {
            playlist_state.select( initial_track_index );
        }

        Ok( Self {
            player,
            should_quit: false,
            view_mode,
            playlist_state,
            browser,
            input_mode: InputMode::Normal,
            input_buffer: InputBuffer::new(),
            edit_mode: false,
            visualizer_style: VisualizerStyle::default(),
            volume: initial_volume,
            scroll_to_playing: false,
            last_click_time: None,
            last_click_row: None,
            playlist_area: None,
            help_scroll: 0,
            status_message: None,
            status_clear_at: None,
            media_controls,
            media_controls_rx,
            last_smtc_state: None,
            last_smtc_track: None,
            force_smtc_update: false,
            discord: discord::DiscordPresence::new(),
            last_discord_track: None,
            settings: settings::Settings::load(),
            settings_selected: 0,
        })
    }


    /// Sets a status message that auto-clears after a delay.
    fn set_status( &mut self, msg: impl Into<String> ) {
        self.status_message = Some( msg.into() );
        self.status_clear_at = Some( std::time::Instant::now() + Duration::from_secs( 3 ) );
    }


    /// Updates app state (clears expired messages, auto-advances tracks, handles media controls).
    fn tick( &mut self ) {
        // Clear expired status messages
        if let Some( clear_at ) = self.status_clear_at {
            if std::time::Instant::now() >= clear_at {
                self.status_message = None;
                self.status_clear_at = None;
            }
        }

        // Auto-advance to next track when current track ends
        if self.player.track_ended() {
            match self.player.play_next() {
                Ok( true ) => {
                    // Successfully started next track - scroll to it without changing selection
                    self.scroll_to_playing = true;
                    self.force_smtc_update = true;
                }
                Ok( false ) => {
                    // No more tracks in playlist
                }
                Err( e ) => {
                    self.set_status( format!( "Auto-advance error: {}", e ) );
                }
            }
        }

        // Handle media control events (SMTC/MPRIS) - only if enabled
        while let Ok( cmd ) = self.media_controls_rx.try_recv() {
            if !self.settings.smtc_enabled {
                continue; // Ignore SMTC commands when disabled
            }
            match cmd {
                MediaControlCommand::Play => {
                    if self.player.state() == PlaybackState::Paused {
                        let _ = self.player.resume();
                    } else if self.player.state() == PlaybackState::Stopped {
                        self.play_selected();
                    }
                }
                MediaControlCommand::Pause => {
                    let _ = self.player.pause();
                }
                MediaControlCommand::Toggle => {
                    match self.player.state() {
                        PlaybackState::Playing => { let _ = self.player.pause(); }
                        PlaybackState::Paused => { let _ = self.player.resume(); }
                        PlaybackState::Stopped => { self.play_selected(); }
                    }
                }
                MediaControlCommand::Stop => {
                    let _ = self.player.stop();
                }
                MediaControlCommand::Next => {
                    self.play_next();
                }
                MediaControlCommand::Previous => {
                    self.play_previous();
                }
            }
        }

        // Update SMTC state if changed
        self.update_media_controls();

        // Update Discord Rich Presence
        self.update_discord();
    }


    /// Updates the system media controls with current playback state and metadata.
    #[cfg( target_os = "windows" )]
    fn update_media_controls( &mut self ) {
        // Check if SMTC is enabled in settings
        if !self.settings.smtc_enabled {
            return;
        }

        let controls = match self.media_controls.as_mut() {
            Some( c ) => c,
            None => return,
        };

        let current_state = self.player.state();
        let current_track = self.player.current_track();

        // Update playback state if changed
        if self.last_smtc_state != Some( current_state ) {
            let playback = match current_state {
                PlaybackState::Playing => MediaPlayback::Playing { progress: None },
                PlaybackState::Paused => MediaPlayback::Paused { progress: None },
                PlaybackState::Stopped => MediaPlayback::Stopped,
            };
            controls.set_playback( playback );
            self.last_smtc_state = Some( current_state );
        }

        // Update metadata if track changed or forced
        let should_update = self.force_smtc_update || self.last_smtc_track != current_track;
        if should_update {
            self.force_smtc_update = false;

            if let Some( ref track_path ) = current_track {
                let metadata = self.player.metadata();

                let title = metadata.as_ref()
                    .and_then( |m| m.title.clone() )
                    .or_else( || {
                        track_path.file_stem()
                            .map( |n| n.to_string_lossy().to_string() )
                    });

                let artist = metadata.as_ref().and_then( |m| m.artist.clone() );
                let album = metadata.as_ref().and_then( |m| m.album.clone() );

                // Find album art and copy to temp (SMTC can't access network paths)
                // Only use cover_url if the file was successfully copied to temp
                let cover_url = Self::find_album_art( track_path ).filter( |_| {
                    // Verify temp cover file exists
                    let temp_path = std::env::temp_dir().join( "oxidio" );
                    temp_path.read_dir()
                        .map( |mut entries| entries.any( |e| {
                            e.map( |e| e.file_name().to_string_lossy().starts_with( "cover." ) )
                                .unwrap_or( false )
                        }))
                        .unwrap_or( false )
                });

                // Debug: show what we're sending to SMTC
                tracing::debug!(
                    "SMTC update: title={:?}, artist={:?}, album={:?}, cover_url={:?}",
                    title, artist, album, cover_url
                );

                if let Some( e ) = controls.set_metadata( MediaMetadata {
                    title: title.as_deref(),
                    artist: artist.as_deref(),
                    album: album.as_deref(),
                    cover_url: cover_url.as_deref(),
                    duration: self.player.duration(),
                }) {
                    tracing::warn!( "SMTC error: {}", e );
                }
            }
            self.last_smtc_track = current_track;
        }
    }


    /// Finds album art in the same folder as the track.
    /// Returns a file:// URL if found.
    #[cfg( target_os = "windows" )]
    fn find_album_art( track_path: &std::path::Path ) -> Option<String> {
        let parent = track_path.parent()?;

        // Common album art filenames (case-insensitive search)
        let art_names = [
            "cover", "folder", "album", "front", "art", "albumart", "album_art",
        ];
        let extensions = [ "jpg", "jpeg", "png", "bmp", "gif" ];

        tracing::debug!( "Looking for album art in: {:?}", parent );

        let mut found_path: Option<std::path::PathBuf> = None;

        // Read directory and do case-insensitive matching (works for UNC paths to Linux NAS)
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
                        // Check if it's an image with a preferred art name
                        if extensions.contains( &ext.as_str() ) {
                            if art_names.contains( &name.as_str() ) {
                                tracing::debug!( "Found album art: {:?}", path );
                                found_path = Some( path );
                                break;
                            }
                            // Remember any image file as fallback
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

        if found_path.is_none() {
            tracing::debug!( "No album art found in {:?}", parent );
        }

        // If we found album art, copy it to temp and return the local path
        let source_path = found_path?;
        Self::copy_to_temp_and_get_url( &source_path )
    }


    /// Copies a file to the temp directory and returns a file:// URL to the copy.
    /// This is needed because SMTC can't access network paths directly.
    #[cfg( target_os = "windows" )]
    fn copy_to_temp_and_get_url( source: &std::path::Path ) -> Option<String> {
        use std::os::windows::fs::MetadataExt;

        let temp_dir = std::env::temp_dir();
        let oxidio_temp = temp_dir.join( "oxidio" );

        tracing::debug!( "Attempting to copy album art from: {:?}", source );

        // Create oxidio temp dir if it doesn't exist
        if !oxidio_temp.exists() {
            if let Err( e ) = std::fs::create_dir_all( &oxidio_temp ) {
                tracing::warn!( "Failed to create temp dir {:?}: {}", oxidio_temp, e );
                return None;
            }
        }

        // Use a fixed filename so we overwrite the old cover each time
        let ext = source.extension().and_then( |e| e.to_str() ).unwrap_or( "jpg" );
        let dest_path = oxidio_temp.join( format!( "cover.{}", ext ) );

        // Remove existing file first to avoid permission issues
        if dest_path.exists() {
            // Clear any read-only attribute before removing
            if let Ok( metadata ) = std::fs::metadata( &dest_path ) {
                let attrs = metadata.file_attributes();
                // FILE_ATTRIBUTE_READONLY = 0x1
                if attrs & 0x1 != 0 {
                    let mut perms = metadata.permissions();
                    perms.set_readonly( false );
                    let _ = std::fs::set_permissions( &dest_path, perms );
                }
            }
            if let Err( e ) = std::fs::remove_file( &dest_path ) {
                tracing::warn!( "Failed to remove old cover file {:?}: {}", dest_path, e );
            }
        }

        // Copy the file
        match std::fs::copy( source, &dest_path ) {
            Ok( bytes ) => {
                tracing::debug!( "Copied {} bytes to {:?}", bytes, dest_path );
            }
            Err( e ) => {
                tracing::warn!( "Failed to copy album art from {:?} to {:?}: {}", source, dest_path, e );
                return None;
            }
        }

        // Clear file attributes (hidden, archive, system, read-only) so SMTC can access it
        {
            use std::os::windows::ffi::OsStrExt;
            use windows::Win32::Storage::FileSystem::{ SetFileAttributesW, FILE_ATTRIBUTE_NORMAL };
            use windows::core::PCWSTR;

            let wide_path: Vec<u16> = dest_path.as_os_str()
                .encode_wide()
                .chain( std::iter::once( 0 ) )
                .collect();

            unsafe {
                if SetFileAttributesW( PCWSTR( wide_path.as_ptr() ), FILE_ATTRIBUTE_NORMAL ).is_err() {
                    tracing::debug!( "Failed to clear file attributes on {:?}", dest_path );
                }
            }
        }

        path_to_file_url( &dest_path )
    }


    /// Stub for non-Windows.
    #[cfg( not( target_os = "windows" ) )]
    fn copy_to_temp_and_get_url( _source: &std::path::Path ) -> Option<String> {
        None
    }


    /// Stub for non-Windows platforms.
    #[cfg( not( target_os = "windows" ) )]
    fn update_media_controls( &mut self ) {
        // Media controls not available on this platform
    }


    /// Updates Discord Rich Presence with current track info.
    fn update_discord( &mut self ) {
        // Check if Discord is enabled in settings
        if !self.settings.discord_enabled {
            if self.last_discord_track.is_some() {
                self.discord.clear();
                self.last_discord_track = None;
            }
            return;
        }

        let current_track = self.player.current_track();
        let current_state = self.player.state();

        // Clear presence if stopped or paused
        if current_state != PlaybackState::Playing {
            if self.last_discord_track.is_some() {
                self.discord.clear();
                self.last_discord_track = None;
            }
            return;
        }

        // Update if track changed
        if self.last_discord_track != current_track {
            if let Some( ref track_path ) = current_track {
                let metadata = self.player.metadata();

                let title = metadata.as_ref()
                    .and_then( |m| m.title.clone() )
                    .or_else( || {
                        track_path.file_stem()
                            .map( |n| n.to_string_lossy().to_string() )
                    });

                let artist = metadata.as_ref().and_then( |m| m.artist.clone() );
                let album = metadata.as_ref().and_then( |m| m.album.clone() );

                self.discord.update(
                    title.as_deref(),
                    artist.as_deref(),
                    album.as_deref(),
                );
            }
            self.last_discord_track = current_track;
        }
    }


    /// Handles a key event.
    fn handle_key( &mut self, code: KeyCode, modifiers: KeyModifiers ) {
        match self.input_mode {
            InputMode::Normal => self.handle_normal_key( code, modifiers ),
            InputMode::Command => self.handle_command_key( code ),
            InputMode::Search => self.handle_search_key( code ),
        }
    }


    /// Handles mouse events.
    fn handle_mouse( &mut self, column: u16, row: u16, kind: MouseEventKind ) {
        match kind {
            MouseEventKind::Down( crossterm::event::MouseButton::Left ) => {
                // Check if click is within the playlist area
                if self.view_mode == ViewMode::Playlist {
                    if let Some( area ) = self.playlist_area {
                        // Check if click is within the playlist (inside borders)
                        if column > area.x && column < area.x + area.width - 1
                            && row > area.y && row < area.y + area.height - 1
                        {
                            // Calculate which item was clicked
                            let offset = self.playlist_state.offset();
                            let clicked_idx = offset + ( row - area.y - 1 ) as usize;

                            let playlist = self.player.playlist();
                            let playlist_len = playlist.read().unwrap().len();

                            if clicked_idx < playlist_len {
                                let now = std::time::Instant::now();
                                let is_double_click = self.last_click_time
                                    .map( |t| now.duration_since( t ) < Duration::from_millis( 400 ) )
                                    .unwrap_or( false )
                                    && self.last_click_row == Some( row );

                                if is_double_click {
                                    // Double-click: select and play
                                    self.playlist_state.select( Some( clicked_idx ) );
                                    self.play_selected();
                                    self.last_click_time = None;
                                    self.last_click_row = None;
                                } else {
                                    // Single click: select
                                    self.playlist_state.select( Some( clicked_idx ) );
                                    self.last_click_time = Some( now );
                                    self.last_click_row = Some( row );
                                }
                            }
                        }
                    }
                }
            }
            MouseEventKind::ScrollUp => {
                // Scroll playlist up
                if self.view_mode == ViewMode::Playlist {
                    self.playlist_select_previous();
                }
            }
            MouseEventKind::ScrollDown => {
                // Scroll playlist down
                if self.view_mode == ViewMode::Playlist {
                    self.playlist_select_next();
                }
            }
            _ => {}
        }
    }


    fn handle_normal_key( &mut self, code: KeyCode, modifiers: KeyModifiers ) {
        // Global keys (work in any view)
        match code {
            KeyCode::Char( '/' ) => {
                self.input_mode = InputMode::Command;
                self.input_buffer.clear();
                return;
            }
            KeyCode::Tab => {
                self.view_mode = self.view_mode.next_tab();
                return;
            }
            KeyCode::BackTab => {
                // Shift+Tab goes to previous view
                self.view_mode = self.view_mode.prev_tab();
                return;
            }
            KeyCode::Char( '?' ) => {
                self.view_mode = ViewMode::Help;
                return;
            }
            KeyCode::Esc => {
                if self.view_mode == ViewMode::Help || self.view_mode == ViewMode::TrackInfo || self.view_mode == ViewMode::Visualizer {
                    self.view_mode = ViewMode::Playlist;
                    return;
                }
                if self.edit_mode {
                    self.edit_mode = false;
                    self.set_status( "Edit mode off" );
                    return;
                }
            }
            _ => {}
        }

        // View-specific keys
        match self.view_mode {
            ViewMode::Playlist => self.handle_playlist_key( code, modifiers ),
            ViewMode::Browser => self.handle_browser_key( code ),
            ViewMode::Help => self.handle_help_key( code ),
            ViewMode::TrackInfo => self.handle_track_info_key( code, modifiers ),
            ViewMode::Visualizer => self.handle_visualizer_key( code, modifiers ),
            ViewMode::Settings => self.handle_settings_key( code ),
        }
    }


    fn handle_playlist_key( &mut self, code: KeyCode, modifiers: KeyModifiers ) {
        match code {
            KeyCode::Char( 'q' ) => {
                self.should_quit = true;
            }
            KeyCode::Char( ' ' ) => {
                // Toggle play/pause
                match self.player.state() {
                    PlaybackState::Playing => {
                        let _ = self.player.pause();
                    }
                    PlaybackState::Paused => {
                        let _ = self.player.resume();
                    }
                    PlaybackState::Stopped => {
                        // Start playing selected track
                        self.play_selected();
                    }
                }
            }
            KeyCode::Char( 's' ) if !self.edit_mode => {
                let _ = self.player.stop();
            }
            KeyCode::Char( 'e' ) => {
                self.edit_mode = !self.edit_mode;
                self.set_status( if self.edit_mode {
                    "Edit mode: J/K to move, d to delete"
                } else {
                    "Edit mode off"
                });
            }
            KeyCode::Up | KeyCode::Char( 'k' ) => {
                self.playlist_select_previous();
            }
            KeyCode::Down | KeyCode::Char( 'j' ) => {
                self.playlist_select_next();
            }
            // Edit mode: Shift+J/K to move tracks
            KeyCode::Char( 'J' ) if self.edit_mode && modifiers.contains( KeyModifiers::SHIFT ) => {
                self.move_track_down();
            }
            KeyCode::Char( 'K' ) if self.edit_mode && modifiers.contains( KeyModifiers::SHIFT ) => {
                self.move_track_up();
            }
            KeyCode::Char( 'd' ) if self.edit_mode => {
                self.delete_selected_track();
            }
            KeyCode::Enter => {
                self.play_selected();
            }
            KeyCode::Char( 'n' ) => {
                self.play_next();
            }
            KeyCode::Char( 'p' ) => {
                self.play_previous();
            }
            KeyCode::Right if modifiers.contains( KeyModifiers::CONTROL ) => {
                // Seek forward 10 seconds
                let pos = self.player.position();
                let new_pos = pos + Duration::from_secs( 10 );
                if let Some( duration ) = self.player.duration() {
                    if new_pos < duration {
                        if let Err( e ) = self.player.seek( new_pos ) {
                            self.set_status( format!( "Seek error: {}", e ) );
                        }
                    }
                }
            }
            KeyCode::Left if modifiers.contains( KeyModifiers::CONTROL ) => {
                // Seek backward 10 seconds
                let pos = self.player.position();
                let new_pos = pos.saturating_sub( Duration::from_secs( 10 ) );
                if let Err( e ) = self.player.seek( new_pos ) {
                    self.set_status( format!( "Seek error: {}", e ) );
                }
            }
            KeyCode::Right => {
                self.play_next();
            }
            KeyCode::Left => {
                self.play_previous();
            }
            KeyCode::Char( 'c' ) => {
                let playlist_arc = self.player.playlist();
                let mut playlist = playlist_arc.write().unwrap();
                playlist.clear();
                self.set_status( "Playlist cleared" );
            }
            KeyCode::Char( 'r' ) => {
                // Cycle repeat mode
                let playlist_arc = self.player.playlist();
                let mut playlist = playlist_arc.write().unwrap();
                let new_mode = match playlist.repeat() {
                    RepeatMode::Off => RepeatMode::One,
                    RepeatMode::One => RepeatMode::All,
                    RepeatMode::All => RepeatMode::Off,
                };
                playlist.set_repeat( new_mode );
                drop( playlist );
                self.set_status( format!( "Repeat: {:?}", new_mode ) );
            }
            KeyCode::Char( 'S' ) => {
                // Toggle shuffle
                let playlist_arc = self.player.playlist();
                let mut playlist = playlist_arc.write().unwrap();
                let new_shuffle = !playlist.shuffle();
                playlist.set_shuffle( new_shuffle );
                drop( playlist );
                self.set_status( format!( "Shuffle: {}", if new_shuffle { "on" } else { "off" } ) );
            }
            KeyCode::Char( 'v' ) => {
                // Cycle visualizer style
                self.visualizer_style = self.visualizer_style.next();
                self.set_status( format!( "Visualizer: {}", self.visualizer_style.name() ) );
            }
            KeyCode::Char( '+' ) | KeyCode::Char( '=' ) => {
                // Volume up
                self.volume = ( self.volume + 0.05 ).min( 1.0 );
                self.player.set_volume( self.volume );
                self.set_status( format!( "Volume: {}%", ( self.volume * 100.0 ) as i32 ) );
            }
            KeyCode::Char( '-' ) | KeyCode::Char( '_' ) => {
                // Volume down
                self.volume = ( self.volume - 0.05 ).max( 0.0 );
                self.player.set_volume( self.volume );
                self.set_status( format!( "Volume: {}%", ( self.volume * 100.0 ) as i32 ) );
            }
            KeyCode::Char( 'm' ) => {
                // Mute/unmute toggle
                if self.volume > 0.0 {
                    self.volume = 0.0;
                    self.set_status( "Muted" );
                } else {
                    self.volume = 1.0;
                    self.set_status( "Volume: 100%" );
                }
                self.player.set_volume( self.volume );
            }
            KeyCode::Char( 'i' ) => {
                // Show track info
                self.view_mode = ViewMode::TrackInfo;
            }
            KeyCode::Home | KeyCode::Char( 'g' ) => {
                self.playlist_state.select( Some( 0 ) );
            }
            KeyCode::End | KeyCode::Char( 'G' ) => {
                let playlist = self.player.playlist();
                let playlist = playlist.read().unwrap();
                if !playlist.is_empty() {
                    self.playlist_state.select( Some( playlist.len() - 1 ) );
                }
            }
            _ => {}
        }
    }


    fn handle_browser_key( &mut self, code: KeyCode ) {
        match code {
            KeyCode::Char( 'q' ) => {
                self.should_quit = true;
            }
            KeyCode::Up | KeyCode::Char( 'k' ) => {
                self.browser.select_previous();
            }
            KeyCode::Down | KeyCode::Char( 'j' ) => {
                self.browser.select_next();
            }
            KeyCode::Enter | KeyCode::Char( 'l' ) => {
                if let Ok( Some( file_path ) ) = self.browser.enter_selected() {
                    // Add file to playlist
                    let playlist_arc = self.player.playlist();
                    let mut playlist = playlist_arc.write().unwrap();
                    playlist.add( file_path );
                    self.set_status( "Added to playlist" );
                }
            }
            KeyCode::Backspace | KeyCode::Char( 'h' ) => {
                let _ = self.browser.go_up();
            }
            KeyCode::Char( 'a' ) => {
                // Add selected to playlist (file or entire folder)
                if let Some( entry ) = self.browser.selected_entry() {
                    let path = entry.path.clone();
                    let is_dir = entry.is_dir;
                    let is_audio = entry.is_audio;

                    if is_dir && entry.name != ".." {
                        let mut scanner = LibraryScanner::new();
                        scanner.add_root( path );
                        if let Ok( tracks ) = scanner.scan() {
                            let count = tracks.len();
                            let playlist_arc = self.player.playlist();
                            let mut playlist = playlist_arc.write().unwrap();
                            playlist.add_many( tracks.into_iter().map( |t| t.path ) );
                            self.set_status( format!( "Added {} tracks", count ) );
                        }
                    } else if is_audio {
                        let playlist_arc = self.player.playlist();
                        let mut playlist = playlist_arc.write().unwrap();
                        playlist.add( path );
                        self.set_status( "Added to playlist" );
                    }
                }
            }
            KeyCode::Char( 'R' ) => {
                let _ = self.browser.refresh();
                self.set_status( "Refreshed" );
            }
            KeyCode::Home | KeyCode::Char( 'g' ) => {
                self.browser.select_first();
            }
            KeyCode::End | KeyCode::Char( 'G' ) => {
                self.browser.select_last();
            }
            KeyCode::Char( '~' ) => {
                if let Some( home ) = dirs::home_dir() {
                    let _ = self.browser.navigate_to( &home );
                }
            }
            // Playback controls
            KeyCode::Char( ' ' ) => {
                match self.player.state() {
                    PlaybackState::Playing => { let _ = self.player.pause(); }
                    PlaybackState::Paused => { let _ = self.player.resume(); }
                    PlaybackState::Stopped => { self.play_selected(); }
                }
            }
            KeyCode::Char( 'n' ) => self.play_next(),
            KeyCode::Char( 'p' ) => self.play_previous(),
            KeyCode::Left => self.play_previous(),
            KeyCode::Right => self.play_next(),
            KeyCode::Char( '+' ) | KeyCode::Char( '=' ) => {
                self.volume = ( self.volume + 0.05 ).min( 1.0 );
                self.player.set_volume( self.volume );
                self.set_status( format!( "Volume: {}%", ( self.volume * 100.0 ) as i32 ) );
            }
            KeyCode::Char( '-' ) | KeyCode::Char( '_' ) => {
                self.volume = ( self.volume - 0.05 ).max( 0.0 );
                self.player.set_volume( self.volume );
                self.set_status( format!( "Volume: {}%", ( self.volume * 100.0 ) as i32 ) );
            }
            KeyCode::Char( 'm' ) => {
                if self.volume > 0.0 {
                    self.volume = 0.0;
                    self.set_status( "Muted" );
                } else {
                    self.volume = 1.0;
                    self.set_status( "Volume: 100%" );
                }
                self.player.set_volume( self.volume );
            }
            _ => {}
        }
    }


    fn handle_help_key( &mut self, code: KeyCode ) {
        match code {
            KeyCode::Char( 'q' ) | KeyCode::Esc | KeyCode::Char( '?' ) => {
                self.view_mode = ViewMode::Playlist;
                self.help_scroll = 0;
            }
            KeyCode::Up | KeyCode::Char( 'k' ) => {
                self.help_scroll = self.help_scroll.saturating_sub( 1 );
            }
            KeyCode::Down | KeyCode::Char( 'j' ) => {
                self.help_scroll = self.help_scroll.saturating_add( 1 );
            }
            KeyCode::PageUp => {
                self.help_scroll = self.help_scroll.saturating_sub( 10 );
            }
            KeyCode::PageDown => {
                self.help_scroll = self.help_scroll.saturating_add( 10 );
            }
            KeyCode::Home => {
                self.help_scroll = 0;
            }
            _ => {}
        }
    }


    fn handle_track_info_key( &mut self, code: KeyCode, modifiers: KeyModifiers ) {
        match code {
            KeyCode::Char( 'q' ) => {
                self.should_quit = true;
            }
            KeyCode::Esc | KeyCode::Char( 'i' ) => {
                self.view_mode = ViewMode::Playlist;
            }
            // Playback controls
            KeyCode::Char( ' ' ) => {
                match self.player.state() {
                    PlaybackState::Playing => { let _ = self.player.pause(); }
                    PlaybackState::Paused => { let _ = self.player.resume(); }
                    PlaybackState::Stopped => { self.play_selected(); }
                }
            }
            KeyCode::Char( 'n' ) => self.play_next(),
            KeyCode::Char( 'p' ) => self.play_previous(),
            KeyCode::Right if modifiers.contains( KeyModifiers::CONTROL ) => {
                let pos = self.player.position();
                let new_pos = pos + Duration::from_secs( 10 );
                if let Some( duration ) = self.player.duration() {
                    if new_pos < duration {
                        let _ = self.player.seek( new_pos );
                    }
                }
            }
            KeyCode::Left if modifiers.contains( KeyModifiers::CONTROL ) => {
                let pos = self.player.position();
                let new_pos = pos.saturating_sub( Duration::from_secs( 10 ) );
                let _ = self.player.seek( new_pos );
            }
            KeyCode::Right => self.play_next(),
            KeyCode::Left => self.play_previous(),
            KeyCode::Char( '+' ) | KeyCode::Char( '=' ) => {
                self.volume = ( self.volume + 0.05 ).min( 1.0 );
                self.player.set_volume( self.volume );
                self.set_status( format!( "Volume: {}%", ( self.volume * 100.0 ) as i32 ) );
            }
            KeyCode::Char( '-' ) | KeyCode::Char( '_' ) => {
                self.volume = ( self.volume - 0.05 ).max( 0.0 );
                self.player.set_volume( self.volume );
                self.set_status( format!( "Volume: {}%", ( self.volume * 100.0 ) as i32 ) );
            }
            KeyCode::Char( 'm' ) => {
                if self.volume > 0.0 {
                    self.volume = 0.0;
                    self.set_status( "Muted" );
                } else {
                    self.volume = 1.0;
                    self.set_status( "Volume: 100%" );
                }
                self.player.set_volume( self.volume );
            }
            _ => {}
        }
    }


    fn handle_visualizer_key( &mut self, code: KeyCode, modifiers: KeyModifiers ) {
        match code {
            KeyCode::Char( 'q' ) => {
                self.should_quit = true;
            }
            KeyCode::Esc => {
                self.view_mode = ViewMode::Playlist;
            }
            KeyCode::Char( 'v' ) => {
                // Cycle visualizer style
                self.visualizer_style = self.visualizer_style.next();
                self.set_status( format!( "Visualizer: {}", self.visualizer_style.name() ) );
            }
            // Playback controls
            KeyCode::Char( ' ' ) => {
                match self.player.state() {
                    PlaybackState::Playing => { let _ = self.player.pause(); }
                    PlaybackState::Paused => { let _ = self.player.resume(); }
                    PlaybackState::Stopped => { self.play_selected(); }
                }
            }
            KeyCode::Char( 'n' ) => self.play_next(),
            KeyCode::Char( 'p' ) => self.play_previous(),
            KeyCode::Right if modifiers.contains( KeyModifiers::CONTROL ) => {
                let pos = self.player.position();
                let new_pos = pos + Duration::from_secs( 10 );
                if let Some( duration ) = self.player.duration() {
                    if new_pos < duration {
                        let _ = self.player.seek( new_pos );
                    }
                }
            }
            KeyCode::Left if modifiers.contains( KeyModifiers::CONTROL ) => {
                let pos = self.player.position();
                let new_pos = pos.saturating_sub( Duration::from_secs( 10 ) );
                let _ = self.player.seek( new_pos );
            }
            KeyCode::Right => self.play_next(),
            KeyCode::Left => self.play_previous(),
            KeyCode::Char( '+' ) | KeyCode::Char( '=' ) => {
                self.volume = ( self.volume + 0.05 ).min( 1.0 );
                self.player.set_volume( self.volume );
                self.set_status( format!( "Volume: {}%", ( self.volume * 100.0 ) as i32 ) );
            }
            KeyCode::Char( '-' ) | KeyCode::Char( '_' ) => {
                self.volume = ( self.volume - 0.05 ).max( 0.0 );
                self.player.set_volume( self.volume );
                self.set_status( format!( "Volume: {}%", ( self.volume * 100.0 ) as i32 ) );
            }
            KeyCode::Char( 'm' ) => {
                if self.volume > 0.0 {
                    self.volume = 0.0;
                    self.set_status( "Muted" );
                } else {
                    self.volume = 1.0;
                    self.set_status( "Volume: 100%" );
                }
                self.player.set_volume( self.volume );
            }
            _ => {}
        }
    }


    fn handle_settings_key( &mut self, code: KeyCode ) {
        // Number of settings items
        const SETTINGS_COUNT: usize = 2;

        match code {
            KeyCode::Char( 'q' ) => {
                self.should_quit = true;
            }
            KeyCode::Esc => {
                self.view_mode = ViewMode::Playlist;
            }
            KeyCode::Up | KeyCode::Char( 'k' ) => {
                if self.settings_selected > 0 {
                    self.settings_selected -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char( 'j' ) => {
                if self.settings_selected < SETTINGS_COUNT - 1 {
                    self.settings_selected += 1;
                }
            }
            KeyCode::Enter => {
                // Toggle the selected setting
                match self.settings_selected {
                    0 => {
                        self.settings.discord_enabled = !self.settings.discord_enabled;
                        if !self.settings.discord_enabled {
                            self.discord.clear();
                            self.last_discord_track = None;
                        }
                    }
                    1 => {
                        self.settings.smtc_enabled = !self.settings.smtc_enabled;
                        if !self.settings.smtc_enabled {
                            // Drop media controls entirely to clear SMTC
                            self.media_controls = None;
                            self.last_smtc_state = None;
                            self.last_smtc_track = None;
                        } else {
                            // Recreate media controls when re-enabled
                            let ( tx, rx ) = create_media_controls_channel();
                            self.media_controls = MediaControlsHandler::new( tx );
                            self.media_controls_rx = rx;
                            self.force_smtc_update = true;
                        }
                    }
                    _ => {}
                }
                self.settings.save();
            }
            // Playback controls
            KeyCode::Char( ' ' ) => {
                match self.player.state() {
                    PlaybackState::Playing => { let _ = self.player.pause(); }
                    PlaybackState::Paused => { let _ = self.player.resume(); }
                    PlaybackState::Stopped => { self.play_selected(); }
                }
            }
            KeyCode::Char( 'n' ) => self.play_next(),
            KeyCode::Char( 'p' ) => self.play_previous(),
            KeyCode::Left => self.play_previous(),
            KeyCode::Right => self.play_next(),
            KeyCode::Char( '+' ) | KeyCode::Char( '=' ) => {
                self.volume = ( self.volume + 0.05 ).min( 1.0 );
                self.player.set_volume( self.volume );
                self.set_status( format!( "Volume: {}%", ( self.volume * 100.0 ) as i32 ) );
            }
            KeyCode::Char( '-' ) | KeyCode::Char( '_' ) => {
                self.volume = ( self.volume - 0.05 ).max( 0.0 );
                self.player.set_volume( self.volume );
                self.set_status( format!( "Volume: {}%", ( self.volume * 100.0 ) as i32 ) );
            }
            KeyCode::Char( 'm' ) => {
                if self.volume > 0.0 {
                    self.volume = 0.0;
                    self.set_status( "Muted" );
                } else {
                    self.volume = 1.0;
                    self.set_status( "Volume: 100%" );
                }
                self.player.set_volume( self.volume );
            }
            _ => {}
        }
    }


    fn handle_command_key( &mut self, code: KeyCode ) {
        match code {
            KeyCode::Enter => {
                let input = self.input_buffer.content().to_string();
                self.execute_command( &input );
                self.input_mode = InputMode::Normal;
                self.input_buffer.clear();
            }
            KeyCode::Esc => {
                self.input_mode = InputMode::Normal;
                self.input_buffer.clear();
            }
            KeyCode::Backspace => {
                if self.input_buffer.is_empty() {
                    self.input_mode = InputMode::Normal;
                } else {
                    self.input_buffer.backspace();
                }
            }
            KeyCode::Delete => {
                self.input_buffer.delete();
            }
            KeyCode::Left => {
                self.input_buffer.move_left();
            }
            KeyCode::Right => {
                self.input_buffer.move_right();
            }
            KeyCode::Home => {
                self.input_buffer.move_home();
            }
            KeyCode::End => {
                self.input_buffer.move_end();
            }
            KeyCode::Char( c ) => {
                self.input_buffer.insert( c );
            }
            _ => {}
        }
    }


    fn handle_search_key( &mut self, code: KeyCode ) {
        match code {
            KeyCode::Enter | KeyCode::Esc => {
                self.input_mode = InputMode::Normal;
                if code == KeyCode::Esc {
                    self.browser.clear_filter();
                }
                self.input_buffer.clear();
            }
            KeyCode::Backspace => {
                self.input_buffer.backspace();
                self.browser.set_filter( self.input_buffer.content().to_string() );
            }
            KeyCode::Char( c ) => {
                self.input_buffer.insert( c );
                self.browser.set_filter( self.input_buffer.content().to_string() );
            }
            _ => {}
        }
    }


    fn execute_command( &mut self, input: &str ) {
        match Command::parse( input ) {
            Ok( cmd ) => {
                if let Err( e ) = self.run_command( cmd ) {
                    self.set_status( format!( "Error: {}", e ) );
                }
            }
            Err( e ) => {
                self.set_status( format!( "{}", e ) );
            }
        }
    }


    fn run_command( &mut self, cmd: Command ) -> Result<()> {
        match cmd {
            Command::Add { path } => {
                if path.is_dir() {
                    let mut scanner = LibraryScanner::new();
                    scanner.add_root( path );
                    let tracks = scanner.scan()?;
                    let count = tracks.len();
                    let playlist_arc = self.player.playlist();
                    let mut playlist = playlist_arc.write().unwrap();
                    playlist.add_many( tracks.into_iter().map( |t| t.path ) );
                    self.set_status( format!( "Added {} tracks", count ) );
                } else {
                    let playlist_arc = self.player.playlist();
                    let mut playlist = playlist_arc.write().unwrap();
                    playlist.add( path );
                    self.set_status( "Added to playlist" );
                }
            }
            Command::Remove => {
                self.delete_selected_track();
            }
            Command::Clear => {
                let playlist_arc = self.player.playlist();
                let mut playlist = playlist_arc.write().unwrap();
                playlist.clear();
                self.set_status( "Playlist cleared" );
            }
            Command::Dedup => {
                let playlist_arc = self.player.playlist();
                let mut playlist = playlist_arc.write().unwrap();
                let removed = playlist.dedup();
                if removed > 0 {
                    self.set_status( format!( "Removed {} duplicate(s)", removed ) );
                } else {
                    self.set_status( "No duplicates found" );
                }
            }
            Command::Shuffle => {
                let playlist_arc = self.player.playlist();
                let mut playlist = playlist_arc.write().unwrap();
                let new_shuffle = !playlist.shuffle();
                playlist.set_shuffle( new_shuffle );
                self.set_status( format!( "Shuffle: {}", if new_shuffle { "on" } else { "off" } ) );
            }
            Command::Repeat { mode } => {
                let playlist_arc = self.player.playlist();
                let mut playlist = playlist_arc.write().unwrap();
                let new_mode = match mode {
                    Some( RepeatModeArg::Off ) => RepeatMode::Off,
                    Some( RepeatModeArg::One ) => RepeatMode::One,
                    Some( RepeatModeArg::All ) => RepeatMode::All,
                    None => match playlist.repeat() {
                        RepeatMode::Off => RepeatMode::One,
                        RepeatMode::One => RepeatMode::All,
                        RepeatMode::All => RepeatMode::Off,
                    },
                };
                playlist.set_repeat( new_mode );
                self.set_status( format!( "Repeat: {:?}", new_mode ) );
            }
            Command::Play => {
                self.play_selected();
            }
            Command::Pause => {
                self.player.pause()?;
                self.set_status( "Paused" );
            }
            Command::Stop => {
                self.player.stop()?;
                self.set_status( "Stopped" );
            }
            Command::Next => {
                self.play_next();
            }
            Command::Prev => {
                self.play_previous();
            }
            Command::Goto { path } => {
                self.browser.navigate_to( &path )?;
                self.view_mode = ViewMode::Browser;
            }
            Command::Home => {
                if let Some( home ) = dirs::home_dir() {
                    self.browser.navigate_to( &home )?;
                    self.view_mode = ViewMode::Browser;
                }
            }
            Command::Search { term } => {
                self.browser.set_filter( term );
                self.view_mode = ViewMode::Browser;
            }
            Command::Help => {
                self.view_mode = ViewMode::Help;
            }
            Command::Quit => {
                self.should_quit = true;
            }
            Command::Save { name } => {
                if let Some( dir ) = oxidio_core::Playlist::ensure_playlist_dir() {
                    let path = dir.join( format!( "{}.m3u", name ) );
                    let playlist_arc = self.player.playlist();
                    let playlist = playlist_arc.read().unwrap();
                    match playlist.save( &path ) {
                        Ok(()) => self.set_status( format!( "Saved playlist to {}", path.display() ) ),
                        Err( e ) => self.set_status( format!( "Failed to save: {}", e ) ),
                    }
                } else {
                    self.set_status( "Could not determine playlist directory".to_string() );
                }
            }
            Command::Load { name } => {
                if let Some( dir ) = oxidio_core::Playlist::playlist_dir() {
                    let path = dir.join( format!( "{}.m3u", name ) );
                    match oxidio_core::Playlist::load( &path ) {
                        Ok( loaded ) => {
                            let playlist_arc = self.player.playlist();
                            let mut playlist = playlist_arc.write().unwrap();
                            *playlist = loaded;
                            self.set_status( format!( "Loaded playlist from {}", path.display() ) );
                        }
                        Err( e ) => self.set_status( format!( "Failed to load: {}", e ) ),
                    }
                } else {
                    self.set_status( "Could not determine playlist directory".to_string() );
                }
            }
            Command::Seek { position } => {
                match self.player.seek( position ) {
                    Ok(()) => {
                        let secs = position.as_secs();
                        self.set_status( format!( "Seeked to {}:{:02}", secs / 60, secs % 60 ) );
                    }
                    Err( e ) => self.set_status( format!( "Seek error: {}", e ) ),
                }
            }
            Command::Vis => {
                self.visualizer_style = self.visualizer_style.next();
                self.set_status( format!( "Visualizer: {}", self.visualizer_style.name() ) );
            }
            Command::Volume { level } => {
                if let Some( level ) = level {
                    self.volume = ( level as f32 / 100.0 ).clamp( 0.0, 1.0 );
                    self.player.set_volume( self.volume );
                    self.set_status( format!( "Volume: {}%", level.min( 100 ) ) );
                } else {
                    self.set_status( format!( "Volume: {}%", ( self.volume * 100.0 ) as i32 ) );
                }
            }
        }
        Ok(())
    }


    fn playlist_select_next( &mut self ) {
        let playlist = self.player.playlist();
        let playlist = playlist.read().unwrap();
        let len = playlist.len();

        if len == 0 {
            return;
        }

        let i = match self.playlist_state.selected() {
            Some( i ) => {
                if i >= len - 1 { 0 } else { i + 1 }
            }
            None => 0,
        };
        self.playlist_state.select( Some( i ) );
    }


    fn playlist_select_previous( &mut self ) {
        let playlist = self.player.playlist();
        let playlist = playlist.read().unwrap();
        let len = playlist.len();

        if len == 0 {
            return;
        }

        let i = match self.playlist_state.selected() {
            Some( i ) => {
                if i == 0 { len - 1 } else { i - 1 }
            }
            None => 0,
        };
        self.playlist_state.select( Some( i ) );
    }


    fn play_selected( &mut self ) {
        if let Some( idx ) = self.playlist_state.selected() {
            let playlist = self.player.playlist();
            let mut playlist = playlist.write().unwrap();
            if let Some( path ) = playlist.jump_to( idx ) {
                let path = path.clone();
                drop( playlist );
                if let Err( e ) = self.player.play( path ) {
                    self.set_status( format!( "Play error: {}", e ) );
                } else {
                    self.force_smtc_update = true;
                }
            }
        }
    }


    fn play_next( &mut self ) {
        let playlist = self.player.playlist();
        let mut playlist = playlist.write().unwrap();
        if let Some( path ) = playlist.next() {
            let path = path.clone();
            drop( playlist );
            if let Err( e ) = self.player.play( path ) {
                self.set_status( format!( "Play error: {}", e ) );
            } else {
                self.force_smtc_update = true;
            }
        }
    }


    fn play_previous( &mut self ) {
        let playlist = self.player.playlist();
        let mut playlist = playlist.write().unwrap();
        if let Some( path ) = playlist.previous() {
            let path = path.clone();
            drop( playlist );
            if let Err( e ) = self.player.play( path ) {
                self.set_status( format!( "Play error: {}", e ) );
            } else {
                self.force_smtc_update = true;
            }
        }
    }


    fn move_track_down( &mut self ) {
        if let Some( idx ) = self.playlist_state.selected() {
            let playlist = self.player.playlist();
            let mut playlist = playlist.write().unwrap();
            if idx < playlist.len().saturating_sub( 1 ) {
                playlist.move_track( idx, idx + 1 );
                drop( playlist );
                self.playlist_state.select( Some( idx + 1 ) );
            }
        }
    }


    fn move_track_up( &mut self ) {
        if let Some( idx ) = self.playlist_state.selected() {
            if idx > 0 {
                let playlist = self.player.playlist();
                let mut playlist = playlist.write().unwrap();
                playlist.move_track( idx, idx - 1 );
                drop( playlist );
                self.playlist_state.select( Some( idx - 1 ) );
            }
        }
    }


    fn delete_selected_track( &mut self ) {
        if let Some( idx ) = self.playlist_state.selected() {
            let playlist = self.player.playlist();
            let mut playlist = playlist.write().unwrap();
            playlist.remove( idx );
            let len = playlist.len();
            drop( playlist );

            if len == 0 {
                self.playlist_state.select( None );
            } else if idx >= len {
                self.playlist_state.select( Some( len - 1 ) );
            }
            self.set_status( "Track removed" );
        }
    }


    /// Saves the current session state for restoration on next startup.
    fn save_session( &self ) {
        let playlist_arc = self.player.playlist();
        let playlist = playlist_arc.read().unwrap();

        // Only save if there's something in the playlist
        if playlist.is_empty() {
            return;
        }

        // Save the playlist as "_last"
        if let Some( dir ) = oxidio_core::Playlist::ensure_playlist_dir() {
            let path = dir.join( "_last.m3u" );
            if let Err( e ) = playlist.save( &path ) {
                tracing::warn!( "Failed to save session playlist: {}", e );
            }
        }

        // Save the session state including shuffle, repeat, and volume
        let state = oxidio_core::playlist::SessionState {
            playlist_name: "_last".to_string(),
            track_index: playlist.current_index().or( self.playlist_state.selected() ),
            shuffle: playlist.shuffle(),
            repeat: playlist.repeat(),
            volume: self.volume,
        };

        if let Err( e ) = oxidio_core::Playlist::save_session( &state ) {
            tracing::warn!( "Failed to save session state: {}", e );
        }
    }
}


fn main() -> Result<()> {
    let args = Args::parse();

    // Setup terminal
    enable_raw_mode()?;
    io::stdout().execute( EnterAlternateScreen )?;
    io::stdout().execute( crossterm::event::EnableMouseCapture )?;

    let mut terminal = Terminal::new( CrosstermBackend::new( io::stdout() ) )?;

    // Create app
    let mut app = App::new( &args )?;

    // Main loop
    loop {
        // Update state
        app.tick();

        // Draw UI
        terminal.draw( |frame| draw_ui( frame, &mut app ) )?;

        // Handle events with timeout
        if event::poll( Duration::from_millis( 100 ) )? {
            match event::read()? {
                Event::Key( key ) if key.kind == KeyEventKind::Press => {
                    app.handle_key( key.code, key.modifiers );
                }
                Event::Mouse( mouse ) => {
                    app.handle_mouse( mouse.column, mouse.row, mouse.kind );
                }
                _ => {}
            }
        }

        if app.should_quit {
            // Save session before quitting
            app.save_session();
            break;
        }
    }

    // Cleanup
    io::stdout().execute( crossterm::event::DisableMouseCapture )?;
    disable_raw_mode()?;
    io::stdout().execute( LeaveAlternateScreen )?;

    Ok(())
}


/// Draws the main UI.
fn draw_ui( frame: &mut Frame, app: &mut App ) {
    let area = frame.area();

    // Create layout
    let chunks = Layout::default()
        .direction( Direction::Vertical )
        .constraints([
            Constraint::Length( 2 ),  // Header
            Constraint::Min( 0 ),     // Main content
            Constraint::Length( 5 ),  // Now playing
            Constraint::Length( 1 ),  // Status bar
        ])
        .split( area );

    // Header with view indicator
    let view_indicator = match app.view_mode {
        ViewMode::Playlist => if app.edit_mode { "PLAYLIST [EDIT]" } else { "PLAYLIST" },
        ViewMode::Browser => "BROWSER",
        ViewMode::Help => "HELP",
        ViewMode::TrackInfo => "TRACK INFO",
        ViewMode::Visualizer => "VISUALIZER",
        ViewMode::Settings => "SETTINGS",
    };

    let header = Paragraph::new( format!( "  OXIDIO - {}", view_indicator ) )
        .style( Style::default().fg( Color::Cyan ).bold() )
        .block( Block::default().borders( Borders::BOTTOM ) );
    frame.render_widget( header, chunks[0] );

    // Main content area based on view mode
    match app.view_mode {
        ViewMode::Playlist => draw_playlist( frame, app, chunks[1] ),
        ViewMode::Browser => draw_browser( frame, app, chunks[1] ),
        ViewMode::Help => draw_help( frame, app, chunks[1] ),
        ViewMode::TrackInfo => draw_track_info( frame, app, chunks[1] ),
        ViewMode::Visualizer => draw_visualizer( frame, app, chunks[1] ),
        ViewMode::Settings => draw_settings( frame, app, chunks[1] ),
    }

    // Now playing
    draw_now_playing( frame, app, chunks[2] );

    // Status bar
    draw_status_bar( frame, app, chunks[3] );
}


fn draw_playlist( frame: &mut Frame, app: &mut App, area: Rect ) {
    // Store area for mouse hit detection
    app.playlist_area = Some( area );

    let playlist = app.player.playlist();
    let playlist = playlist.read().unwrap();

    let playing_index = playlist.current_index();

    // Handle scroll-to-playing without changing selection
    if app.scroll_to_playing {
        if let Some( playing_idx ) = playing_index {
            // Calculate visible height (area height minus borders)
            let visible_height = area.height.saturating_sub( 2 ) as usize;
            if visible_height > 0 {
                let current_offset = app.playlist_state.offset();

                // Check if playing track is visible
                let is_visible = playing_idx >= current_offset
                    && playing_idx < current_offset + visible_height;

                if !is_visible {
                    // Scroll to center the playing track
                    let new_offset = playing_idx.saturating_sub( visible_height / 2 );
                    *app.playlist_state.offset_mut() = new_offset;
                }
            }
        }
        app.scroll_to_playing = false;
    }

    let items: Vec<ListItem> = playlist
        .tracks()
        .iter()
        .enumerate()
        .map( |( i, path )| {
            let filename = path
                .file_name()
                .and_then( |n| n.to_str() )
                .unwrap_or( "Unknown" );
            let prefix = if Some( i ) == playing_index {
                "▶ "
            } else if app.edit_mode {
                "≡ "
            } else {
                "  "
            };
            ListItem::new( format!( "{}{}", prefix, filename ) )
        })
        .collect();

    let title = format!(
        " Playlist ({}) {} {} ",
        playlist.len(),
        if playlist.shuffle() { "[S]" } else { "" },
        match playlist.repeat() {
            RepeatMode::Off => "",
            RepeatMode::One => "[R1]",
            RepeatMode::All => "[R∞]",
        }
    );

    let border_style = if app.edit_mode {
        Style::default().fg( Color::Yellow )
    } else {
        Style::default()
    };

    let highlight_style = if app.edit_mode {
        Style::default().bg( Color::Yellow ).fg( Color::Black )
    } else {
        Style::default().bg( Color::DarkGray )
    };

    let playlist_widget = List::new( items )
        .block( Block::default()
            .title( title )
            .borders( Borders::ALL )
            .border_style( border_style )
        )
        .highlight_style( highlight_style )
        .highlight_symbol( ">> " );

    frame.render_stateful_widget( playlist_widget, area, &mut app.playlist_state );
}


fn draw_browser( frame: &mut Frame, app: &mut App, area: Rect ) {
    let path_str = app.browser.current_dir().display().to_string();
    let title = if path_str.len() > 50 {
        format!( " ...{} ", &path_str[ path_str.len() - 47.. ] )
    } else {
        format!( " {} ", path_str )
    };

    let items: Vec<ListItem> = app.browser.visible_entries()
        .iter()
        .map( |entry| {
            let icon = if entry.is_dir {
                "📁"
            } else if entry.is_audio {
                "🎵"
            } else {
                "  "
            };

            let style = if entry.is_dir {
                Style::default().fg( Color::Blue )
            } else if entry.is_audio {
                Style::default().fg( Color::Green )
            } else {
                Style::default().fg( Color::DarkGray )
            };

            ListItem::new( format!( " {} {}", icon, entry.name ) )
                .style( style )
        })
        .collect();

    let mut state = ListState::default();
    state.select( Some( app.browser.selected_index() ) );

    let browser_widget = List::new( items )
        .block( Block::default().title( title ).borders( Borders::ALL ) )
        .highlight_style( Style::default().bg( Color::DarkGray ) )
        .highlight_symbol( ">> " );

    frame.render_stateful_widget( browser_widget, area, &mut state );
}


fn draw_help( frame: &mut Frame, app: &mut App, area: Rect ) {
    let help_text = command::help_text();
    let line_count = help_text.lines().count() as u16;
    let visible_height = area.height.saturating_sub( 2 ); // Account for borders

    // Clamp scroll to valid range
    let max_scroll = line_count.saturating_sub( visible_height );
    if app.help_scroll > max_scroll {
        app.help_scroll = max_scroll;
    }

    let help = Paragraph::new( help_text )
        .block( Block::default()
            .title( " Help (↑↓ scroll, ? or Esc to close) " )
            .borders( Borders::ALL )
        )
        .wrap( Wrap { trim: false } )
        .scroll(( app.help_scroll, 0 ));

    frame.render_widget( help, area );
}


fn draw_track_info( frame: &mut Frame, app: &App, area: Rect ) {
    let mut lines = Vec::new();

    // Get the track path - either currently playing or selected
    let track_path = app.player.current_track().or_else( || {
        app.playlist_state.selected().and_then( |idx| {
            let playlist = app.player.playlist();
            let playlist = playlist.read().unwrap();
            playlist.tracks().get( idx ).cloned()
        })
    });

    if let Some( ref path ) = track_path {
        // Get metadata
        let meta = app.player.metadata();

        // Title (always show)
        let title = meta.as_ref()
            .and_then( |m| m.title.clone() )
            .or_else( || path.file_stem().and_then( |n| n.to_str() ).map( String::from ) )
            .unwrap_or_else( || "Unknown".to_string() );
        lines.push( Line::from( vec![
            Span::styled( "Title:  ", Style::default().fg( Color::Gray ) ),
            Span::styled( title, Style::default().fg( Color::Cyan ).bold() ),
        ]));

        // Artist (always show)
        let artist = meta.as_ref()
            .and_then( |m| m.artist.clone() )
            .unwrap_or_else( || "Unknown".to_string() );
        lines.push( Line::from( vec![
            Span::styled( "Artist: ", Style::default().fg( Color::Gray ) ),
            Span::styled( artist, Style::default().fg( Color::Yellow ) ),
        ]));

        // Album (always show)
        let album = meta.as_ref()
            .and_then( |m| m.album.clone() )
            .unwrap_or_else( || "Unknown".to_string() );
        lines.push( Line::from( vec![
            Span::styled( "Album:  ", Style::default().fg( Color::Gray ) ),
            Span::styled( album, Style::default().fg( Color::Green ) ),
        ]));

        lines.push( Line::from( "" ) );

        // Additional metadata (only if available)
        if let Some( ref meta ) = meta {
            if let Some( album_artist ) = &meta.album_artist {
                lines.push( Line::from( vec![
                    Span::styled( "Album Artist: ", Style::default().fg( Color::Gray ) ),
                    Span::raw( album_artist.clone() ),
                ]));
            }
            if let Some( track_num ) = meta.track_number {
                lines.push( Line::from( vec![
                    Span::styled( "Track #: ", Style::default().fg( Color::Gray ) ),
                    Span::raw( track_num.to_string() ),
                ]));
            }
            if let Some( genre ) = &meta.genre {
                lines.push( Line::from( vec![
                    Span::styled( "Genre:  ", Style::default().fg( Color::Gray ) ),
                    Span::raw( genre.clone() ),
                ]));
            }
            if let Some( year ) = meta.year {
                lines.push( Line::from( vec![
                    Span::styled( "Year:   ", Style::default().fg( Color::Gray ) ),
                    Span::raw( year.to_string() ),
                ]));
            }
        }

        lines.push( Line::from( "" ) );
        lines.push( Line::from( Span::styled( "─── Audio Format ───", Style::default().fg( Color::DarkGray ) ) ) );

        // Audio format information
        if let Some( ref meta ) = meta {
            // Codec
            if let Some( codec ) = &meta.codec {
                lines.push( Line::from( vec![
                    Span::styled( "Codec:       ", Style::default().fg( Color::Gray ) ),
                    Span::raw( codec.clone() ),
                ]));
            }

            // Bitrate
            if let Some( bitrate ) = meta.bitrate {
                lines.push( Line::from( vec![
                    Span::styled( "Bitrate:     ", Style::default().fg( Color::Gray ) ),
                    Span::raw( format!( "{} kbps", bitrate ) ),
                ]));
            }

            // Sample rate
            if let Some( sample_rate ) = meta.sample_rate {
                lines.push( Line::from( vec![
                    Span::styled( "Sample Rate: ", Style::default().fg( Color::Gray ) ),
                    Span::raw( format!( "{} Hz", sample_rate ) ),
                ]));
            }

            // Channels
            if let Some( channels ) = meta.channels {
                let ch_str = match channels {
                    1 => "Mono".to_string(),
                    2 => "Stereo".to_string(),
                    n => format!( "{} channels", n ),
                };
                lines.push( Line::from( vec![
                    Span::styled( "Channels:    ", Style::default().fg( Color::Gray ) ),
                    Span::raw( ch_str ),
                ]));
            }
        }

        // Duration
        if let Some( duration ) = app.player.duration() {
            let secs = duration.as_secs();
            lines.push( Line::from( vec![
                Span::styled( "Duration:    ", Style::default().fg( Color::Gray ) ),
                Span::raw( format!( "{}:{:02}", secs / 60, secs % 60 ) ),
            ]));
        }

        lines.push( Line::from( "" ) );
        lines.push( Line::from( Span::styled( "─── File ───", Style::default().fg( Color::DarkGray ) ) ) );

        // Filename
        let filename = path.file_name()
            .and_then( |n| n.to_str() )
            .unwrap_or( "Unknown" );
        lines.push( Line::from( vec![
            Span::styled( "File: ", Style::default().fg( Color::Gray ) ),
            Span::raw( filename.to_string() ),
        ]));

        // Full path
        lines.push( Line::from( vec![
            Span::styled( "Path: ", Style::default().fg( Color::Gray ) ),
            Span::raw( path.display().to_string() ),
        ]));
    } else {
        lines.push( Line::from( Span::styled(
            "No track selected or playing",
            Style::default().fg( Color::DarkGray ).italic(),
        )));
        lines.push( Line::from( "" ) );
        lines.push( Line::from( Span::styled(
            "Select a track in the playlist and press 'i' to view its info,",
            Style::default().fg( Color::DarkGray ),
        )));
        lines.push( Line::from( Span::styled(
            "or start playing a track first.",
            Style::default().fg( Color::DarkGray ),
        )));
    }

    let info = Paragraph::new( lines )
        .block( Block::default()
            .title( " Track Info (press i or Esc to close) " )
            .borders( Borders::ALL )
        )
        .wrap( Wrap { trim: false } );

    frame.render_widget( info, area );
}


fn draw_now_playing( frame: &mut Frame, app: &App, area: Rect ) {
    let state = app.player.state();
    let state_str = match state {
        PlaybackState::Playing => "▶",
        PlaybackState::Paused => "⏸",
        PlaybackState::Stopped => "⏹",
    };

    // Get metadata if available
    let metadata = app.player.metadata();

    // Build track display string from metadata or filename
    let ( title, artist_album ) = if let Some( ref meta ) = metadata {
        let title = meta.title.clone().unwrap_or_else( || {
            app.player
                .current_track()
                .and_then( |p| p.file_name().map( |n| n.to_string_lossy().to_string() ) )
                .unwrap_or_else( || "Unknown".to_string() )
        });
        let artist_album = match ( &meta.artist, &meta.album ) {
            ( Some( artist ), Some( album ) ) => format!( "{} - {}", artist, album ),
            ( Some( artist ), None ) => artist.clone(),
            ( None, Some( album ) ) => album.clone(),
            ( None, None ) => String::new(),
        };
        ( title, artist_album )
    } else {
        let title = app
            .player
            .current_track()
            .and_then( |p| p.file_name().map( |n| n.to_string_lossy().to_string() ) )
            .unwrap_or_else( || "No track".to_string() );
        ( title, String::new() )
    };

    // Get position and duration
    let position = app.player.position();
    let duration = app.player.duration().unwrap_or( std::time::Duration::ZERO );

    // Format time as M:SS
    let format_time = |d: std::time::Duration| -> String {
        let secs = d.as_secs();
        format!( "{}:{:02}", secs / 60, secs % 60 )
    };

    // Calculate progress bar
    let progress_width = 20;
    let progress = if duration.as_secs() > 0 {
        ( position.as_secs_f64() / duration.as_secs_f64() ).min( 1.0 )
    } else {
        0.0
    };
    let filled = ( progress * progress_width as f64 ).round() as usize;
    let bar = format!(
        "[{}{}]",
        "━".repeat( filled ),
        "─".repeat( progress_width - filled )
    );

    let mut lines = vec![
        Line::from( Span::styled( format!( " {} {} ", state_str, title ), Style::default().bold() ) ),
    ];

    // Only add artist/album line if there's content
    if !artist_album.is_empty() {
        lines.push( Line::from( Span::styled( format!( "   {} ", artist_album ), Style::default().fg( Color::Gray ) ) ) );
    }

    // Show volume indicator
    let vol_pct = ( app.volume * 100.0 ) as i32;
    let vol_str = if vol_pct == 0 { "🔇".to_string() } else { format!( "🔊{}%", vol_pct ) };

    lines.push( Line::from( format!( " {} {} / {}  {} ", bar, format_time( position ), format_time( duration ), vol_str ) ) );

    let now_playing = Paragraph::new( lines )
        .block( Block::default().title( " Now Playing " ).borders( Borders::ALL ) );

    frame.render_widget( now_playing, area );
}


fn draw_visualizer( frame: &mut Frame, app: &App, area: Rect ) {
    let vis_data = app.player.vis_data();

    // Use the full height of the content area for visualization
    let inner_height = area.height.saturating_sub( 2 ) as usize; // Account for borders
    let inner_width = area.width.saturating_sub( 2 ) as usize;

    let mut lines = Vec::with_capacity( inner_height );

    if let Some( data ) = vis_data {
        match app.visualizer_style {
            VisualizerStyle::Bars => {
                draw_vis_bars( &mut lines, &data, inner_height, inner_width );
            }
            VisualizerStyle::Spectrum => {
                draw_vis_spectrum( &mut lines, &data, inner_height, inner_width );
            }
            VisualizerStyle::Waveform => {
                draw_vis_waveform( &mut lines, &data, inner_height, inner_width );
            }
            VisualizerStyle::LevelMeter => {
                draw_vis_level_meter( &mut lines, &data, inner_height, inner_width );
            }
        }
    } else {
        // No audio data - show a message
        let msg = "No audio playing";
        let padding = ( inner_height / 2 ).saturating_sub( 1 );
        for _ in 0..padding {
            lines.push( Line::from( "" ) );
        }
        lines.push( Line::from( Span::styled( msg, Style::default().fg( Color::DarkGray ).italic() ) ) );
    }

    let title = format!( " Visualizer: {} (v to change, Esc to close) ", app.visualizer_style.name() );
    let visualizer = Paragraph::new( lines )
        .block( Block::default()
            .title( title )
            .borders( Borders::ALL )
        )
        .alignment( Alignment::Center );

    frame.render_widget( visualizer, area );
}


fn draw_vis_bars( lines: &mut Vec<Line<'static>>, data: &[f32; 32], height: usize, width: usize ) {
    let vis_chars = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

    let bar_width = 2;
    let max_bars = width / ( bar_width + 1 );
    let num_bars = max_bars.min( 32 );

    for row in ( 0..height ).rev() {
        let threshold = ( row as f32 + 0.5 ) / height as f32;
        let mut line_content = String::new();

        for bar_idx in 0..num_bars {
            let data_idx = ( bar_idx * 32 ) / num_bars;
            let amp = data[ data_idx.min( 31 ) ];
            let scaled_amp = ( amp * 4.0 ).min( 1.0 );

            if scaled_amp >= threshold {
                let level = ((( scaled_amp - threshold ) * height as f32 * 8.0 ) as usize ).min( 7 );
                let ch = vis_chars[ level ];
                line_content.push( ch );
                line_content.push( ch );
            } else {
                line_content.push_str( "  " );
            }
            line_content.push( ' ' );
        }

        lines.push( Line::from( Span::styled( line_content, Style::default().fg( Color::Cyan ) ) ) );
    }
}


fn draw_vis_spectrum( lines: &mut Vec<Line<'static>>, data: &[f32; 32], height: usize, width: usize ) {
    let vis_chars = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

    let bar_width = 1;
    let max_bars = width / ( bar_width + 1 );
    let num_bars = max_bars.min( 32 );
    let half_height = height / 2;

    // Draw mirrored spectrum (top half mirrors bottom half)
    for row in 0..height {
        let is_top_half = row < half_height;
        let row_in_half = if is_top_half {
            half_height - row - 1
        } else {
            row - half_height
        };
        let threshold = ( row_in_half as f32 + 0.5 ) / half_height as f32;

        let mut line_content = String::new();

        for bar_idx in 0..num_bars {
            let data_idx = ( bar_idx * 32 ) / num_bars;
            let amp = data[ data_idx.min( 31 ) ];
            let scaled_amp = ( amp * 4.0 ).min( 1.0 );

            if scaled_amp >= threshold {
                let level = ((( scaled_amp - threshold ) * half_height as f32 * 8.0 ) as usize ).min( 7 );
                let ch = vis_chars[ level ];
                line_content.push( ch );
            } else {
                line_content.push( ' ' );
            }
            line_content.push( ' ' );
        }

        let color = if is_top_half { Color::Magenta } else { Color::Cyan };
        lines.push( Line::from( Span::styled( line_content, Style::default().fg( color ) ) ) );
    }
}


fn draw_vis_waveform( lines: &mut Vec<Line<'static>>, data: &[f32; 32], height: usize, width: usize ) {
    let center_row = height / 2;

    // Build the waveform grid
    let mut grid: Vec<Vec<char>> = vec![vec![' '; width]; height];

    for x in 0..width {
        let data_idx = ( x * 32 ) / width;
        let amp = data[ data_idx.min( 31 ) ];

        // Convert amplitude to y offset from center
        let y_offset = ( amp * 3.0 * center_row as f32 ) as isize;
        let y = ( center_row as isize - y_offset ).clamp( 0, ( height - 1 ) as isize ) as usize;

        grid[ y ][ x ] = '●';

        // Draw vertical line from center to point
        let start_y = center_row.min( y );
        let end_y = center_row.max( y );
        for row in start_y..=end_y {
            if grid[ row ][ x ] == ' ' {
                grid[ row ][ x ] = '│';
            }
        }
    }

    // Draw center line
    for x in 0..width {
        if grid[ center_row ][ x ] == ' ' {
            grid[ center_row ][ x ] = '─';
        }
    }

    // Convert grid to lines
    for row in &grid {
        let line_str: String = row.iter().collect();
        lines.push( Line::from( Span::styled( line_str, Style::default().fg( Color::Green ) ) ) );
    }
}


fn draw_vis_level_meter( lines: &mut Vec<Line<'static>>, data: &[f32; 32], height: usize, width: usize ) {
    // Calculate average amplitude for left and right channels (simple stereo simulation)
    let left_amp: f32 = data[ 0..16 ].iter().sum::<f32>() / 16.0;
    let right_amp: f32 = data[ 16..32 ].iter().sum::<f32>() / 16.0;
    let total_amp: f32 = data.iter().sum::<f32>() / 32.0;

    let meter_width = width.saturating_sub( 10 );
    let left_filled = (( left_amp * 4.0 ).min( 1.0 ) * meter_width as f32 ) as usize;
    let right_filled = (( right_amp * 4.0 ).min( 1.0 ) * meter_width as f32 ) as usize;
    let total_filled = (( total_amp * 4.0 ).min( 1.0 ) * meter_width as f32 ) as usize;

    // Create meter characters
    let create_meter = |filled: usize, total: usize| -> String {
        let mut result = String::new();
        for i in 0..total {
            if i < filled {
                result.push( '█' );
            } else {
                result.push( '░' );
            }
        }
        result
    };

    // Pad vertically to center
    let content_height = 7;
    let padding = ( height.saturating_sub( content_height ) ) / 2;

    for _ in 0..padding {
        lines.push( Line::from( "" ) );
    }

    lines.push( Line::from( "" ) );
    lines.push( Line::from( vec![
        Span::styled( "  L  [", Style::default().fg( Color::Gray ) ),
        Span::styled( create_meter( left_filled, meter_width ), Style::default().fg( Color::Cyan ) ),
        Span::styled( "]", Style::default().fg( Color::Gray ) ),
    ]));
    lines.push( Line::from( "" ) );
    lines.push( Line::from( vec![
        Span::styled( "  R  [", Style::default().fg( Color::Gray ) ),
        Span::styled( create_meter( right_filled, meter_width ), Style::default().fg( Color::Magenta ) ),
        Span::styled( "]", Style::default().fg( Color::Gray ) ),
    ]));
    lines.push( Line::from( "" ) );
    lines.push( Line::from( vec![
        Span::styled( " Mix [", Style::default().fg( Color::Gray ) ),
        Span::styled( create_meter( total_filled, meter_width ), Style::default().fg( Color::Green ) ),
        Span::styled( "]", Style::default().fg( Color::Gray ) ),
    ]));
    lines.push( Line::from( "" ) );
}


fn draw_settings( frame: &mut Frame, app: &App, area: Rect ) {
    let settings_items = vec![
        ( "Discord Rich Presence", app.settings.discord_enabled ),
        ( "System Media Controls (SMTC)", app.settings.smtc_enabled ),
    ];

    let items: Vec<ListItem> = settings_items.iter().enumerate().map( |( idx, ( name, enabled ) )| {
        let checkbox = if *enabled { "[x]" } else { "[ ]" };
        let style = if idx == app.settings_selected {
            Style::default().fg( Color::Yellow ).bold()
        } else {
            Style::default().fg( Color::White )
        };

        ListItem::new( format!( " {} {}", checkbox, name ) ).style( style )
    }).collect();

    let list = List::new( items )
        .block(
            Block::default()
                .title( " Settings " )
                .borders( Borders::ALL )
                .border_style( Style::default().fg( Color::Cyan ) )
        )
        .highlight_style( Style::default().fg( Color::Yellow ).bold() );

    frame.render_widget( list, area );
}


fn draw_status_bar( frame: &mut Frame, app: &App, area: Rect ) {
    let ( text, style ) = match app.input_mode {
        InputMode::Command => {
            ( format!( "/{}", app.input_buffer.content() ), Style::default().fg( Color::Yellow ) )
        }
        InputMode::Search => {
            ( format!( "Search: {}", app.input_buffer.content() ), Style::default().fg( Color::Yellow ) )
        }
        InputMode::Normal => {
            if let Some( ref msg ) = app.status_message {
                ( msg.clone(), Style::default().fg( Color::Green ) )
            } else {
                let hint = match app.view_mode {
                    ViewMode::Playlist => " [/]Cmd [Tab]Views [Space]Play [e]Edit [v]Vis [i]Info [?]Help [q]Quit ",
                    ViewMode::Browser => " [/]Cmd [Tab]Views [Enter]Open [a]Add [~]Home [?]Help ",
                    ViewMode::Help => " [?]Close [Esc]Close ",
                    ViewMode::TrackInfo => " [Tab]Views [Space]Play [←→]Skip [i/Esc]Close ",
                    ViewMode::Visualizer => " [Tab]Views [Space]Play [←→]Skip [v]Style [Esc]Close ",
                    ViewMode::Settings => " [↑↓]Navigate [Enter/Space]Toggle [Tab]Views [Esc]Close ",
                };
                ( hint.to_string(), Style::default().fg( Color::DarkGray ) )
            }
        }
    };

    let status = Paragraph::new( text ).style( style );
    frame.render_widget( status, area );

    // Show cursor in command/search mode
    if app.input_mode != InputMode::Normal {
        let cursor_x = area.x + 2 + app.input_buffer.cursor_char_pos() as u16;
        frame.set_cursor_position(( cursor_x, area.y ));
    }
}
