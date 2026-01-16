//! Audio output via cpal
//!
//! Handles sending decoded PCM samples to the system audio device.

use std::sync::{ Arc, Mutex };
use std::sync::atomic::{ AtomicBool, AtomicU32, Ordering };
use std::collections::VecDeque;

use cpal::traits::{ DeviceTrait, HostTrait, StreamTrait };
use thiserror::Error;


/// Errors that can occur with audio output.
#[derive( Debug, Error )]
pub enum OutputError {
    #[error( "No output device available" )]
    NoDevice,

    #[error( "Failed to get default stream config: {0}" )]
    StreamConfig( String ),

    #[error( "Failed to build output stream: {0}" )]
    BuildStream( String ),

    #[error( "Failed to play stream: {0}" )]
    PlayStream( String ),
}


/// Number of visualization bars to display
pub const VIS_BARS: usize = 32;


/// Shared sample buffer between producer (decoder) and consumer (audio callback).
/// This is Send + Sync and can be shared across threads.
/// Handles channel conversion between source and output.
pub struct SampleBuffer {
    buffer: Mutex<VecDeque<f32>>,
    capacity: usize,
    paused: AtomicBool,
    /// Volume level stored as f32 bits (0.0 to 1.0+)
    volume: AtomicU32,
    source_channels: u16,
    output_channels: u16,
    /// Visualization data - RMS amplitudes for display
    vis_data: Mutex<[f32; VIS_BARS]>,
}


impl SampleBuffer {
    /// Creates a new sample buffer with the given capacity and channel configuration.
    ///
    /// - `capacity`: Maximum number of samples to buffer
    /// - `source_channels`: Number of channels in the source audio (from decoder)
    /// - `output_channels`: Number of channels expected by the output device
    pub fn new( capacity: usize, source_channels: u16, output_channels: u16 ) -> Self {
        Self {
            buffer: Mutex::new( VecDeque::with_capacity( capacity ) ),
            vis_data: Mutex::new( [0.0; VIS_BARS] ),
            capacity,
            paused: AtomicBool::new( false ),
            volume: AtomicU32::new( 1.0_f32.to_bits() ),
            source_channels,
            output_channels,
        }
    }


    /// Pushes samples to the buffer. Returns number of samples actually pushed.
    /// Also updates visualization data with RMS values.
    pub fn push( &self, samples: &[f32] ) -> usize {
        let mut buf = self.buffer.lock().unwrap();
        let available = self.capacity.saturating_sub( buf.len() );
        let to_push = samples.len().min( available );
        buf.extend( samples[ ..to_push ].iter().copied() );

        // Update visualization data if we have enough samples
        if to_push >= VIS_BARS {
            let mut vis = self.vis_data.lock().unwrap();
            let samples_per_bar = to_push / VIS_BARS;

            for ( bar_idx, bar ) in vis.iter_mut().enumerate() {
                let start = bar_idx * samples_per_bar;
                let end = ( start + samples_per_bar ).min( to_push );

                // Calculate RMS for this bar
                let sum_sq: f32 = samples[ start..end ]
                    .iter()
                    .map( |s| s * s )
                    .sum();
                let rms = ( sum_sq / ( end - start ) as f32 ).sqrt();

                // Smooth with previous value (decay)
                *bar = ( *bar * 0.7 ) + ( rms * 0.3 );
            }
        }

        to_push
    }


