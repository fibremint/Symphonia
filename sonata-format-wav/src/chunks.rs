// Sonata
// Copyright (c) 2019 The Sonata Project Developers.
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::fmt;
use std::marker::PhantomData;

use sonata_core::audio::Channels;
use sonata_core::codecs::{
    CODEC_TYPE_PCM_U8, 
    CODEC_TYPE_PCM_S16LE, 
    CODEC_TYPE_PCM_S24LE, 
    CODEC_TYPE_PCM_S32LE, 
    CODEC_TYPE_PCM_F32LE, 
    CODEC_TYPE_PCM_F64LE,
    CODEC_TYPE_PCM_ALAW,
    CODEC_TYPE_PCM_MULAW,
};
use sonata_core::codecs::CodecType;
use sonata_core::errors::{Result, decode_error, unsupported_error};
use sonata_core::io::ByteStream;
use sonata_core::meta::Tag;
use sonata_metadata::riff;

use log::info;

/// `ParseChunkTag` implements `parse_tag` to map between the 4-byte chunk identifier and the enumeration 
pub trait ParseChunkTag : Sized {
    fn parse_tag(tag: [u8; 4], len: u32) -> Option<Self>;
}

enum NullChunks {}

impl ParseChunkTag for NullChunks {
    fn parse_tag(_tag: [u8; 4], _len: u32) -> Option<Self> { None }
}

/// `ChunksReader` reads chunks from a `ByteStream`. It is generic across a type, usually an enum, implementing the 
/// `ParseChunkTag` trait. When a new chunk is encountered in the stream, `parse_tag` on T is called to return an 
/// object capable of parsing/reading that chunk or `None`. This makes reading the actual chunk data lazy in that the 
/// chunk is not read until the object is consumed.
pub struct ChunksReader<T: ParseChunkTag> {
    len: u32,
    consumed: u32,
    phantom: PhantomData<T>,
}

impl<T: ParseChunkTag> ChunksReader<T> {
    pub fn new(len: u32) -> Self {
        ChunksReader { 
            len, 
            consumed: 0, 
            phantom: PhantomData
        }
    }

    pub fn next<B: ByteStream>(&mut self, reader: &mut B) -> Result<Option<T>> {
        // Loop until a chunk is recognized and returned, or the end of stream is reached.
        loop {
            // Align to the next 2-byte boundary if not currently aligned..
            if self.consumed & 0x1 == 1 {
                reader.read_u8()?;
                self.consumed += 1;
            }

            // Check if there are enough bytes for another chunk, if not, there are no more chunks.
            if self.consumed + 8 > self.len {
                return Ok(None);
            }

            // Read tag and len, the chunk header.
            let tag = reader.read_quad_bytes()?;
            let len = reader.read_u32()?;
            self.consumed += 8;

            // Check if the ChunkReader has enough unread bytes to fully read the chunk. Warning: the formulation of 
            // this conditional is critical because len is untrusted input, it may overflow when if added to anything.
            if self.len - self.consumed < len {
                return decode_error("Info chunk length exceeds parent List chunk length.");
            }

            // The length of the chunk has been validated, so "consume" the chunk.
            self.consumed += len;

            match T::parse_tag(tag, len) {
                Some(chunk) => return Ok(Some(chunk)),
                None => {
                    // As per the RIFF spec, unknown chunks are to be ignored.
                    info!("Ignoring unknown chunk: tag={}, len={}.", String::from_utf8_lossy(&tag), len);
                    reader.ignore_bytes(u64::from(len))?
                }
            }
        }
    }

    pub fn finish<B: ByteStream>(&mut self, reader: &mut B) -> Result<()>{
        // If data is remaining in this chunk, skip it.
        if self.consumed < self.len {
            let remaining = self.len - self.consumed;
            reader.ignore_bytes(u64::from(remaining))?;
            self.consumed += remaining;
        }

        // Pad the chunk to the next 2-byte boundary.
        if self.len & 0x1 == 1 {
            reader.read_u8()?;
        }

        Ok(())
    }
}

/// Common trait implemented for all chunks that are parsed by a `ChunkParser`.
pub trait ParseChunk : Sized {
    fn parse<B: ByteStream>(reader: &mut B, tag: [u8; 4], len: u32) -> Result<Self>;
}

