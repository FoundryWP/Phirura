//! Container header reads for the `sasa` shim's audio ABI.
//!
//! The ABI carries exactly two numbers - `sample_rate` and `frame_count` - because `AudioClip` and
//! `Music` only ever need `frames / rate` to answer `length()`. Nothing on the Rust side ever asks
//! for samples: the bytes stay with whoever will play them. So what is needed here is a *header*
//! read, never a decode, and every parser below is a pure `&[u8]` walk - no `unsafe`, no
//! allocation, no decoding.
//!
//! Three containers appear in practice: the chart's music (WAVE or MP3 depending on who exported
//! it) and every sound effect in the resource pack (Ogg Vorbis).
//!
//! Why these live here and not in the shell: `wp81_audio_decode` used to forward to a C# callback
//! that returned failure, and the very first resource-pack sound it saw - `respack/click.ogg`,
//! 10380 bytes - turned `GameCreate` into `-2 (ParseFailed)` with the reason buried in a `tracing`
//! event no subscriber on the phone was listening for. The walk is a couple of hundred bytes of
//! table-driven parsing that was already written and already measured against the real chart on the
//! desktop; calling across the FFI to have it done again was never buying anything. The shell
//! remains the fallback for a container this module does not recognise.

/// RIFF/WAVE: `fmt ` for rate and frame size, `data` for the count.
fn wav_info(b: &[u8]) -> Option<(u32, u64)> {
    let mut rate = 0u32;
    let mut channels = 0u16;
    let mut bits = 0u16;
    let mut data_len = 0u64;
    let mut i = 12usize;
    while i + 8 <= b.len() {
        let id = &b[i..i + 4];
        let size = u32::from_le_bytes([b[i + 4], b[i + 5], b[i + 6], b[i + 7]]) as usize;
        let body = i + 8;
        if body + size > b.len() {
            break;
        }
        if id == b"fmt " && size >= 16 {
            channels = u16::from_le_bytes([b[body + 2], b[body + 3]]);
            rate = u32::from_le_bytes([b[body + 4], b[body + 5], b[body + 6], b[body + 7]]);
            bits = u16::from_le_bytes([b[body + 14], b[body + 15]]);
        } else if id == b"data" {
            data_len = size as u64;
        }
        i = body + size + (size & 1);
    }
    if rate == 0 || channels == 0 || bits == 0 {
        return None;
    }
    let frame = (channels as u64) * ((bits as u64) / 8).max(1);
    Some((rate, data_len / frame.max(1)))
}

/// Ogg Vorbis (and Opus) page walk.
///
/// Rate comes from the identification packet on the first page; length from the granule position
/// of the last page that finished a packet. A granule of `u64::MAX` is the "no packet completed on
/// this page" sentinel and is skipped rather than taken as the length.
fn ogg_info(b: &[u8]) -> Option<(u32, u64)> {
    if b.len() < 27 || &b[0..4] != b"OggS" {
        return None;
    }

    // Identification packet: the first page's payload.
    let nseg = b[26] as usize;
    if b.len() < 27 + nseg {
        return None;
    }
    let head = b.get(27 + nseg..)?;

    let (rate, pre_skip) = if head.len() >= 16 && head[0] == 1 && &head[1..7] == b"vorbis" {
        // type(1) "vorbis"(6) version(u32) channels(u8) sample_rate(u32)
        let r = u32::from_le_bytes([head[12], head[13], head[14], head[15]]);
        if r == 0 {
            return None;
        }
        (r, 0u64)
    } else if head.len() >= 12 && &head[0..8] == b"OpusHead" {
        // Opus always decodes at 48 kHz regardless of the input rate in the header, and granule
        // positions are counted in that 48 kHz domain, minus the pre-skip.
        let skip = u16::from_le_bytes([head[10], head[11]]) as u64;
        (48000, skip)
    } else {
        return None;
    };

    // Walk every page forward, remembering the last usable granule.
    let mut granule = 0u64;
    let mut i = 0usize;
    while i + 27 <= b.len() {
        if &b[i..i + 4] != b"OggS" {
            break;
        }
        let nseg = b[i + 26] as usize;
        if i + 27 + nseg > b.len() {
            break;
        }
        let seglen: usize = b[i + 27..i + 27 + nseg].iter().map(|&x| x as usize).sum();
        let body = i + 27 + nseg;
        if body.saturating_add(seglen) > b.len() {
            break;
        }
        let g = u64::from_le_bytes([
            b[i + 6], b[i + 7], b[i + 8], b[i + 9], b[i + 10], b[i + 11], b[i + 12], b[i + 13],
        ]);
        if g != u64::MAX {
            granule = g;
        }
        i = body + seglen;
    }

    let frames = granule.saturating_sub(pre_skip);
    if frames == 0 {
        return None;
    }
    Some((rate, frames))
}

