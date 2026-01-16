//! Discord Rich Presence integration
//!
//! Shows currently playing track in Discord status.

use discord_rich_presence::{ activity, DiscordIpc, DiscordIpcClient };
use std::time::{ SystemTime, UNIX_EPOCH };

const DISCORD_APP_ID: &str = "1461837956825485343";


/// Handles Discord Rich Presence updates.
pub struct DiscordPresence {
    client: Option<DiscordIpcClient>,
    connected: bool,
    last_track: Option<String>,
}


impl DiscordPresence {
    /// Creates a new Discord presence handler and attempts to connect.
    pub fn new() -> Self {
        let mut handler = Self {
            client: None,
            connected: false,
            last_track: None,
        };
        handler.connect();
        handler
    }


    /// Attempts to connect to Discord.
    fn connect( &mut self ) {
        let mut client = DiscordIpcClient::new( DISCORD_APP_ID );

        match client.connect() {
            Ok(()) => {
                tracing::info!( "Connected to Discord Rich Presence" );
                self.client = Some( client );
                self.connected = true;
            }
            Err( e ) => {
                tracing::debug!( "Discord not available: {:?}", e );
            }
        }
    }


    /// Updates the Discord presence with current track info.
    pub fn update( &mut self, title: Option<&str>, artist: Option<&str>, album: Option<&str> ) {
        // Create a track identifier to avoid unnecessary updates
        let track_id = title.map( |t| t.to_string() );

        if track_id == self.last_track {
            return;
        }
        self.last_track = track_id;

        // Try to reconnect if not connected
        if !self.connected {
            self.connect();
            if !self.connected {
                return;
            }
        }

        let client = match &mut self.client {
            Some( c ) => c,
            None => return,
        };

        let title = title.unwrap_or( "Unknown Track" );
        let artist = artist.unwrap_or( "Unknown Artist" );

        // Build activity
        let details = title;
        let state = if let Some( alb ) = album {
            format!( "{} - {}", artist, alb )
        } else {
            artist.to_string()
        };

        // Get current timestamp for "elapsed" display
        let timestamp = SystemTime::now()
            .duration_since( UNIX_EPOCH )
            .map( |d| d.as_secs() as i64 )
            .unwrap_or( 0 );

        let activity = activity::Activity::new()
            .details( details )
            .state( &state )
            .assets(
                activity::Assets::new()
                    .large_image( "icon" )
                    .large_text( "Oxidio Music Player" )
            )
            .timestamps(
                activity::Timestamps::new()
                    .start( timestamp )
            );

        if let Err( e ) = client.set_activity( activity ) {
            tracing::debug!( "Failed to set Discord activity: {:?}", e );
            self.connected = false;
        }
    }


    /// Clears the Discord presence (when stopped/paused).
    pub fn clear( &mut self ) {
        if !self.connected {
            return;
        }

        if let Some( client ) = &mut self.client {
            let _ = client.clear_activity();
        }

        self.last_track = None;
    }
}


impl Drop for DiscordPresence {
    fn drop( &mut self ) {
        if let Some( client ) = &mut self.client {
            let _ = client.clear_activity();
            let _ = client.close();
        }
    }
}
