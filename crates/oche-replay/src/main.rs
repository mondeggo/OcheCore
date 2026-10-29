//! Offline tools for recorded sessions. Development use only; not shipped.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use oche_calibration::Pixel;
use oche_calibration::detect::detect;
use oche_calibration::{Homography, refine};
use oche_domain::board::{BoardDimensions, BoardPoint};
use oche_replay::{darts, open_session, png};
use oche_vision::image::RgbImage;

const WIRE: [u8; 3] = [255, 230, 0];
const ANCHOR: [u8; 3] = [0, 255, 255];
const TWENTY: [u8; 3] = [255, 0, 255];

/// Draws the measured board model over the frame: wire centres in yellow,
/// the wire points used by the fit in cyan, and a magenta dot in the middle
/// of S20.
fn overlay(
    mut image: RgbImage,
    mapping: &Homography,
    refinement: &refine::Refinement,
    view: usize,
    board: &BoardDimensions,
) -> RgbImage {
    for point in refinement.points.iter().filter(|p| p.view == view) {
        image.set(
            point.pixel.x.round() as i64,
            point.pixel.y.round() as i64,
            ANCHOR,
        );
    }
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
    /// Exploratory dart detection using a calibration report from `calibrate`.
    Darts {
        session: PathBuf,
        #[arg(long)]
        calibration: PathBuf,
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
            let nominal = BoardDimensions::wdf();
            let mut detected = Vec::new();
            for (camera, (x, y)) in hints {
                let image = decode(&session, camera, seq)?;
                match detect(&image, Pixel::new(x, y), &nominal) {
                    Ok(detection) => {
                        let fit = &detection.fit.residuals;
                        println!(
                            "cam{camera}: colour detection {} anchors, fit rms {:.2} mm",
                            detection.anchors.len(),
                            fit.rms_mm
                        );
                        detected.push((camera, image, detection));
                    }
                    Err(error) => println!("cam{camera}: colour detection failed: {error}"),
                }
            }
            anyhow::ensure!(!detected.is_empty(), "no camera could be detected");

            // Measure the board's wire radii and refine every camera together,
            // then each camera alone as a consistency check on the radii.
            let grays: Vec<_> = detected.iter().map(|(_, image, _)| image.luma()).collect();
            let views: Vec<_> = grays
                .iter()
                .zip(&detected)
                .map(|(gray, (_, _, d))| (gray, d.fit.homography))
                .collect();
            let settings = refine::Settings::default();
            let joint = refine::refine(&views, &nominal, &settings).map_err(anyhow::Error::msg)?;
            let format_radii = |radii: [f64; 6]| {
                radii
                    .iter()
                    .map(|r| format!("{r:7.2}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            println!(
                "ring radii (mm):    {}",
                refine::RINGS
                    .map(|n| format!("{:>7}", &n[..n.len().min(7)]))
                    .join(" ")
            );
            println!("  WDF nominal       {}", format_radii(nominal.radii()));
            println!("  measured, joint   {}", format_radii(joint.board.radii()));
            let mut singles = Vec::new();
            for (i, (camera, _, _)) in detected.iter().enumerate() {
                match refine::refine(&views[i..i + 1], &nominal, &settings) {
                    Ok(single) => {
                        println!(
                            "  cam{camera} alone       {}",
                            format_radii(single.board.radii())
                        );
                        singles.push(serde_json::json!({"camera": camera, "board": single.board}));
                    }
                    Err(error) => println!("  cam{camera} alone: {error}"),
                }
            }
            let mut cameras = Vec::new();
            for (i, (camera, image, detection)) in detected.into_iter().enumerate() {
                let fit = &joint.views[i];
                println!(
                    "cam{camera}: wire rms by ring {} mm ({} points); radial rms {:.2} mm ({} points)",
                    fit.ring_rms_mm
                        .iter()
                        .map(|r| format!("{r:.2}"))
                        .collect::<Vec<_>>()
                        .join("/"),
                    fit.ring_points.iter().sum::<usize>(),
                    fit.radial_rms_mm,
                    fit.radial_points
                );
                let mapping = joint.homographies[i];
                let path = out.join(format!("cam{camera}-overlay.png"));
                fs::write(
                    &path,
                    png::encode(&overlay(image, &mapping, &joint, i, &joint.board)),
                )?;
                println!("  {}", path.display());
                cameras.push(serde_json::json!({
                    "camera": camera,
                    "homography": mapping,
                    "colour_homography": detection.fit.homography,
                    "colour_fit_rms_mm": detection.fit.residuals.rms_mm,
                    "wire_ring_rms_mm": fit.ring_rms_mm,
                    "wire_ring_points": fit.ring_points,
                    "wire_radial_rms_mm": fit.radial_rms_mm,
                    "wire_radial_points": fit.radial_points,
                }));
            }
            let report = serde_json::json!({
                "board": joint.board,
                "scale": format!("{} fixed at {} mm", refine::RINGS[settings.fixed_ring], nominal.radii()[settings.fixed_ring]),
                "single_camera_boards": singles,
                "cameras": cameras,
            });
            let path = out.join("calibration.json");
            fs::write(&path, serde_json::to_vec_pretty(&report)?)?;
            println!("{}", path.display());
        }
        Command::Darts {
            session,
            calibration,
            out,
        } => {
            fs::create_dir_all(&out)?;
            let report: serde_json::Value = serde_json::from_slice(&fs::read(&calibration)?)?;
            let board: BoardDimensions = serde_json::from_value(report["board"].clone())
                .context("calibration report lacks a board")?;
            let mut homographies = std::collections::BTreeMap::new();
            for entry in report["cameras"]
                .as_array()
                .context("calibration report lacks cameras")?
            {
                if let (Some(camera), Some(h)) = (entry["camera"].as_u64(), entry.get("homography"))
                {
                    homographies.insert(camera as u8, serde_json::from_value(h.clone())?);
                }
            }
            let cameras = open_session(&session)?;
            let settings = darts::Settings::default();
            let (stills, events) = darts::analyse(&cameras, &homographies, &settings)?;
            let origin = cameras[0].frames[0].timestamp;
            println!("{} still periods:", stills.len());
            for s in &stills {
                println!("  {:6.2}-{:6.2} s", s.start - origin, s.end - origin);
            }
            for (n, event) in events.iter().enumerate() {
                let kind = if event.is_arrival() {
                    "arrival"
                } else {
                    "removal/other"
                };
                println!(
                    "event {n}: {:.2}->{:.2} s {kind}",
                    event.before.end - origin,
                    event.after.start - origin
                );
                for c in &event.cameras {
                    let region = c.region.map_or("no region".into(), |(count, axis)| {
                        format!(
                            "region {count} px, spread {:.1}/{:.1}",
                            axis.spread.0, axis.spread.1
                        )
                    });
                    let region = match &c.shaft {
                        Some(s) => format!(
                            "{region}, shaft {} slices rms {:.2} px",
                            s.midpoints.len(),
                            s.rms_px
                        ),
                        None => format!("{region}, no shaft"),
                    };
                    println!(
                        "  cam{}: {} changed px, arrival {:+.1}, {region}, line {}",
                        c.camera,
                        c.changed_pixels,
                        c.arrival_score,
                        if c.line.is_some() { "yes" } else { "no" }
                    );
                }
                let fused = darts::fuse(event, &settings);
                let impact = fused.as_ref().ok().map(|i| i.point);
                match &fused {
                    Ok(i) => println!(
                        "  impact {}; sigma {:.1}/{:.1} mm; cameras {:?} rejected {:?} checked {}",
                        darts::describe(i.point, &board),
                        i.semi_axes_mm.0,
                        i.semi_axes_mm.1,
                        i.cameras,
                        i.rejected_cameras,
                        i.consistency_checked
                    ),
                    Err(a) => println!("  abstained: {a:?}"),
                }
                if let Some(point) = impact {
                    for line in event.lines() {
                        println!(
                            "    cam{} line residual {:.1} mm",
                            line.camera,
                            line.line.distance(point)
                        );
                    }
                }
                if event.is_arrival() {
                    for c in &event.cameras {
                        let image = darts::overlay(c, homographies.get(&c.camera), impact);
                        fs::write(
                            out.join(format!("event{n:02}-cam{}.png", c.camera)),
                            png::encode(&image),
                        )?;
                    }
                }
            }
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
