//! The export's MP4 writer: a Media Foundation sink writer, H.264 video and AAC audio (U1; retail encoded VP6/FLV
//! for its servers, research Q8). Frames go in as top-down RGBA and are converted to the RGB32 (BGRA) input the
//! sink writer's own colour converter accepts; audio goes in as interleaved f32 and leaves as 16-bit PCM.
//!
//! Review C1: Media Foundation is loaded at run time (`LoadLibraryExW` from System32 + `GetProcAddress`), never
//! imported, so the exe starts on Windows N / KN without the Media Feature Pack; there [`available`] is false and
//! hud.rs shows the "Media Feature Pack required" popup. Only the free functions need this: the interfaces they
//! return are called through their vtables. `import_table_has_no_media_foundation` checks the built test exe.
use super::export::ExportSettings;
use std::path::Path;

#[cfg(windows)]
use windows::{
    Win32::Media::MediaFoundation::*,
    core::{GUID, HRESULT, Interface, PCWSTR},
};

/// 100 ns units per second (MF sample times).
const UNITS: i64 = 10_000_000;
pub(crate) const CHANNELS: u32 = 2;

/// The six Media Foundation entry points the writer uses (and the test probe's reader), resolved once.
#[cfg(windows)]
struct Api {
    startup: unsafe extern "system" fn(u32, u32) -> HRESULT,
    attributes: unsafe extern "system" fn(*mut *mut std::ffi::c_void, u32) -> HRESULT,
    media_type: unsafe extern "system" fn(*mut *mut std::ffi::c_void) -> HRESULT,
    memory_buffer: unsafe extern "system" fn(u32, *mut *mut std::ffi::c_void) -> HRESULT,
    sample: unsafe extern "system" fn(*mut *mut std::ffi::c_void) -> HRESULT,
    sink_writer: unsafe extern "system" fn(PCWSTR, *mut std::ffi::c_void, *mut std::ffi::c_void, *mut *mut std::ffi::c_void) -> HRESULT,
    #[cfg_attr(not(test), allow(dead_code))]
    source_reader: unsafe extern "system" fn(PCWSTR, *mut std::ffi::c_void, *mut *mut std::ffi::c_void) -> HRESULT,
}