/// `ChunkParser` is a utility struct for unifying the parsing of chunks.
pub struct ChunkParser<P: ParseChunk> {
    tag: [u8; 4],
    len: u32,
    phantom: PhantomData<P>,
}

impl<P: ParseChunk> ChunkParser<P> {
    fn new(tag: [u8; 4], len: u32) -> Self {
        ChunkParser {
            tag,
            len,
            phantom: PhantomData,
        }
    }

    pub fn parse<B: ByteStream>(&self, reader: &mut B) -> Result<P> {
        P::parse(reader, self.tag, self.len)
    }
}

fn map_wave_channels(channel_mask: u32) -> Channels {
    const SPEAKER_FRONT_LEFT: u32            = 0x1;
    const SPEAKER_FRONT_RIGHT: u32           = 0x2;
    const SPEAKER_FRONT_CENTER: u32          = 0x4;
    const SPEAKER_LOW_FREQUENCY: u32         = 0x8;
    const SPEAKER_BACK_LEFT: u32             = 0x10;
    const SPEAKER_BACK_RIGHT: u32            = 0x20;
    const SPEAKER_FRONT_LEFT_OF_CENTER: u32  = 0x40;
    const SPEAKER_FRONT_RIGHT_OF_CENTER: u32 = 0x80;
    const SPEAKER_BACK_CENTER: u32           = 0x100;
    const SPEAKER_SIDE_LEFT: u32             = 0x200;
    const SPEAKER_SIDE_RIGHT: u32            = 0x400;
    const SPEAKER_TOP_CENTER: u32            = 0x800;
    const SPEAKER_TOP_FRONT_LEFT: u32        = 0x1000;
    const SPEAKER_TOP_FRONT_CENTER: u32      = 0x2000;
    const SPEAKER_TOP_FRONT_RIGHT: u32       = 0x4000;
    const SPEAKER_TOP_BACK_LEFT: u32         = 0x8000;
    const SPEAKER_TOP_BACK_CENTER: u32       = 0x10000;
    const SPEAKER_TOP_BACK_RIGHT: u32        = 0x20000;

    let mut channels = Channels::empty();

    if channel_mask & SPEAKER_FRONT_LEFT            != 0 { channels |= Channels::FRONT_LEFT;         }
    if channel_mask & SPEAKER_FRONT_RIGHT           != 0 { channels |= Channels::FRONT_RIGHT;        }
    if channel_mask & SPEAKER_FRONT_CENTER          != 0 { channels |= Channels::FRONT_CENTRE;       }
    if channel_mask & SPEAKER_LOW_FREQUENCY         != 0 { channels |= Channels::LFE1;               }
    if channel_mask & SPEAKER_BACK_LEFT             != 0 { channels |= Channels::REAR_LEFT;          }
    if channel_mask & SPEAKER_BACK_RIGHT            != 0 { channels |= Channels::REAR_RIGHT;         }
    if channel_mask & SPEAKER_FRONT_LEFT_OF_CENTER  != 0 { channels |= Channels::FRONT_LEFT_CENTRE;  }
    if channel_mask & SPEAKER_FRONT_RIGHT_OF_CENTER != 0 { channels |= Channels::FRONT_RIGHT_CENTRE; }
    if channel_mask & SPEAKER_BACK_CENTER           != 0 { channels |= Channels::REAR_CENTRE;        }
    if channel_mask & SPEAKER_SIDE_LEFT             != 0 { channels |= Channels::SIDE_LEFT;          }
    if channel_mask & SPEAKER_SIDE_RIGHT            != 0 { channels |= Channels::SIDE_RIGHT;         }
    if channel_mask & SPEAKER_TOP_CENTER            != 0 { channels |= Channels::TOP_CENTRE;         }
    if channel_mask & SPEAKER_TOP_FRONT_LEFT        != 0 { channels |= Channels::TOP_FRONT_LEFT;     }
    if channel_mask & SPEAKER_TOP_FRONT_CENTER      != 0 { channels |= Channels::TOP_FRONT_CENTRE;   }
    if channel_mask & SPEAKER_TOP_FRONT_RIGHT       != 0 { channels |= Channels::TOP_FRONT_RIGHT;    }
    if channel_mask & SPEAKER_TOP_BACK_LEFT         != 0 { channels |= Channels::TOP_REAR_LEFT;      }
    if channel_mask & SPEAKER_TOP_BACK_CENTER       != 0 { channels |= Channels::TOP_REAR_CENTRE;    }
    if channel_mask & SPEAKER_TOP_BACK_RIGHT        != 0 { channels |= Channels::TOP_REAR_RIGHT;     }

    channels
}

