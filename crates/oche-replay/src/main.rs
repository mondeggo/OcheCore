//! Offline tools for recorded sessions. Development use only; not shipped.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use oche_calibration::Pixel;
use oche_calibration::detect::{Detection, detect};
use oche_domain::board::{BoardDimensions, BoardPoint};
use oche_replay::{open_session, png};
use oche_vision::image::RgbImage;

const WIRE: [u8; 3] = [255, 230, 0];
const ANCHOR: [u8; 3] = [0, 255, 255];
const TWENTY: [u8; 3] = [255, 0, 255];

/// Draws the fitted board model over the frame: rings and radial wires in
/// yellow, detected anchors in cyan, and a magenta dot in the middle of S20.
fn overlay(mut image: RgbImage, detection: &Detection, board: &BoardDimensions) -> RgbImage {
    let mapping = &detection.fit.homography;
    let mut plot = |point: BoardPoint, colour: [u8; 3]| {
        if let Some(p) = mapping.to_image(point) {
            image.set(p.x.round() as i64, p.y.round() as i64, colour);
        }
    };
    for radius in board.radii() {
        for step in 0..3600 {
            plot(
                BoardPoint::polar_cw_from_top(radius, step as f64 / 10.0),
                WIRE,
            );
        }
    }
    for boundary in 0..20 {
        let degrees = 18.0 * (boundary as f64 + 0.5);
        let mut radius = board.outer_bull;
        while radius <= board.double_outer {
            plot(BoardPoint::polar_cw_from_top(radius, degrees), WIRE);
            radius += 0.25;
        }
    }
    for dy in -40..=40 {
        for dx in -40..=40 {
            if dx * dx + dy * dy <= 1600 {
                plot(
                    BoardPoint::new(f64::from(dx) / 10.0, 135.0 + f64::from(dy) / 10.0),
                    TWENTY,
                );
            }
        }
    }
    for anchor in &detection.anchors {
        let p = anchor.correspondence.image;
        for d in -3..=3 {
            image.set(p.x.round() as i64 + d, p.y.round() as i64, ANCHOR);
            image.set(p.x.round() as i64, p.y.round() as i64 + d, ANCHOR);
        }
    }
    image
}

#[derive(Parser)]
#[command(name = "oche-replay", about = "Inspect recorded OcheCore sessions")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List cameras, frame counts, drops, and durations.
    Info { session: PathBuf },
    /// Decode one frame per camera to PNG.
    Frame {
        session: PathBuf,
        /// Driver sequence number; the nearest recorded frame is used.
        #[arg(long, default_value_t = 1)]
        seq: u64,
        #[arg(long)]
        out: PathBuf,
    },
    /// Detect ring anchors per camera, fit board mappings, and write overlays.
    Calibrate {
        session: PathBuf,
        #[arg(long, default_value_t = 1)]
        seq: u64,
        /// Pixel near the 20 segment per camera, as camera=x,y.
        #[arg(long = "hint", required = true, value_parser = parse_hint)]
        hints: Vec<(u8, (f64, f64))>,
        #[arg(long)]
        out: PathBuf,
    },
    /// Print RGB values of one camera's frame at the given pixels.
    Probe {
        session: PathBuf,
        #[arg(long)]
        camera: u8,
        #[arg(long, default_value_t = 1)]
        seq: u64,
        /// Pixels as x,y.
        #[arg(required = true, value_parser = parse_pixel)]
        pixels: Vec<(f64, f64)>,
    },
}

fn parse_pixel(text: &str) -> Result<(f64, f64), String> {
    let (x, y) = text.split_once(',').ok_or("use x,y")?;
    Ok((
        x.trim().parse().map_err(|_| "invalid x")?,
        y.trim().parse().map_err(|_| "invalid y")?,
    ))
}

fn parse_hint(text: &str) -> Result<(u8, (f64, f64)), String> {
    let (camera, pixel) = text.split_once('=').ok_or("use camera=x,y")?;
    Ok((
        camera.trim().parse().map_err(|_| "invalid camera")?,
        parse_pixel(pixel)?,
    ))
}

