//! WAV decoding for extracted `DirectSound` samples.
//!
//! The decoded fields follow `tools/wav2agb`:
//!
//! - `fmt ` supplies the format tag, channel count, sample rate, block
//!   alignment, and sample width. The supported mono layouts are unsigned
//!   8-bit PCM, signed 16/24/32-bit PCM, and 32/64-bit IEEE float
//!   (`wav_file.cpp:125-156`).
//! - Repeated chunks: the last `fmt `, `data`, `smpl`, `agbp`, and `agbl`
//!   chunk is the effective one, as wav2agb overwrites its active state for
//!   each later match (`wav_file.cpp:125-192`). Every `fmt ` and `smpl` chunk
//!   is still validated, as wav2agb throws on any bad one. A later `smpl`
//!   with zero loops updates only the key and tuning, leaving an earlier
//!   chunk's loop enabled. A `agbp`/`agbl` chunk shorter than four bytes is
//!   ignored, leaving any earlier valid one in place, as wav2agb assigns
//!   only when the chunk is at least four bytes (`wav_file.cpp:183-193`).
//! - `smpl` supplies the MIDI unity note, pitch fraction, and optional forward
//!   loop. wav2agb divides the pitch fraction by `2^32 * 100` despite its
//!   adjacent comment claiming a `0..100` cent range; this decoder preserves
//!   the implemented conversion (`wav_file.cpp:166-180`). The loop end is
//!   inclusive, so it becomes an exclusive sample count capped by `data`.
//! - Non-zero `agbp` and `agbl` words replace the derived pitch and sample
//!   count. Zero preserves the derived value (`converter.cpp:385-402`).
//! - `data` holds samples in the declared layout. Each value is normalized and
//!   converted to signed 8-bit PCM with wav2agb's floor-and-clamp operation
//!   (`wav_file.cpp:235-297`, `converter.cpp:56-92`).
//!
//! `WavSample::sample_count` is the logical sample count written to the
//! compiled header: `agbl` overrides only that header word
//! (`converter.cpp:399-401`). wav2agb's binary payload writer ignores the
//! override and always emits every sample through the *unoverridden* sampler
//! end, then zero-pads to a four-byte boundary (`converter.cpp:77-90`;
//! `audio_rules.mk:22,26` builds that `-b` binary output into the ROM).
//! `SoundMainRAM`
//! prefetches the sample immediately after the logical end for its final
//! linear-interpolation step before a loop wraps or a one-shot retires
//! (`m4a_1.s:399-407`), so that extra encoded byte is observable, not inert
//! padding.
//!
//! `WavSample::data` therefore always holds exactly `sample_count + 1`
//! values: the `sample_count` logical samples, followed by one retained
//! interpolation-guard sample. When `agbl` trims the logical count below the
//! unoverridden sampler end, that guard is the real encoded sample wav2agb
//! would have retained. When `agbl` extends past that end, the override
//! changes only the header word (`converter.cpp:399-401`); logical positions
//! past the sampler end, and the guard after them, are the writer's zero
//! alignment padding (`converter.cpp:77-90`), not raw samples the payload
//! never emits. Otherwise (no override, or an override exactly at that
//! natural boundary) wav2agb's own payload has nothing left to retain as the
//! guard; this decoder documents that boundary as `0`. Every
//! `sound/direct_sound_samples/*.wav` upstream ships sets `agbl` below its
//! raw sample count, so the pack never exercises the past-the-end fallback.
//!
//! Missing required chunks, unsupported fields, partial records, misaligned
//! sample data, and out-of-range loop metadata fail closed.

use std::{fmt, mem::size_of};

const RIFF_HEADER_SIZE: usize = 12;
const RIFF_LENGTH_PREFIX_SIZE: usize = 8;
const CHUNK_HEADER_SIZE: usize = 8;

const PCM_FORMAT_TAG: u16 = 1;
const IEEE_FLOAT_FORMAT_TAG: u16 = 3;
const MONO_CHANNEL_COUNT: u16 = 1;
const FMT_CHUNK_REQUIRED_SIZE: usize = 16;
const FMT_FORMAT_TAG_OFFSET: usize = 0;
const FMT_CHANNEL_COUNT_OFFSET: usize = 2;
const FMT_SAMPLE_RATE_OFFSET: usize = 4;
const FMT_BLOCK_ALIGN_OFFSET: usize = 12;
const FMT_BITS_PER_SAMPLE_OFFSET: usize = 14;