pub enum WaveFormatData {
    Pcm(WaveFormatPcm),
    IeeeFloat(WaveFormatIeeeFloat),
    Extensible(WaveFormatExtensible),
    ALaw(WaveFormatALaw),
    MuLaw(WaveFormatMuLaw),
}

pub struct WaveFormatPcm {
    /// The number of bits per sample. In the PCM format, this is always a multiple of 8-bits.
    pub bits_per_sample: u16,
    /// Channel bitmask.
    pub channels: Channels,
    /// Codec type.
    pub codec: CodecType,
}

pub struct WaveFormatIeeeFloat {
    /// Channel bitmask.
    pub channels: Channels,
    /// Codec type.
    pub codec: CodecType,
}

pub struct WaveFormatExtensible {
    /// The number of bits per sample as stored in the stream. This value is always a multiple of 8-bits.
    pub bits_per_sample: u16,
    /// The number of bits per sample that are valid. This number is always less than the number of bits per sample.
    pub bits_per_coded_sample: u16,
    /// Channel bitmask.
    pub channels: Channels,
    /// Globally unique identifier of the format.
    pub sub_format_guid: [u8; 16],
    /// Codec type.
    pub codec: CodecType,
}

pub struct WaveFormatALaw {
    /// Channel bitmask.
    pub channels: Channels,
    /// Codec type.
    pub codec: CodecType,
}

pub struct WaveFormatMuLaw {
    /// Channel bitmask.
    pub channels: Channels,
    /// Codec type.
    pub codec: CodecType,
}

pub struct WaveFormatChunk {
    /// The number of channels.
    pub n_channels: u16,
    /// The sample rate in Hz. For non-PCM formats, this value must be interpreted as per the format's specifications.
    pub sample_rate: u32,
    /// The required average data rate required in bytes/second. For non-PCM formats, this value must be interpreted as 
    /// per the format's specifications.
    pub avg_bytes_per_sec: u32,
    /// The byte alignment of one audio frame. For PCM formats, this is equal to 
    /// `(n_channels * format_data.bits_per_sample) / 8`. For non-PCM formats, this value must be interpreted as per 
    /// the format's specifications.
    pub block_align: u16,
    /// Extra data associated with the format block conditional upon the format tag.
    pub format_data: WaveFormatData,
}

impl WaveFormatChunk {

    fn read_pcm_fmt<B: ByteStream>(reader: &mut B, bits_per_sample: u16, n_channels: u16, len: u32) -> Result<WaveFormatData> {
        // WaveFormat for a PCM format /may/ be extended with an extra data length parameter followed by the 
        // extra data itself. Use the chunk length to determine if the format chunk is extended.
        let is_extended = match len {
            // Minimal WavFormat struct, no extension.
            16 => false,
            // WaveFormatEx with exta data length field present, but not extra data.
            18 => true,
            // WaveFormatEx with extra data length field and extra data.
            40 => true,
            _ => return decode_error("Malformed fmt_pcm chunk."),
        };

        // If there is extra data, read the length, and discard the extra data.
        if is_extended {
            let extra_size = reader.read_u16()?; 

            if extra_size > 0 {
                reader.ignore_bytes(u64::from(extra_size))?;
            }
        }

        // Bits per sample for PCM is both the encoded sample width, and the actual sample width. Strictly, this must 
        // either be 8 or 16 bits, but there is no reason why 24 and 32 bits can't be supported. Since these files do 
        // exist, allow for 8/16/24/32-bit samples, but error if not a multiple of 8 or greater than 32-bits.
        //
        // Select the appropriate codec using bits per sample. Samples are always interleaved and little-endian 
        // encoded for the PCM format.
        let codec = match bits_per_sample {
            8  => CODEC_TYPE_PCM_U8,
            16 => CODEC_TYPE_PCM_S16LE,
            24 => CODEC_TYPE_PCM_S24LE,
            32 => CODEC_TYPE_PCM_S32LE,
            _  => return decode_error("Bits per sample for fmt_pcm must be 8, 16, 24 or 32 bits.")
        };

        // The PCM format only supports 1 or 2 channels, for mono and stereo channel layouts, respectively.
        let channels = match n_channels {
            1 => Channels::FRONT_LEFT,
            2 => Channels::FRONT_LEFT | Channels::FRONT_RIGHT,
            _ => return decode_error("Only mono and stereo channel layouts are supported for fmt_pcm.")
        };

        Ok(WaveFormatData::Pcm(WaveFormatPcm { bits_per_sample, channels, codec }))
    }

