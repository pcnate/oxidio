//! Oxidio CLI - Terminal UI music player

mod browser;
mod cli;
mod discord;
mod input;
mod integrations;
mod media_controls;
mod settings;
mod view;

use std::io;
use std::path::PathBuf;
use std::sync::Arc;
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

use browser::FileBrowser;
use cli::Args;
use input::{ InputBuffer, InputMode };
use view::{ ViewMode, VisualizerStyle };

use oxidio_core::{
    command::{ self, RepeatModeArg },
    library::LibraryScanner,
    player::PlaybackState,
    Command, Player, RepeatMode,
};
use oxidio_ctl::{ CommandProcessor, CommandSender, ControlChannel, ProcessorSettings };
use oxidio_protocol::{ AppCommand, StateUpdate };


/// Application state.
struct App {
    player: Arc<Player>,
    command_sender: CommandSender,
    state_rx: tokio::sync::broadcast::Receiver<StateUpdate>,
    should_quit: bool,

    // CLI web override (Some(true) = --web, Some(false) = --no-web, None = use settings)
    cli_web_override: Option<bool>,

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

    // Track change detection (for auto-scroll on advance)
    last_track: Option<PathBuf>,

    // Status message (shown in status bar)
    status_message: Option<String>,
    status_clear_at: Option<std::time::Instant>,

    // Settings
    settings: settings::Settings,
    settings_selected: usize,
}


impl App {
    /// Creates a new App instance.
    ///
    /// The Player and playlist must already be initialized (created and loaded
    /// in main). App reads initial state (volume, playlist index) from the player.
    ///
    /// @param player - Shared player instance (also held by the CommandProcessor)
    /// @param command_sender - Sender for the control channel
    /// @param state_rx - Broadcast receiver for state updates from the processor
    /// @param args - CLI arguments
    fn new( player: Arc<Player>, command_sender: CommandSender, state_rx: tokio::sync::broadcast::Receiver<StateUpdate>, args: &Args ) -> Result<Self> {
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

        // Read initial state from the shared player
        let volume = player.volume();
        let playlist_index = {
            let playlist_arc = player.playlist();
            let playlist = playlist_arc.read().unwrap();
            playlist.current_index()
        };

        let mut playlist_state = ListState::default();
        if playlist_index.is_some() {
            playlist_state.select( playlist_index );
        }

        Ok( Self {
            player,
            command_sender,
            state_rx,
            should_quit: false,
            cli_web_override: args.cli_web_override(),
            view_mode,
            playlist_state,
            browser,
            input_mode: InputMode::Normal,
            input_buffer: InputBuffer::new(),
            edit_mode: false,
            visualizer_style: VisualizerStyle::default(),
            volume,
            scroll_to_playing: false,
            last_click_time: None,
            last_click_row: None,
            playlist_area: None,
            help_scroll: 0,
            last_track: None,
            status_message: None,
            status_clear_at: None,
            settings: settings::Settings::load(),
            settings_selected: 0,
        })
    }


    /// Sets a status message that auto-clears after a delay.
    fn set_status( &mut self, msg: impl Into<String> ) {
        self.status_message = Some( msg.into() );
        self.status_clear_at = Some( std::time::Instant::now() + Duration::from_secs( 3 ) );
    }


    /// Sends a command to the control channel (non-blocking).
    fn send_command( &self, cmd: AppCommand ) {
        if let Err( e ) = self.command_sender.try_send( cmd ) {
            tracing::warn!( "Failed to send command: {}", e );
        }
    }