/// MPEG Layer III frame walk.
///
/// Layer III only - every `.mp3` ever exported is, and Layer I and II have a different frame-length
/// formula that nothing here will ever see. Rate comes from the first frame header; length comes
/// from the `Xing`/`Info` tag when the encoder wrote one (a single number, exact), and otherwise
/// from walking every frame, which is how a VBR file without a tag is measured.
fn mp3_info(b: &[u8]) -> Option<(u32, u64)> {
    // Bitrates in kbit/s, index 0 = "free format" and 15 = "bad", both unusable.
    const BR_V1: [u32; 16] = [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0];
    const BR_V2: [u32; 16] = [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0];
    // Sample rates by MPEG version: 0 = 2.5, 1 = reserved, 2 = 2, 3 = 1.
    const SR: [[u32; 3]; 4] = [
        [11025, 12000, 8000],
        [0, 0, 0],
        [22050, 24000, 16000],
        [44100, 48000, 32000],
    ];

    let frame_len = |pos: usize| -> Option<(u32, u64, usize)> {
        if pos + 4 > b.len() || b[pos] != 0xff || b[pos + 1] & 0xe0 != 0xe0 {
            return None;
        }
        let version = (b[pos + 1] >> 3) & 0b11;
        let layer = (b[pos + 1] >> 1) & 0b11;
        let bi = (b[pos + 2] >> 4) as usize;
        let si = ((b[pos + 2] >> 2) & 0b11) as usize;
        let padding = ((b[pos + 2] >> 1) & 1) as u32;
        if version == 1 || layer != 1 || si == 3 || bi == 0 || bi == 15 {
            return None;
        }
        let rate = SR[version as usize][si];
        let bitrate = (if version == 3 { BR_V1 } else { BR_V2 })[bi] * 1000;
        // Bytes per frame = (samples_per_frame / 8) * bitrate / sample_rate, plus the padding bit.
        // 144 for MPEG-1 (1152 samples), 72 for MPEG-2/2.5 (576 samples) - both fall out of this.
        let samples: u64 = if version == 3 { 1152 } else { 576 };
        let len = (samples / 8) * bitrate as u64 / rate as u64;
        Some((rate, samples, len as usize + padding as usize))
    };

    // Skip an ID3v2 tag when the file opens with one: "ID3" + version(2) + flags(1) + four
    // synchsafe size bytes, plus a 10-byte footer when the footer flag says there is one.
    let mut i = 0usize;
    if b.len() >= 10 && &b[0..3] == b"ID3" {
        let size = ((b[6] as usize & 0x7f) << 21)
            | ((b[7] as usize & 0x7f) << 14)
            | ((b[8] as usize & 0x7f) << 7)
            | (b[9] as usize & 0x7f);
        i = 10usize.saturating_add(size);
        if b[5] & 0x10 != 0 {
            i = i.saturating_add(10);
        }
    }
    // Then the first frame, however far the tag or any leading junk put it.
    while i + 4 <= b.len() && frame_len(i).is_none() {
        i += 1;
    }
    let (rate, samples, _) = frame_len(i)?;

    // A Xing/Info header, when present, states the frame count outright. It sits after the side
    // information in the first frame: 17/32 bytes for MPEG-1 (mono/other), 9/17 for MPEG-2 and 2.5.
    let mono = b[i + 3] >> 6 == 0b11;
    let side = match ((b[i + 1] >> 3) & 0b11, mono) {
        (3, true) => 17,
        (3, false) => 32,
        (_, true) => 9,
        (_, false) => 17,
    };
    let xing = i + 4 + side;
    let mut frames = if xing + 12 <= b.len()
        && (&b[xing..xing + 4] == b"Xing" || &b[xing..xing + 4] == b"Info")
        && u32::from_be_bytes([b[xing + 4], b[xing + 5], b[xing + 6], b[xing + 7]]) & 1 != 0
    {
        Some(
            u32::from_be_bytes([b[xing + 8], b[xing + 9], b[xing + 10], b[xing + 11]]) as u64,
        )
    } else {
        None
    };

    // Otherwise walk. Bitrate may change from frame to frame in VBR, so the header is re-read each
    // time rather than the first frame's length being applied to the whole file.
    if frames.is_none() {
        let mut count = 0u64;
        let mut pos = i;
        while let Some((_, _, len)) = frame_len(pos) {
            if len == 0 {
                break;
            }
            count += 1;
            pos = pos.saturating_add(len);
        }
        frames = Some(count);
    }

    let frames = frames?;
    if frames == 0 {
        return None;
    }
    Some((rate, frames * samples))
}

/// Route a byte buffer to the right container parser.
///
/// Dispatches on the leading four bytes rather than the file name: the shell hands the bytes over
/// and the extension is whatever the chart author happened to write.
pub fn audio_info(b: &[u8]) -> Option<(u32, u64)> {
    if b.len() < 16 {
        return None;
    }
    match &b[0..4] {
        b"RIFF" => wav_info(b),
        b"OggS" => ogg_info(b),
        // MPEG frames open with the 11-bit sync `11111111 111`, but an MP3 usually opens with an
        // ID3v2 tag instead, which `mp3_info` skips on its own. `ID3` cannot collide with either
        // of the other two signatures, so the fourth arm is unambiguous.
        _ if b[0] == 0xff || &b[0..3] == b"ID3" => mp3_info(b),
        _ => None,
    }
}
