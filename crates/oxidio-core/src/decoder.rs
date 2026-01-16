//! Audio decoding via Symphonia
//!
//! Handles decoding of various audio formats into raw PCM samples.

use std::fs::File;
use std::path::Path;

use symphonia::core::audio::{ SampleBuffer, SignalSpec };
use symphonia::core::codecs::{ Decoder as SymphoniaDecoder, DecoderOptions, CODEC_TYPE_NULL };
use symphonia::core::formats::{ FormatOptions, FormatReader, SeekMode, SeekTo };
use symphonia::core::io::{ MediaSourceStream, MediaSourceStreamOptions };
use symphonia::core::meta::{ MetadataOptions, StandardTagKey };
use symphonia::core::probe::ProbedMetadata;
use symphonia::core::probe::Hint;
use symphonia::core::units::Time;
use thiserror::Error;


/// Audio metadata extracted from the file.
#[derive( Debug, Clone, Default )]
pub struct AudioMetadata {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_number: Option<u32>,
    pub genre: Option<String>,
    pub year: Option<u32>,
    pub codec: Option<String>,
    pub bitrate: Option<u32>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u32>,
}


/// Errors that can occur during decoding.
#[derive( Debug, Error )]
pub enum DecoderError {
    #[error( "Failed to open file: {0}" )]
    FileOpen( #[from] std::io::Error ),

    #[error( "Unsupported format" )]
    UnsupportedFormat,

    #[error( "No audio tracks found" )]
    NoAudioTrack,

    #[error( "Decoder creation failed: {0}" )]
    DecoderCreation( String ),

    #[error( "Decode error: {0}" )]
    Decode( String ),

    #[error( "Seek error: {0}" )]
    Seek( String ),
}


/// Audio decoder wrapper around Symphonia.
pub struct Decoder {
    format_reader: Box<dyn FormatReader>,
    decoder: Box<dyn SymphoniaDecoder>,
    track_id: u32,
    sample_rate: u32,
    channels: usize,
    sample_buf: Option<SampleBuffer<f32>>,
    duration: Option<f64>,
    /// Metadata from probe result (ID3 tags, etc.)
    probe_metadata: ProbedMetadata,
}


impl Decoder {
    /// Opens an audio file for decoding.
    ///
    /// Supports SMB/UNC paths transparently via std::fs.
    pub fn open( path: &Path ) -> Result<Self, DecoderError> {
        // Use larger buffer for network paths (SMB)
        let buffer_len = if path.starts_with( r"\\" ) {
            256 * 1024 // 256KB for network paths
        } else {
            64 * 1024 // 64KB for local paths
        };

        let file = File::open( path )?;
        let mss_opts = MediaSourceStreamOptions { buffer_len };
        let mss = MediaSourceStream::new( Box::new( file ), mss_opts );

        // Provide hint based on file extension
        let mut hint = Hint::new();
        if let Some( ext ) = path.extension().and_then( |e| e.to_str() ) {
            hint.with_extension( ext );
        }

        let format_opts = FormatOptions::default();
        let metadata_opts = MetadataOptions::default();

        // Probe the file to determine format
        let probed = symphonia::default::get_probe()
            .format( &hint, mss, &format_opts, &metadata_opts )
            .map_err( |_| DecoderError::UnsupportedFormat )?;

        // Capture metadata from the probe result (ID3 tags, etc.)
        let probe_metadata = probed.metadata;
        let format_reader = probed.format;

        // Find the first audio track
        let track = format_reader
            .tracks()
            .iter()
            .find( |t| t.codec_params.codec != CODEC_TYPE_NULL )
            .ok_or( DecoderError::NoAudioTrack )?;

        let track_id = track.id;
        let codec_params = &track.codec_params;

        let sample_rate = codec_params.sample_rate.unwrap_or( 44100 );
        let channels = codec_params.channels.map( |c| c.count() ).unwrap_or( 2 );

        // Calculate duration if available
        let duration = codec_params.n_frames.map( |frames| {
            frames as f64 / sample_rate as f64
        });

        tracing::info!(
            "Opened audio: {} Hz, {} channels, duration: {:?}s",
            sample_rate,
            channels,
            duration
        );

        // Create the decoder
        let decoder_opts = DecoderOptions::default();
        let decoder = symphonia::default::get_codecs()
            .make( codec_params, &decoder_opts )
            .map_err( |e| DecoderError::DecoderCreation( e.to_string() ) )?;

        Ok( Self {
            format_reader,
            decoder,
            track_id,
            sample_rate,
            channels,
            sample_buf: None,
            duration,
            probe_metadata,
        })
    }


    /// Returns the sample rate of the audio.
    pub fn sample_rate( &self ) -> u32 {
        self.sample_rate
    }


    /// Returns the number of channels.
    pub fn channels( &self ) -> usize {
        self.channels
    }


    /// Returns the duration in seconds, if known.
    pub fn duration( &self ) -> Option<f64> {
        self.duration
    }


