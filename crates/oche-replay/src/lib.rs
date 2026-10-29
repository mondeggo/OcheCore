//! Recorded-session access and debug output for offline evaluation.
//!
//! A session directory holds, per camera `N`, `camN.mjpeg` (concatenated JPEG
//! frames as dequeued) and `camN.log` (`v4l2-ctl --verbose` output with one
//! `seq:`/`bytesused:`/`ts:` line per frame). This is the provisional format of
//! the recordings made during discovery, not a stable interchange format.

pub mod darts;
pub mod png;

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameIndex {
    /// Driver sequence number; gaps mean dropped frames.
    pub seq: u64,
    /// Kernel monotonic timestamp of the start of exposure, in seconds.
    pub timestamp: f64,
    pub offset: usize,
    pub len: usize,
}

#[derive(Debug)]
pub struct CameraRecording {
    pub camera: u8,
    pub frames: Vec<FrameIndex>,
    stream: PathBuf,
}

impl CameraRecording {
    pub fn open(dir: &Path, camera: u8) -> Result<Self> {
        let log_path = dir.join(format!("cam{camera}.log"));
        let stream = dir.join(format!("cam{camera}.mjpeg"));
        let log = fs::read_to_string(&log_path)
            .with_context(|| format!("reading {}", log_path.display()))?;
        let frames = parse_log(&log)?;
        let expected = frames.last().map_or(0, |f| f.offset + f.len);
        let actual = fs::metadata(&stream)
            .with_context(|| format!("reading {}", stream.display()))?
            .len();
        ensure!(
            actual == expected as u64,
            "{} has {actual} bytes but its log describes {expected}",
            stream.display()
        );
        Ok(Self {
            camera,
            frames,
            stream,
        })
    }

    /// Frame whose sequence number is closest to `seq`.
    pub fn nearest(&self, seq: u64) -> Option<&FrameIndex> {
        self.frames.iter().min_by_key(|f| f.seq.abs_diff(seq))
    }

    pub fn read(&self, frame: &FrameIndex) -> Result<Vec<u8>> {
        use std::io::{Read, Seek, SeekFrom};
        let mut file = fs::File::open(&self.stream)?;
        file.seek(SeekFrom::Start(frame.offset as u64))?;
        let mut bytes = vec![0; frame.len];
        file.read_exact(&mut bytes)?;
        Ok(bytes)
    }

    pub fn dropped(&self) -> u64 {
        self.frames
            .windows(2)
            .map(|pair| pair[1].seq.saturating_sub(pair[0].seq + 1))
            .sum()
    }
}

/// Opens every `camN.log` in a session directory, ordered by camera number.
pub fn open_session(dir: &Path) -> Result<Vec<CameraRecording>> {
    let mut cameras = Vec::new();
    for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let name = entry?.file_name();
        let name = name.to_string_lossy();
        if let Some(number) = name
            .strip_prefix("cam")
            .and_then(|rest| rest.strip_suffix(".log"))
            .and_then(|n| n.parse::<u8>().ok())
        {
            cameras.push(CameraRecording::open(dir, number)?);
        }
    }
    ensure!(
        !cameras.is_empty(),
        "no camN.log files in {}",
        dir.display()
    );
    cameras.sort_by_key(|c| c.camera);
    Ok(cameras)
}

fn parse_log(log: &str) -> Result<Vec<FrameIndex>> {
    let mut frames: Vec<FrameIndex> = Vec::new();
    let mut offset = 0;
    for (number, line) in log.lines().enumerate() {
        if !line.contains("bytesused:") {
            continue;
        }
        let field = |key: &str| -> Result<&str> {
            let mut words = line.split_whitespace();
            words.find(|w| *w == key);
            words
                .next()
                .with_context(|| format!("log line {} lacks {key}", number + 1))
        };
        let seq: u64 = field("seq:")?.parse()?;
        let len: usize = field("bytesused:")?.parse()?;
        let timestamp: f64 = field("ts:")?.parse()?;
        if let Some(previous) = frames.last()
            && seq <= previous.seq
        {
            bail!("log line {}: sequence {seq} is not increasing", number + 1);
        }
        frames.push(FrameIndex {
            seq,
            timestamp,
            offset,
            len,
        });
        offset += len;
    }
    Ok(frames)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = "\
cap dqbuf: 0 seq:      1 bytesused: 10 ts: 100.000000 field: None (ts-monotonic, ts-src-soe)
cap dqbuf: 1 seq:      2 bytesused: 20 ts: 100.033000 delta: 33.000 ms field: None
<<<<
cap dqbuf: 2 seq:      5 bytesused: 5 ts: 100.133000 delta: 100.000 ms fps: 29.98 field: None
";

    #[test]
    fn log_lines_index_contiguous_frames_and_count_drops() {
        let frames = parse_log(LOG).unwrap();
        assert_eq!(frames.len(), 3);
        assert_eq!((frames[1].offset, frames[1].len), (10, 20));
        assert_eq!(frames[2].offset, 30);
        assert!((frames[2].timestamp - 100.133).abs() < 1e-9);

        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        fs::write(dir.join("cam2.log"), LOG).unwrap();
        fs::write(dir.join("cam2.mjpeg"), [7u8; 35]).unwrap();
        let session = open_session(dir).unwrap();
        assert_eq!(session[0].camera, 2);
        assert_eq!(session[0].dropped(), 2);
        assert_eq!(session[0].nearest(4).unwrap().seq, 5);
        assert_eq!(session[0].read(&session[0].frames[2]).unwrap(), vec![7; 5]);

        fs::write(dir.join("cam2.mjpeg"), [7u8; 34]).unwrap();
        assert!(
            open_session(dir).is_err(),
            "truncated stream must be rejected"
        );
    }

    #[test]
    fn malformed_logs_are_rejected() {
        assert!(parse_log("cap dqbuf: 0 seq: 1 bytesused: x ts: 1.0").is_err());
        assert!(parse_log("cap dqbuf: 0 seq: 1 bytesused: 3").is_err());
        let repeated = "seq: 2 bytesused: 1 ts: 1.0\nseq: 2 bytesused: 1 ts: 1.1";
        assert!(parse_log(repeated).is_err());
    }
}