    fn read_ieee_fmt<B: ByteStream>(reader: &mut B, bits_per_sample: u16, n_channels: u16, len: u32) -> Result<WaveFormatData> {
        // WaveFormat for a IEEE format should not be extended, but it may still have an extra data length 
        // parameter.
        if len == 18 {
            let extra_size = reader.read_u16()?; 
            if extra_size != 0 {
                return decode_error("Extra data not expected for fmt_ieee chunk.");
            }
        }
        else if len > 16 {
            return decode_error("Malformed fmt_ieee chunk.");
        }

        // Officially, only 32-bit floats are supported, but Sonata can handle 64-bit floats.
        //
        // Select the appropriate codec using bits per sample. Samples are always interleaved and little-endian 
        // encoded for the IEEE Float format.
        let codec = match bits_per_sample {
            32 => CODEC_TYPE_PCM_F32LE,
            64 => CODEC_TYPE_PCM_F64LE,
            _  => return decode_error("Bits per sample for fmt_ieee must be 32 or 64 bits.")
        };

        // The IEEE format only supports 1 or 2 channels, for mono and stereo channel layouts, respectively.
        let channels = match n_channels {
            1 => Channels::FRONT_LEFT,
            2 => Channels::FRONT_LEFT | Channels::FRONT_RIGHT,
            _ => return decode_error("Only mono and stereo channel layouts are supported for fmt_ieee.")
        };

        Ok(WaveFormatData::IeeeFloat(WaveFormatIeeeFloat { channels, codec }))
    }

