//! Playlist and queue management
//!
//! Handles track ordering, shuffle, repeat, and queue operations.

use std::fs::{ self, File };
use std::io::{ BufRead, BufReader, Write };
use std::path::{ Path, PathBuf };

use thiserror::Error;


/// Errors that can occur with playlist operations.
#[derive( Debug, Error )]
pub enum PlaylistError {
    #[error( "IO error: {0}" )]
    Io( #[from] std::io::Error ),

    #[error( "Invalid playlist format" )]
    InvalidFormat,
}


/// Repeat mode for the playlist.
#[derive( Debug, Clone, Copy, PartialEq, Eq, Default )]
pub enum RepeatMode {
    #[default]
    Off,
    One,
    All,
}


/// Session state for persistence across restarts.
#[derive( Debug, Clone )]
pub struct SessionState {
    pub playlist_name: String,
    pub track_index: Option<usize>,
    pub shuffle: bool,
    pub repeat: RepeatMode,
    pub volume: f32,
}


/// Playlist/queue manager.
#[derive( Debug, Default )]
pub struct Playlist {
    tracks: Vec<PathBuf>,
    current_index: Option<usize>,
    shuffle: bool,
    repeat: RepeatMode,
    // Shuffle order (indices into tracks)
    shuffle_order: Vec<usize>,
    shuffle_position: usize,
}


impl Playlist {
    /// Creates a new empty playlist.
    pub fn new() -> Self {
        Self::default()
    }


    /// Adds a track to the end of the playlist.
    pub fn add( &mut self, path: PathBuf ) {
        self.tracks.push( path );
        self.regenerate_shuffle_order();
    }


    /// Adds multiple tracks to the playlist.
    pub fn add_many( &mut self, paths: impl IntoIterator<Item = PathBuf> ) {
        self.tracks.extend( paths );
        self.regenerate_shuffle_order();
    }


    /// Clears the playlist.
    pub fn clear( &mut self ) {
        self.tracks.clear();
        self.current_index = None;
        self.shuffle_order.clear();
        self.shuffle_position = 0;
    }


    /// Removes a track at the specified index.
    pub fn remove( &mut self, index: usize ) -> Option<PathBuf> {
        if index >= self.tracks.len() {
            return None;
        }

        let removed = self.tracks.remove( index );

        // Adjust current index if needed
        if let Some( current ) = self.current_index {
            if index < current {
                self.current_index = Some( current - 1 );
            } else if index == current {
                self.current_index = None;
            }
        }

        self.regenerate_shuffle_order();
        Some( removed )
    }


    /// Gets the current track.
    pub fn current( &self ) -> Option<&PathBuf> {
        self.current_index.and_then( |i| self.tracks.get( i ) )
    }


    /// Advances to the next track.
    ///
    /// Returns the next track path, or None if at the end (and repeat is off).
    pub fn next( &mut self ) -> Option<&PathBuf> {
        if self.tracks.is_empty() {
            return None;
        }

        let next_index = if self.shuffle {
            self.shuffle_position += 1;
            if self.shuffle_position >= self.shuffle_order.len() {
                match self.repeat {
                    RepeatMode::Off => return None,
                    RepeatMode::All => {
                        self.regenerate_shuffle_order();
                        self.shuffle_position = 0;
                    }
                    RepeatMode::One => {
                        self.shuffle_position -= 1;
                    }
                }
            }
            self.shuffle_order.get( self.shuffle_position ).copied()
        } else {
            match self.repeat {
                RepeatMode::One => self.current_index,
                RepeatMode::Off | RepeatMode::All => {
                    let current = self.current_index.unwrap_or( 0 );
                    let next = current + 1;
                    if next >= self.tracks.len() {
                        match self.repeat {
                            RepeatMode::Off => return None,
                            RepeatMode::All => Some( 0 ),
                            RepeatMode::One => unreachable!(),
                        }
                    } else {
                        Some( next )
                    }
                }
            }
        };

        self.current_index = next_index;
        self.current()
    }