    /// Pops samples from the buffer into the output slice, handling channel conversion.
    /// Returns the number of output samples actually written.
    pub fn pop( &self, output: &mut [f32] ) -> usize {
        // If paused, output silence
        if self.paused.load( Ordering::Relaxed ) {
            for sample in output.iter_mut() {
                *sample = 0.0;
            }
            return 0;
        }

        let volume = f32::from_bits( self.volume.load( Ordering::Relaxed ) );
        let mut buf = self.buffer.lock().unwrap();
        let src_ch = self.source_channels as usize;
        let out_ch = self.output_channels as usize;

        let written = if src_ch == out_ch {
            // No conversion needed
            let to_pop = output.len().min( buf.len() );
            for i in 0..to_pop {
                output[ i ] = buf.pop_front().unwrap();
            }
            // Fill remaining with silence
            for i in to_pop..output.len() {
                output[ i ] = 0.0;
            }
            to_pop
        } else if src_ch == 1 && out_ch == 2 {
            // Mono to stereo: duplicate each sample
            let output_frames = output.len() / out_ch;
            let available_frames = buf.len() / src_ch;
            let frames_to_process = output_frames.min( available_frames );

            for i in 0..frames_to_process {
                let sample = buf.pop_front().unwrap();
                output[ i * 2 ] = sample;
                output[ i * 2 + 1 ] = sample;
            }
            // Fill remaining with silence
            for i in ( frames_to_process * out_ch )..output.len() {
                output[ i ] = 0.0;
            }
            frames_to_process * out_ch
        } else if src_ch == 2 && out_ch == 1 {
            // Stereo to mono: mix down
            let output_frames = output.len() / out_ch;
            let available_frames = buf.len() / src_ch;
            let frames_to_process = output_frames.min( available_frames );

            for i in 0..frames_to_process {
                let left = buf.pop_front().unwrap();
                let right = buf.pop_front().unwrap();
                output[ i ] = ( left + right ) * 0.5;
            }
            // Fill remaining with silence
            for i in frames_to_process..output.len() {
                output[ i ] = 0.0;
            }
            frames_to_process
        } else {
            // General case: simple remix (duplicate first channel or mix all to fewer)
            let output_frames = output.len() / out_ch;
            let available_frames = buf.len() / src_ch;
            let frames_to_process = output_frames.min( available_frames );

            for frame in 0..frames_to_process {
                // Read source frame
                let mut src_samples = Vec::with_capacity( src_ch );
                for _ in 0..src_ch {
                    src_samples.push( buf.pop_front().unwrap() );
                }

                // Write output frame
                for ch in 0..out_ch {
                    if ch < src_ch {
                        output[ frame * out_ch + ch ] = src_samples[ ch ];
                    } else {
                        // Duplicate last channel if output has more channels
                        output[ frame * out_ch + ch ] = src_samples[ src_ch - 1 ];
                    }
                }
            }
            // Fill remaining with silence
            for i in ( frames_to_process * out_ch )..output.len() {
                output[ i ] = 0.0;
            }
            frames_to_process * out_ch
        };

        // Apply volume to all output samples
        if volume != 1.0 {
            for sample in output[ ..written ].iter_mut() {
                *sample *= volume;
            }
        }

        written
    }


    /// Returns the number of samples currently in the buffer.
    pub fn len( &self ) -> usize {
        self.buffer.lock().unwrap().len()
    }


    /// Returns true if the buffer is empty.
    pub fn is_empty( &self ) -> bool {
        self.buffer.lock().unwrap().is_empty()
    }


    /// Clears the buffer.
    pub fn clear( &self ) {
        self.buffer.lock().unwrap().clear();
    }


    /// Sets paused state.
    pub fn set_paused( &self, paused: bool ) {
        self.paused.store( paused, Ordering::Relaxed );
    }


    /// Gets paused state.
    pub fn is_paused( &self ) -> bool {
        self.paused.load( Ordering::Relaxed )
    }


    /// Gets the current visualization data (RMS amplitudes for each bar).
    pub fn vis_data( &self ) -> [f32; VIS_BARS] {
        *self.vis_data.lock().unwrap()
    }


    /// Sets the volume level (0.0 = mute, 1.0 = normal, >1.0 = boost).
    pub fn set_volume( &self, volume: f32 ) {
        self.volume.store( volume.to_bits(), Ordering::Relaxed );
    }


    /// Gets the current volume level.
    pub fn volume( &self ) -> f32 {
        f32::from_bits( self.volume.load( Ordering::Relaxed ) )
    }
}


