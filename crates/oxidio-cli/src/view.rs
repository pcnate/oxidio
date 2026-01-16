//! View mode management for the TUI.
//!
//! Defines the different views available in the application.


/// Visualization style for the visualizer view.
#[derive( Debug, Clone, Copy, PartialEq, Eq, Default )]
pub enum VisualizerStyle {
    /// Vertical bars (default)
    #[default]
    Bars,

    /// Spectrum analyzer with mirrored bars
    Spectrum,

    /// Oscilloscope waveform
    Waveform,

    /// Horizontal level meter
    LevelMeter,
}


impl VisualizerStyle {
    /// Returns the next visualization style.
    pub fn next( self ) -> Self {
        match self {
            VisualizerStyle::Bars => VisualizerStyle::Spectrum,
            VisualizerStyle::Spectrum => VisualizerStyle::Waveform,
            VisualizerStyle::Waveform => VisualizerStyle::LevelMeter,
            VisualizerStyle::LevelMeter => VisualizerStyle::Bars,
        }
    }


    /// Returns the name of the visualization style.
    pub fn name( &self ) -> &'static str {
        match self {
            VisualizerStyle::Bars => "Bars",
            VisualizerStyle::Spectrum => "Spectrum",
            VisualizerStyle::Waveform => "Waveform",
            VisualizerStyle::LevelMeter => "Level Meter",
        }
    }
}


/// Current view mode of the application.
#[derive( Debug, Clone, Copy, PartialEq, Eq, Default )]
pub enum ViewMode {
    /// Playlist view - main view showing current playlist.
    #[default]
    Playlist,

    /// Browser view - file/directory browser.
    Browser,

    /// Help overlay - shows available commands.
    Help,

    /// Track info - shows metadata and details for current/selected track.
    TrackInfo,

    /// Large visualizer - full screen audio visualization.
    Visualizer,

    /// Settings view - configure app options.
    Settings,
}


impl ViewMode {
    /// Returns the next view in tab order (excluding Help overlay).
    pub fn next_tab( self ) -> Self {
        match self {
            ViewMode::Playlist => ViewMode::Browser,
            ViewMode::Browser => ViewMode::TrackInfo,
            ViewMode::TrackInfo => ViewMode::Visualizer,
            ViewMode::Visualizer => ViewMode::Settings,
            ViewMode::Settings => ViewMode::Playlist,
            ViewMode::Help => ViewMode::Help, // Help stays on Help until dismissed
        }
    }


    /// Returns the previous view in tab order (excluding Help overlay).
    pub fn prev_tab( self ) -> Self {
        match self {
            ViewMode::Playlist => ViewMode::Settings,
            ViewMode::Browser => ViewMode::Playlist,
            ViewMode::TrackInfo => ViewMode::Browser,
            ViewMode::Visualizer => ViewMode::TrackInfo,
            ViewMode::Settings => ViewMode::Visualizer,
            ViewMode::Help => ViewMode::Help, // Help stays on Help until dismissed
        }
    }
}
