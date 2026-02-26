//! Control channel and command processor for Oxidio.
//!
//! This crate provides the central message bus that all frontends (TUI, web, CLI)
//! communicate through. The `CommandProcessor` owns all mutable state (player,
//! settings, browser) and processes commands from any client.

mod channel;
mod processor;

pub use channel::{ CommandSender, ControlChannel };
pub use processor::{ CommandProcessor, ProcessorSettings };
