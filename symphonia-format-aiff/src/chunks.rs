use std::convert::TryInto;
use std::io::{Read, Seek, SeekFrom};
use std::marker::PhantomData;

use symphonia_core::audio::Channels;
use symphonia_core::codecs::CodecType;
use symphonia_core::codecs::{
    CODEC_TYPE_PCM_ALAW, CODEC_TYPE_PCM_MULAW, CODEC_TYPE_PCM_F32BE, CODEC_TYPE_PCM_F64BE,
    CODEC_TYPE_PCM_S16BE, CODEC_TYPE_PCM_S24BE, CODEC_TYPE_PCM_S32BE, CODEC_TYPE_PCM_U8
};
use symphonia_core::errors::{decode_error, unsupported_error, Result};
use symphonia_core::io::{ReadBytes, SeekBuffered,  };
use symphonia_core::conv;
use symphonia_core::meta::Tag;
// TODO support id3v1?
use symphonia_metadata::id3v2;


use log::{info, debug};

use super::{
    ids,
    extended::parse_extended_precision_bytes,
    util
};

/// `ParseChunkTag` implements `parse_tag` to map between the 4-byte chunk identifier and the
/// enumeration
pub trait ParseChunkTag: Sized {
    fn parse_tag(tag: [u8; 4], len: u32) -> Option<Self>;
}

enum NullChunks {}

impl ParseChunkTag for NullChunks {
    fn parse_tag(_tag: [u8; 4], _len: u32) -> Option<Self> {
        None
    }
}

macro_rules! parser {
    ($class:expr, $result:ty, $tag:expr, $len:expr) => {
        Some($class(ChunkParser::<$result>::new($tag, $len)))
    };
}

pub struct ChunksReader<T: ParseChunkTag> {
    len: u32,
    consumed: u32,
    phantom: PhantomData<T>,
}

impl<T: ParseChunkTag> ChunksReader<T> {
    pub fn new(len: u32) -> Self {
        ChunksReader { len, consumed: 0, phantom: PhantomData }
    }

    pub fn next<B: ReadBytes>(&mut self, reader: &mut B) -> Result<Option<T>> {
        // Loop until a chunk is recognized and returned, or the end of stream is reached.
        loop {
            // Align to the next 2-byte boundary if not currently aligned.
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
            // let len = reader.read_u32()?;
            let len = util::read_i32_be(reader) as u32;
            // self.consumed += 8;

            // Check if the ChunkReader has enough unread bytes to fully read the chunk.
            //
            // Warning: the formulation of this conditional is critical because len is untrusted
            // input, it may overflow when if added to anything.
            if self.len - self.consumed < len {
                // When ffmpeg encodes wave to stdout the riff (parent) and data chunk lengths are
                // (2^32)-1 since the size can't be known ahead of time.
                if !(self.len == len && len == u32::MAX) {
                    return decode_error("wav: chunk length exceeds parent (list) chunk length");
                }
            }

            // The length of the chunk has been validated, so "consume" the chunk.
            self.consumed = self.consumed.saturating_add(len);

            match T::parse_tag(tag, len) {
                Some(chunk) => return Ok(Some(chunk)),
                None => {
                    return Ok(None);
                    // // As per the RIFF spec, unknown chunks are to be ignored.
                    // info!(
                    //     "ignoring unknown chunk: tag={}, len={}.",
                    //     String::from_utf8_lossy(&tag),
                    //     len
                    // );

                    // reader.ignore_bytes(u64::from(len))?
                }
            }
        }
    }

    // pub fn seek_to(&mut self, position: u32) {
    //     self.consumed += position;
    // }

