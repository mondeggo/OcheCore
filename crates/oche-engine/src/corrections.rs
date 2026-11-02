//! Debug entries for corrected darts: what the detector saw and what the
//! operator says is true, for later analysis and replay.
use oche_domain::*;
use std::{
    fs,
    path::{Path, PathBuf},
};

/// What the cameras saw when a dart was reported, kept with the visit.
#[derive(Debug, Clone, Default)]
pub struct ArrivalEvidence {
    /// The detector's impact on the board plane (mm, +y to 20), if placed.
    pub position: Option<[f64; 2]>,
    /// The detector's own description of the arrival and each camera's view.
    pub detection: String,
    /// Each camera's JPEG of the settled scene before and after the dart.
    pub before: Vec<(u8, Vec<u8>)>,
    pub after: Vec<(u8, Vec<u8>)>,
}

/// Everything an entry records besides the pictures.
pub struct Correction<'a> {
    pub snapshot: &'a Snapshot,
    pub dart: u8,
    pub detected: &'a Throw,
    pub corrected: &'a Throw,
    pub evidence: Option<&'a ArrivalEvidence>,
}

/// Writes `<debug>/<time>-dart<N>-<detected>-to-<corrected>/` with
/// `entry.json` and the cameras' pictures, then keeps the newest `keep`
/// entries (0 keeps all). The folder is written under a temporary name and
/// renamed, so an entry is complete or absent.
pub fn save(debug: &Path, correction: &Correction, keep: usize) -> std::io::Result<PathBuf> {
    let Correction {
        snapshot,
        dart,
        detected,
        corrected,
        evidence,
    } = correction;
    let created = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    let clean = |label: &str| -> String {
        label
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { 'X' })
            .collect()
    };
    let name = format!(
        "{created}-dart{dart}-{}-to-{}",
        clean(&detected.label),
        clean(&corrected.label)
    );
    fs::create_dir_all(debug)?;
    let partial = debug.join(format!(".{name}.partial"));
    fs::create_dir_all(&partial)?;
    let mut pictures = Vec::new();
    if let Some(evidence) = evidence {
        for (when, frames) in [("before", &evidence.before), ("after", &evidence.after)] {
            for (slot, jpeg) in frames {
                let file = format!("{when}-cam{slot}.jpg");
                fs::write(partial.join(&file), jpeg)?;
                pictures.push(file);
            }
        }
    }
    let entry = serde_json::json!({
        "schema_version": 1,
        "created_unix_ms": created,
        "app_version": env!("CARGO_PKG_VERSION"),
        "board": snapshot.config.board,
        "session_id": snapshot.session_id,
        "revision": snapshot.revision,
        "mode": snapshot.mode,
        "dart": dart,
        "detected": {
            "label": detected.label,
            "score": detected.score,
            "uncertain": detected.uncertain,
            "uncertainty": detected.uncertainty,
            "position_mm": detected.position,
            "synthetic": detected.synthetic,
        },
        "corrected": {
            "label": corrected.label,
            "score": corrected.score,
            "position_mm": corrected.position,
        },
        "visit": snapshot.throws.iter().map(|t| &t.label).collect::<Vec<_>>(),
        "detection": evidence.map(|e| e.detection.as_str()),
        "pictures": pictures,
        "pictures_note": "before: the settled scene before this dart; after: the settled scene the detector reported it in",
        "detection_settings": snapshot.config.detection,
        "cameras": snapshot.config.cameras,
        "camera_status": snapshot.cameras,
        "calibration": snapshot.calibration,
    });
    let json = serde_json::to_vec_pretty(&entry).map_err(std::io::Error::other)?;
    fs::write(partial.join("entry.json"), json)?;
    let path = debug.join(&name);
    fs::rename(&partial, &path)?;
    if keep > 0 {
        trim(debug, keep)?;
    }
    Ok(path)
}

/// Keeps the newest `keep` entries; names start with the time.
fn trim(debug: &Path, keep: usize) -> std::io::Result<()> {
    let mut entries: Vec<PathBuf> = fs::read_dir(debug)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.is_dir()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.as_bytes().first().is_some_and(u8::is_ascii_digit))
        })
        .collect();
    if entries.len() <= keep {
        return Ok(());
    }
    entries.sort();
    for old in &entries[..entries.len() - keep] {
        fs::remove_dir_all(old)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_newest_entries_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "100-dart1-S1-to-S2",
            "200-dart1-S1-to-S2",
            "300-dart1-S1-to-S2",
        ] {
            fs::create_dir(dir.path().join(name)).unwrap();
        }
        // Not an entry: never removed.
        fs::create_dir(dir.path().join("notes")).unwrap();
        trim(dir.path(), 2).unwrap();
        let mut left: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(left, ["200-dart1-S1-to-S2", "300-dart1-S1-to-S2", "notes"]);
    }
}