#[cfg(windows)]
fn api() -> Option<&'static Api> {
    use windows::{
        Win32::System::LibraryLoader::{GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW},
        core::{PCSTR, w},
    };
    static API: std::sync::OnceLock<Option<Api>> = std::sync::OnceLock::new();
    API.get_or_init(|| {
        // SAFETY: System32 DLLs by name; each symbol is cast to the signature the SDK headers give it. The modules
        // stay loaded for the process (never freed), so the pointers stay valid.
        unsafe {
            let plat = LoadLibraryExW(w!("mfplat.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32).ok()?;
            let readwrite = LoadLibraryExW(w!("mfreadwrite.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32).ok()?;
            macro_rules! get {
                ($module:expr, $name:literal) => {
                    std::mem::transmute(GetProcAddress($module, PCSTR(concat!($name, "\0").as_ptr()))?)
                };
            }
            Some(Api {
                startup: get!(plat, "MFStartup"),
                attributes: get!(plat, "MFCreateAttributes"),
                media_type: get!(plat, "MFCreateMediaType"),
                memory_buffer: get!(plat, "MFCreateMemoryBuffer"),
                sample: get!(plat, "MFCreateSample"),
                sink_writer: get!(readwrite, "MFCreateSinkWriterFromURL"),
                source_reader: get!(readwrite, "MFCreateSourceReaderFromURL"),
            })
        }
    })
    .as_ref()
}

/// Whether Media Foundation is installed (false on Windows N / KN without the Media Feature Pack).
pub(crate) fn available() -> bool {
    #[cfg(windows)]
    {
        api().is_some()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// What [`Mp4Writer::create`] returns when [`available`] is false.
pub(crate) const UNAVAILABLE: &str = "Media Foundation is not installed (Media Feature Pack)";

/// RGB32's bytes (B, G, R, X) from RGBA: one copy, swizzled in place (render speed 10-27: the old flat_map grew its
/// Vec a pixel at a time).
fn bgrx(rgba: &[u8]) -> Vec<u8> {
    let mut bgrx = rgba.to_vec();
    for p in bgrx.chunks_exact_mut(4) {
        p.swap(0, 2);
        p[3] = 255;
    }
    bgrx
}

#[cfg(windows)]
fn wide(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

/// An out-parameter interface from a resolved entry point.
#[cfg(windows)]
unsafe fn out<T: Interface>(what: &'static str, call: impl FnOnce(*mut *mut std::ffi::c_void) -> HRESULT) -> Result<T, String> {
    let mut raw = std::ptr::null_mut();
    call(&mut raw).ok().map_err(context(what))?;
    if raw.is_null() {
        return Err(format!("Media Foundation {what}: no object"));
    }
    // SAFETY: the entry point returned an owned reference of this interface type.
    Ok(unsafe { T::from_raw(raw) })
}

pub(crate) struct Mp4Writer {
    #[cfg(windows)]
    writer: IMFSinkWriter,
    #[cfg(windows)]
    streams: (u32, u32),
    width: u32,
    height: u32,
    fps: u32,
    sample_rate: u32,
    /// Audio frames written (per channel).
    audio_frames: u64,
}

// SAFETY: the sink writer is created in the multithreaded apartment (see `create`), so its interface may be called
// from any thread; the HUD resource holding it is only touched by one system at a time.
#[cfg(windows)]
unsafe impl Send for Mp4Writer {}
#[cfg(windows)]
unsafe impl Sync for Mp4Writer {}

#[cfg(windows)]
fn context(what: &'static str) -> impl Fn(windows::core::Error) -> String {
    move |e| format!("Media Foundation {what}: {}", e.message())
}

#[cfg(windows)]
fn set(kind: &IMFMediaType, what: &'static str, guids: &[(&GUID, &GUID)], words: &[(&GUID, u32)], longs: &[(&GUID, u64)]) -> Result<(), String> {
    // SAFETY: attribute setters on a media type this function's caller owns.
    unsafe {
        for (key, value) in guids {
            kind.SetGUID(*key, *value).map_err(context(what))?;
        }
        for (key, value) in words {
            kind.SetUINT32(*key, *value).map_err(context(what))?;
        }
        for (key, value) in longs {
            kind.SetUINT64(*key, *value).map_err(context(what))?;
        }
    }
    Ok(())
}

impl Mp4Writer {
    #[cfg(windows)]
    pub fn create(path: &Path, s: &ExportSettings, sample_rate: u32) -> Result<Self, String> {
        let api = api().ok_or(UNAVAILABLE)?;
        // SAFETY: Media Foundation calls through the resolved entry points and the interfaces they return.
        unsafe {
            // Joins the MTA so the writer is free-threaded; a thread already in an STA keeps it (RPC_E_CHANGED_MODE).
            let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
            // ponytail: MFStartup is reference counted and never shut down; one count per export, process-lifetime.
            (api.startup)(MF_VERSION, MFSTARTUP_FULL).ok().map_err(context("startup"))?;
            let attributes: IMFAttributes = out("attributes", |p| (api.attributes)(p, 2))?;
            attributes.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1).map_err(context("attributes"))?;
            attributes.SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_MPEG4).map_err(context("attributes"))?;
            let url = wide(path);
            let writer: IMFSinkWriter =
                out("sink writer", |p| (api.sink_writer)(PCWSTR(url.as_ptr()), std::ptr::null_mut(), attributes.as_raw(), p))?;
            let size = (u64::from(s.width) << 32) | u64::from(s.height);
            let rate = (u64::from(s.fps) << 32) | 1;
            let progressive = MFVideoInterlace_Progressive.0 as u32;
            let media_type = || out::<IMFMediaType>("media type", |p| (api.media_type)(p));

            let video_out = media_type()?;
            set(
                &video_out,
                "video type",
                &[(&MF_MT_MAJOR_TYPE, &MFMediaType_Video), (&MF_MT_SUBTYPE, &MFVideoFormat_H264)],
                &[(&MF_MT_AVG_BITRATE, s.bitrate()), (&MF_MT_INTERLACE_MODE, progressive)],
                &[(&MF_MT_FRAME_SIZE, size), (&MF_MT_FRAME_RATE, rate), (&MF_MT_PIXEL_ASPECT_RATIO, (1 << 32) | 1)],
            )?;
            let video = writer.AddStream(&video_out).map_err(context("video stream"))?;
            let video_in = media_type()?;
            // Top-down rows (RGB's default stride is bottom-up).
            set(
                &video_in,
                "video input",
                &[(&MF_MT_MAJOR_TYPE, &MFMediaType_Video), (&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)],
                &[(&MF_MT_INTERLACE_MODE, progressive), (&MF_MT_DEFAULT_STRIDE, s.width * 4)],
                &[(&MF_MT_FRAME_SIZE, size), (&MF_MT_FRAME_RATE, rate), (&MF_MT_PIXEL_ASPECT_RATIO, (1 << 32) | 1)],
            )?;
            writer.SetInputMediaType(video, &video_in, None).map_err(context("video input"))?;

            let aac = media_type()?;
            // 160 kbps, one of the four rates the AAC encoder takes.
            set(
                &aac,
                "audio type",
                &[(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio), (&MF_MT_SUBTYPE, &MFAudioFormat_AAC)],
                &[
                    (&MF_MT_AUDIO_BITS_PER_SAMPLE, 16),
                    (&MF_MT_AUDIO_SAMPLES_PER_SECOND, sample_rate),
                    (&MF_MT_AUDIO_NUM_CHANNELS, CHANNELS),
                    (&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 20_000),
                ],
                &[],
            )?;
            let audio = writer.AddStream(&aac).map_err(context("audio stream"))?;
            let pcm = media_type()?;
            set(
                &pcm,
                "audio input",
                &[(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio), (&MF_MT_SUBTYPE, &MFAudioFormat_PCM)],
                &[
                    (&MF_MT_AUDIO_BITS_PER_SAMPLE, 16),
                    (&MF_MT_AUDIO_SAMPLES_PER_SECOND, sample_rate),
                    (&MF_MT_AUDIO_NUM_CHANNELS, CHANNELS),
                    (&MF_MT_AUDIO_BLOCK_ALIGNMENT, CHANNELS * 2),
                    (&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, sample_rate * CHANNELS * 2),
                ],
                &[],
            )?;
            writer.SetInputMediaType(audio, &pcm, None).map_err(context("audio input"))?;

            writer.BeginWriting().map_err(context("begin"))?;
            Ok(Self { writer, streams: (video, audio), width: s.width, height: s.height, fps: s.fps, sample_rate, audio_frames: 0 })
        }
    }
    #[cfg(not(windows))]
    pub fn create(_: &Path, _: &ExportSettings, _: u32) -> Result<Self, String> {
        Err(UNAVAILABLE.into())
    }

    #[cfg(windows)]
    fn sample(&self, bytes: &[u8], time: i64, duration: i64) -> Result<IMFSample, String> {
        let api = api().ok_or(UNAVAILABLE)?;
        // SAFETY: the buffer is locked for exactly `bytes.len()` bytes while we copy into it.
        unsafe {
            let buffer: IMFMediaBuffer = out("buffer", |p| (api.memory_buffer)(bytes.len() as u32, p))?;
            let mut data = std::ptr::null_mut();
            buffer.Lock(&mut data, None, None).map_err(context("buffer"))?;
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), data, bytes.len());
            buffer.Unlock().map_err(context("buffer"))?;
            buffer.SetCurrentLength(bytes.len() as u32).map_err(context("buffer"))?;
            let sample: IMFSample = out("sample", |p| (api.sample)(p))?;
            sample.AddBuffer(&buffer).map_err(context("sample"))?;
            sample.SetSampleTime(time).map_err(context("sample"))?;
            sample.SetSampleDuration(duration).map_err(context("sample"))?;
            Ok(sample)
        }
    }

    /// One frame of top-down RGBA at the writer's size; `frame` numbers its time.
    pub fn video(&mut self, rgba: &[u8], frame: u32) -> Result<(), String> {
        if rgba.len() != (self.width * self.height * 4) as usize {
            return Err(format!("frame is {} bytes, not {}x{}", rgba.len(), self.width, self.height));
        }
        let bgra = bgrx(rgba);
        let duration = UNITS / i64::from(self.fps);
        #[cfg(windows)]
        {
            let sample = self.sample(&bgra, i64::from(frame) * UNITS / i64::from(self.fps), duration)?;
            // SAFETY: the sample was built above and the stream index came from AddStream.
            unsafe { self.writer.WriteSample(self.streams.0, &sample) }.map_err(context("video sample"))?;
        }
        #[cfg(not(windows))]
        let _ = (bgra, duration, frame);
        Ok(())
    }

    /// Interleaved stereo f32 (-1..1).
    pub fn audio(&mut self, pcm_interleaved_f32: &[f32]) -> Result<(), String> {
        let frames = (pcm_interleaved_f32.len() / CHANNELS as usize) as u64;
        if frames == 0 {
            return Ok(());
        }
        let bytes: Vec<u8> =
            pcm_interleaved_f32.iter().flat_map(|v| ((v.clamp(-1., 1.) * 32767.) as i16).to_le_bytes()).collect();
        let rate = i64::from(self.sample_rate);
        let time = self.audio_frames as i64 * UNITS / rate;
        let duration = frames as i64 * UNITS / rate;
        self.audio_frames += frames;
        #[cfg(windows)]
        {
            let sample = self.sample(&bytes, time, duration)?;
            // SAFETY: as in `video`.
            unsafe { self.writer.WriteSample(self.streams.1, &sample) }.map_err(context("audio sample"))?;
        }
        #[cfg(not(windows))]
        let _ = (bytes, time, duration);
        Ok(())
    }

    pub fn finish(self) -> Result<(), String> {
        #[cfg(windows)]
        // SAFETY: finalising the writer this value owns.
        unsafe {
            self.writer.Finalize().map_err(context("finalise"))?;
        }
        Ok(())
    }
}

/// What a written file holds (tests): duration in seconds, video frames, frame size, audio peak (0..1).
#[cfg(all(test, windows))]
pub(crate) fn probe(path: &Path) -> Result<(f64, u32, (u32, u32), f32), String> {
    let api = api().ok_or(UNAVAILABLE)?;
    // SAFETY: a source reader over the file just written; every sample is released as it is read.
    unsafe {
        (api.startup)(MF_VERSION, MFSTARTUP_FULL).ok().map_err(context("startup"))?;
        let url = wide(path);
        let reader: IMFSourceReader =
            out("reader", |p| (api.source_reader)(PCWSTR(url.as_ptr()), std::ptr::null_mut(), p))?;
        let video = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
        let kind = reader.GetCurrentMediaType(video).map_err(context("reader type"))?;
        let size = kind.GetUINT64(&MF_MT_FRAME_SIZE).map_err(context("reader size"))?;
        let (mut frames, mut end) = (0u32, 0i64);
        loop {
            let (mut flags, mut time, mut sample) = (0u32, 0i64, None::<IMFSample>);
            reader.ReadSample(video, 0, None, Some(&mut flags), Some(&mut time), Some(&mut sample)).map_err(context("read"))?;
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                break;
            }
            if let Some(sample) = sample {
                frames += 1;
                end = end.max(time + sample.GetSampleDuration().unwrap_or(0));
            }
        }
        // The audio decoded back to 16-bit PCM: its peak says whether the track is silent.
        let audio = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
        let pcm: IMFMediaType = out("reader audio", |p| (api.media_type)(p))?;
        pcm.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio).map_err(context("reader audio"))?;
        pcm.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM).map_err(context("reader audio"))?;
        reader.SetCurrentMediaType(audio, None, &pcm).map_err(context("reader audio"))?;
        let mut peak = 0f32;
        loop {
            let (mut flags, mut sample) = (0u32, None::<IMFSample>);
            reader.ReadSample(audio, 0, None, Some(&mut flags), None, Some(&mut sample)).map_err(context("read audio"))?;
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                break;
            }
            let Some(buffer) = sample.and_then(|s| s.ConvertToContiguousBuffer().ok()) else { continue };
            let (mut data, mut len) = (std::ptr::null_mut(), 0u32);
            buffer.Lock(&mut data, None, Some(&mut len)).map_err(context("read audio"))?;
            let bytes = std::slice::from_raw_parts(data, len as usize);
            peak = bytes.chunks_exact(2).fold(peak, |p, b| p.max(f32::from(i16::from_le_bytes([b[0], b[1]])).abs() / 32768.));
            buffer.Unlock().map_err(context("read audio"))?;
        }
        Ok((end as f64 / UNITS as f64, frames, ((size >> 32) as u32, size as u32), peak))
    }
}

