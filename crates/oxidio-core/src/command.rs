//! Slash command parsing and execution.
//!
//! Provides the command infrastructure for the TUI slash commands.
//! Commands are parsed from user input and can be executed against
//! the player and playlist.

use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use thiserror::Error;


/// Errors that can occur during command parsing or execution.
#[derive( Debug, Error )]
pub enum CommandError {
    #[error( "Unknown command: {0}" )]
    Unknown( String ),

    #[error( "Invalid argument: {0}" )]
    InvalidArgument( String ),

    #[error( "Missing argument: {0}" )]
    MissingArgument( String ),

    #[error( "Execution failed: {0}" )]
    ExecutionFailed( String ),
}


/// Parsed slash command.
#[derive( Debug, Clone, PartialEq )]
pub enum Command {
    // Playlist commands
    Add { path: PathBuf },
    Remove,
    Clear,
    Dedup,
    Save { name: String },
    Load { name: String },
    Shuffle,
    Repeat { mode: Option<RepeatModeArg> },

    // Navigation commands
    Goto { path: PathBuf },
    Search { term: String },
    Home,

    // Playback commands
    Play,
    Pause,
    Stop,
    Next,
    Prev,
    Seek { position: Duration },

    // UI commands
    Vis,
    Volume { level: Option<u32> },
    Help,
    Quit,
}


/// Repeat mode argument for parsing.
#[derive( Debug, Clone, Copy, PartialEq, Eq )]
pub enum RepeatModeArg {
    Off,
    One,
    All,
}


impl FromStr for RepeatModeArg {
    type Err = CommandError;


    fn from_str( s: &str ) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "off" | "0" => Ok( RepeatModeArg::Off ),
            "one" | "1" => Ok( RepeatModeArg::One ),
            "all" | "2" => Ok( RepeatModeArg::All ),
            _ => Err( CommandError::InvalidArgument(
                format!( "Invalid repeat mode: '{}'. Use 'off', 'one', or 'all'", s )
            )),
        }
    }
}


impl Command {
    /// Parses a command string (without the leading `/`).
    ///
    /// @param input - The command string to parse
    ///
    /// @returns The parsed command or an error
    pub fn parse( input: &str ) -> Result<Self, CommandError> {
        let input = input.trim();
        let mut parts = input.splitn( 2, ' ' );
        let cmd = parts.next().unwrap_or( "" ).to_lowercase();
        let args = parts.next().map( |s| s.trim() );

        match cmd.as_str() {
            // Playlist commands
            "add" | "a" => {
                let path = args
                    .ok_or_else( || CommandError::MissingArgument( "path".into() ) )?;
                Ok( Command::Add { path: PathBuf::from( path ) } )
            }
            "remove" | "rm" | "del" => Ok( Command::Remove ),
            "clear" | "cl" => Ok( Command::Clear ),
            "dedup" | "dedupe" | "unique" => Ok( Command::Dedup ),
            "save" => {
                let name = args
                    .ok_or_else( || CommandError::MissingArgument( "playlist name".into() ) )?;
                Ok( Command::Save { name: name.to_string() } )
            }
            "load" => {
                let name = args
                    .ok_or_else( || CommandError::MissingArgument( "playlist name".into() ) )?;
                Ok( Command::Load { name: name.to_string() } )
            }
            "shuffle" | "sh" => Ok( Command::Shuffle ),
            "repeat" | "rep" => {
                let mode = args.map( |s| s.parse() ).transpose()?;
                Ok( Command::Repeat { mode } )
            }

            // Navigation commands
            "goto" | "go" | "cd" => {
                let path = args
                    .ok_or_else( || CommandError::MissingArgument( "path".into() ) )?;
                Ok( Command::Goto { path: PathBuf::from( path ) } )
            }
            "search" | "find" | "?" => {
                let term = args
                    .ok_or_else( || CommandError::MissingArgument( "search term".into() ) )?;
                Ok( Command::Search { term: term.to_string() } )
            }
            "home" | "~" => Ok( Command::Home ),

            // Playback commands
            "play" | "p" => Ok( Command::Play ),
            "pause" | "pa" => Ok( Command::Pause ),
            "stop" | "st" => Ok( Command::Stop ),
            "next" | "n" => Ok( Command::Next ),
            "prev" | "previous" | "pr" => Ok( Command::Prev ),
            "seek" | "sk" => {
                let time_str = args
                    .ok_or_else( || CommandError::MissingArgument( "time position".into() ) )?;
                let position = parse_time( time_str )?;
                Ok( Command::Seek { position } )
            }

            // UI commands
            "vis" | "visualizer" => Ok( Command::Vis ),
            "vol" | "volume" => {
                let level = args.and_then( |s| s.parse().ok() );
                Ok( Command::Volume { level } )
            }
            "help" | "h" => Ok( Command::Help ),
            "quit" | "q" | "exit" => Ok( Command::Quit ),

            "" => Err( CommandError::Unknown( "empty command".into() ) ),
            other => Err( CommandError::Unknown( other.to_string() ) ),
        }
    }


