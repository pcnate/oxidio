//! System media transport controls integration
//!
//! Provides integration with OS media controls:
//! - Windows: System Media Transport Controls (SMTC)
//! - Linux: MPRIS D-Bus interface (not currently supported in cross-compilation)

#[cfg( target_os = "windows" )]
mod platform {
    use std::sync::mpsc::Sender;
    use std::ffi::c_void;

    use souvlaki::{ MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, PlatformConfig };
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{ GetLastError, HWND, LPARAM, LRESULT, WPARAM };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, RegisterClassW,
        CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, WNDCLASSW, WS_OVERLAPPEDWINDOW,
    };

    // ERROR_CLASS_ALREADY_EXISTS = 1410
    const ERROR_CLASS_ALREADY_EXISTS: u32 = 1410;


    /// Window procedure for the hidden SMTC window.
    unsafe extern "system" fn wnd_proc( hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM ) -> LRESULT {
        DefWindowProcW( hwnd, msg, wparam, lparam )
    }


    /// Events from media controls that the app should handle.
    #[derive( Debug, Clone )]
    pub enum MediaControlCommand {
        Play,
        Pause,
        Toggle,
        Stop,
        Next,
        Previous,
    }


    /// Wrapper around souvlaki MediaControls.
    pub struct MediaControlsHandler {
        controls: MediaControls,
        #[allow( dead_code )]
        hwnd: HWND, // Keep the window alive
    }


    impl MediaControlsHandler {
        /// Creates a new media controls handler.
        ///
        /// Returns None if media controls are not available on this platform.
        pub fn new( event_sender: Sender<MediaControlCommand> ) -> Option<Self> {
            // Create a hidden window for SMTC (console windows don't work well)
            let hwnd = Self::create_hidden_window()?;

            let config = PlatformConfig {
                dbus_name: "oxidio",
                display_name: "Oxidio Music Player",
                hwnd: Some( hwnd.0 as *mut c_void ),
            };

            let mut controls = match MediaControls::new( config ) {
                Ok( c ) => c,
                Err( e ) => {
                    tracing::warn!( "Failed to create media controls: {:?}", e );
                    return None;
                }
            };

            // Attach event handler
            if let Err( e ) = controls.attach( move |event: MediaControlEvent| {
                let cmd = match event {
                    MediaControlEvent::Play => Some( MediaControlCommand::Play ),
                    MediaControlEvent::Pause => Some( MediaControlCommand::Pause ),
                    MediaControlEvent::Toggle => Some( MediaControlCommand::Toggle ),
                    MediaControlEvent::Stop => Some( MediaControlCommand::Stop ),
                    MediaControlEvent::Next => Some( MediaControlCommand::Next ),
                    MediaControlEvent::Previous => Some( MediaControlCommand::Previous ),
                    _ => None,
                };
                if let Some( cmd ) = cmd {
                    let _ = event_sender.send( cmd );
                }
            }) {
                tracing::warn!( "Failed to attach media control handler: {:?}", e );
                return None;
            }

            tracing::info!( "SMTC initialized with hidden window" );
            Some( Self { controls, hwnd })
        }


        /// Creates a hidden window for SMTC binding.
        fn create_hidden_window() -> Option<HWND> {
            unsafe {
                // Set AppUserModelID so Windows identifies our app correctly in SMTC
                use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
                use windows::core::HSTRING;

                let app_id = HSTRING::from( "Oxidio.MusicPlayer" );
                let _ = SetCurrentProcessExplicitAppUserModelID( &app_id );

                // Define window class name
                let class_name: Vec<u16> = "OxidioSMTC\0".encode_utf16().collect();

                // Register window class
                let wc = WNDCLASSW {
                    style: CS_HREDRAW | CS_VREDRAW,
                    lpfnWndProc: Some( wnd_proc ),
                    hInstance: windows::Win32::Foundation::HINSTANCE::default(),
                    lpszClassName: PCWSTR( class_name.as_ptr() ),
                    ..Default::default()
                };

                let atom = RegisterClassW( &wc );
                if atom == 0 {
                    // Check if class already exists (can happen when re-enabling SMTC)
                    let error = GetLastError();
                    if error.0 != ERROR_CLASS_ALREADY_EXISTS {
                        tracing::warn!( "Failed to register SMTC window class: {:?}", error );
                        return None;
                    }
                    // Class already exists, that's fine - continue to create window
                    tracing::debug!( "SMTC window class already registered, reusing" );
                }

                // Create the hidden window
                let window_name: Vec<u16> = "Oxidio Music Player\0".encode_utf16().collect();

                let hwnd = match CreateWindowExW(
                    windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE::default(),
                    PCWSTR( class_name.as_ptr() ),
                    PCWSTR( window_name.as_ptr() ),
                    WS_OVERLAPPEDWINDOW, // Don't use WS_VISIBLE - keep it hidden
                    CW_USEDEFAULT,
                    CW_USEDEFAULT,
                    CW_USEDEFAULT,
                    CW_USEDEFAULT,
                    HWND::default(),
                    None,
                    None,
                    None,
                ) {
                    Ok( hwnd ) => hwnd,
                    Err( e ) => {
                        tracing::warn!( "Failed to create SMTC hidden window: {:?}", e );
                        return None;
                    }
                };

                if hwnd.0.is_null() {
                    tracing::warn!( "SMTC hidden window handle is null" );
                    return None;
                }

                Some( hwnd )
            }
        }


        /// Updates the playback state.
        pub fn set_playback( &mut self, playback: MediaPlayback ) {
            if let Err( e ) = self.controls.set_playback( playback ) {
                tracing::debug!( "Failed to set playback state: {:?}", e );
            }
        }


        /// Updates the metadata displayed in the media controls.
        /// Returns an error string if the operation failed.
        pub fn set_metadata( &mut self, metadata: MediaMetadata ) -> Option<String> {
            tracing::debug!(
                "Setting SMTC metadata: title={:?}, artist={:?}, album={:?}",
                metadata.title, metadata.artist, metadata.album
            );
            if let Err( e ) = self.controls.set_metadata( metadata ) {
                let err_msg = format!( "SMTC metadata error: {:?}", e );
                tracing::warn!( "{}", err_msg );
                return Some( err_msg );
            }
            None
        }
    }
}


// Stub module for non-Windows platforms
#[cfg( not( target_os = "windows" ) )]
mod platform {
    use std::sync::mpsc::Sender;


    /// Events from media controls that the app should handle.
    #[derive( Debug, Clone )]
    pub enum MediaControlCommand {
        Play,
        Pause,
        Toggle,
        Stop,
        Next,
        Previous,
    }


    /// Stub for platforms without media control support.
    pub struct MediaControlsHandler;


    impl MediaControlsHandler {
        /// Returns None on unsupported platforms.
        pub fn new( _event_sender: Sender<MediaControlCommand> ) -> Option<Self> {
            None
        }


        pub fn set_playback( &mut self, _playback: () ) {}


        pub fn set_metadata( &mut self, _metadata: () ) -> Option<String> { None }
    }
}


pub use platform::{ MediaControlCommand, MediaControlsHandler };
use std::sync::mpsc;


/// Creates a channel for media control events.
pub fn create_media_controls_channel() -> ( mpsc::Sender<MediaControlCommand>, mpsc::Receiver<MediaControlCommand> ) {
    mpsc::channel()
}
