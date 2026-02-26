//! WebSocket handler for individual browser connections.
//!
//! Each connection subscribes to the control channel broadcast and forwards
//! state updates as JSON. Incoming messages are parsed as `AppCommand` and
//! sent through the `CommandSender`.

use axum::extract::ws::{ Message, WebSocket };
use futures_util::{ SinkExt, StreamExt };
use tokio::sync::broadcast;

use oxidio_ctl::CommandSender;
use oxidio_protocol::{ AppCommand, StateUpdate };


/// Handles a single WebSocket connection.
///
/// @param socket - The upgraded WebSocket
/// @param sender - Command sender for this client
/// @param state_rx - Broadcast receiver for state updates
pub async fn handle_ws(
    socket: WebSocket,
    sender: CommandSender,
    mut state_rx: broadcast::Receiver<StateUpdate>,
) {
    let ( mut ws_tx, mut ws_rx ) = socket.split();

    // Request full state on connect so the client gets an initial snapshot
    if let Err( e ) = sender.send( AppCommand::RequestFullState ).await {
        tracing::warn!( "Failed to request initial state: {}", e );
        return;
    }

    loop {
        tokio::select! {
            // Forward state updates to the browser
            Ok( update ) = state_rx.recv() => {
                match serde_json::to_string( &update ) {
                    Ok( json ) => {
                        if ws_tx.send( Message::Text( json.into() ) ).await.is_err() {
                            break;  // Client disconnected
                        }
                    }
                    Err( e ) => {
                        tracing::warn!( "Failed to serialize state update: {}", e );
                    }
                }
            }

            // Receive commands from the browser
            msg = ws_rx.next() => {
                match msg {
                    Some( Ok( Message::Text( text ) ) ) => {
                        match serde_json::from_str::<AppCommand>( &text ) {
                            Ok( cmd ) => {
                                // Filter: web clients cannot toggle web_enabled
                                if matches!(
                                    &cmd,
                                    AppCommand::ToggleSetting { key } if key == "web_enabled"
                                ) {
                                    tracing::debug!( "Rejected web_enabled toggle from web client" );
                                    continue;
                                }

                                if let Err( e ) = sender.send( cmd ).await {
                                    tracing::warn!( "Failed to send command: {}", e );
                                    break;
                                }
                            }
                            Err( e ) => {
                                tracing::debug!( "Invalid command from web client: {}", e );
                            }
                        }
                    }
                    Some( Ok( Message::Close( _ ) ) ) | None => {
                        break;  // Client disconnected
                    }
                    _ => {}
                }
            }
        }
    }

    tracing::debug!( "WebSocket client disconnected" );
}