    /// Returns a brief description of the command for help text.
    pub fn description( &self ) -> &'static str {
        match self {
            Command::Add { .. } => "Add file/folder to playlist",
            Command::Remove => "Remove selected track",
            Command::Clear => "Clear playlist",
            Command::Dedup => "Remove duplicate tracks",
            Command::Save { .. } => "Save playlist",
            Command::Load { .. } => "Load playlist",
            Command::Shuffle => "Toggle shuffle",
            Command::Repeat { .. } => "Set repeat mode",
            Command::Goto { .. } => "Navigate to path",
            Command::Search { .. } => "Search/filter",
            Command::Home => "Go to home directory",
            Command::Play => "Play selected track",
            Command::Pause => "Pause playback",
            Command::Stop => "Stop playback",
            Command::Next => "Next track",
            Command::Prev => "Previous track",
            Command::Seek { .. } => "Seek to position",
            Command::Vis => "Toggle visualizer",
            Command::Volume { .. } => "Set volume (0-100)",
            Command::Help => "Show help",
            Command::Quit => "Quit application",
        }
    }
}


/// Parses a time string like "1:30" or "90" into a Duration.
///
/// @param s - Time string in format "MM:SS", "M:SS", or just seconds
///
/// @returns Duration or error
fn parse_time( s: &str ) -> Result<Duration, CommandError> {
    let s = s.trim();

    if let Some(( min, sec )) = s.split_once( ':' ) {
        let minutes: u64 = min.parse()
            .map_err( |_| CommandError::InvalidArgument( format!( "Invalid minutes: {}", min ) ) )?;
        let seconds: u64 = sec.parse()
            .map_err( |_| CommandError::InvalidArgument( format!( "Invalid seconds: {}", sec ) ) )?;
        Ok( Duration::from_secs( minutes * 60 + seconds ) )
    } else {
        let seconds: u64 = s.parse()
            .map_err( |_| CommandError::InvalidArgument( format!( "Invalid time: {}", s ) ) )?;
        Ok( Duration::from_secs( seconds ) )
    }
}


/// Returns help text listing all available commands.
pub fn help_text() -> &'static str {
    r#"Playlist Commands:
  /add <path>     Add file/folder to playlist
  /remove         Remove selected track
  /clear          Clear playlist
  /dedup          Remove duplicate tracks
  /shuffle        Toggle shuffle mode
  /repeat [mode]  Set repeat (off/one/all)

Navigation Commands:
  /goto <path>    Navigate browser to path
  /search <term>  Filter current view
  /home           Go to home directory

Playback Commands:
  /play           Play selected track
  /pause          Pause playback
  /stop           Stop playback
  /next           Next track
  /prev           Previous track
  /seek <time>    Seek to position (e.g., 1:30)

Other Commands:
  /vis            Toggle visualizer      [v]
  /vol [0-100]    Set volume             [+/-]
  /help           Show this help         [?]
  /quit           Exit oxidio            [q]"#
}


#[cfg( test )]
mod tests {
    use super::*;


    #[test]
    fn test_parse_add() {
        let cmd = Command::parse( "add /path/to/file.mp3" ).unwrap();
        assert_eq!( cmd, Command::Add { path: PathBuf::from( "/path/to/file.mp3" ) } );
    }


    #[test]
    fn test_parse_add_alias() {
        let cmd = Command::parse( "a /music" ).unwrap();
        assert_eq!( cmd, Command::Add { path: PathBuf::from( "/music" ) } );
    }


    #[test]
    fn test_parse_seek() {
        let cmd = Command::parse( "seek 1:30" ).unwrap();
        assert_eq!( cmd, Command::Seek { position: Duration::from_secs( 90 ) } );
    }


    #[test]
    fn test_parse_seek_seconds() {
        let cmd = Command::parse( "seek 45" ).unwrap();
        assert_eq!( cmd, Command::Seek { position: Duration::from_secs( 45 ) } );
    }


    #[test]
    fn test_parse_repeat_with_mode() {
        let cmd = Command::parse( "repeat all" ).unwrap();
        assert_eq!( cmd, Command::Repeat { mode: Some( RepeatModeArg::All ) } );
    }


    #[test]
    fn test_parse_repeat_toggle() {
        let cmd = Command::parse( "repeat" ).unwrap();
        assert_eq!( cmd, Command::Repeat { mode: None } );
    }


    #[test]
    fn test_parse_unknown() {
        let result = Command::parse( "foobar" );
        assert!( matches!( result, Err( CommandError::Unknown( _ ) ) ) );
    }


    #[test]
    fn test_parse_missing_arg() {
        let result = Command::parse( "add" );
        assert!( matches!( result, Err( CommandError::MissingArgument( _ ) ) ) );
    }
}