const SMPL_HEADER_SIZE: usize = 36;
const SMPL_LOOP_RECORD_SIZE: usize = 24;
const SMPL_MIDI_UNITY_NOTE_OFFSET: usize = 12;
const SMPL_MIDI_PITCH_FRACTION_OFFSET: usize = 16;
const SMPL_LOOP_COUNT_OFFSET: usize = 28;
const SMPL_LOOP_TYPE_OFFSET: usize = 40;
const SMPL_LOOP_START_OFFSET: usize = 44;
const SMPL_LOOP_END_OFFSET: usize = 48;
const MAX_MIDI_KEY: u32 = 127;
const UNITY_MIDI_KEY: u8 = 60;
const FORWARD_LOOP_TYPE: u32 = 0;
const MIDI_PITCH_FRACTION_DENOMINATOR: f64 = 4_294_967_296.0 * 100.0;
const SEMITONES_PER_OCTAVE: f64 = 12.0;
const CENTS_PER_OCTAVE: f64 = 1200.0;

const PITCH_FIXED_POINT_SCALE: f64 = 1024.0;
const OUTPUT_SAMPLE_SCALE: f64 = 128.0;
const OUTPUT_SAMPLE_MIN: f64 = -128.0;
const OUTPUT_SAMPLE_MAX: f64 = 127.0;
const S24_SIGN_BIT: u32 = 1 << 23;
const S24_VALUE_RANGE: i64 = 1 << 24;

/// A WAV decoding failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WavError {
    /// The file lacks the `RIFF` signature.
    BadRiffMagic,
    /// The declared RIFF length differs from the file length.
    RiffSizeMismatch { declared: u32, actual: usize },
    /// The RIFF form is not `WAVE`.
    BadWaveMagic,
    /// A chunk header, body, or required pad byte is incomplete.
    ChunkTruncated,
    /// The file has no `fmt ` chunk.
    MissingFmtChunk,
    /// The file has no `data` chunk.
    MissingDataChunk,
    /// The format does not have exactly one channel.
    NotMono { channels: u16 },
    /// The format tag, alignment, and sample width are unsupported.
    UnsupportedFormat {
        format_tag: u16,
        block_align: u16,
        bits_per_sample: u16,
    },
    /// The sample data does not contain a whole number of samples.
    DataLengthNotAligned {
        data_len: usize,
        bytes_per_sample: u32,
    },
    /// The `smpl` chunk is shorter than its declared contents.
    TruncatedSmplChunk,
    /// The `smpl` chunk declares multiple loops.
    TooManySampleLoops { count: u32 },
    /// The sampler loop is not forward-only.
    UnsupportedLoopType { loop_type: u32 },
    /// The derived sample count exceeds the available sample data.
    SizeOutOfRange { size: u32, num_samples: u32 },
    /// The loop starts outside the derived sample range.
    LoopStartOutOfRange { loop_start: u32, size: u32 },
}

