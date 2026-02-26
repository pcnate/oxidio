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
    ListPlaylists,
    DeletePlaylist { name: String },
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
            "playlist" | "pl" => {
                let sub_args = args
                    .ok_or_else( || CommandError::MissingArgument( "subcommand (list|save|load|delete)".into() ) )?;
                let mut sub_parts = sub_args.splitn( 2, ' ' );
                let sub_cmd = sub_parts.next().unwrap_or( "" ).to_lowercase();
                let sub_arg = sub_parts.next().map( |s| s.trim() );

                match sub_cmd.as_str() {
                    "list" | "ls" => Ok( Command::ListPlaylists ),
                    "save" => {
                        let name = sub_arg
                            .ok_or_else( || CommandError::MissingArgument( "playlist name".into() ) )?;
                        Ok( Command::Save { name: name.to_string() } )
                    }
                    "load" => {
                        let name = sub_arg
                            .ok_or_else( || CommandError::MissingArgument( "playlist name".into() ) )?;
                        Ok( Command::Load { name: name.to_string() } )
                    }
                    "delete" | "del" | "rm" => {
                        let name = sub_arg
                            .ok_or_else( || CommandError::MissingArgument( "playlist name".into() ) )?;
                        Ok( Command::DeletePlaylist { name: name.to_string() } )
                    }
                    "" => Err( CommandError::MissingArgument( "subcommand (list|save|load|delete)".into() ) ),
                    other => Err( CommandError::Unknown( format!( "playlist {}", other ) ) ),
                }
            }
            "queue" | "q" => {
                let sub_args = args
                    .ok_or_else( || CommandError::MissingArgument( "subcommand (add|remove|clear|dedup)".into() ) )?;
                let mut sub_parts = sub_args.splitn( 2, ' ' );
                let sub_cmd = sub_parts.next().unwrap_or( "" ).to_lowercase();
                let sub_arg = sub_parts.next().map( |s| s.trim() );

                match sub_cmd.as_str() {
                    "add" | "a" => {
                        let path = sub_arg
                            .ok_or_else( || CommandError::MissingArgument( "path".into() ) )?;
                        Ok( Command::Add { path: PathBuf::from( path ) } )
                    }
                    "remove" | "rm" | "del" => Ok( Command::Remove ),
                    "clear" | "cl" => Ok( Command::Clear ),
                    "dedup" | "dedupe" | "unique" => Ok( Command::Dedup ),
                    "" => Err( CommandError::MissingArgument( "subcommand (add|remove|clear|dedup)".into() ) ),
                    other => Err( CommandError::Unknown( format!( "queue {}", other ) ) ),
                }
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
            "quit" | "exit" => Ok( Command::Quit ),

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
            Command::ListPlaylists => "List saved playlists",
            Command::DeletePlaylist { .. } => "Delete saved playlist",
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
    r#"Queue Commands:
  /queue add <path>   Add file/folder to queue
  /queue remove       Remove selected track
  /queue clear        Clear queue
  /queue dedup        Remove duplicate tracks

Playlist Commands:
  /playlist list      List saved playlists
  /playlist save <n>  Save playlist
  /playlist load <n>  Load playlist
  /playlist delete <n>  Delete saved playlist
  /shuffle            Toggle shuffle mode
  /repeat [mode]      Set repeat (off/one/all)

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


/// A command definition for the suggestion engine.
struct CommandDef {
    /// The canonical command name.
    name: &'static str,

    /// Argument hint (e.g. "<path>", "[off|one|all]"). Empty if no args.
    hint: &'static str,

    /// Subcommands, if any.
    subs: &'static [CommandDef],
}


/// Subcommand definitions for /playlist.
const PLAYLIST_SUBS: &[CommandDef] = &[
    CommandDef { name: "delete", hint: "<name>", subs: &[] },
    CommandDef { name: "list", hint: "", subs: &[] },
    CommandDef { name: "load", hint: "<name>", subs: &[] },
    CommandDef { name: "save", hint: "<name>", subs: &[] },
];


/// Subcommand definitions for /queue.
const QUEUE_SUBS: &[CommandDef] = &[
    CommandDef { name: "add", hint: "<path>", subs: &[] },
    CommandDef { name: "clear", hint: "", subs: &[] },
    CommandDef { name: "dedup", hint: "", subs: &[] },
    CommandDef { name: "remove", hint: "", subs: &[] },
];


/// All top-level command definitions, sorted alphabetically.
const COMMAND_DEFS: &[CommandDef] = &[
    CommandDef { name: "goto", hint: "<path>", subs: &[] },
    CommandDef { name: "help", hint: "", subs: &[] },
    CommandDef { name: "home", hint: "", subs: &[] },
    CommandDef { name: "next", hint: "", subs: &[] },
    CommandDef { name: "pause", hint: "", subs: &[] },
    CommandDef { name: "play", hint: "", subs: &[] },
    CommandDef { name: "playlist", hint: "", subs: PLAYLIST_SUBS },
    CommandDef { name: "prev", hint: "", subs: &[] },
    CommandDef { name: "queue", hint: "", subs: QUEUE_SUBS },
    CommandDef { name: "quit", hint: "", subs: &[] },
    CommandDef { name: "repeat", hint: "[off|one|all]", subs: &[] },
    CommandDef { name: "search", hint: "<term>", subs: &[] },
    CommandDef { name: "seek", hint: "<time>", subs: &[] },
    CommandDef { name: "shuffle", hint: "", subs: &[] },
    CommandDef { name: "stop", hint: "", subs: &[] },
    CommandDef { name: "vis", hint: "", subs: &[] },
    CommandDef { name: "vol", hint: "[0-100]", subs: &[] },
];


/// Builds a full hint string for a command definition.
///
/// Returns the first subcommand with its hint, or just the hint text.
fn build_def_hint( def: &CommandDef ) -> String {
    if !def.subs.is_empty() {
        let sub = &def.subs[0];
        if sub.hint.is_empty() {
            sub.name.to_string()
        } else {
            format!( "{} {}", sub.name, sub.hint )
        }
    } else {
        def.hint.to_string()
    }
}


/// Returns ghost text suggestion for the given command input.
///
/// The input should NOT include the leading `/`. Returns the text that
/// should be displayed after the cursor as dim/ghost text. The suggestion
/// follows fish-shell style: shows the best alphabetical match for the
/// current prefix, including subcommands and argument placeholders.
///
/// @param input - The current command input text (without leading `/`)
///
/// @returns Ghost text suggestion, or None if no suggestion available
pub fn get_suggestion( input: &str ) -> Option<String> {
    if input.is_empty() {
        return None;
    }

    let has_trailing_space = input.ends_with( ' ' );
    let words: Vec<&str> = input.split_whitespace().collect();

    if words.is_empty() {
        return None;
    }

    let first_word = words[0].to_lowercase();

    // Still typing the first word (no space after it)
    if words.len() == 1 && !has_trailing_space {
        for def in COMMAND_DEFS {
            if !def.name.starts_with( &first_word ) {
                continue;
            }

            if def.name == first_word {
                // Exact match — show hint/subs with leading space
                let hint = build_def_hint( def );
                if hint.is_empty() {
                    return None;
                }
                return Some( format!( " {}", hint ) );
            }

            // Partial match — complete the name + show hint
            let rest = &def.name[first_word.len()..];
            let hint = build_def_hint( def );
            if hint.is_empty() {
                return Some( rest.to_string() );
            }
            return Some( format!( "{} {}", rest, hint ) );
        }
        return None;
    }

    // First word is complete — find exact match
    let matched_def = COMMAND_DEFS.iter().find( |d| d.name == first_word )?;

    // Command has subcommands
    if !matched_def.subs.is_empty() {
        if words.len() == 1 && has_trailing_space {
            // Just typed "queue " — suggest first subcommand
            let sub = &matched_def.subs[0];
            if sub.hint.is_empty() {
                return Some( sub.name.to_string() );
            }
            return Some( format!( "{} {}", sub.name, sub.hint ) );
        }

        if words.len() >= 2 {
            let second_word = words[1].to_lowercase();
            let has_space_after_second = words.len() > 2
                || ( words.len() == 2 && has_trailing_space );

            if !has_space_after_second {
                // Still typing second word — match subcommand prefix
                for sub in matched_def.subs {
                    if !sub.name.starts_with( &second_word ) {
                        continue;
                    }

                    if sub.name == second_word {
                        // Exact sub match — show hint with leading space
                        if sub.hint.is_empty() {
                            return None;
                        }
                        return Some( format!( " {}", sub.hint ) );
                    }

                    // Partial sub match
                    let rest = &sub.name[second_word.len()..];
                    if sub.hint.is_empty() {
                        return Some( rest.to_string() );
                    }
                    return Some( format!( "{} {}", rest, sub.hint ) );
                }
                return None;
            }

            // Second word is complete — check if it has a hint
            if let Some( sub ) = matched_def.subs.iter().find( |s| s.name == second_word ) {
                if words.len() == 2 && has_trailing_space && !sub.hint.is_empty() {
                    return Some( sub.hint.to_string() );
                }
            }
            // Typing actual args — no suggestion
            return None;
        }
    }

    // Command has hint (no subcommands)
    if !matched_def.hint.is_empty() {
        if words.len() == 1 && has_trailing_space {
            return Some( matched_def.hint.to_string() );
        }
        // Typing actual args — no suggestion
        return None;
    }

    None
}


/// Extracts the next word chunk from ghost text.
///
/// Consumes leading spaces and the next non-space word. Used when
/// the user presses Right arrow to accept the next portion of the
/// suggestion.
///
/// @param ghost - The current ghost text
///
/// @returns The next word chunk (including any leading spaces)
pub fn get_next_word_chunk( ghost: &str ) -> String {
    if ghost.is_empty() {
        return String::new();
    }

    let mut chunk = String::new();
    let mut chars = ghost.chars().peekable();

    // Consume leading spaces
    while let Some( &ch ) = chars.peek() {
        if ch != ' ' {
            break;
        }
        chunk.push( ch );
        chars.next();
    }

    // Consume the next word (non-space characters)
    while let Some( &ch ) = chars.peek() {
        if ch == ' ' {
            break;
        }
        chunk.push( ch );
        chars.next();
    }

    chunk
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


    // --- Suggestion engine tests ---

    #[test]
    fn test_suggestion_partial_first_word() {
        assert_eq!( get_suggestion( "go" ), Some( "to <path>".into() ) );
        assert_eq!( get_suggestion( "pa" ), Some( "use".into() ) );
        assert_eq!( get_suggestion( "sh" ), Some( "uffle".into() ) );
        assert_eq!( get_suggestion( "n" ), Some( "ext".into() ) );
        assert_eq!( get_suggestion( "h" ), Some( "elp".into() ) );
    }


    #[test]
    fn test_suggestion_exact_command_with_hint() {
        assert_eq!( get_suggestion( "goto" ), Some( " <path>".into() ) );
        assert_eq!( get_suggestion( "repeat" ), Some( " [off|one|all]".into() ) );
        assert_eq!( get_suggestion( "vol" ), Some( " [0-100]".into() ) );
    }


    #[test]
    fn test_suggestion_exact_command_no_hint() {
        assert_eq!( get_suggestion( "play" ), None );
        assert_eq!( get_suggestion( "next" ), None );
        assert_eq!( get_suggestion( "help" ), None );
        assert_eq!( get_suggestion( "stop" ), None );
    }


    #[test]
    fn test_suggestion_command_space_hint() {
        assert_eq!( get_suggestion( "goto " ), Some( "<path>".into() ) );
        assert_eq!( get_suggestion( "repeat " ), Some( "[off|one|all]".into() ) );
        assert_eq!( get_suggestion( "seek " ), Some( "<time>".into() ) );
    }


    #[test]
    fn test_suggestion_command_space_no_hint() {
        assert_eq!( get_suggestion( "play " ), None );
        assert_eq!( get_suggestion( "next " ), None );
    }


    #[test]
    fn test_suggestion_typing_args() {
        assert_eq!( get_suggestion( "goto /music" ), None );
        assert_eq!( get_suggestion( "seek 1:30" ), None );
        assert_eq!( get_suggestion( "search foo" ), None );
    }


    #[test]
    fn test_suggestion_subcommand_parent_partial() {
        assert_eq!( get_suggestion( "qu" ), Some( "eue add <path>".into() ) );
        assert_eq!( get_suggestion( "playl" ), Some( "ist delete <name>".into() ) );
    }


    #[test]
    fn test_suggestion_subcommand_parent_exact() {
        assert_eq!( get_suggestion( "queue" ), Some( " add <path>".into() ) );
        assert_eq!( get_suggestion( "playlist" ), Some( " delete <name>".into() ) );
    }


    #[test]
    fn test_suggestion_subcommand_parent_space() {
        assert_eq!( get_suggestion( "queue " ), Some( "add <path>".into() ) );
        assert_eq!( get_suggestion( "playlist " ), Some( "delete <name>".into() ) );
    }


    #[test]
    fn test_suggestion_subcommand_partial() {
        assert_eq!( get_suggestion( "queue a" ), Some( "dd <path>".into() ) );
        assert_eq!( get_suggestion( "queue cl" ), Some( "ear".into() ) );
        assert_eq!( get_suggestion( "queue d" ), Some( "edup".into() ) );
        assert_eq!( get_suggestion( "playlist d" ), Some( "elete <name>".into() ) );
        assert_eq!( get_suggestion( "playlist s" ), Some( "ave <name>".into() ) );
    }


    #[test]
    fn test_suggestion_subcommand_exact_with_hint() {
        assert_eq!( get_suggestion( "queue add" ), Some( " <path>".into() ) );
        assert_eq!( get_suggestion( "playlist save" ), Some( " <name>".into() ) );
        assert_eq!( get_suggestion( "playlist delete" ), Some( " <name>".into() ) );
    }


    #[test]
    fn test_suggestion_subcommand_exact_no_hint() {
        assert_eq!( get_suggestion( "queue clear" ), None );
        assert_eq!( get_suggestion( "queue remove" ), None );
        assert_eq!( get_suggestion( "playlist list" ), None );
    }


    #[test]
    fn test_suggestion_subcommand_space_hint() {
        assert_eq!( get_suggestion( "queue add " ), Some( "<path>".into() ) );
        assert_eq!( get_suggestion( "playlist save " ), Some( "<name>".into() ) );
    }


    #[test]
    fn test_suggestion_subcommand_typing_args() {
        assert_eq!( get_suggestion( "queue add /music" ), None );
        assert_eq!( get_suggestion( "playlist save my_list" ), None );
    }


    #[test]
    fn test_suggestion_empty_input() {
        assert_eq!( get_suggestion( "" ), None );
    }


    #[test]
    fn test_suggestion_no_match() {
        assert_eq!( get_suggestion( "xyz" ), None );
        assert_eq!( get_suggestion( "z" ), None );
    }


    // --- Next word chunk tests ---

    #[test]
    fn test_next_word_chunk_word_only() {
        assert_eq!( get_next_word_chunk( "ause" ), "ause" );
        assert_eq!( get_next_word_chunk( "to" ), "to" );
    }


    #[test]
    fn test_next_word_chunk_first_of_multiple() {
        assert_eq!( get_next_word_chunk( "eue add <path>" ), "eue" );
        assert_eq!( get_next_word_chunk( "to <path>" ), "to" );
    }


    #[test]
    fn test_next_word_chunk_with_leading_space() {
        assert_eq!( get_next_word_chunk( " list" ), " list" );
        assert_eq!( get_next_word_chunk( " add <path>" ), " add" );
        assert_eq!( get_next_word_chunk( " <path>" ), " <path>" );
    }


    #[test]
    fn test_next_word_chunk_empty() {
        assert_eq!( get_next_word_chunk( "" ), "" );
    }
}
