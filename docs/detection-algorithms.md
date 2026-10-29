# Detection algorithms to evaluate

Status: proposals, not implemented detectors. Native controls and requested camera
settings work today; acquisition, calibration, and live scoring remain deferred.
No algorithm below has been verified on this board's cameras or recordings.

## Recommended first baseline: frame difference and multiple views

For two to four fixed cameras, start with this proposed pipeline:

1. Retain an empty-board reference and the last accepted stable scene separately.
2. Compare low-resolution luminance frames to detect motion; require stabilization
   over elapsed time before examining a new arrival at full resolution.
3. Subtract the stable pre-arrival image, threshold the difference, clean small
   components, and select elongated changed regions. Reject broad hand occlusion
   and large lighting changes rather than accepting a dart.
4. Fit a shaft line robustly (RANSAC with a final least-squares refinement), or
   compare against Canny edges plus probabilistic Hough line segments. Hough
   extracts line candidates; its segment endpoints are not automatically dart tips.
   See [OpenCV's line-transform reference](https://docs.opencv.org/4.x/d9/db0/tutorial_hough_lines.html).
5. After lens correction and per-camera board calibration, map each observed
   shaft line into a constraint on the board plane. Find the best common impact
   from distinct views using weighted residuals. Near-parallel constraints or
   disagreement require abstention, not an arbitrary score.
6. Convert a valid board-plane impact into radius/angle and the scoring region.
   Report uncertainty near wires. Preserve already accepted darts during a new
   arrival and confirm physical removal against the empty-board reference.

This is an engineering proposal chosen for explainable intermediate outputs and
bounded processing. CPU cost, latency, acceptance thresholds, and accuracy require
measurements. A single image of a shaft only constrains the impact to a line;
one-camera operation needs direct evidence of the actual entry point.

## Alternative foreground model: MOG2 or KNN

MOG2 and KNN maintain adaptive backgrounds and produce foreground masks for fixed
cameras. They can be compared with plain frame differences when illumination
changes. [OpenCV background-subtraction reference](https://docs.opencv.org/4.12.0/d1/dc5/tutorial_background_subtraction.html).

For OcheCore, evaluate them as alternative motion/foreground stages, not complete
scorers. Control adaptation during arrivals and hand occlusion, and keep a separate
occupancy reference: an adaptive model absorbing a stationary dart must not imply
that the dart was removed. Measure memory and CPU before choosing either method.

## Learned tip localization: a DeepDarts-style model

DeepDarts predicts dart landing positions and board calibration keypoints, then
maps them to a board plane for scoring. Its published method uses YOLOv4-tiny.
See the [authors' paper](https://arxiv.org/abs/2105.09880).

For this project, consider a compact tip/keypoint model after collecting labeled
images from the actual camera angles, lighting, and darts. It could assist a
single camera with a visible entry point, or difficult candidates in the classical
pipeline. It still needs geometric validation, temporal association, uncertainty
handling, and removal logic. Published dataset performance is not evidence of
accuracy on this installation. Bundle any chosen model for offline operation;
review its code, model, and training-data licenses before distribution.

## Parameters and verification before exposing detector choices

Future calibration should establish lens distortion, board-plane mapping,
orientation, and verification residuals for each camera. Future detection tuning
may include a board mask, change threshold, stabilization duration, component
size/elongation, line-fit residual, cross-view agreement, and wire uncertainty.
Choose ranges from recordings; do not add functioning-looking UI controls before
these parameters affect a real detector.

Compare candidates on independently labeled held-out sessions: near-wire impacts,
grouped darts, shadows, slow removal, partial takeout, bouncing/falling darts,
camera loss, and board vibration. Record wrong scores, abstentions, duplicate
arrivals, false takeout, latency, CPU, and memory separately. Synthetic tests check
math and state transitions; they do not establish live scoring accuracy.