    /// Updates app state (clears expired messages, detects track changes, syncs settings).
    fn tick( &mut self ) {
        // Clear expired status messages
        if let Some( clear_at ) = self.status_clear_at {
            if std::time::Instant::now() >= clear_at {
                self.status_message = None;
                self.status_clear_at = None;
            }
        }

        // Drain broadcast receiver for settings display sync
        // (Discord/SMTC are managed by the integrations worker, but the TUI
        // needs to know current settings values for the settings view)
        loop {
            match self.state_rx.try_recv() {
                Ok( StateUpdate::SettingsChanged { settings } ) => {
                    self.settings.discord_enabled = settings.discord_enabled;
                    self.settings.smtc_enabled = settings.smtc_enabled;
                    self.settings.web_enabled = settings.web_enabled;
                }
                Ok( _ ) => {} // Ignore other updates
                Err( tokio::sync::broadcast::error::TryRecvError::Empty ) => break,
                Err( tokio::sync::broadcast::error::TryRecvError::Lagged( _ ) ) => continue,
                Err( tokio::sync::broadcast::error::TryRecvError::Closed ) => break,
            }
        }

        // Detect track changes from the CommandProcessor (auto-advance)
        {
            let current_track = self.player.current_track();
            let playlist_index = {
                let playlist_arc = self.player.playlist();
                let playlist = playlist_arc.read().unwrap();
                playlist.current_index()
            };

            // If the playing track changed (e.g. auto-advance by processor), scroll to it
            if playlist_index.is_some() && self.player.state() == PlaybackState::Playing {
                if current_track != self.last_track {
                    self.scroll_to_playing = true;
                }
            }
            self.last_track = current_track;
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
                    PlaybackState::Playing | PlaybackState::Paused => {
                        self.send_command( AppCommand::TogglePlayback );
                    }
                    PlaybackState::Stopped => {
                        // Start playing selected track
                        self.play_selected();
                    }
                }
            }
            KeyCode::Char( 's' ) if !self.edit_mode => {
                self.send_command( AppCommand::Stop );
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
                        self.send_command( AppCommand::Seek { position_secs: new_pos.as_secs_f64() } );
                    }
                }
            }
            KeyCode::Left if modifiers.contains( KeyModifiers::CONTROL ) => {
                // Seek backward 10 seconds
                let pos = self.player.position();
                let new_pos = pos.saturating_sub( Duration::from_secs( 10 ) );
                self.send_command( AppCommand::Seek { position_secs: new_pos.as_secs_f64() } );
            }
            KeyCode::Right => {
                self.play_next();
            }
            KeyCode::Left => {
                self.play_previous();
            }
            KeyCode::Char( 'c' ) => {
                self.send_command( AppCommand::ClearPlaylist );
                self.set_status( "Playlist cleared" );
            }
            KeyCode::Char( 'r' ) => {
                // Cycle repeat mode
                let playlist_arc = self.player.playlist();
                let playlist = playlist_arc.read().unwrap();
                let new_mode = match playlist.repeat() {
                    RepeatMode::Off => RepeatMode::One,
                    RepeatMode::One => RepeatMode::All,
                    RepeatMode::All => RepeatMode::Off,
                };
                drop( playlist );
                self.send_command( AppCommand::CycleRepeat );
                self.set_status( format!( "Repeat: {:?}", new_mode ) );
            }
            KeyCode::Char( 'S' ) => {
                // Toggle shuffle
                let playlist_arc = self.player.playlist();
                let playlist = playlist_arc.read().unwrap();
                let new_shuffle = !playlist.shuffle();
                drop( playlist );
                self.send_command( AppCommand::ToggleShuffle );
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
                self.send_command( AppCommand::SetVolume { level: self.volume } );
                self.set_status( format!( "Volume: {}%", ( self.volume * 100.0 ) as i32 ) );
            }
            KeyCode::Char( '-' ) | KeyCode::Char( '_' ) => {
                // Volume down
                self.volume = ( self.volume - 0.05 ).max( 0.0 );
                self.send_command( AppCommand::SetVolume { level: self.volume } );
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
                self.send_command( AppCommand::SetVolume { level: self.volume } );
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
                    self.send_command( AppCommand::AddPath { path: file_path.to_string_lossy().to_string() } );
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
                        self.send_command( AppCommand::AddPath { path: path.to_string_lossy().to_string() } );
                        self.set_status( "Adding to playlist..." );
                    } else if is_audio {
                        self.send_command( AppCommand::AddPath { path: path.to_string_lossy().to_string() } );
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
                self.send_command( AppCommand::TogglePlayback );
            }
            KeyCode::Char( 'n' ) => self.play_next(),
            KeyCode::Char( 'p' ) => self.play_previous(),
            KeyCode::Left => self.play_previous(),
            KeyCode::Right => self.play_next(),
            KeyCode::Char( '+' ) | KeyCode::Char( '=' ) => {
                self.volume = ( self.volume + 0.05 ).min( 1.0 );
                self.send_command( AppCommand::SetVolume { level: self.volume } );
                self.set_status( format!( "Volume: {}%", ( self.volume * 100.0 ) as i32 ) );
            }
            KeyCode::Char( '-' ) | KeyCode::Char( '_' ) => {
                self.volume = ( self.volume - 0.05 ).max( 0.0 );
                self.send_command( AppCommand::SetVolume { level: self.volume } );
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
                self.send_command( AppCommand::SetVolume { level: self.volume } );
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
                self.send_command( AppCommand::TogglePlayback );
            }
            KeyCode::Char( 'n' ) => self.play_next(),
            KeyCode::Char( 'p' ) => self.play_previous(),
            KeyCode::Right if modifiers.contains( KeyModifiers::CONTROL ) => {
                let pos = self.player.position();
                let new_pos = pos + Duration::from_secs( 10 );
                if let Some( duration ) = self.player.duration() {
                    if new_pos < duration {
                        self.send_command( AppCommand::Seek { position_secs: new_pos.as_secs_f64() } );
                    }
                }
            }
            KeyCode::Left if modifiers.contains( KeyModifiers::CONTROL ) => {
                let pos = self.player.position();
                let new_pos = pos.saturating_sub( Duration::from_secs( 10 ) );
                self.send_command( AppCommand::Seek { position_secs: new_pos.as_secs_f64() } );
            }
            KeyCode::Right => self.play_next(),
            KeyCode::Left => self.play_previous(),
            KeyCode::Char( '+' ) | KeyCode::Char( '=' ) => {
                self.volume = ( self.volume + 0.05 ).min( 1.0 );
                self.send_command( AppCommand::SetVolume { level: self.volume } );
                self.set_status( format!( "Volume: {}%", ( self.volume * 100.0 ) as i32 ) );
            }
            KeyCode::Char( '-' ) | KeyCode::Char( '_' ) => {
                self.volume = ( self.volume - 0.05 ).max( 0.0 );
                self.send_command( AppCommand::SetVolume { level: self.volume } );
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
                self.send_command( AppCommand::SetVolume { level: self.volume } );
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
                self.send_command( AppCommand::TogglePlayback );
            }
            KeyCode::Char( 'n' ) => self.play_next(),
            KeyCode::Char( 'p' ) => self.play_previous(),
            KeyCode::Right if modifiers.contains( KeyModifiers::CONTROL ) => {
                let pos = self.player.position();
                let new_pos = pos + Duration::from_secs( 10 );
                if let Some( duration ) = self.player.duration() {
                    if new_pos < duration {
                        self.send_command( AppCommand::Seek { position_secs: new_pos.as_secs_f64() } );
                    }
                }
            }
            KeyCode::Left if modifiers.contains( KeyModifiers::CONTROL ) => {
                let pos = self.player.position();
                let new_pos = pos.saturating_sub( Duration::from_secs( 10 ) );
                self.send_command( AppCommand::Seek { position_secs: new_pos.as_secs_f64() } );
            }
            KeyCode::Right => self.play_next(),
            KeyCode::Left => self.play_previous(),
            KeyCode::Char( '+' ) | KeyCode::Char( '=' ) => {
                self.volume = ( self.volume + 0.05 ).min( 1.0 );
                self.send_command( AppCommand::SetVolume { level: self.volume } );
                self.set_status( format!( "Volume: {}%", ( self.volume * 100.0 ) as i32 ) );
            }
            KeyCode::Char( '-' ) | KeyCode::Char( '_' ) => {
                self.volume = ( self.volume - 0.05 ).max( 0.0 );
                self.send_command( AppCommand::SetVolume { level: self.volume } );
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
                self.send_command( AppCommand::SetVolume { level: self.volume } );
            }
            _ => {}
        }
    }


    fn handle_settings_key( &mut self, code: KeyCode ) {
        // Number of settings items
        const SETTINGS_COUNT: usize = 3;

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
                // Toggle the selected setting (sends command to processor;
                // integrations worker handles the actual enable/disable)
                match self.settings_selected {
                    0 => {
                        self.send_command( AppCommand::ToggleSetting { key: "discord_enabled".to_string() } );
                        self.settings.discord_enabled = !self.settings.discord_enabled;
                    }
                    1 => {
                        self.send_command( AppCommand::ToggleSetting { key: "smtc_enabled".to_string() } );
                        self.settings.smtc_enabled = !self.settings.smtc_enabled;
                    }
                    2 => {
                        // Web toggle — skip if locked by CLI args
                        if self.cli_web_override.is_none() {
                            self.send_command( AppCommand::ToggleSetting { key: "web_enabled".to_string() } );
                            self.settings.web_enabled = !self.settings.web_enabled;
                            self.set_status( "Web setting saved. Restart oxidio to apply." );
                        } else {
                            self.set_status( "Web interface is locked by CLI args" );
                        }
                    }
                    _ => {}
                }
            }
            // Playback controls
            KeyCode::Char( ' ' ) => {
                self.send_command( AppCommand::TogglePlayback );
            }
            KeyCode::Char( 'n' ) => self.play_next(),
            KeyCode::Char( 'p' ) => self.play_previous(),
            KeyCode::Left => self.play_previous(),
            KeyCode::Right => self.play_next(),
            KeyCode::Char( '+' ) | KeyCode::Char( '=' ) => {
                self.volume = ( self.volume + 0.05 ).min( 1.0 );
                self.send_command( AppCommand::SetVolume { level: self.volume } );
                self.set_status( format!( "Volume: {}%", ( self.volume * 100.0 ) as i32 ) );
            }
            KeyCode::Char( '-' ) | KeyCode::Char( '_' ) => {
                self.volume = ( self.volume - 0.05 ).max( 0.0 );
                self.send_command( AppCommand::SetVolume { level: self.volume } );
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
                self.send_command( AppCommand::SetVolume { level: self.volume } );
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
                self.send_command( AppCommand::AddPath { path: path.to_string_lossy().to_string() } );
                self.set_status( "Adding to playlist..." );
            }
            Command::Remove => {
                self.delete_selected_track();
            }
            Command::Clear => {
                self.send_command( AppCommand::ClearPlaylist );
                self.set_status( "Playlist cleared" );
            }
            Command::Dedup => {
                self.send_command( AppCommand::Dedup );
                self.set_status( "Deduplicating..." );
            }
            Command::Shuffle => {
                self.send_command( AppCommand::ToggleShuffle );
                self.set_status( "Shuffle toggled" );
            }
            Command::Repeat { mode } => {
                match mode {
                    Some( RepeatModeArg::Off ) => self.send_command( AppCommand::SetRepeat { mode: oxidio_protocol::RepeatModeValue::Off } ),
                    Some( RepeatModeArg::One ) => self.send_command( AppCommand::SetRepeat { mode: oxidio_protocol::RepeatModeValue::One } ),
                    Some( RepeatModeArg::All ) => self.send_command( AppCommand::SetRepeat { mode: oxidio_protocol::RepeatModeValue::All } ),
                    None => self.send_command( AppCommand::CycleRepeat ),
                };
                self.set_status( "Repeat mode changed" );
            }
            Command::Play => {
                self.play_selected();
            }
            Command::Pause => {
                self.send_command( AppCommand::Pause );
                self.set_status( "Paused" );
            }
            Command::Stop => {
                self.send_command( AppCommand::Stop );
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
                self.send_command( AppCommand::SavePlaylist { name } );
                self.set_status( "Saving playlist..." );
            }
            Command::Load { name } => {
                self.send_command( AppCommand::LoadPlaylist { name } );
                self.set_status( "Loading playlist..." );
            }
            Command::ListPlaylists => {
                self.send_command( AppCommand::ListPlaylists );
            }
            Command::DeletePlaylist { name } => {
                self.send_command( AppCommand::DeletePlaylist { name } );
            }
            Command::Seek { position } => {
                self.send_command( AppCommand::Seek { position_secs: position.as_secs_f64() } );
                self.set_status( format!( "Seeking to {}:{:02}", position.as_secs() / 60, position.as_secs() % 60 ) );
            }
            Command::Vis => {
                self.visualizer_style = self.visualizer_style.next();
                self.set_status( format!( "Visualizer: {}", self.visualizer_style.name() ) );
            }
            Command::Volume { level } => {
                if let Some( level ) = level {
                    self.volume = ( level as f32 / 100.0 ).clamp( 0.0, 1.0 );
                    self.send_command( AppCommand::SetVolume { level: self.volume } );
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
            self.send_command( AppCommand::PlayTrack { index: idx } );
        }
    }


    fn play_next( &mut self ) {
        self.send_command( AppCommand::Next );
    }


    fn play_previous( &mut self ) {
        self.send_command( AppCommand::Previous );
    }


    fn move_track_down( &mut self ) {
        if let Some( idx ) = self.playlist_state.selected() {
            let len = self.player.playlist().read().unwrap().len();
            if idx < len.saturating_sub( 1 ) {
                self.send_command( AppCommand::MoveTrack { from: idx, to: idx + 1 } );
                self.playlist_state.select( Some( idx + 1 ) );
            }
        }
    }


    fn move_track_up( &mut self ) {
        if let Some( idx ) = self.playlist_state.selected() {
            if idx > 0 {
                self.send_command( AppCommand::MoveTrack { from: idx, to: idx - 1 } );
                self.playlist_state.select( Some( idx - 1 ) );
            }
        }
    }


    fn delete_selected_track( &mut self ) {
        if let Some( idx ) = self.playlist_state.selected() {
            let len = self.player.playlist().read().unwrap().len();
            self.send_command( AppCommand::RemoveTrack { index: idx } );
            // Adjust selection for the removed item
            let new_len = len.saturating_sub( 1 );
            if new_len == 0 {
                self.playlist_state.select( None );
            } else if idx >= new_len {
                self.playlist_state.select( Some( new_len - 1 ) );
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

    // Create the shared player
    let player = Arc::new( Player::new()? );

    // Load initial playlist from CLI files or last session
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
                    if let Some( idx ) = session.track_index {
                        playlist.jump_to( idx );
                    }
                    player.set_volume( session.volume );
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

    // Determine starting directory for the processor's browser
    let start_path = args.path.clone()
        .or_else( || dirs::home_dir() )
        .unwrap_or_else( || PathBuf::from( "." ) );

    // Create control channel
    let mut channel = ControlChannel::new();
    let command_sender = channel.sender();
    let command_rx = channel.take_command_rx().expect( "Command receiver already taken" );
    let broadcast_tx = channel.broadcast_tx();

    // Load processor settings and create command processor
    let proc_settings = ProcessorSettings::load();
    let settings_web_enabled = proc_settings.web_enabled;
    let integrations_discord = proc_settings.discord_enabled;
    let integrations_smtc = proc_settings.smtc_enabled;
    let processor_player = Arc::clone( &player );
    let browse = args.browse;
    let mut processor = CommandProcessor::new(
        processor_player,
        proc_settings,
        start_path,
        browse,
        command_rx,
        broadcast_tx,
    );

    // Spawn command processor on a background thread with its own tokio runtime
    let web_sender = channel.sender();
    let web_broadcast_tx = channel.broadcast_tx();
    let web_enabled = args.resolve_web_enabled( settings_web_enabled );
    let web_bind = args.web_bind.clone();
    let web_port = args.web_port;

    std::thread::Builder::new()
        .name( "oxidio-processor".to_string() )
        .spawn( move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect( "Failed to create tokio runtime for command processor" );
            rt.block_on( processor.run() );
        })
        .expect( "Failed to spawn command processor thread" );

    // Spawn web server if enabled
    #[cfg( feature = "web" )]
    let _web_handle = if web_enabled {
        let handle = std::thread::Builder::new()
            .name( "oxidio-web".to_string() )
            .spawn( move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect( "Failed to create tokio runtime for web server" );
                rt.block_on( async {
                    match oxidio_web::start_web_server(
                        &web_bind, web_port, web_sender, web_broadcast_tx,
                    ).await {
                        Ok( handle ) => {
                            tracing::info!( "Web server started on http://{}:{}", web_bind, web_port );
                            // Keep the runtime alive while the server runs
                            // The handle will be dropped when this thread exits
                            std::future::pending::<()>().await;
                            drop( handle );
                        }
                        Err( e ) => {
                            tracing::error!( "Failed to start web server: {}", e );
                        }
                    }
                });
            })
            .ok();
        handle
    } else {
        None
    };

    // Spawn integrations worker (Discord Rich Presence + SMTC)
    // Runs on its own thread so these work in both TUI and daemon modes
    {
        let integrations_player = Arc::clone( &player );
        let integrations_sender = channel.sender();
        let integrations_rx = channel.subscribe();
        let integrations_settings = ProcessorSettings {
            discord_enabled: integrations_discord,
            smtc_enabled: integrations_smtc,
            web_enabled: settings_web_enabled,
            ..ProcessorSettings::default()
        };
        std::thread::Builder::new()
            .name( "oxidio-integrations".to_string() )
            .spawn( move || {
                integrations::run_integrations(
                    integrations_player,
                    integrations_sender,
                    integrations_rx,
                    &integrations_settings,
                );
            })
            .expect( "Failed to spawn integrations thread" );
    }

    // Branch: daemon mode (headless) vs TUI mode
    if args.daemon {
        tracing::info!( "Running in daemon mode (headless). Press Ctrl+C to stop." );

        // Block until the process is killed
        // Future: this is where IPC socket listening for CLI oneshots will go
        loop {
            std::thread::sleep( Duration::from_secs( 1 ) );
        }
    }

    // --- TUI mode ---

    // Setup terminal
    enable_raw_mode()?;
    io::stdout().execute( EnterAlternateScreen )?;
    io::stdout().execute( crossterm::event::EnableMouseCapture )?;

    let mut terminal = Terminal::new( CrosstermBackend::new( io::stdout() ) )?;

    // Create TUI app (reads initial state from the shared player)
    let state_rx = channel.subscribe();
    let mut app = App::new( player, command_sender, state_rx, &args )?;

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
    let web_locked = app.cli_web_override.is_some();
    let web_enabled = if let Some( locked ) = app.cli_web_override {
        locked
    } else {
        app.settings.web_enabled
    };

    // Build settings entries: ( name, enabled, locked )
    let settings_items: Vec<( &str, bool, bool )> = vec![
        ( "Discord Rich Presence", app.settings.discord_enabled, false ),
        ( "System Media Controls (SMTC)", app.settings.smtc_enabled, false ),
        ( "Web Interface", web_enabled, web_locked ),
    ];

    let items: Vec<ListItem> = settings_items.iter().enumerate().map( |( idx, ( name, enabled, locked ) )| {
        let checkbox = if *locked {
            if *enabled { "[-]" } else { "[-]" }
        } else if *enabled {
            "[x]"
        } else {
            "[ ]"
        };

        let label = if *locked {
            format!( " {} {} (locked by CLI)", checkbox, name )
        } else if idx == 2 {
            if *enabled {
                format!( " {} {} (port {}, restart to apply changes)", checkbox, name, app.settings.web_port )
            } else {
                format!( " {} {} (restart to apply changes)", checkbox, name )
            }
        } else {
            format!( " {} {}", checkbox, name )
        };

        let style = if *locked {
            if idx == app.settings_selected {
                Style::default().fg( Color::DarkGray ).bold()
            } else {
                Style::default().fg( Color::DarkGray )
            }
        } else if idx == app.settings_selected {
            Style::default().fg( Color::Yellow ).bold()
        } else {
            Style::default().fg( Color::White )
        };

        ListItem::new( label ).style( style )
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