    /// Goes to the previous track.
    pub fn previous( &mut self ) -> Option<&PathBuf> {
        if self.tracks.is_empty() {
            return None;
        }

        let prev_index = if self.shuffle {
            if self.shuffle_position > 0 {
                self.shuffle_position -= 1;
                self.shuffle_order.get( self.shuffle_position ).copied()
            } else {
                self.shuffle_order.first().copied()
            }
        } else {
            let current = self.current_index.unwrap_or( 0 );
            if current > 0 {
                Some( current - 1 )
            } else if self.repeat == RepeatMode::All {
                Some( self.tracks.len() - 1 )
            } else {
                Some( 0 )
            }
        };

        self.current_index = prev_index;
        self.current()
    }


    /// Jumps to a specific track by index.
    pub fn jump_to( &mut self, index: usize ) -> Option<&PathBuf> {
        if index < self.tracks.len() {
            self.current_index = Some( index );
            self.current()
        } else {
            None
        }
    }


    /// Sets shuffle mode.
    pub fn set_shuffle( &mut self, shuffle: bool ) {
        if shuffle != self.shuffle {
            self.shuffle = shuffle;
            if shuffle {
                self.regenerate_shuffle_order();
            }
        }
    }


    /// Gets shuffle mode.
    pub fn shuffle( &self ) -> bool {
        self.shuffle
    }


    /// Sets repeat mode.
    pub fn set_repeat( &mut self, repeat: RepeatMode ) {
        self.repeat = repeat;
    }


    /// Gets repeat mode.
    pub fn repeat( &self ) -> RepeatMode {
        self.repeat
    }


    /// Gets all tracks in the playlist.
    pub fn tracks( &self ) -> &[PathBuf] {
        &self.tracks
    }


    /// Gets the number of tracks.
    pub fn len( &self ) -> usize {
        self.tracks.len()
    }


    /// Returns true if the playlist is empty.
    pub fn is_empty( &self ) -> bool {
        self.tracks.is_empty()
    }


    /// Gets the current track index.
    pub fn current_index( &self ) -> Option<usize> {
        self.current_index
    }


    /// Moves a track from one position to another.
    ///
    /// @param from - Source index
    /// @param to - Destination index
    ///
    /// @returns true if the move was successful
    pub fn move_track( &mut self, from: usize, to: usize ) -> bool {
        if from >= self.tracks.len() || to >= self.tracks.len() {
            return false;
        }

        if from == to {
            return true;
        }

        let track = self.tracks.remove( from );
        self.tracks.insert( to, track );

        // Adjust current index if affected
        if let Some( current ) = self.current_index {
            if current == from {
                self.current_index = Some( to );
            } else if from < current && current <= to {
                self.current_index = Some( current - 1 );
            } else if to <= current && current < from {
                self.current_index = Some( current + 1 );
            }
        }

        self.regenerate_shuffle_order();
        true
    }


    /// Removes duplicate tracks from the playlist, keeping the first occurrence.
    ///
    /// @returns The number of duplicates removed
    pub fn dedup( &mut self ) -> usize {
        use std::collections::HashSet;

        let original_len = self.tracks.len();
        let mut seen = HashSet::new();
        let mut new_tracks = Vec::with_capacity( original_len );
        let mut index_map = Vec::with_capacity( original_len );

        for ( old_idx, track ) in self.tracks.drain( .. ).enumerate() {
            if seen.insert( track.clone() ) {
                index_map.push(( old_idx, new_tracks.len() ));
                new_tracks.push( track );
            }
        }

        self.tracks = new_tracks;

        // Adjust current index if needed
        if let Some( current ) = self.current_index {
            self.current_index = index_map.iter()
                .find( |( old, _ )| *old == current )
                .map( |( _, new )| *new );
        }

        self.regenerate_shuffle_order();
        original_len - self.tracks.len()
    }


    /// Saves the playlist to a file (M3U format).
    pub fn save( &self, path: &Path ) -> Result<(), PlaylistError> {
        let mut file = File::create( path )?;

        // Write M3U header
        writeln!( file, "#EXTM3U" )?;

        for track in &self.tracks {
            // Write path as-is (supports both local and UNC paths)
            writeln!( file, "{}", track.display() )?;
        }

        Ok(())
    }


