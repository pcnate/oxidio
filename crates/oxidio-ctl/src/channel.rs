//! Control channel types for sending commands and receiving state updates.

use tokio::sync::{ broadcast, mpsc };

use oxidio_protocol::{ AppCommand, StateUpdate };


/// Default capacity for the command channel.
const COMMAND_CHANNEL_CAPACITY: usize = 256;

/// Default capacity for the broadcast channel.
const BROADCAST_CHANNEL_CAPACITY: usize = 64;


/// A cloneable handle for sending commands into the control channel.
///
/// Any frontend (TUI, web, CLI oneshot) can clone this and send commands.
#[derive( Clone )]
pub struct CommandSender {
    tx: mpsc::Sender<AppCommand>,
}


impl CommandSender {
    /// Sends a command to the control channel.
    ///
    /// @param cmd - The command to send
    pub async fn send( &self, cmd: AppCommand ) -> Result<(), mpsc::error::SendError<AppCommand>> {
        self.tx.send( cmd ).await
    }


    /// Tries to send a command without blocking.
    ///
    /// Useful for synchronous contexts (e.g., TUI event handlers).
    ///
    /// @param cmd - The command to send
    pub fn try_send( &self, cmd: AppCommand ) -> Result<(), mpsc::error::TrySendError<AppCommand>> {
        self.tx.try_send( cmd )
    }
}


/// Handle for creating senders and subscribers to the control channel.
///
/// This is the factory for creating client connections to the message bus.
pub struct ControlChannel {
    command_tx: mpsc::Sender<AppCommand>,
    command_rx: Option<mpsc::Receiver<AppCommand>>,
    broadcast_tx: broadcast::Sender<StateUpdate>,
}


impl ControlChannel {
    /// Creates a new control channel.
    ///
    /// @returns A new ControlChannel instance
    pub fn new() -> Self {
        let ( command_tx, command_rx ) = mpsc::channel( COMMAND_CHANNEL_CAPACITY );
        let ( broadcast_tx, _broadcast_rx ) = broadcast::channel( BROADCAST_CHANNEL_CAPACITY );

        Self {
            command_tx,
            command_rx: Some( command_rx ),
            broadcast_tx,
        }
    }


    /// Creates a new `CommandSender` for a frontend client.
    pub fn sender( &self ) -> CommandSender {
        CommandSender {
            tx: self.command_tx.clone(),
        }
    }


    /// Subscribes to state updates from the command processor.
    ///
    /// @returns A broadcast receiver for state updates
    pub fn subscribe( &self ) -> broadcast::Receiver<StateUpdate> {
        self.broadcast_tx.subscribe()
    }


    /// Takes the command receiver (can only be called once).
    ///
    /// This is consumed by the `CommandProcessor` during initialization.
    ///
    /// @returns The command receiver, or None if already taken
    pub fn take_command_rx( &mut self ) -> Option<mpsc::Receiver<AppCommand>> {
        self.command_rx.take()
    }


    /// Gets a clone of the broadcast sender.
    ///
    /// Used by the `CommandProcessor` to broadcast state updates.
    pub fn broadcast_tx( &self ) -> broadcast::Sender<StateUpdate> {
        self.broadcast_tx.clone()
    }
}