fn decode(session: &std::path::Path, camera: u8, seq: u64) -> Result<RgbImage> {
    let recording = open_session(session)?
        .into_iter()
        .find(|c| c.camera == camera)
        .with_context(|| format!("no cam{camera} in session"))?;
    let frame = recording.nearest(seq).context("empty recording")?;
    RgbImage::decode_jpeg(&recording.read(frame)?)
        .map_err(anyhow::Error::msg)
        .with_context(|| format!("decoding cam{camera} seq {}", frame.seq))
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Info { session } => {
            for camera in open_session(&session)? {
                let (first, last) = (camera.frames.first(), camera.frames.last());
                let duration = first
                    .zip(last)
                    .map_or(0.0, |(a, b)| b.timestamp - a.timestamp);
                println!(
                    "cam{}: {} frames, {} dropped, {duration:.2} s",
                    camera.camera,
                    camera.frames.len(),
                    camera.dropped()
                );
            }
        }
        Command::Frame { session, seq, out } => {
            fs::create_dir_all(&out)?;
            for camera in open_session(&session)? {
                let frame = camera.nearest(seq).context("empty recording")?;
                let image = RgbImage::decode_jpeg(&camera.read(frame)?)
                    .map_err(anyhow::Error::msg)
                    .with_context(|| format!("decoding cam{} seq {}", camera.camera, frame.seq))?;
                let path = out.join(format!("cam{}-seq{}.png", camera.camera, frame.seq));
                fs::write(&path, png::encode(&image))?;
                println!("{}", path.display());
            }
        }
        Command::Calibrate {
            session,
            seq,
            hints,
            out,
        } => {
            fs::create_dir_all(&out)?;
            let board = BoardDimensions::wdf();
            let mut report = Vec::new();
            for (camera, (x, y)) in hints {
                let image = decode(&session, camera, seq)?;
                match detect(&image, Pixel::new(x, y), &board) {
                    Ok(detection) => {
                        let fit = &detection.fit.residuals;
                        let holdout = &detection.treble_holdout;
                        println!(
                            "cam{camera}: {} anchors, fit rms {:.2} mm max {:.2} mm; \
                             treble holdout rms {:.2} mm max {:.2} mm",
                            detection.anchors.len(),
                            fit.rms_mm,
                            fit.max_mm,
                            holdout.rms_mm,
                            holdout.max_mm
                        );
                        let path = out.join(format!("cam{camera}-overlay.png"));
                        fs::write(&path, png::encode(&overlay(image, &detection, &board)))?;
                        println!("  {}", path.display());
                        report.push(serde_json::json!({
                            "camera": camera,
                            "centre": [detection.centre.x, detection.centre.y],
                            "fit_rms_mm": fit.rms_mm,
                            "fit_max_mm": fit.max_mm,
                            "treble_holdout_rms_mm": holdout.rms_mm,
                            "treble_holdout_max_mm": holdout.max_mm,
                            "anchors": detection.anchors.iter().zip(&fit.per_point_mm).map(|(a, r)| serde_json::json!({
                                "name": a.name,
                                "image": [a.correspondence.image.x, a.correspondence.image.y],
                                "board": [a.correspondence.board.x, a.correspondence.board.y],
                                "residual_mm": r,
                                "mapped_radius_mm": detection.fit.homography.to_board(a.correspondence.image).map(|m| m.radius()),
                            })).collect::<Vec<_>>(),
                        }));
                    }
                    Err(error) => {
                        println!("cam{camera}: {error}");
                        report.push(
                            serde_json::json!({"camera": camera, "error": error.to_string()}),
                        );
                    }
                }
            }
            let path = out.join("calibration.json");
            fs::write(&path, serde_json::to_vec_pretty(&report)?)?;
            println!("{}", path.display());
        }
        Command::Probe {
            session,
            camera,
            seq,
            pixels,
        } => {
            let image = decode(&session, camera, seq)?;
            for (x, y) in pixels {
                println!("{x},{y}: {:?}", image.sample(x, y));
            }
        }
    }
    Ok(())
}