impl fmt::Display for WavError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadRiffMagic => write!(f, "not a WAV file (missing RIFF magic)"),
            Self::RiffSizeMismatch { declared, actual } => write!(
                f,
                "RIFF chunk length ({declared}) plus header doesn't match file length ({actual})"
            ),
            Self::BadWaveMagic => write!(f, "RIFF form type is not WAVE"),
            Self::ChunkTruncated => write!(f, "WAV chunk truncated or runs past end of file"),
            Self::MissingFmtChunk => write!(f, "WAV file has no `fmt ` chunk"),
            Self::MissingDataChunk => write!(f, "WAV file has no `data` chunk"),
            Self::NotMono { channels } => {
                write!(f, "WAV file is not mono ({channels} channels)")
            }
            Self::UnsupportedFormat {
                format_tag,
                block_align,
                bits_per_sample,
            } => write!(
                f,
                "unsupported WAV format: tag={format_tag}, block_align={block_align}, \
                 bits_per_sample={bits_per_sample}"
            ),
            Self::DataLengthNotAligned {
                data_len,
                bytes_per_sample,
            } => write!(
                f,
                "WAV data chunk length ({data_len}) is not a multiple of the sample width \
                 ({bytes_per_sample})"
            ),
            Self::TruncatedSmplChunk => write!(f, "WAV `smpl` chunk is truncated"),
            Self::TooManySampleLoops { count } => write!(
                f,
                "WAV `smpl` chunk declares {count} loops (at most 1 supported)"
            ),
            Self::UnsupportedLoopType { loop_type } => write!(
                f,
                "WAV `smpl` chunk loop type {loop_type} is not supported (only forward-only, type 0)"
            ),
            Self::SizeOutOfRange { size, num_samples } => write!(
                f,
                "derived sample count {size} exceeds the {num_samples} samples in the data chunk"
            ),
            Self::LoopStartOutOfRange { loop_start, size } => write!(
                f,
                "loop start {loop_start} is at or past the derived sample count {size}"
            ),
        }
    }
}

impl std::error::Error for WavError {}

/// A decoded `DirectSound` sample.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WavSample {
    pub(super) base_frequency: u32,
    pub(super) loop_start: Option<u32>,
    /// The logical sample count written to the compiled header (`agbl`'s
    /// resolved value). `data` holds exactly one more value than this.
    pub(super) sample_count: u32,
    pub(super) data: Vec<i8>,
}

struct Chunk<'a> {
    id: [u8; 4],
    data: &'a [u8],
}

fn read_chunks(bytes: &[u8]) -> Result<Vec<Chunk<'_>>, WavError> {
    let mut chunks = Vec::new();
    let mut position = 0usize;
    while position + CHUNK_HEADER_SIZE <= bytes.len() {
        let id = [
            bytes[position],
            bytes[position + 1],
            bytes[position + 2],
            bytes[position + 3],
        ];
        let body_len = u32::from_le_bytes([
            bytes[position + 4],
            bytes[position + 5],
            bytes[position + 6],
            bytes[position + 7],
        ]) as usize;
        let body_start = position + CHUNK_HEADER_SIZE;
        let body_end = body_start
            .checked_add(body_len)
            .ok_or(WavError::ChunkTruncated)?;
        if body_end > bytes.len() {
            return Err(WavError::ChunkTruncated);
        }
        chunks.push(Chunk {
            id,
            data: &bytes[body_start..body_end],
        });
        position = body_end + (body_len % 2);
    }
    if position != bytes.len() {
        return Err(WavError::ChunkTruncated);
    }
    Ok(chunks)
}

/// Every chunk body with `id`, in file order.
fn chunks_with_id<'a>(chunks: &'a [Chunk<'a>], id: [u8; 4]) -> impl Iterator<Item = &'a [u8]> + 'a {
    chunks
        .iter()
        .filter(move |chunk| chunk.id == id)
        .map(|chunk| chunk.data)
}

