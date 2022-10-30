use std::convert::TryInto;
use std::io::{Read, BufReader, SeekFrom, Seek};

use symphonia_core::codecs::CodecParameters;
use symphonia_core::errors::{end_of_stream_error, seek_error, unsupported_error, decode_error};
use symphonia_core::errors::{Result, SeekErrorKind};
use symphonia_core::formats::prelude::*;
use symphonia_core::io::*;
use symphonia_core::meta::{Metadata, MetadataBuilder, MetadataLog, MetadataRevision, MetadataReader};
use symphonia_core::probe::{Descriptor, Instantiate, QueryDescriptor};
use symphonia_core::support_format;

use log::{debug, error};

mod chunks;
mod extended;
mod ids;
mod util;

use chunks::{ChunksReader, AiffChunks, SoundDataChunk, CommonChunk, AiffFormatData};
// use util::read_i32_be;

const AIFF_DATA_MARKER: [u8; 4] = *b"FORM";

const AIFF_FORM: [u8; 4] = *b"AIFF";

const AIFF_MAX_FRAMES_PER_PACKET: u64 = 1029;


pub struct AiffReader {
    reader: MediaSourceStream,
    tracks: Vec<Track>,
    cues: Vec<Cue>,
    // metadata: MetadataLog,
    frame_len: u32,
    data_start_pos: u64,
    data_end_pos: u64,
}

impl QueryDescriptor for AiffReader {
    fn query() -> &'static [Descriptor] {
        &[
            // WAVE RIFF form
            support_format!(
                "aiff",
                "Audio Interchange File Format",
                &["aif", "aiff"],
                &["audio/rmf", "audio/vnd.qcelp", "audio/x-gsm", "audio/x-midi", "audio/x-mod", "audio/x-pn-aiff", "audio-x-rmf", "sound/aiff"],
                &[b"FORM"]
                // &[b"ID3 "]

            ),
        ]
    }

    fn score(_context: &[u8]) -> u8 {
        0
    }
}

impl FormatReader for AiffReader {
    fn try_new(mut source: MediaSourceStream, _options: &FormatOptions) -> Result<Self> {
        // let form_reader_buf = [0u8; 1024];
        // let mut form_reader = BufReader::new(&form_reader_buf);

        let marker = source.read_quad_bytes()?;
        // let marker = form_reader.read_quad_bytes()?;

        if marker != AIFF_DATA_MARKER {
            error!("invalid aiff marker ({})", String::from_utf8_lossy(&marker));

            return unsupported_error("aiff: missing form marker");
        }

        let form_len = util::read_i32_be(&mut source);
        // let form_len = util::read_i32_be(&mut form_reader);
        debug!("aiff: form chunk bytes: {}", form_len);
        
        // let mut chunk_reader = ScopedStream::new(source, form_len.try_into().unwrap());

        let form_type = source.read_boxed_slice_exact(4).unwrap();
        // let form_type = form_reader.read_boxed_slice_exact(4).unwrap();
        match form_type[..].try_into().unwrap() {
            ids::AIFF => (),
            ids::AIFF_C => return unsupported_error("aiff: aiff-c is not supported"),
            _ => return decode_error("aiff: unsupported form type"),
        }

        // let chunk_reader_buf = BufReader::new(inner)
        // let mut chunk_reader = ScopedStream::new(source, form_len.try_into().unwrap());
        let mut aiff_chunks = ChunksReader::<AiffChunks>::new(form_len as u32);

        let mut codec_params = CodecParameters::new();
        let mut metadata: MetadataLog = Default::default();
        
        let mut frame_len = 0;

        let mut data_start_pos = 0;
        let mut data_end_pos = 0;

        loop {
            let chunk = aiff_chunks.next(&mut source)?;
            // let chunk = aiff_chunks.next(&mut form_reader)?;
            // let chunk = aiff_chunks.next(&mut chunk_reader)?;

            // if chunk.is_none() {
            //     break;
            //     // return unsupported_error("aiff: missing data chunk");
            // }

            match chunk.unwrap() {
                AiffChunks::Common(parser) => {
                    let common = parser.parse(&mut source)?;

                    append_common_params(&mut codec_params, &common);
                    // let common = parser.parse(&mut chunk_reader)?;

                    frame_len = common.num_sample_frames;
                },

                AiffChunks::Name(parser) => {
                    let name = parser.parse(&mut source)?;
                }
                AiffChunks::Sound(parser) => {
                    let sound = parser.parse(&mut source)?;

                    data_start_pos = sound.data_range.start_pos;
                    data_end_pos = sound.data_range.end_pos;

                    // source.seek(SeekFrom::Current(data_end_pos as i64)).unwrap();
                    source.seek(SeekFrom::Start(data_start_pos as u64)).unwrap();

                    return Ok(AiffReader {
                        reader: source, 
                        tracks: vec![Track::new(0, codec_params)], 
                        cues: Vec::new(), 
                        // metadata: (), 
                        frame_len: 4, // TODO: fix me
                        data_start_pos: sound.data_range.start_pos, 
                        data_end_pos: sound.data_range.end_pos 
                    })
                    // Record the bounds of the data chunk.
                    // let data_start_pos = source.pos();
                    // let data_end_pos = data_start_pos + u64::from(sound..len);

                    // // Append Data chunk fields to codec parameters.
                    // append_data_params(&mut codec_params, &data, frame_len);
                },
                AiffChunks::ID3(parser) => {
                    let id3 = parser.parse(&mut source)?;
                },
            }
        }

        // source.seek(SeekFrom::Start(data_start_pos)).unwrap();

        // return Ok(AiffReader {
        //     reader: source, 
        //     tracks: vec![Track::new(0, codec_params)], 
        //     cues: Vec::new(), 
        //     // metadata: (), 
        //     frame_len, 
        //     data_start_pos,
        //     data_end_pos
        // })

        // Ok(Self {
        //     reader: todo!(),
        //     tracks: todo!(),
        //     cues: todo!(),
        //     metadata: todo!(),
        //     frame_len: todo!(),
        //     data_start_pos: todo!(),
        //     data_end_pos: todo!(),
        // })
    }