#[cfg(all(test, windows))]
mod tests {
    /// The DLL names a PE file imports at load time (its import directory; delay loads are a separate directory).
    pub(crate) fn imports(pe: &[u8]) -> Result<Vec<String>, String> {
        let u16_at = |at: usize| pe.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]])).ok_or("truncated");
        let u32_at = |at: usize| pe.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).ok_or("truncated");
        let nt = u32_at(0x3c)? as usize;
        if pe.get(nt..nt + 4) != Some(b"PE\0\0") {
            return Err("not a PE file".into());
        }
        let file = nt + 4;
        let sections = u16_at(file + 2)? as usize;
        let optional = file + 20;
        let optional_size = u16_at(file + 16)? as usize;
        // PE32+ data directory 1 (imports) sits at optional + 112 + 8.
        if u16_at(optional)? != 0x20b {
            return Err("not PE32+".into());
        }
        let import_rva = u32_at(optional + 120)? as usize;
        let section_table = optional + optional_size;
        let to_offset = |rva: usize| -> Result<usize, String> {
            for i in 0..sections {
                let s = section_table + i * 40;
                let (size, va, raw) = (u32_at(s + 8)? as usize, u32_at(s + 12)? as usize, u32_at(s + 20)? as usize);
                if (va..va + size.max(1)).contains(&rva) {
                    return Ok(rva - va + raw);
                }
            }
            Err(format!("rva {rva:#x} outside the sections"))
        };
        let mut names = Vec::new();
        let mut at = to_offset(import_rva)?;
        loop {
            let name_rva = u32_at(at + 12)? as usize;
            if name_rva == 0 {
                break;
            }
            let start = to_offset(name_rva)?;
            let end = pe[start..].iter().position(|&b| b == 0).ok_or("unterminated name")? + start;
            names.push(String::from_utf8_lossy(&pe[start..end]).to_ascii_lowercase());
            at += 20;
        }
        Ok(names)
    }

    /// RGB32 is B, G, R, X in memory: red and blue swap, the fourth byte is opaque.
    #[test]
    fn rgba_becomes_rgb32_byte_order() {
        assert_eq!(super::bgrx(&[1, 2, 3, 4, 5, 6, 7, 8]), [3, 2, 1, 255, 7, 6, 5, 255]);
    }

    /// Review C1: the exe this test runs in (the whole skate3rust binary plus its tests) imports no Media Foundation
    /// DLL at load time, so a Windows N machine without the Media Feature Pack can still start the game.
    #[test]
    fn import_table_has_no_media_foundation() {
        let exe = std::env::current_exe().unwrap();
        let names = imports(&std::fs::read(&exe).unwrap()).unwrap();
        assert!(names.iter().any(|n| n == "kernel32.dll"), "the parse found the import table: {names:?}");
        let mf: Vec<_> = names.iter().filter(|n| n.starts_with("mf")).collect();
        assert!(mf.is_empty(), "{} imports {mf:?}", exe.display());
        println!("{}: {} load-time imports, none from Media Foundation", exe.display(), names.len());
    }
}
