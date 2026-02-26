//! Axum HTTP server with embedded static assets and WebSocket upgrade.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ State, WebSocketUpgrade };
use axum::http::{ header, StatusCode, Uri };
use axum::response::{ Html, IntoResponse };
use axum::routing::get;
use axum::Router;
use rust_embed::Embed;
use tokio::sync::broadcast;
use tower_http::cors::CorsLayer;

use oxidio_ctl::CommandSender;
use oxidio_protocol::StateUpdate;

use crate::websocket;


/// Embedded static assets (HTML, JS, CSS, WASM).
///
/// These are compiled into the binary from the `static/` directory.
#[derive( Embed )]
#[folder = "static/"]
struct StaticAssets;


/// Shared state for the axum handlers.
#[derive( Clone )]
struct AppState {
    sender: CommandSender,
    broadcast_tx: Arc<broadcast::Sender<StateUpdate>>,
}


/// Handle returned from `start_web_server` to manage the server lifecycle.
pub struct WebServerHandle {
    shutdown_tx: tokio::sync::oneshot::Sender<()>,
}


impl WebServerHandle {
    /// Signals the web server to shut down gracefully.
    pub fn shutdown( self ) {
        let _ = self.shutdown_tx.send(());
    }
}


/// Starts the web server in the background.
///
/// @param bind - Address to bind to (e.g. "127.0.0.1")
/// @param port - Port number
/// @param sender - Command sender for WebSocket clients
/// @param broadcast_tx - Broadcast sender for subscribing new clients
///
/// @returns A handle for shutting down the server
pub async fn start_web_server(
    bind: &str,
    port: u16,
    sender: CommandSender,
    broadcast_tx: broadcast::Sender<StateUpdate>,
) -> Result<WebServerHandle, std::io::Error> {
    let state = AppState {
        sender,
        broadcast_tx: Arc::new( broadcast_tx ),
    };

    let app = Router::new()
        .route( "/ws", get( ws_handler ) )
        .route( "/", get( index_handler ) )
        .fallback( static_handler )
        .layer( CorsLayer::permissive() )
        .with_state( state );

    let addr: SocketAddr = format!( "{}:{}", bind, port )
        .parse()
        .expect( "Invalid bind address" );

    let listener = tokio::net::TcpListener::bind( addr ).await?;
    let ( shutdown_tx, shutdown_rx ) = tokio::sync::oneshot::channel::<()>();

    tracing::info!( "Web server listening on http://{}", addr );

    tokio::spawn( async move {
        axum::serve( listener, app )
            .with_graceful_shutdown( async {
                let _ = shutdown_rx.await;
            })
            .await
            .unwrap_or_else( |e| {
                tracing::error!( "Web server error: {}", e );
            });
    });

    Ok( WebServerHandle { shutdown_tx } )
}


/// WebSocket upgrade handler.
async fn ws_handler(
    ws: WebSocketUpgrade,
    State( state ): State<AppState>,
) -> impl IntoResponse {
    let sender = state.sender.clone();
    let state_rx = state.broadcast_tx.subscribe();

    ws.on_upgrade( move |socket| {
        websocket::handle_ws( socket, sender, state_rx )
    })
}


/// Serves the index.html page.
async fn index_handler() -> impl IntoResponse {
    match StaticAssets::get( "index.html" ) {
        Some( content ) => Html( content.data.to_vec() ).into_response(),
        None => ( StatusCode::NOT_FOUND, "index.html not found" ).into_response(),
    }
}


/// Serves embedded static assets by path.
async fn static_handler( uri: Uri ) -> impl IntoResponse {
    let path = uri.path().trim_start_matches( '/' );

    match StaticAssets::get( path ) {
        Some( content ) => {
            let mime = mime_guess::from_path( path ).first_or_octet_stream();
            ( [( header::CONTENT_TYPE, mime.as_ref() )], content.data ).into_response()
        }
        None => ( StatusCode::NOT_FOUND, "Not found" ).into_response(),
    }
}
