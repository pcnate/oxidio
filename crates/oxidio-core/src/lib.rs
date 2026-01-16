//! Oxidio Core - Audio playback engine
//!
//! This crate provides the core functionality for audio playback,
//! including decoding, output, playlist management, and library scanning.

pub mod command;
pub mod decoder;
pub mod library;
pub mod output;
pub mod player;
pub mod playlist;

pub use command::{ Command, CommandError };
pub use decoder::AudioMetadata;
pub use output::VIS_BARS;
pub use player::Player;
pub use playlist::{ Playlist, PlaylistError, RepeatMode, SessionState };
