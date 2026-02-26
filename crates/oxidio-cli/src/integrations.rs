//! Background integrations worker.
//!
//! Manages Discord Rich Presence and System Media Transport Controls (SMTC)
//! independently of the TUI. Runs on its own thread so these integrations
//! work in both TUI and daemon modes.

use std::path::PathBuf;
use std::sync::{ mpsc, Arc };
use std::time::Duration;

use tokio::sync::broadcast;

#[cfg( target_os = "windows" )]
use souvlaki::{ MediaMetadata, MediaPlayback };

use oxidio_core::player::PlaybackState;
use oxidio_core::Player;
use oxidio_ctl::ProcessorSettings;
use oxidio_protocol::{ AppCommand, StateUpdate };

use crate::discord::DiscordPresence;
use crate::media_controls::{ MediaControlCommand, MediaControlsHandler };


/// Converts a file path to a file:// URL for SMTC album art.
#[cfg( target_os = "windows" )]
fn path_to_file_url( path: &std::path::Path ) -> Option<String> {
    let abs_path = path.canonicalize().ok()?;
    let path_str = abs_path.to_string_lossy();
    let clean_path = path_str.strip_prefix( r"\\?\" ).unwrap_or( &path_str );
    let url = format!( "file://{}", clean_path );
    tracing::debug!( "Generated cover URL: {}", url );
    Some( url )
}


/// Finds album art in the same folder as the track.
/// Returns a file:// URL if found.
#[cfg( target_os = "windows" )]
fn find_album_art( track_path: &std::path::Path ) -> Option<String> {
    let parent = track_path.parent()?;

    let art_names = [
        "cover", "folder", "album", "front", "art", "albumart", "album_art",
    ];
    let extensions = [ "jpg", "jpeg", "png", "bmp", "gif" ];

    tracing::debug!( "Looking for album art in: {:?}", parent );

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
                            tracing::debug!( "Found album art: {:?}", path );
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

    if found_path.is_none() {
        tracing::debug!( "No album art found in {:?}", parent );
    }

    let source_path = found_path?;
    copy_to_temp_and_get_url( &source_path )
}


/// Copies a file to the temp directory and returns a file:// URL to the copy.
/// This is needed because SMTC can't access network paths directly.
#[cfg( target_os = "windows" )]
fn copy_to_temp_and_get_url( source: &std::path::Path ) -> Option<String> {
    use std::os::windows::fs::MetadataExt;

    let temp_dir = std::env::temp_dir();
    let oxidio_temp = temp_dir.join( "oxidio" );

    tracing::debug!( "Attempting to copy album art from: {:?}", source );

    if !oxidio_temp.exists() {
        if let Err( e ) = std::fs::create_dir_all( &oxidio_temp ) {
            tracing::warn!( "Failed to create temp dir {:?}: {}", oxidio_temp, e );
            return None;
        }
    }

    let ext = source.extension().and_then( |e| e.to_str() ).unwrap_or( "jpg" );
    let dest_path = oxidio_temp.join( format!( "cover.{}", ext ) );

    if dest_path.exists() {
        if let Ok( metadata ) = std::fs::metadata( &dest_path ) {
            let attrs = metadata.file_attributes();
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

    match std::fs::copy( source, &dest_path ) {
        Ok( bytes ) => {
            tracing::debug!( "Copied {} bytes to {:?}", bytes, dest_path );
        }
        Err( e ) => {
            tracing::warn!( "Failed to copy album art from {:?} to {:?}: {}", source, dest_path, e );
            return None;
        }
    }

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


/// Updates SMTC metadata and playback state.
#[cfg( target_os = "windows" )]
fn update_smtc(
    player: &Player,
    controls: &mut MediaControlsHandler,
    last_smtc_state: &mut Option<PlaybackState>,
    last_smtc_track: &mut Option<PathBuf>,
    force_update: &mut bool,
) {
    let current_state = player.state();
    let current_track = player.current_track();

    // Update playback state if changed
    if *last_smtc_state != Some( current_state ) {
        let playback = match current_state {
            PlaybackState::Playing => MediaPlayback::Playing { progress: None },
            PlaybackState::Paused => MediaPlayback::Paused { progress: None },
            PlaybackState::Stopped => MediaPlayback::Stopped,
        };
        controls.set_playback( playback );
        *last_smtc_state = Some( current_state );
    }

    // Update metadata if track changed or forced
    let should_update = *force_update || *last_smtc_track != current_track;
    if should_update {
        *force_update = false;

        if let Some( ref track_path ) = current_track {
            let metadata = player.metadata();

            let title = metadata.as_ref()
                .and_then( |m| m.title.clone() )
                .or_else( || {
                    track_path.file_stem()
                        .map( |n| n.to_string_lossy().to_string() )
                });

            let artist = metadata.as_ref().and_then( |m| m.artist.clone() );
            let album = metadata.as_ref().and_then( |m| m.album.clone() );

            let cover_url = find_album_art( track_path ).filter( |_| {
                let temp_path = std::env::temp_dir().join( "oxidio" );
                temp_path.read_dir()
                    .map( |mut entries| entries.any( |e| {
                        e.map( |e| e.file_name().to_string_lossy().starts_with( "cover." ) )
                            .unwrap_or( false )
                    }))
                    .unwrap_or( false )
            });

            tracing::debug!(
                "SMTC update: title={:?}, artist={:?}, album={:?}, cover_url={:?}",
                title, artist, album, cover_url
            );

            if let Some( e ) = controls.set_metadata( MediaMetadata {
                title: title.as_deref(),
                artist: artist.as_deref(),
                album: album.as_deref(),
                cover_url: cover_url.as_deref(),
                duration: player.duration(),
            }) {
                tracing::warn!( "SMTC error: {}", e );
            }
        }
        *last_smtc_track = current_track;
    }
}


/// Stub for non-Windows platforms.
#[cfg( not( target_os = "windows" ) )]
fn update_smtc(
    _player: &Player,
    _controls: &mut MediaControlsHandler,
    _last_smtc_state: &mut Option<PlaybackState>,
    _last_smtc_track: &mut Option<PathBuf>,
    _force_update: &mut bool,
) {
    // Media controls not available on this platform
}


/// Updates Discord Rich Presence based on player state.
fn update_discord(
    player: &Player,
    discord: &mut DiscordPresence,
    last_discord_track: &mut Option<PathBuf>,
) {
    let current_track = player.current_track();
    let current_state = player.state();

    // Clear presence if stopped or paused
    if current_state != PlaybackState::Playing {
        if last_discord_track.is_some() {
            discord.clear();
            *last_discord_track = None;
        }
        return;
    }

    // Update if track changed
    if *last_discord_track != current_track {
        if let Some( ref track_path ) = current_track {
            let metadata = player.metadata();

            let title = metadata.as_ref()
                .and_then( |m| m.title.clone() )
                .or_else( || {
                    track_path.file_stem()
                        .map( |n| n.to_string_lossy().to_string() )
                });

            let artist = metadata.as_ref().and_then( |m| m.artist.clone() );
            let album = metadata.as_ref().and_then( |m| m.album.clone() );

            discord.update(
                title.as_deref(),
                artist.as_deref(),
                album.as_deref(),
            );
        }
        *last_discord_track = current_track;
    }
}


/// Runs the background integrations worker.
///
/// Manages Discord Rich Presence and SMTC based on player state and
/// settings changes received from the broadcast channel. Forwards SMTC
/// media key events as AppCommands through the command sender.
///
/// @param player - Shared player instance
/// @param command_sender - Command sender for forwarding media key commands
/// @param state_rx - Broadcast receiver for state updates
/// @param settings - Initial settings snapshot
pub fn run_integrations(
    player: Arc<Player>,
    command_sender: oxidio_ctl::CommandSender,
    mut state_rx: broadcast::Receiver<StateUpdate>,
    settings: &ProcessorSettings,
) {
    // Discord Rich Presence
    let mut discord_enabled = settings.discord_enabled;
    let mut discord = if discord_enabled {
        DiscordPresence::new()
    } else {
        DiscordPresence::new_inactive()
    };
    let mut last_discord_track: Option<PathBuf> = None;

    // System Media Controls (SMTC/MPRIS)
    let mut smtc_enabled = settings.smtc_enabled;
    let ( smtc_tx, smtc_rx ) = mpsc::channel::<MediaControlCommand>();
    let mut media_controls: Option<MediaControlsHandler> = if smtc_enabled {
        MediaControlsHandler::new( smtc_tx.clone() )
    } else {
        None
    };
    let mut last_smtc_state: Option<PlaybackState> = None;
    let mut last_smtc_track: Option<PathBuf> = None;
    let mut force_smtc_update = smtc_enabled;

    if discord_enabled {
        tracing::info!( "Discord Rich Presence initialized" );
    }
    if smtc_enabled && media_controls.is_some() {
        tracing::info!( "System media controls initialized" );
    }

    loop {
        // Drain broadcast for settings changes
        loop {
            match state_rx.try_recv() {
                Ok( StateUpdate::SettingsChanged { settings } ) => {
                    // Discord toggle
                    if settings.discord_enabled != discord_enabled {
                        discord_enabled = settings.discord_enabled;
                        if discord_enabled {
                            tracing::info!( "Discord Rich Presence enabled" );
                            discord = DiscordPresence::new();
                        } else {
                            tracing::info!( "Discord Rich Presence disabled" );
                            discord.clear();
                            last_discord_track = None;
                        }
                    }
                    // SMTC toggle
                    if settings.smtc_enabled != smtc_enabled {
                        smtc_enabled = settings.smtc_enabled;
                        if smtc_enabled {
                            tracing::info!( "System media controls enabled" );
                            media_controls = MediaControlsHandler::new( smtc_tx.clone() );
                            force_smtc_update = true;
                        } else {
                            tracing::info!( "System media controls disabled" );
                            media_controls = None;
                            last_smtc_state = None;
                            last_smtc_track = None;
                        }
                    }
                }
                Ok( _ ) => {} // Ignore other updates
                Err( broadcast::error::TryRecvError::Empty ) => break,
                Err( broadcast::error::TryRecvError::Lagged( _ ) ) => continue,
                Err( broadcast::error::TryRecvError::Closed ) => return,
            }
        }

        // Process SMTC media key events and forward as AppCommands
        if smtc_enabled {
            while let Ok( cmd ) = smtc_rx.try_recv() {
                let app_cmd = match cmd {
                    MediaControlCommand::Play => AppCommand::TogglePlayback,
                    MediaControlCommand::Pause => AppCommand::Pause,
                    MediaControlCommand::Toggle => AppCommand::TogglePlayback,
                    MediaControlCommand::Stop => AppCommand::Stop,
                    MediaControlCommand::Next => AppCommand::Next,
                    MediaControlCommand::Previous => AppCommand::Previous,
                };
                let _ = command_sender.try_send( app_cmd );
            }
        }

        // Update SMTC
        if smtc_enabled {
            if let Some( ref mut controls ) = media_controls {
                update_smtc(
                    &player, controls,
                    &mut last_smtc_state, &mut last_smtc_track,
                    &mut force_smtc_update,
                );
            }
        }

        // Update Discord
        if discord_enabled {
            update_discord( &player, &mut discord, &mut last_discord_track );
        }

        std::thread::sleep( Duration::from_millis( 100 ) );
    }
}