    pub fn finish<B: ReadBytes>(&mut self, reader: &mut B) -> Result<()> {
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
pub trait ParseChunk: Sized {
    fn parse<B: ReadBytes + Seek>(reader: &mut B, tag: [u8; 4], len: u32) -> Result<Self>;
}

pub trait Parse: Sized {
    fn parse<B: ReadBytes + Seek>(reader: &mut B) -> Result<Self>;
}


/// `ChunkParser` is a utility struct for unifying the parsing of chunks.
pub struct ChunkParser<P: ParseChunk> {
    tag: [u8; 4],
    len: u32,
    phantom: PhantomData<P>,
}

impl<P: ParseChunk> ChunkParser<P> {
    fn new(tag: [u8; 4], len: u32) -> Self {
        ChunkParser { tag, len, phantom: PhantomData }
    }

    pub fn parse<B: ReadBytes + Seek>(&self, reader: &mut B) -> Result<P> {
        P::parse(reader, self.tag, self.len)
    }
}

// pub enum AiffFormatData {
//     Pcm()
// }

pub enum AiffChunks {
    Common(ChunkParser<CommonChunk>),
    Comment(ChunkParser<CommentsChunk>),
    ID3(ChunkParser<ID3v2Chunk>),
    Name(ChunkParser<TextChunk>),
    Sound(ChunkParser<SoundDataChunk>),

}

impl ParseChunkTag for AiffChunks {
    fn parse_tag(tag: [u8; 4], len: u32) -> Option<Self> {
        match &tag {
            ids::COMMON => parser!(AiffChunks::Common, CommonChunk, tag, len),
            ids::COMMENTS => parser!(AiffChunks::Comment, CommentsChunk, tag, len),
            ids::ID3 => parser!(AiffChunks::ID3, ID3v2Chunk, tag, len),
            ids::NAME => parser!(AiffChunks::Name, TextChunk, tag, len),
            ids::SOUND => parser!(AiffChunks::Sound, SoundDataChunk, tag, len),
            _ => None,
        }
    }
}

pub enum AiffFormatData {
    Pcm(AiffFormatPcm),
}

pub struct AiffFormatPcm {
    pub channels: Channels,
    pub bits_per_sample: u32,
    pub codec: CodecType
}

// pub struct FormChunk {
//     pub size: i32,
// }

// impl ParseChunk for FormChunk {
//     fn parse<B: ReadBytes + Read>(reader: &mut B, tag: [u8; 4], len: u32) -> Result<Self> {
//         if &tag != ids::FORM {
//             return decode_error("aiff: malformed form chunk");
//         }

//         // let size: i32 = reader.read_bits_leq32_signed(32).unwrap();
//         let size = util::read_i32_be(reader);
//         debug!("form chunk bytes: {}", size);

//         let form_type = reader.read_boxed_slice_exact(4).unwrap();

//         match form_type[..].try_into().unwrap() {
//             ids::AIFF => Ok(FormChunk { size }),
//             ids::AIFF_C => return unsupported_error("aiff: aiff-c is not supported"),
//             _ => return decode_error("aiff: unsupported forn type"),
//         }
//     }
// }

pub enum TextChunkType {
    Name,
    Author,
    Copyright,
    Annotation,
}

pub struct TextChunk {
    pub chunk_type: TextChunkType,
    pub chunk_size: i32,

    pub text: String,
    pub seek_size: Option<u32>,
}

impl ParseChunk for TextChunk {
    fn parse<B: ReadBytes>(reader: &mut B, tag: [u8; 4], len: u32) -> Result<Self> {
        let chunk_type = match &tag {
            ids::NAME => TextChunkType::Name,
            ids::AUTHOR => TextChunkType::Author,
            ids::COPYRIGHT => TextChunkType::Copyright,
            ids::ANNOTATION => TextChunkType::Annotation,
            _ => return decode_error("aiff: unsupported text chunk"),
        };

        // let size = util::read_i32_be(reader);

        // is handled by parserchunktag loop
        // let buf_pos_offset = if len % 2 > 0 { 1 } else { 0 };

        // println!("size: {}", len);
        debug!("aiff: text chunk len: {}", len);

        let text = reader.read_boxed_slice_exact(len.try_into().unwrap()).unwrap();
        let text = String::from_utf8(text.to_vec()).unwrap();
        debug!("aiff: text value: {}", text);

        // reader.seek_buffered(buf_pos_offset);
        

        Ok(Self {
            chunk_type,
            chunk_size: len.try_into().unwrap(),
            text,
            seek_size: None
        })
    }
}

pub struct CommonChunk {
    pub chunk_size: i32,

    pub num_channels: i16,
    pub num_sample_frames: u32,
    pub sample_size: i16,
    // 80 bit extended floating point num
    pub sample_rate: f64,
    
    pub format_data: AiffFormatData,
    // pub seek_size: Option<u32>,
}

impl CommonChunk {
    fn read_pcm_fmt<B: ReadBytes>(
        reader: &mut B,
        bits_per_sample: i16,
        num_channels: i16,
        len: u32,
    ) -> Result<AiffFormatData> {
        let codec = match bits_per_sample {
            8 => CODEC_TYPE_PCM_U8,
            16 => CODEC_TYPE_PCM_S16BE,
            24 => CODEC_TYPE_PCM_S24BE,
            32 => CODEC_TYPE_PCM_S32BE,
            _ => {
                return decode_error(
                    "aiff: bits per sample for fmt must be 8, 16, 24 or 32 bits"
                )
            }
        };

        // TODO: Check 
        let channels = match num_channels {
            1 => Channels::FRONT_LEFT,
            2 => Channels::FRONT_LEFT | Channels::FRONT_RIGHT,
            3 => Channels::FRONT_LEFT | Channels::FRONT_RIGHT | Channels::FRONT_CENTRE,
            4 => Channels::FRONT_LEFT | Channels::FRONT_CENTRE | Channels::FRONT_RIGHT | Channels::REAR_CENTRE,
            6 => Channels::FRONT_LEFT | Channels::FRONT_LEFT_CENTRE | Channels::FRONT_CENTRE | Channels::FRONT_RIGHT | Channels::FRONT_RIGHT_CENTRE | Channels::REAR_CENTRE,
            _ => return decode_error("aiff: unsupported channels"),
        };

        Ok(AiffFormatData::Pcm(
            AiffFormatPcm {
                bits_per_sample: bits_per_sample as u32, 
                channels, 
                codec 
            }
        ))
    }
}

impl ParseChunk for CommonChunk {
    fn parse<B: ReadBytes>(reader: &mut B, tag: [u8; 4], len: u32) -> Result<CommonChunk> {
        if &tag != ids::COMMON {
            return decode_error("aiff: malformed common chunk");
        }

        let num_channels = util::read_i16_be(reader);
        let num_sample_frames: u32 = reader.read_be_u32()?;
        let sample_size = util::read_i16_be(reader);
        let sample_rate = reader.read_boxed_slice_exact(10).unwrap();
        
        let sample_rate = match parse_extended_precision_bytes(sample_rate[..].try_into().unwrap()) {
            Ok(s) => s,
            Err(()) => return decode_error("aiff: failed to parse sample rate"),
        };

        let format_data = 
            Self::read_pcm_fmt(reader, sample_size, num_channels, len).unwrap();

        Ok(CommonChunk {
            chunk_size: len.try_into().unwrap(), 
            num_channels: num_channels.try_into().unwrap(),
            num_sample_frames, 
            sample_size: sample_size.try_into().unwrap(), 
            sample_rate, 
            format_data,
            // seek_size: None
        })
    }
}

pub struct ByteRange {
    pub start_pos: u64,
    pub end_pos: u64,
}

pub struct SoundDataChunk {
    pub chunk_size: i32,

    pub offset: u32,
    pub block_size: u32,
    // pub sound_data: Vec<u8>
    pub sound_size: u32,

    pub data_range: ByteRange
    // pub seek_size: Option<u32>,
}

impl ParseChunk for SoundDataChunk {
    fn parse<B: ReadBytes + Seek>(reader: &mut B, tag: [u8; 4], len: u32) -> Result<Self> {
        if &tag != ids::SOUND {
            return decode_error("aiff: malformed sound chunk");
        }

        let data_start_pos = reader.pos();

        let offset = reader.read_be_u32()?;
        let block_size = reader.read_be_u32()?;

        // A number 8 account for 'offset' + 'block' size bytes  
        let sound_size = len - 8;
        let data_end_pos = data_start_pos + u64::from(sound_size);

        reader.seek(SeekFrom::Current(sound_size.try_into().unwrap())).unwrap();
        
        Ok(SoundDataChunk {
            chunk_size: len.try_into().unwrap(), 
            offset,
            block_size,
            sound_size,
            data_range: ByteRange { 
                start_pos: data_start_pos, 
                end_pos: data_end_pos 
            }
        })
    }
}

pub struct ID3v2Chunk {
    pub chunk_size: i32,
}

impl ParseChunk for ID3v2Chunk {
    fn parse<B: ReadBytes + Seek>(reader: &mut B, tag: [u8; 4], len: u32) -> Result<Self> {
        if &tag != ids::ID3 {
            return decode_error("aiff: malformed id3v2 chunk");
        }

        reader.seek(SeekFrom::Current(i64::from(len)));

        Ok(Self { chunk_size: len as i32})
    }
}

pub struct Comment {
    pub timestamp: u32,
    pub marker_id: MarkerId,
    pub count: u16,
    pub text: String,
}

impl Parse for Comment {
    fn parse<B: ReadBytes + Seek>(reader: &mut B) -> Result<Self> {
        let timestamp = reader.read_be_u32()?;
        let marker_id = util::read_i16_be(reader);
        let count = reader.read_be_u16()?;

        let mut str_buf = vec![0; count as usize];
        reader.read_buf_exact(&mut str_buf).unwrap();
        let text = String::from_utf8(str_buf).unwrap();

        Ok(Self {
            timestamp,
            marker_id,
            count,
            text,
        })
    }
}

pub struct CommentsChunk {
    pub size: i32,
    pub num_comments: u16,
    pub comments: Vec<Comment>
}

impl ParseChunk for CommentsChunk {
    fn parse<B: ReadBytes + Seek>(reader: &mut B, tag: [u8; 4], len: u32) -> Result<Self> {
        if &tag != ids::COMMENTS {
            return decode_error("aiff: malformed id3v2 chunk");
        }

        let num_comments = reader.read_be_u16()?;

        let mut comments = Vec::with_capacity(num_comments.try_into().unwrap());
        for _ in 0..num_comments {
            comments.push(Comment::parse(reader)?);
        }

        Ok(Self {
            size: len.try_into().unwrap(),
            num_comments,
            comments,
        })
    }
}

type MarkerId = i16;
pub struct Marker {
    pub id: MarkerId,
    pub position: u32,
    pub marker_name: String,
}

// pub struct DataChunk {
//     pub len: u32,
// }

// impl ParseChunk for DataChunk {
//     fn parse<B: ReadBytes>(_: &mut B, _: [u8; 4], len: u32) -> Result<DataChunk> {
//         Ok(DataChunk { len })
//     }
