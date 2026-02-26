//! Web server for Oxidio.
//!
//! Provides an HTTP + WebSocket server that serves the web UI and bridges
//! browser clients to the control channel. Each WebSocket connection is
//! just another client of the same message bus that the TUI uses.

mod server;
mod websocket;

pub use server::{ start_web_server, WebServerHandle };
