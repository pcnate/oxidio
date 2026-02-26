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

    /// Run as a headless daemon (no TUI).
    #[arg( short = 'D', long )]
    pub daemon: bool,

    /// Force-enable the web interface (overrides settings).
    #[arg( long )]
    pub web: bool,

    /// Force-disable the web interface (overrides settings).
    #[arg( long, conflicts_with = "web" )]
    pub no_web: bool,

    /// Web server port.
    #[arg( long, default_value = "8384" )]
    pub web_port: u16,

    /// Web server bind address.
    #[arg( long, default_value = "127.0.0.1" )]
    pub web_bind: String,

    /// Add files/directories to playlist and start playing.
    #[arg( trailing_var_arg = true )]
    pub files: Vec<PathBuf>,
}


impl Args {
    /// Resolves whether the web server should be enabled.
    ///
    /// CLI flags take absolute precedence over settings.
    ///
    /// @param settings_web_enabled - The value from settings.json
    ///
    /// @returns Whether the web server should be enabled
    pub fn resolve_web_enabled( &self, settings_web_enabled: bool ) -> bool {
        if self.web {
            true
        } else if self.no_web {
            false
        } else {
            settings_web_enabled
        }
    }


    /// Returns the CLI web override state.
    ///
    /// `Some(true)` if `--web`, `Some(false)` if `--no-web`, `None` if neither.
    pub fn cli_web_override( &self ) -> Option<bool> {
        if self.web {
            Some( true )
        } else if self.no_web {
            Some( false )
        } else {
            None
        }
    }
}