    fn cues(&self) -> &[Cue] {
        &self.cues
    }

    fn metadata(&mut self) -> symphonia_core::meta::Metadata<'_> {
        todo!()
    }

    fn seek(&mut self, mode: SeekMode, to: SeekTo) -> symphonia_core::errors::Result<SeekedTo> {
        if self.tracks.is_empty() || self.frame_len == 0 {
            return seek_error(SeekErrorKind::Unseekable);
        }

        let params = &self.tracks[0].codec_params;

        let ts = match to {
            SeekTo::TimeStamp { ts, .. } => ts,

            SeekTo::Time { time, .. } => {
                if let Some(sample_rate) = params.sample_rate {
                    TimeBase::new(1, sample_rate).calc_timestamp(time)
                } else {
                    return seek_error(SeekErrorKind::Unseekable)
                }
            }
        };

        if let Some(n_frames) = params.n_frames {
            if ts > n_frames {
                return seek_error(SeekErrorKind::OutOfRange);
            }
        }

        debug!("seeking to frame_ts={}", ts);
        let seek_pos = self.data_start_pos + (ts * u64::from(self.frame_len));

        if self.reader.is_seekable() {
            self.reader.seek(SeekFrom::Start(seek_pos))?;
        } else {
            let current_pos = self.reader.pos();
            if seek_pos >= current_pos {
                self.reader.ignore_bytes(seek_pos - current_pos)?;
            } else {
                return seek_error(SeekErrorKind::ForwardOnly);
            }
        }

        // debug!("seeked to packet_ts={} (delta={})", actual_ts, actual_ts as i64 - ts as i64);
        debug!("seeked to packet_ts={}", ts);

        Ok(SeekedTo { track_id: 0, actual_ts: ts, required_ts: ts })
    }

    fn tracks(&self) -> &[Track] {
        &self.tracks
    }

    fn next_packet(&mut self) -> symphonia_core::errors::Result<Packet> {
        let pos = self.reader.pos();

        let num_frames_left = if pos < self.data_end_pos {
            (self.data_end_pos - pos) / u64::from(self.frame_len)
        } else {
            0
        };

        if num_frames_left == 0 {
            return end_of_stream_error();
        }

        let dur = num_frames_left.min(AIFF_MAX_FRAMES_PER_PACKET);

        let packet_len = dur * u64::from(self.frame_len);
        let packet_buf = self.reader.read_boxed_slice(packet_len as usize)?;

        let pts = (pos - self.data_start_pos) / u64::from(self.frame_len);

        Ok(Packet::new_from_boxed_slice(0, pts, dur, packet_buf))
    }

    fn into_inner(self: Box<Self>) -> MediaSourceStream {
        todo!()
    }
}

fn append_common_params(codec_params: &mut CodecParameters, common: &CommonChunk) {
    codec_params
        .with_max_frames_per_packet(AIFF_MAX_FRAMES_PER_PACKET)
        .with_sample_rate(common.sample_rate as u32)
        .with_time_base(TimeBase::new(1, common.sample_rate as u32));

    match common.format_data {
        AiffFormatData::Pcm(ref pcm) => {
            codec_params
                .for_codec(pcm.codec)
                .with_bits_per_coded_sample(pcm.bits_per_sample)
                .with_channels(pcm.channels);
        }
    }

    if common.num_sample_frames > 0 {
        codec_params.with_n_frames(u64::from(common.num_sample_frames));
    }
}