    /// Loads a playlist from a file (M3U format).
    pub fn load( path: &Path ) -> Result<Self, PlaylistError> {
        let file = File::open( path )?;
        let reader = BufReader::new( file );

        let mut playlist = Self::new();

        for line in reader.lines() {
            let line = line?;
            let trimmed = line.trim();

            // Skip empty lines and comments
            if trimmed.is_empty() || trimmed.starts_with( '#' ) {
                continue;
            }

            // Add the track path
            playlist.add( PathBuf::from( trimmed ) );
        }

        Ok( playlist )
    }


    /// Gets the default playlist directory.
    /// Uses Music/Oxidio on Windows, or ~/.local/share/oxidio/playlists on Linux.
    pub fn playlist_dir() -> Option<PathBuf> {
        #[cfg( target_os = "windows" )]
        {
            dirs::audio_dir().map( |d| d.join( "Oxidio" ) )
        }
        #[cfg( not( target_os = "windows" ) )]
        {
            dirs::data_local_dir().map( |d| d.join( "oxidio" ).join( "playlists" ) )
        }
    }


    /// Ensures the playlist directory exists.
    pub fn ensure_playlist_dir() -> Option<PathBuf> {
        let dir = Self::playlist_dir()?;
        fs::create_dir_all( &dir ).ok()?;
        Some( dir )
    }


    /// Gets the session file path for storing last playlist state.
    pub fn session_file() -> Option<PathBuf> {
        Self::playlist_dir().map( |d| d.join( ".session" ) )
    }


    /// Saves session state (current playlist file, track index, shuffle, repeat, volume).
    pub fn save_session( state: &SessionState ) -> Result<(), PlaylistError> {
        if let Some( session_path ) = Self::session_file() {
            if let Some( parent ) = session_path.parent() {
                fs::create_dir_all( parent )?;
            }
            let mut file = File::create( session_path )?;
            writeln!( file, "playlist={}", state.playlist_name )?;
            writeln!( file, "track={}", state.track_index.map( |i| i.to_string() ).unwrap_or_default() )?;
            writeln!( file, "shuffle={}", if state.shuffle { "1" } else { "0" } )?;
            writeln!( file, "repeat={}", match state.repeat {
                RepeatMode::Off => "off",
                RepeatMode::One => "one",
                RepeatMode::All => "all",
            })?;
            writeln!( file, "volume={}", ( state.volume * 100.0 ).round() as i32 )?;
        }
        Ok(())
    }


    /// Loads session state.
    pub fn load_session() -> Option<SessionState> {
        let session_path = Self::session_file()?;
        let file = File::open( session_path ).ok()?;
        let reader = BufReader::new( file );

        let mut playlist_name = String::new();
        let mut track_index = None;
        let mut shuffle = false;
        let mut repeat = RepeatMode::Off;
        let mut volume = 1.0_f32;

        for line in reader.lines().map_while( Result::ok ) {
            if let Some(( key, value )) = line.split_once( '=' ) {
                match key.trim() {
                    "playlist" => playlist_name = value.trim().to_string(),
                    "track" => track_index = value.trim().parse().ok(),
                    "shuffle" => shuffle = value.trim() == "1",
                    "repeat" => repeat = match value.trim() {
                        "one" | "1" => RepeatMode::One,
                        "all" | "2" => RepeatMode::All,
                        _ => RepeatMode::Off,
                    },
                    "volume" => volume = value.trim().parse::<i32>().map( |v| v as f32 / 100.0 ).unwrap_or( 1.0 ),
                    _ => {}
                }
            }
        }

        if playlist_name.is_empty() {
            return None;
        }

        Some( SessionState {
            playlist_name,
            track_index,
            shuffle,
            repeat,
            volume,
        })
    }


    fn regenerate_shuffle_order( &mut self ) {
        use std::collections::hash_map::RandomState;
        use std::hash::{ BuildHasher, Hasher };

        self.shuffle_order = ( 0..self.tracks.len() ).collect();

        // Simple Fisher-Yates shuffle
        let hasher = RandomState::new();
        for i in ( 1..self.shuffle_order.len() ).rev() {
            let mut h = hasher.build_hasher();
            h.write_usize( i );
            let j = h.finish() as usize % ( i + 1 );
            self.shuffle_order.swap( i, j );
        }

        self.shuffle_position = 0;
    }
}
