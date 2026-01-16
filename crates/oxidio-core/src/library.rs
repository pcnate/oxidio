//! Library scanning and management
//!
//! Handles discovering audio files, caching metadata, and managing
//! music libraries including SMB/network paths.

use std::path::{ Path, PathBuf };

use thiserror::Error;


/// Supported audio file extensions.
const SUPPORTED_EXTENSIONS: &[&str] = &[
    "mp3", "flac", "ogg", "wav", "m4a", "aac", "opus", "wma", "aiff", "alac",
];


/// Errors that can occur during library operations.
#[derive( Debug, Error )]
pub enum LibraryError {
    #[error( "IO error: {0}" )]
    Io( #[from] std::io::Error ),

    #[error( "Path not found: {0}" )]
    NotFound( PathBuf ),

    #[error( "Access denied: {0}" )]
    AccessDenied( PathBuf ),
}


/// Track metadata.
#[derive( Debug, Clone, Default )]
pub struct TrackMetadata {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub track_number: Option<u32>,
    pub duration_secs: Option<f64>,
    pub year: Option<i32>,
    pub genre: Option<String>,
}


/// A scanned audio file.
#[derive( Debug, Clone )]
pub struct ScannedTrack {
    pub path: PathBuf,
    pub metadata: TrackMetadata,
}


/// Library scanner for discovering audio files.
pub struct LibraryScanner {
    roots: Vec<PathBuf>,
}


impl LibraryScanner {
    /// Creates a new scanner with no root directories.
    pub fn new() -> Self {
        Self { roots: Vec::new() }
    }


    /// Adds a root directory to scan.
    ///
    /// Supports local paths and SMB/UNC paths (e.g., `\\server\share\music`).
    pub fn add_root( &mut self, path: PathBuf ) {
        if !self.roots.contains( &path ) {
            self.roots.push( path );
        }
    }


    /// Removes a root directory.
    pub fn remove_root( &mut self, path: &Path ) -> bool {
        if let Some( pos ) = self.roots.iter().position( |p| p == path ) {
            self.roots.remove( pos );
            true
        } else {
            false
        }
    }


    /// Gets all root directories.
    pub fn roots( &self ) -> &[PathBuf] {
        &self.roots
    }


    /// Scans all roots and returns discovered audio files.
    pub fn scan( &self ) -> Result<Vec<ScannedTrack>, LibraryError> {
        let mut tracks = Vec::new();

        for root in &self.roots {
            tracing::info!( "Scanning: {:?}", root );
            self.scan_directory( root, &mut tracks )?;
        }

        tracing::info!( "Found {} tracks", tracks.len() );
        Ok( tracks )
    }


    /// Scans a single directory (non-recursive entry point).
    pub fn scan_directory(
        &self,
        dir: &Path,
        tracks: &mut Vec<ScannedTrack>,
    ) -> Result<(), LibraryError> {
        self.scan_recursive( dir, tracks )
    }


    fn scan_recursive(
        &self,
        dir: &Path,
        tracks: &mut Vec<ScannedTrack>,
    ) -> Result<(), LibraryError> {
        let entries = match std::fs::read_dir( dir ) {
            Ok( e ) => e,
            Err( e ) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                tracing::warn!( "Access denied: {:?}", dir );
                return Ok(()); // Skip inaccessible directories
            }
            Err( e ) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err( LibraryError::NotFound( dir.to_path_buf() ) );
            }
            Err( e ) => return Err( LibraryError::Io( e ) ),
        };

        for entry in entries.flatten() {
            let path = entry.path();

            if path.is_dir() {
                // Recurse into subdirectories
                self.scan_recursive( &path, tracks )?;
            } else if Self::is_audio_file( &path ) {
                // Found an audio file
                tracks.push( ScannedTrack {
                    path,
                    metadata: TrackMetadata::default(), // TODO: Read actual metadata
                } );
            }
        }

        Ok(())
    }


    /// Checks if a file has a supported audio extension.
    fn is_audio_file( path: &Path ) -> bool {
        path.extension()
            .and_then( |e| e.to_str() )
            .map( |e| SUPPORTED_EXTENSIONS.contains( &e.to_lowercase().as_str() ) )
            .unwrap_or( false )
    }
}


impl Default for LibraryScanner {
    fn default() -> Self {
        Self::new()
    }
}


/// Checks if a path is a network/SMB path.
pub fn is_network_path( path: &Path ) -> bool {
    path.to_str()
        .map( |s| s.starts_with( r"\\" ) || s.starts_with( "//" ) )
        .unwrap_or( false )
}