    fn read_ext_fmt<B: ByteStream>(reader: &mut B, bits_per_coded_sample: u16, len: u32) -> Result<WaveFormatData> {
        // WaveFormat for the extensible format must be extended to 40 bytes in length.
        if len < 40 {
            return decode_error("Malformed fmt_ext chunk.");
        }

        let extra_size = reader.read_u16()?; 

        // The size of the extra data for the Extensible format is exactly 22 bytes.
        if extra_size != 22 {
            return decode_error("Extra data size not 22 bytes for fmt_ext chunk.");
        }

        let bits_per_sample = reader.read_u16()?;

        // Bits per coded sample for extensible formats is the width per sample as stored in the stream. This must be a 
        // multiple of 8.
        if (bits_per_coded_sample & 0x7) != 0 {
            return decode_error("Bits per coded sample for fmt_ext must be a multiple of 8 bits.");
        }

        // Bits per sample indicates the number of valid bits in the encoded sample. The sample is encoded in a bits per 
        // coded sample width value, therefore the valid number of bits must be at most bits per coded sample long.
        if bits_per_sample > bits_per_coded_sample {
            return decode_error("Bits per sample must be <= bits per coded sample for fmt_ext.");
        }

        let channel_mask = reader.read_u32()?;

        let mut sub_format_guid = [0u8; 16];
        reader.read_buf_exact(&mut sub_format_guid)?;

        // These GUIDs identifiy the format of the data chunks. These definitions can be found in ksmedia.h of the 
        // Microsoft Windows Platform SDK.
        const KSDATAFORMAT_SUBTYPE_PCM: [u8; 16] = 
            [0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71];
        // const KSDATAFORMAT_SUBTYPE_ADPCM: [u8; 16] = 
        //  [0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71];
        const KSDATAFORMAT_SUBTYPE_IEEE_FLOAT: [u8; 16] = 
            [0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71];
        const KSDATAFORMAT_SUBTYPE_ALAW: [u8; 16] = 
            [0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71];
        const KSDATAFORMAT_SUBTYPE_MULAW: [u8; 16] = 
            [0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71];

        // Verify support based on the format GUID.
        let codec = match sub_format_guid {
            KSDATAFORMAT_SUBTYPE_PCM => {
                // Only support up-to 32-bit integer samples.
                if bits_per_coded_sample > 32 {
                    return decode_error("Bits per sample for fmt_ext PCM sub-type must be <= 32 bits.");
                }

                // Use bits per coded sample to select the codec to use. If bits per sample is less than the bits per 
                // coded sample, the codec will expand the sample during decode.
                match bits_per_coded_sample {
                    8  => CODEC_TYPE_PCM_U8,
                    16 => CODEC_TYPE_PCM_S16LE,
                    24 => CODEC_TYPE_PCM_S24LE,
                    32 => CODEC_TYPE_PCM_S32LE,
                    _ => unreachable!()
                }
            },
            KSDATAFORMAT_SUBTYPE_IEEE_FLOAT => {
                // IEEE floating formats do not support truncated sample widths.
                if bits_per_sample != bits_per_coded_sample {
                    return decode_error("Bits per sample for fmt_ext IEEE sub-type must equal bits per coded sample.");
                }

                // Select the appropriate codec based on the bits per coded sample.
                match bits_per_coded_sample {
                    32 => CODEC_TYPE_PCM_F32LE,
                    64 => CODEC_TYPE_PCM_F64LE,
                    _ =>  return decode_error("Bits per sample for fmt_ext IEEE sub-type must be 32 or 64 bits.")
                }
            },
            KSDATAFORMAT_SUBTYPE_ALAW => CODEC_TYPE_PCM_ALAW,
            KSDATAFORMAT_SUBTYPE_MULAW => CODEC_TYPE_PCM_MULAW,
            _ => return unsupported_error("Unsupported fmt_ext sub-type."),
        };

        let channels = map_wave_channels(channel_mask);

        Ok(WaveFormatData::Extensible(WaveFormatExtensible { 
            bits_per_sample, bits_per_coded_sample, channels, sub_format_guid, codec }))
    }

    fn read_alaw_pcm_fmt<B: ByteStream>(reader: &mut B, n_channels: u16, len: u32) -> Result<WaveFormatData> {
        if len != 18 {
            return decode_error("Malformed fmt_alaw chunk.");
        }

        let extra_size = reader.read_u16()?; 

        if extra_size > 0 {
            reader.ignore_bytes(u64::from(extra_size))?;
        }

        let channels = match n_channels {
            1 => Channels::FRONT_LEFT,
            2 => Channels::FRONT_LEFT | Channels::FRONT_RIGHT,
            _ => return decode_error("Only mono and stereo channel layouts are supported for fmt_alaw.")
        };

        Ok(WaveFormatData::ALaw(WaveFormatALaw{ codec: CODEC_TYPE_PCM_ALAW, channels }))

    }

    fn read_mulaw_pcm_fmt<B: ByteStream>(reader: &mut B, n_channels: u16, len: u32) -> Result<WaveFormatData> {
        if len != 18 {
            return decode_error("Malformed fmt_mulaw chunk.");
        }

        let extra_size = reader.read_u16()?; 

        if extra_size > 0 {
            reader.ignore_bytes(u64::from(extra_size))?;
        }

        let channels = match n_channels {
            1 => Channels::FRONT_LEFT,
            2 => Channels::FRONT_LEFT | Channels::FRONT_RIGHT,
            _ => return decode_error("Only mono and stereo channel layouts are supported for fmt_mulaw.")
        };

        Ok(WaveFormatData::MuLaw(WaveFormatMuLaw{ codec: CODEC_TYPE_PCM_MULAW, channels }))
    }
}