/// The effective chunk for `id`: the LAST one in file order, because wav2agb
/// overwrites its active state for every later matching chunk
/// (`wav_file.cpp:125-192`).
fn find_chunk<'a>(chunks: &'a [Chunk<'a>], id: [u8; 4]) -> Option<&'a [u8]> {
    chunks_with_id(chunks, id).last()
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, WavError> {
    let field = bytes
        .get(offset..offset + size_of::<u16>())
        .ok_or(WavError::ChunkTruncated)?;
    Ok(u16::from_le_bytes([field[0], field[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, WavError> {
    let field = bytes
        .get(offset..offset + size_of::<u32>())
        .ok_or(WavError::ChunkTruncated)?;
    Ok(u32::from_le_bytes([field[0], field[1], field[2], field[3]]))
}

fn sign_extend_s24(encoded: u32) -> i32 {
    let signed = if encoded & S24_SIGN_BIT == 0 {
        i64::from(encoded)
    } else {
        i64::from(encoded) - S24_VALUE_RANGE
    };
    i32::try_from(signed).expect("signed 24-bit samples fit in i32")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SampleFormat {
    U8,
    S16,
    S24,
    S32,
    F32,
    F64,
}

impl SampleFormat {
    const fn bytes_per_sample(self) -> u32 {
        match self {
            Self::U8 => 1,
            Self::S16 => 2,
            Self::S24 => 3,
            Self::S32 | Self::F32 => 4,
            Self::F64 => 8,
        }
    }

    fn resolve(format_tag: u16, block_align: u16, bits_per_sample: u16) -> Result<Self, WavError> {
        let format = match (format_tag, block_align, bits_per_sample) {
            (PCM_FORMAT_TAG, 1, 8) => Some(Self::U8),
            (PCM_FORMAT_TAG, 2, 16) => Some(Self::S16),
            (PCM_FORMAT_TAG, 3, 24) => Some(Self::S24),
            (PCM_FORMAT_TAG, 4, 32) => Some(Self::S32),
            (IEEE_FLOAT_FORMAT_TAG, 4, 32) => Some(Self::F32),
            (IEEE_FLOAT_FORMAT_TAG, 8, 64) => Some(Self::F64),
            _ => None,
        };
        format.ok_or(WavError::UnsupportedFormat {
            format_tag,
            block_align,
            bits_per_sample,
        })
    }

    fn normalize(self, bytes: &[u8], sample_index: usize) -> f64 {
        let start = sample_index * self.bytes_per_sample() as usize;
        match self {
            Self::U8 => (f64::from(bytes[start]) - 128.0) / 128.0,
            Self::S16 => {
                let value = i16::from_le_bytes([bytes[start], bytes[start + 1]]);
                f64::from(value) / 32_768.0
            }
            Self::S24 => {
                let encoded = u32::from(bytes[start])
                    | (u32::from(bytes[start + 1]) << 8)
                    | (u32::from(bytes[start + 2]) << 16);
                let value = sign_extend_s24(encoded);
                f64::from(value) / 8_388_608.0
            }
            Self::S32 => {
                let value = i32::from_le_bytes([
                    bytes[start],
                    bytes[start + 1],
                    bytes[start + 2],
                    bytes[start + 3],
                ]);
                f64::from(value) / 2_147_483_648.0
            }
            Self::F32 => {
                let value = f32::from_le_bytes([
                    bytes[start],
                    bytes[start + 1],
                    bytes[start + 2],
                    bytes[start + 3],
                ]);
                f64::from(value)
            }
            Self::F64 => f64::from_le_bytes([
                bytes[start],
                bytes[start + 1],
                bytes[start + 2],
                bytes[start + 3],
                bytes[start + 4],
                bytes[start + 5],
                bytes[start + 6],
                bytes[start + 7],
            ]),
        }
    }
}

struct SampleMetadata {
    midi_key: u8,
    tuning_cents: f64,
    loop_start: Option<u32>,
    unoverridden_sample_count: u32,
}

impl SampleMetadata {
    const fn without_sampler_chunk(num_samples: u32) -> Self {
        Self {
            midi_key: UNITY_MIDI_KEY,
            tuning_cents: 0.0,
            loop_start: None,
            unoverridden_sample_count: num_samples,
        }
    }
}

fn parse_fmt_chunk(fmt: &[u8]) -> Result<(SampleFormat, u32), WavError> {
    if fmt.len() < FMT_CHUNK_REQUIRED_SIZE {
        return Err(WavError::ChunkTruncated);
    }
    let format_tag = read_u16(fmt, FMT_FORMAT_TAG_OFFSET)?;
    let channel_count = read_u16(fmt, FMT_CHANNEL_COUNT_OFFSET)?;
    if channel_count != MONO_CHANNEL_COUNT {
        return Err(WavError::NotMono {
            channels: channel_count,
        });
    }
    let sample_rate = read_u32(fmt, FMT_SAMPLE_RATE_OFFSET)?;
    let block_align = read_u16(fmt, FMT_BLOCK_ALIGN_OFFSET)?;
    let bits_per_sample = read_u16(fmt, FMT_BITS_PER_SAMPLE_OFFSET)?;
    let format = SampleFormat::resolve(format_tag, block_align, bits_per_sample)?;
    Ok((format, sample_rate))
}

fn parse_smpl_chunk(smpl: &[u8], num_samples: u32) -> Result<SampleMetadata, WavError> {
    if smpl.len() < SMPL_HEADER_SIZE {
        return Err(WavError::TruncatedSmplChunk);
    }
    let midi_unity_note = read_u32(smpl, SMPL_MIDI_UNITY_NOTE_OFFSET)?;
    #[allow(clippy::cast_possible_truncation)]
    let midi_key = midi_unity_note.min(MAX_MIDI_KEY) as u8;
    let midi_pitch_fraction = read_u32(smpl, SMPL_MIDI_PITCH_FRACTION_OFFSET)?;
    let tuning_cents = f64::from(midi_pitch_fraction) / MIDI_PITCH_FRACTION_DENOMINATOR;
    let loop_count = read_u32(smpl, SMPL_LOOP_COUNT_OFFSET)?;
    if loop_count > 1 {
        return Err(WavError::TooManySampleLoops { count: loop_count });
    }
    if loop_count == 0 {
        return Ok(SampleMetadata {
            midi_key,
            tuning_cents,
            loop_start: None,
            unoverridden_sample_count: num_samples,
        });
    }
    if smpl.len() < SMPL_HEADER_SIZE + SMPL_LOOP_RECORD_SIZE {
        return Err(WavError::TruncatedSmplChunk);
    }
    let loop_type = read_u32(smpl, SMPL_LOOP_TYPE_OFFSET)?;
    if loop_type != FORWARD_LOOP_TYPE {
        return Err(WavError::UnsupportedLoopType { loop_type });
    }
    let loop_start = read_u32(smpl, SMPL_LOOP_START_OFFSET)?;
    let inclusive_loop_end = read_u32(smpl, SMPL_LOOP_END_OFFSET)?;
    let unoverridden_sample_count = inclusive_loop_end.saturating_add(1).min(num_samples);
    Ok(SampleMetadata {
        midi_key,
        tuning_cents,
        loop_start: Some(loop_start),
        unoverridden_sample_count,
    })
}

/// The last `agbp`/`agbl` chunk of at least four bytes, when non-zero.
/// wav2agb assigns these words only for `chunkLen >= 4`, so a shorter chunk
/// neither errors nor replaces an earlier one (`wav_file.cpp:183-193`).
fn read_nonzero_override(chunks: &[Chunk<'_>], id: [u8; 4]) -> Option<u32> {
    chunks_with_id(chunks, id)
        .filter(|chunk| chunk.len() >= size_of::<u32>())
        .last()
        .and_then(|chunk| read_u32(chunk, 0).ok())
        .filter(|&word| word != 0)
}

fn derive_pitch(sample_rate: u32, midi_key: u8, tuning_cents: f64, exact: Option<u32>) -> u32 {
    if let Some(exact) = exact {
        return exact;
    }
    let pitch = if midi_key == UNITY_MIDI_KEY && tuning_cents == 0.0 {
        f64::from(sample_rate)
    } else {
        f64::from(sample_rate)
            * 2f64.powf(
                (f64::from(UNITY_MIDI_KEY) - f64::from(midi_key)) / SEMITONES_PER_OCTAVE
                    + tuning_cents / CENTS_PER_OCTAVE,
            )
    };
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let fixed_point_pitch = (pitch * PITCH_FIXED_POINT_SCALE) as u32;
    fixed_point_pitch
}

fn decode_pcm(format: SampleFormat, data: &[u8], sample_count: u32) -> Vec<i8> {
    let mut pcm = Vec::with_capacity(sample_count as usize);
    for sample_index in 0..sample_count as usize {
        let normalized = format.normalize(data, sample_index);
        let scaled = (normalized * OUTPUT_SAMPLE_SCALE)
            .floor()
            .clamp(OUTPUT_SAMPLE_MIN, OUTPUT_SAMPLE_MAX);
        #[allow(clippy::cast_possible_truncation)]
        pcm.push(scaled as i8);
    }
    pcm
}

/// Decodes a WAV file into one `DirectSound` sample.
///
/// # Errors
///
/// Returns [`WavError`] when the RIFF structure or supported sample contract
/// is invalid.
pub(super) fn decode(bytes: &[u8]) -> Result<WavSample, WavError> {
    if bytes.len() < RIFF_HEADER_SIZE || &bytes[0..4] != b"RIFF" {
        return Err(WavError::BadRiffMagic);
    }
    let declared_riff_len = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    if declared_riff_len as usize + RIFF_LENGTH_PREFIX_SIZE != bytes.len() {
        return Err(WavError::RiffSizeMismatch {
            declared: declared_riff_len,
            actual: bytes.len(),
        });
    }
    if &bytes[8..RIFF_HEADER_SIZE] != b"WAVE" {
        return Err(WavError::BadWaveMagic);
    }

    let chunks = read_chunks(&bytes[RIFF_HEADER_SIZE..])?;
    // wav2agb validates every `fmt ` chunk as it scans, then keeps the last.
    let mut fmt_result = None;
    for fmt in chunks_with_id(&chunks, *b"fmt ") {
        fmt_result = Some(parse_fmt_chunk(fmt)?);
    }
    let (format, sample_rate) = fmt_result.ok_or(WavError::MissingFmtChunk)?;

    let data = find_chunk(&chunks, *b"data").ok_or(WavError::MissingDataChunk)?;
    let bytes_per_sample = format.bytes_per_sample();
    if data.len() % bytes_per_sample as usize != 0 {
        return Err(WavError::DataLengthNotAligned {
            data_len: data.len(),
            bytes_per_sample,
        });
    }
    #[allow(clippy::cast_possible_truncation)]
    let num_samples = (data.len() / bytes_per_sample as usize) as u32;

    // Every `smpl` chunk is validated; the last one supplies the metadata.
    let mut metadata = SampleMetadata::without_sampler_chunk(num_samples);
    for smpl in chunks_with_id(&chunks, *b"smpl") {
        let next = parse_smpl_chunk(smpl, num_samples)?;
        // A zero-loop chunk updates only key and tuning; an earlier chunk's
        // loop stays enabled, as in wav2agb (`wav_file.cpp:174-182`).
        metadata = if next.loop_start.is_none() && metadata.loop_start.is_some() {
            SampleMetadata {
                midi_key: next.midi_key,
                tuning_cents: next.tuning_cents,
                ..metadata
            }
        } else {
            next
        };
    }
    let exact_pitch = read_nonzero_override(&chunks, *b"agbp");
    let exact_sample_count = read_nonzero_override(&chunks, *b"agbl");
    let base_frequency = derive_pitch(
        sample_rate,
        metadata.midi_key,
        metadata.tuning_cents,
        exact_pitch,
    );
    let sample_count = exact_sample_count.unwrap_or(metadata.unoverridden_sample_count);
    if sample_count > num_samples {
        return Err(WavError::SizeOutOfRange {
            size: sample_count,
            num_samples,
        });
    }
    if let Some(loop_start) = metadata.loop_start {
        if loop_start >= sample_count {
            return Err(WavError::LoopStartOutOfRange {
                loop_start,
                size: sample_count,
            });
        }
    }

    // wav2agb's binary payload writer runs to the unoverridden sampler end
    // regardless of `agbl` (converter.cpp:77-90). A shorter override keeps
    // the next encoded sample as the mixer's interpolation guard
    // (m4a_1.s:399-407). A longer override changes only the header word
    // (converter.cpp:399-401); logical positions past the sampler end are
    // the writer's zero alignment padding, not later raw samples.
    let natural_end = metadata.unoverridden_sample_count;
    let has_natural_guard = sample_count < natural_end;
    let retained_len = if has_natural_guard {
        sample_count + 1
    } else {
        natural_end
    };
    let mut sample_data = decode_pcm(format, data, retained_len);
    if sample_count > natural_end {
        sample_data.resize(sample_count as usize, 0);
    }
    if !has_natural_guard {
        sample_data.push(0);
    }

    Ok(WavSample {
        base_frequency,
        loop_start: metadata.loop_start,
        sample_count,
        data: sample_data,
    })
}

#[cfg(test)]
mod tests;