/// Audio output handler.
/// Note: This struct is NOT Send/Sync due to cpal::Stream.
/// Keep it on the thread where it was created.
pub struct AudioOutput {
    stream: cpal::Stream,
    sample_rate: u32,
    channels: u16,
}


impl AudioOutput {
    /// Creates a new audio output with the specified source sample rate and channels.
    ///
    /// Returns both the AudioOutput and a shared SampleBuffer that the caller should
    /// use to push decoded samples. The buffer handles channel conversion if needed.
    pub fn new(
        source_sample_rate: u32,
        source_channels: u16,
    ) -> Result<( Self, Arc<SampleBuffer> ), OutputError> {
        let host = cpal::default_host();

        let device = host
            .default_output_device()
            .ok_or( OutputError::NoDevice )?;

        tracing::info!( "Using output device: {:?}", device.name() );

        // Try to get a config matching our requirements
        // Priority: 1) exact match, 2) same sample rate any channels, 3) default with warning
        let supported_configs: Vec<_> = device
            .supported_output_configs()
            .map_err( |e| OutputError::StreamConfig( e.to_string() ) )?
            .collect();

        // First try: exact match (channels + sample rate)
        let config = if let Some( supported_config ) = supported_configs.iter().find( |c| {
            c.channels() == source_channels
                && c.min_sample_rate().0 <= source_sample_rate
                && c.max_sample_rate().0 >= source_sample_rate
        }) {
            supported_config.clone()
                .with_sample_rate( cpal::SampleRate( source_sample_rate ) )
                .config()
        }
        // Second try: any config that supports our sample rate (we'll handle channel conversion)
        else if let Some( supported_config ) = supported_configs.iter().find( |c| {
            c.min_sample_rate().0 <= source_sample_rate
                && c.max_sample_rate().0 >= source_sample_rate
        }) {
            tracing::info!(
                "Channel conversion: file has {} channels, device using {} channels",
                source_channels,
                supported_config.channels()
            );
            supported_config.clone()
                .with_sample_rate( cpal::SampleRate( source_sample_rate ) )
                .config()
        }
        // Last resort: default config (may have wrong sample rate!)
        else {
            let default_config = device
                .default_output_config()
                .map_err( |e| OutputError::StreamConfig( e.to_string() ) )?;
            tracing::warn!(
                "Sample rate mismatch: file is {} Hz, device defaulting to {} Hz - playback speed may be incorrect!",
                source_sample_rate,
                default_config.sample_rate().0
            );
            default_config.config()
        };

        tracing::info!(
            "Audio output config: {} Hz, {} channels",
            config.sample_rate.0,
            config.channels
        );

        // Create shared sample buffer with channel conversion info
        // Buffer size: ~500ms of audio
        let buffer_capacity = ( source_sample_rate as usize ) * ( source_channels as usize ) / 2;
        let sample_buffer = Arc::new( SampleBuffer::new(
            buffer_capacity,
            source_channels,
            config.channels,
        ));
        let sample_buffer_clone = Arc::clone( &sample_buffer );

        let stream = device
            .build_output_stream(
                &config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    sample_buffer_clone.pop( data );
                },
                |err| {
                    tracing::error!( "Audio output error: {}", err );
                },
                None,
            )
            .map_err( |e| OutputError::BuildStream( e.to_string() ) )?;

        Ok((
            Self {
                stream,
                sample_rate: config.sample_rate.0,
                channels: config.channels,
            },
            sample_buffer,
        ))
    }


    /// Starts audio output.
    pub fn play( &self ) -> Result<(), OutputError> {
        self.stream
            .play()
            .map_err( |e| OutputError::PlayStream( e.to_string() ) )
    }


    /// Pauses the audio stream.
    pub fn pause( &self ) -> Result<(), OutputError> {
        self.stream
            .pause()
            .map_err( |e| OutputError::PlayStream( e.to_string() ) )
    }


    /// Gets the actual sample rate.
    pub fn sample_rate( &self ) -> u32 {
        self.sample_rate
    }


    /// Gets the actual number of channels.
    pub fn channels( &self ) -> u16 {
        self.channels
    }
}