    /// Extracts metadata from the audio file.
    pub fn metadata( &mut self ) -> AudioMetadata {
        let mut meta = AudioMetadata::default();

        // Helper to extract tags from a metadata revision
        let extract_tags = |meta: &mut AudioMetadata, tags: &[symphonia::core::meta::Tag]| {
            for tag in tags {
                if let Some( std_key ) = tag.std_key {
                    let value = tag.value.to_string();
                    match std_key {
                        StandardTagKey::TrackTitle => {
                            if meta.title.is_none() {
                                meta.title = Some( value );
                            }
                        }
                        StandardTagKey::Artist => {
                            if meta.artist.is_none() {
                                meta.artist = Some( value );
                            }
                        }
                        StandardTagKey::Album => {
                            if meta.album.is_none() {
                                meta.album = Some( value );
                            }
                        }
                        StandardTagKey::AlbumArtist => {
                            if meta.album_artist.is_none() {
                                meta.album_artist = Some( value );
                            }
                        }
                        StandardTagKey::TrackNumber => {
                            if meta.track_number.is_none() {
                                meta.track_number = value.parse().ok();
                            }
                        }
                        StandardTagKey::Genre => {
                            if meta.genre.is_none() {
                                meta.genre = Some( value );
                            }
                        }
                        StandardTagKey::Date | StandardTagKey::ReleaseDate => {
                            if meta.year.is_none() {
                                // Extract year from date string (e.g., "2023" or "2023-01-15")
                                if let Some( year_str ) = value.split( '-' ).next() {
                                    meta.year = year_str.parse().ok();
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        };

        // First check probe metadata (ID3 tags, etc.)
        if let Some( metadata_log ) = self.probe_metadata.get() {
            if let Some( metadata_rev ) = metadata_log.current() {
                extract_tags( &mut meta, metadata_rev.tags() );
            }
        }

        // Then check format reader metadata (may have additional tags)
        if let Some( metadata_rev ) = self.format_reader.metadata().current() {
            extract_tags( &mut meta, metadata_rev.tags() );
        }

        // Add audio format information
        meta.sample_rate = Some( self.sample_rate );
        meta.channels = Some( self.channels as u32 );

        // Get codec and bitrate from track info
        if let Some( track ) = self.format_reader.tracks().iter().find( |t| t.id == self.track_id ) {
            // Get codec name
            let codec_type = track.codec_params.codec;
            meta.codec = Some( format!( "{:?}", codec_type ).replace( "CODEC_TYPE_", "" ) );

            // Get bitrate if available
            if let Some( bit_rate ) = track.codec_params.bits_per_sample {
                // Calculate approximate bitrate: bits_per_sample * sample_rate * channels
                let bitrate = bit_rate * self.sample_rate * self.channels as u32 / 1000;
                meta.bitrate = Some( bitrate );
            }
        }

        meta
    }


    /// Returns the signal specification.
    pub fn signal_spec( &self ) -> SignalSpec {
        SignalSpec::new( self.sample_rate, symphonia::core::audio::Channels::FRONT_LEFT | symphonia::core::audio::Channels::FRONT_RIGHT )
    }


    /// Decodes the next packet and returns interleaved f32 samples.
    ///
    /// Returns None when EOF is reached.
    pub fn decode_next( &mut self ) -> Result<Option<Vec<f32>>, DecoderError> {
        loop {
            // Get the next packet
            let packet = match self.format_reader.next_packet() {
                Ok( packet ) => packet,
                Err( symphonia::core::errors::Error::IoError( ref e ) )
                    if e.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    return Ok( None ); // EOF
                }
                Err( e ) => {
                    return Err( DecoderError::Decode( e.to_string() ) );
                }
            };

            // Skip packets not for our track
            if packet.track_id() != self.track_id {
                continue;
            }

            // Decode the packet
            let decoded = match self.decoder.decode( &packet ) {
                Ok( decoded ) => decoded,
                Err( symphonia::core::errors::Error::DecodeError( _ ) ) => {
                    // Decode errors are recoverable, skip this packet
                    continue;
                }
                Err( e ) => {
                    return Err( DecoderError::Decode( e.to_string() ) );
                }
            };

            // Convert to f32 samples
            let spec = *decoded.spec();
            let num_frames = decoded.frames();

            // Create or resize sample buffer
            if self.sample_buf.is_none() || self.sample_buf.as_ref().unwrap().capacity() < num_frames {
                self.sample_buf = Some( SampleBuffer::new( num_frames as u64, spec ) );
            }

            let sample_buf = self.sample_buf.as_mut().unwrap();
            sample_buf.copy_interleaved_ref( decoded );

            return Ok( Some( sample_buf.samples().to_vec() ) );
        }
    }


    /// Seeks to a position in seconds.
    pub fn seek( &mut self, position_secs: f64 ) -> Result<(), DecoderError> {
        let seek_to = SeekTo::Time {
            time: Time::from( position_secs ),
            track_id: Some( self.track_id ),
        };

        self.format_reader
            .seek( SeekMode::Accurate, seek_to )
            .map_err( |e| DecoderError::Seek( e.to_string() ) )?;

        // Reset decoder state after seek
        self.decoder.reset();

        Ok(())
    }
}
