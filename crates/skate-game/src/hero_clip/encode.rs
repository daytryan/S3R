//! Raw RGBA frames -> the pinned ffmpeg (skate_data::audio::ffmpeg::program(), the same lookup audio uses) -> VP9 with
//! alpha. Writes <out>.partial.webm and renames it, so the launcher never reads half a file.
use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
};
pub const W: u32 = 480;
pub const H: u32 = 360;
pub const FPS: u32 = 30;

pub fn args(out: &Path) -> Vec<String> {
    let size = format!("{W}x{H}");
    let fps = FPS.to_string();
    [
        "-hide_banner", "-loglevel", "error", "-y", "-f", "rawvideo", "-pix_fmt", "rgba", "-s", &size, "-r", &fps, "-i", "-",
        "-c:v", "libvpx-vp9", "-pix_fmt", "yuva420p", "-b:v", "0", "-crf", "32", "-auto-alt-ref", "0", "-an",
    ]
    .iter()
    .map(|s| s.to_string())
    .chain([out.to_string_lossy().into_owned()])
    .collect()
}

pub struct Encoder {
    child: Child,
    partial: PathBuf,
    out: PathBuf,
}

impl Encoder {
    pub fn spawn(ffmpeg: &Path, out: &Path) -> Result<Self, String> {
        let partial = out.with_extension("partial.webm");
        let mut command = Command::new(ffmpeg);
        command.args(args(&partial)).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
        #[cfg(windows)]
        std::os::windows::process::CommandExt::creation_flags(&mut command, 0x0800_0000); // CREATE_NO_WINDOW
        let child = command.spawn().map_err(|e| format!("ffmpeg: {e}"))?;
        Ok(Self { child, partial, out: out.to_path_buf() })
    }

    pub fn push(&mut self, rgba: &[u8]) -> Result<(), String> {
        self.child.stdin.as_mut().ok_or("ffmpeg stdin closed")?.write_all(rgba).map_err(|e| format!("ffmpeg: {e}"))
    }

    pub fn finish(mut self) -> Result<(), String> {
        drop(self.child.stdin.take());
        let status = self.child.wait().map_err(|e| format!("ffmpeg: {e}"))?;
        if !status.success() {
            return Err(format!("ffmpeg exited with {status}"));
        }
        std::fs::rename(&self.partial, &self.out).map_err(|e| format!("rename: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ffmpeg_reads_raw_rgba_and_writes_vp9_with_alpha() {
        let a = args(std::path::Path::new("C:/x/hero.partial.webm"));
        let s = a.join(" ");
        assert!(s.contains("-f rawvideo -pix_fmt rgba -s 480x360 -r 30 -i -"));
        assert!(s.contains("-c:v libvpx-vp9 -pix_fmt yuva420p") && s.contains("-auto-alt-ref 0") && s.contains("-an"));
        assert_eq!(a.last().unwrap(), "C:/x/hero.partial.webm");
    }
}