impl ParseChunk for WaveFormatChunk {
    fn parse<B: ByteStream>(reader: &mut B, _tag: [u8; 4], len: u32) -> Result<WaveFormatChunk> {
        // WaveFormat has a minimal length of 16 bytes. This may be extended with format specific data later.
        if len < 16 {
            return decode_error("Malformed fmt chunk.");
        }

        let format = reader.read_u16()?;
        let n_channels = reader.read_u16()?;
        let sample_rate = reader.read_u32()?;
        let avg_bytes_per_sec = reader.read_u32()?;
        let block_align = reader.read_u16()?;
        let bits_per_sample = reader.read_u16()?;

        // The definition of these format identifiers can be found in mmreg.h of the Microsoft Windows Platform SDK.
        const WAVE_FORMAT_PCM: u16        = 0x0001;
        // const WAVE_FORMAT_ADPCM: u16      = 0x0002;
        const WAVE_FORMAT_IEEE_FLOAT: u16 = 0x0003;
        const WAVE_FORMAT_ALAW: u16       = 0x0006;
        const WAVE_FORMAT_MULAW: u16      = 0x0007;
        const WAVE_FORMAT_EXTENSIBLE: u16 = 0xfffe;

        let format_data = match format {
            // The PCM Wave Format
            WAVE_FORMAT_PCM => Self::read_pcm_fmt(reader, bits_per_sample, n_channels, len)?,
            // The IEEE Float Wave Format
            WAVE_FORMAT_IEEE_FLOAT => Self::read_ieee_fmt(reader, bits_per_sample, n_channels, len)?,
            // The Extensible Wave Format
            WAVE_FORMAT_EXTENSIBLE => Self::read_ext_fmt(reader, bits_per_sample, len)?,
            // The Alaw Wave Format.
            WAVE_FORMAT_ALAW => Self::read_alaw_pcm_fmt(reader, n_channels, len)?,
            // The MuLaw Wave Format.
            WAVE_FORMAT_MULAW => Self::read_mulaw_pcm_fmt(reader, n_channels, len)?,
            // Unsupported format.
            _ => return unsupported_error("Unsupported wave format."),
        };

        Ok(WaveFormatChunk { n_channels, sample_rate, avg_bytes_per_sec, block_align, format_data })
    }
}

impl fmt::Display for WaveFormatChunk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "WaveFormatChunk {{")?;
        writeln!(f, "\tn_channels: {},", self.n_channels)?;
        writeln!(f, "\tsample_rate: {} Hz,", self.sample_rate)?;
        writeln!(f, "\tavg_bytes_per_sec: {},", self.avg_bytes_per_sec)?;
        writeln!(f, "\tblock_align: {},", self.block_align)?;

        match self.format_data {
            WaveFormatData::Pcm(ref pcm) => {
                writeln!(f, "\tformat_data: Pcm {{")?;
                writeln!(f, "\t\tbits_per_sample: {},", pcm.bits_per_sample)?;
                writeln!(f, "\t\tchannels: {},", pcm.channels)?;
                writeln!(f, "\t\tcodec: {},", pcm.codec)?;
            },
            WaveFormatData::IeeeFloat(ref ieee) => {
                writeln!(f, "\tformat_data: IeeeFloat {{")?;
                writeln!(f, "\t\tchannels: {},", ieee.channels)?;
                writeln!(f, "\t\tcodec: {},", ieee.codec)?;
            },
            WaveFormatData::Extensible(ref ext) => {
                writeln!(f, "\tformat_data: Extensible {{")?;
                writeln!(f, "\t\tbits_per_sample: {},", ext.bits_per_sample)?;
                writeln!(f, "\t\tbits_per_coded_sample: {},", ext.bits_per_coded_sample)?;
                writeln!(f, "\t\tchannels: {},", ext.channels)?;
                writeln!(f, "\t\tsub_format_guid: {:?},", &ext.sub_format_guid)?;
                writeln!(f, "\t\tcodec: {},", ext.codec)?;
            },
            WaveFormatData::ALaw(ref alaw) => {
                writeln!(f, "\tformat_data: ALaw {{")?;
                writeln!(f, "\t\tchannels: {},", alaw.channels)?;
                writeln!(f, "\t\tcodec: {},", alaw.codec)?;
            },
            WaveFormatData::MuLaw(ref mulaw) => {
                writeln!(f, "\tformat_data: MuLaw {{")?;
                writeln!(f, "\t\tchannels: {},", mulaw.channels)?;
                writeln!(f, "\t\tcodec: {},", mulaw.codec)?;
            }
        };

        writeln!(f, "\t}}")?;
        writeln!(f, "}}")
    }
}

