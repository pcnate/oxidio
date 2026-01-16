//! File browser for directory navigation.
//!
//! Provides a file browser that can navigate directories,
//! filter entries, and select files for adding to the playlist.

use std::fs;
use std::path::{ Path, PathBuf };

use anyhow::Result;


/// Supported audio extensions for highlighting.
const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "flac", "ogg", "wav", "m4a", "aac", "opus", "wma", "aiff", "alac",
];


/// A file or directory entry in the browser.
#[derive( Debug, Clone )]
pub struct BrowserEntry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub is_audio: bool,
}


/// File browser state.
#[derive( Debug )]
pub struct FileBrowser {
    current_dir: PathBuf,
    entries: Vec<BrowserEntry>,
    filtered_indices: Vec<usize>,
    selected: usize,
    filter: String,
}


impl FileBrowser {
    /// Creates a new file browser at the given path.
    pub fn new( path: PathBuf ) -> Result<Self> {
        let mut browser = Self {
            current_dir: path,
            entries: Vec::new(),
            filtered_indices: Vec::new(),
            selected: 0,
            filter: String::new(),
        };
        browser.refresh()?;
        Ok( browser )
    }


    /// Refreshes the directory listing.
    pub fn refresh( &mut self ) -> Result<()> {
        self.entries.clear();
        self.filtered_indices.clear();
        self.selected = 0;

        // Add parent directory entry (unless at root)
        if let Some( parent ) = self.current_dir.parent() {
            self.entries.push( BrowserEntry {
                path: parent.to_path_buf(),
                name: "..".to_string(),
                is_dir: true,
                is_audio: false,
            });
        }

        // Read directory contents
        let mut dirs = Vec::new();
        let mut files = Vec::new();

        let read_result = fs::read_dir( &self.current_dir );
        if let Ok( entries ) = read_result {
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();

                // Skip hidden files (starting with .)
                if name.starts_with( '.' ) {
                    continue;
                }

                let is_dir = path.is_dir();
                let is_audio = !is_dir && Self::is_audio_file( &path );

                let browser_entry = BrowserEntry {
                    path,
                    name,
                    is_dir,
                    is_audio,
                };

                if is_dir {
                    dirs.push( browser_entry );
                } else {
                    files.push( browser_entry );
                }
            }
        }

        // Sort directories and files separately (case-insensitive)
        dirs.sort_by( |a, b| a.name.to_lowercase().cmp( &b.name.to_lowercase() ) );
        files.sort_by( |a, b| a.name.to_lowercase().cmp( &b.name.to_lowercase() ) );

        // Directories first, then files
        self.entries.extend( dirs );
        self.entries.extend( files );

        self.apply_filter();
        Ok(())
    }


    /// Navigates to a specific path.
    pub fn navigate_to( &mut self, path: &Path ) -> Result<()> {
        let canonical = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.current_dir.join( path )
        };

        if canonical.is_dir() {
            self.current_dir = canonical;
            self.filter.clear();
            self.refresh()?;
        }
        Ok(())
    }


    /// Enters the selected directory or returns the selected file path.
    ///
    /// @returns Some(path) if a file was selected, None if entered a directory
    pub fn enter_selected( &mut self ) -> Result<Option<PathBuf>> {
        if let Some( entry ) = self.selected_entry() {
            let entry = entry.clone();
            if entry.is_dir {
                self.navigate_to( &entry.path )?;
                Ok( None )
            } else {
                Ok( Some( entry.path ) )
            }
        } else {
            Ok( None )
        }
    }


    /// Goes up to parent directory.
    pub fn go_up( &mut self ) -> Result<()> {
        if let Some( parent ) = self.current_dir.parent() {
            let parent = parent.to_path_buf();
            self.navigate_to( &parent )?;
        }
        Ok(())
    }


    /// Sets the filter text and updates visible entries.
    pub fn set_filter( &mut self, filter: String ) {
        self.filter = filter;
        self.apply_filter();
    }


    /// Clears the filter.
    pub fn clear_filter( &mut self ) {
        self.filter.clear();
        self.apply_filter();
    }


    /// Gets the current filter.
    pub fn filter( &self ) -> &str {
        &self.filter
    }


    fn apply_filter( &mut self ) {
        self.filtered_indices.clear();

        let filter_lower = self.filter.to_lowercase();

        for ( idx, entry ) in self.entries.iter().enumerate() {
            // Always show ".." parent directory
            if entry.name == ".." {
                self.filtered_indices.push( idx );
                continue;
            }

            if self.filter.is_empty() || entry.name.to_lowercase().contains( &filter_lower ) {
                self.filtered_indices.push( idx );
            }
        }

        // Adjust selection if out of bounds
        if self.selected >= self.filtered_indices.len() {
            self.selected = self.filtered_indices.len().saturating_sub( 1 );
        }
    }


    /// Moves selection down.
    pub fn select_next( &mut self ) {
        if !self.filtered_indices.is_empty() {
            self.selected = ( self.selected + 1 ) % self.filtered_indices.len();
        }
    }


    /// Moves selection up.
    pub fn select_previous( &mut self ) {
        if !self.filtered_indices.is_empty() {
            self.selected = if self.selected == 0 {
                self.filtered_indices.len() - 1
            } else {
                self.selected - 1
            };
        }
    }


    /// Jumps to first entry.
    pub fn select_first( &mut self ) {
        self.selected = 0;
    }


    /// Jumps to last entry.
    pub fn select_last( &mut self ) {
        if !self.filtered_indices.is_empty() {
            self.selected = self.filtered_indices.len() - 1;
        }
    }


    /// Gets the currently selected entry.
    pub fn selected_entry( &self ) -> Option<&BrowserEntry> {
        self.filtered_indices
            .get( self.selected )
            .and_then( |&idx| self.entries.get( idx ) )
    }


    /// Gets visible entries (filtered).
    pub fn visible_entries( &self ) -> Vec<&BrowserEntry> {
        self.filtered_indices
            .iter()
            .filter_map( |&idx| self.entries.get( idx ) )
            .collect()
    }


    /// Gets the selected index for UI state.
    pub fn selected_index( &self ) -> usize {
        self.selected
    }


    /// Gets the current directory path.
    pub fn current_dir( &self ) -> &Path {
        &self.current_dir
    }


    /// Gets the total number of visible entries.
    pub fn len( &self ) -> usize {
        self.filtered_indices.len()
    }


    /// Returns true if there are no visible entries.
    pub fn is_empty( &self ) -> bool {
        self.filtered_indices.is_empty()
    }


    fn is_audio_file( path: &Path ) -> bool {
        path.extension()
            .and_then( |e| e.to_str() )
            .map( |e| AUDIO_EXTENSIONS.contains( &e.to_lowercase().as_str() ) )
            .unwrap_or( false )
    }
}


impl Default for FileBrowser {
    fn default() -> Self {
        let start_dir = dirs::home_dir()
            .or_else( || std::env::current_dir().ok() )
            .unwrap_or_else( || PathBuf::from( "/" ) );
        Self::new( start_dir ).expect( "Failed to create file browser" )
    }
}
