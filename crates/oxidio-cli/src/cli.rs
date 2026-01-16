//! Command-line argument parsing for Oxidio.

use std::path::PathBuf;

use clap::Parser;


/// Oxidio - A lightweight terminal UI music player.
#[derive( Parser, Debug )]
#[command( name = "oxidio" )]
#[command( version, about, long_about = None )]
pub struct Args {
    /// Directory or file to open on startup.
    #[arg( short, long )]
    pub path: Option<PathBuf>,

    /// Start in file browser mode.
    #[arg( short, long )]
    pub browse: bool,

    /// Add files/directories to playlist and start playing.
    #[arg( trailing_var_arg = true )]
    pub files: Vec<PathBuf>,
}