pub struct FactChunk {
    pub n_frames: u32,
}

impl ParseChunk for FactChunk {
    fn parse<B: ByteStream>(reader: &mut B, _tag: [u8; 4], len: u32) -> Result<Self> {
        // A Fact chunk is exactly 4 bytes long, though there is some mystery as to whether there can be more fields
        // in the chunk.
        if len != 4 {
            return decode_error("Malformed fact chunk.");
        }

        Ok(FactChunk{ n_frames: reader.read_u32()? })
    }
}

impl fmt::Display for FactChunk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "FactChunk {{")?;
        writeln!(f, "\tn_frames: {},", self.n_frames)?;
        writeln!(f, "}}")
    }
}

pub struct ListChunk {
    pub form: [u8; 4],
    pub len: u32, 
}

impl ListChunk {
    pub fn skip<B: ByteStream>(&self, reader: &mut B) -> Result<()> {
        ChunksReader::<NullChunks>::new(self.len).finish(reader)
    }
}

impl ParseChunk for ListChunk {
    fn parse<B: ByteStream>(reader: &mut B, _tag: [u8; 4], len: u32) -> Result<Self> {
        // A List chunk must contain atleast the list/form identifier. However, an empty list (len = 4) is permissible.
        if len < 4 {
            return decode_error("Malformed list chunk.");
        }

        Ok(ListChunk{ 
            form: reader.read_quad_bytes()?,
            len: len - 4
        })
    }
}

impl fmt::Display for ListChunk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "ListChunk {{")?;
        writeln!(f, "\tform: {},", String::from_utf8_lossy(&self.form))?;
        writeln!(f, "\tlen: {},", self.len)?;
        writeln!(f, "}}")
    }
}

pub struct InfoChunk {
    pub tag: Tag,
}

impl ParseChunk for InfoChunk {
    fn parse<B: ByteStream>(reader: &mut B, tag: [u8; 4], len: u32) -> Result<InfoChunk> {
        // TODO: Apply limit.
        let mut value_buf = vec![0u8; len as usize];
        reader.read_buf_exact(&mut value_buf)?;

        Ok(InfoChunk {
            tag: riff::parse(tag, &value_buf)
        })
    }
}

pub struct DataChunk {
    pub len: u32,
}

impl ParseChunk for DataChunk {
    fn parse<B: ByteStream>(_: &mut B, _: [u8; 4], len: u32) -> Result<DataChunk> {
        Ok(DataChunk { len })
    }
}

pub enum RiffWaveChunks {
    Format(ChunkParser<WaveFormatChunk>),
    List(ChunkParser<ListChunk>),
    Fact(ChunkParser<FactChunk>),
    Data(ChunkParser<DataChunk>),
}

macro_rules! parser {
    ($class:expr, $result:ty, $tag:expr, $len:expr) => {
        Some($class(ChunkParser::<$result>::new($tag, $len)))
    };
}

impl ParseChunkTag for RiffWaveChunks {
    fn parse_tag(tag: [u8; 4], len: u32) -> Option<Self> {
        match &tag {
            b"fmt " => parser!(RiffWaveChunks::Format, WaveFormatChunk, tag, len),
            b"LIST" => parser!(RiffWaveChunks::List, ListChunk, tag, len),
            b"fact" => parser!(RiffWaveChunks::Fact, FactChunk, tag, len),
            b"data" => parser!(RiffWaveChunks::Data, DataChunk, tag, len),
            _ => None,
        }
    }
}

pub enum RiffInfoListChunks {
    Info(ChunkParser<InfoChunk>),
}

impl ParseChunkTag for RiffInfoListChunks {
    fn parse_tag(tag: [u8; 4], len: u32) -> Option<Self> {
        // Right now it is assumed all list chunks are INFO chunks, but that's not really guaranteed.
        // TODO: Actually validate that the chunk is an info chunk.
        parser!(RiffInfoListChunks::Info, InfoChunk, tag, len)
    }
}