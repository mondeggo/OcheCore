"use strict";

/* OcheCore's browser UI, served by the daemon on its own port. The daemon is
   the only source of truth: pages render the snapshots it streams on
   /api/v1/events and change it only through /api/v1 commands and config
   changes (docs/native-api.md). localStorage keeps per-browser conveniences. */

const SEGMENTS = [20, 1, 18, 4, 13, 6, 10, 15, 2, 17, 3, 19, 7, 16, 8, 11, 14, 9, 12, 5];
/* Nominal radii of wire centres in millimetres, like BoardDimensions::wdf in
   oche-domain: rings 9.7 mm wide between wire centres. */
const WDF_BOARD = {
  inner_bull: 6.35,
  outer_bull: 15.9,
  treble_inner: 97.7,
  treble_outer: 107.4,
  double_inner: 160.3,
  double_outer: 170
};
/* The radii boards are drawn and clicks scored with: the calibration's
   measured board when there is one, as the detector scores, else WDF. */
const R = {};

/* Returns whether the radii changed, so drawn boards can be redrawn. */
function useBoard(board = WDF_BOARD) {
  const next = {
    bull: board.inner_bull,
    outerBull: board.outer_bull,
    tripleIn: board.treble_inner,
    tripleOut: board.treble_outer,
    doubleIn: board.double_inner,
    doubleOut: board.double_outer
  };
  const changed = Object.keys(next).some(key => next[key] !== R[key]);
  Object.assign(R, next);
  return changed;
}
useBoard();

/* The drawn board's dark rim, just outside the numbers; boards are drawn in
   a 448 mm square around it. */
const SURROUND = 222;
const boardRadii = () => [R.bull, R.outerBull, R.tripleIn, R.tripleOut, R.doubleIn, R.doubleOut];
/* High-contrast board colours: near-black and warm cream singles,
   saturated red and green rings. */
const COLORS = {
  dark: "#07090b",
  light: "#ecd5c4",
  red: "#e3263f",
  green: "#00975a",
  surround: "#131a17",
  wire: "#a7b8bf",
  number: "#fbf4e4",
  lime: "#ddf56f",
  yellow: "#ffd463",
  ink: "#17261f"
};
const SVG_NS = "http://www.w3.org/2000/svg";
const DART_COLORS = ["#ffd463", "#8bdef4", "#ff7b66"];
/* Values accepted by detection.speed and detection.standby_seconds. */
const SPEEDS = [
  ["very_low", "Very low"],
  ["low", "Low"],
  ["default", "Default"],
  ["high", "High"],
  ["very_high", "Very high"]
];
const STANDBY = [
  [0, "Never"],
  [300, "5 minutes"],
  [600, "10 minutes"],
  [900, "15 minutes"],
  [1800, "30 minutes"],
  [3600, "1 hour"]
];
const MAX_CAMERAS = 4;
/* One camera sees a dart only as a line; scoring needs lines from two
   (MIN_SCORING_CAMERAS in oche-domain). Setup never goes below this. */
const MIN_CAMERAS = 2;
/* Where the daemon looks for the 20 when it has only a calibration
   (hint_for in oche-engine): the outer single of 20. */
const TWENTY_HINT_MM = 140;
const VIEW_NAMES = {
  1: ["MAIN VIEW"],
  2: ["LEFT VIEW", "RIGHT VIEW"],
  3: ["LEFT VIEW", "CENTRE VIEW", "RIGHT VIEW"],
  4: ["LEFT VIEW", "TOP VIEW", "RIGHT VIEW", "BOTTOM VIEW"]
};
const ICONS = {
  rotate: '<path d="M20 11a8 8 0 1 0-2.34 5.66M20 4v7h-7"/>',
  refresh: '<path d="M4 12a8 8 0 0 1 13.66-5.66L20 8m0-5v5h-5M20 12a8 8 0 0 1-13.66 5.66L4 16m0 5v-5h5"/>',
  remove: '<path d="M6 6l12 12M18 6 6 18"/>',
  /* Turning, not sideways: a camera may show the 20 anywhere, so a left or
     right arrow would point the wrong way for some marks. */
  clockwise: '<path d="M20 12a8 8 0 1 1-2.34-5.66L20 8"/><path d="M20 3v5h-5"/>',
  anticlockwise: '<path d="M4 12a8 8 0 1 0 2.34-5.66L4 8"/><path d="M4 3v5h5"/>'
};
const icon = name =>
  `<svg viewBox="0 0 24 24" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${ICONS[name]}</svg>`;

/* Per-browser conveniences only; board state always comes from the daemon. */
function makeStorage(area) {
  const store = () => window[area];
  return {
    get(key, fallback) {
      try {
        const value = store().getItem(`oche-studio.${key}`);
        return value ? JSON.parse(value) : fallback;
      } catch {
        return fallback;
      }
    },
    set(key, value) {
      try {
        store().setItem(`oche-studio.${key}`, JSON.stringify(value));
      } catch {
        /* Storage can be disabled; the pages still work without it. */
      }
    },
    remove(key) {
      try {
        store().removeItem(`oche-studio.${key}`);
      } catch {
        /* Nothing stored, nothing to remove. */
      }
    }
  };
}
const storage = makeStorage("localStorage");
const session = makeStorage("sessionStorage");

const sleep = ms => new Promise(resolve => window.setTimeout(resolve, ms));

/* ---------- Drawing ---------- */

function svgEl(tag, attrs = {}, parent) {
  const node = document.createElementNS(SVG_NS, tag);
  Object.entries(attrs).forEach(([key, value]) => node.setAttribute(key, value));
  if (parent) parent.appendChild(node);
  return node;
}

function polar(radius, degrees) {
  const angle = degrees * Math.PI / 180;
  return [radius * Math.cos(angle), radius * Math.sin(angle)];
}

function circlePoints(radius, steps = 96) {
  return Array.from({ length: steps }, (_, index) => polar(radius, index * 360 / steps));
}

function sectorPoints(inner, outer, start, end, steps = 6) {
  const points = [];
  for (let index = 0; index <= steps; index += 1) {
    points.push(polar(outer, start + (end - start) * index / steps));
  }
  for (let index = steps; index >= 0; index -= 1) {
    points.push(polar(inner, start + (end - start) * index / steps));
  }
  return points;
}

const round1 = value => value.toFixed(1);
const pointsAttr = points => points.map(([x, y]) => `${round1(x)},${round1(y)}`).join(" ");
const pathOf = (points, close = false) =>
  `M${points.map(([x, y]) => `${round1(x)} ${round1(y)}`).join("L")}${close ? "Z" : ""}`;
const identity = point => point;

function drawBoard(parent, map = identity, options = {}) {
  const { numbers = true, surround = true, numberSize = 20 } = options;
  const group = svgEl("g", {}, parent);
  const polygon = (points, fill) =>
    svgEl("polygon", { points: pointsAttr(points.map(map)), fill }, group);

  if (surround) polygon(circlePoints(SURROUND), COLORS.surround);

  SEGMENTS.forEach((number, index) => {
    const start = -99 + index * 18;
    const end = start + 18;
    const single = index % 2 === 0 ? COLORS.dark : COLORS.light;
    const ring = index % 2 === 0 ? COLORS.red : COLORS.green;
    polygon(sectorPoints(R.outerBull, R.tripleIn, start, end), single);
    polygon(sectorPoints(R.tripleIn, R.tripleOut, start, end, 3), ring);
    polygon(sectorPoints(R.tripleOut, R.doubleIn, start, end), single);
    polygon(sectorPoints(R.doubleIn, R.doubleOut, start, end, 3), ring);
  });

  polygon(circlePoints(R.outerBull, 36), COLORS.green);
  polygon(circlePoints(R.bull, 24), COLORS.red);
  drawWires(group, map, { opacity: 0.72 });

  if (numbers) {
    SEGMENTS.forEach((number, index) => {
      const [x, y] = map(polar(199, -90 + index * 18));
      const label = svgEl("text", {
        x: round1(x),
        y: round1(y),
        "text-anchor": "middle",
        "dominant-baseline": "central",
        fill: COLORS.number,
        "font-size": numberSize,
        "font-weight": 800,
        "font-family": "Arial Rounded MT Bold, Trebuchet MS, sans-serif"
      }, group);
      label.textContent = number;
    });
  }
  return group;
}

function drawWires(parent, map = identity, attrs = {}, radii = boardRadii()) {
  const group = svgEl("g", {
    fill: "none",
    stroke: COLORS.wire,
    "stroke-width": 0.75,
    "stroke-linejoin": "round",
    ...attrs
  }, parent);

  radii.forEach(radius => {
    svgEl("path", { d: pathOf(circlePoints(radius).map(map), true) }, group);
  });

  for (let index = 0; index < 20; index += 1) {
    const angle = -99 + index * 18;
    svgEl("path", {
      d: pathOf([
        polar(radii[1], angle),
        polar(radii[5], angle)
      ].map(map))
    }, group);
  }
  return group;
}

/* ---------- Calibration geometry ---------- */

/* A calibration's to_board maps image pixels to board millimetres with +y
   toward the 20; the drawings use SVG board coordinates with +y down. */
function invert3([[a, b, c], [d, e, f], [g, h, i]]) {
  const A = e * i - f * h;
  const B = -(d * i - f * g);
  const C = d * h - e * g;
  const det = a * A + b * B + c * C;
  if (!Number.isFinite(det) || Math.abs(det) < 1e-12) return null;
  return [
    [A / det, -(b * i - c * h) / det, (b * f - c * e) / det],
    [B / det, (a * i - c * g) / det, -(a * f - c * d) / det],
    [C / det, -(a * h - b * g) / det, (a * e - b * d) / det]
  ];
}

function project(m, [x, y]) {
  const w = m[2][0] * x + m[2][1] * y + m[2][2];
  return [(m[0][0] * x + m[0][1] * y + m[0][2]) / w, (m[1][0] * x + m[1][1] * y + m[1][2]) / w];
}

/* SVG board coordinates to image pixels for one camera's calibration. */
function boardToImage(calibration) {
  const toImage = invert3(calibration.to_board);
  return toImage && (([x, y]) => project(toImage, [x, -y]));
}

/* The pixel of a board point given clockwise from the 20, as a hint. */
function hintAt(calibration, degrees) {
  const map = boardToImage(calibration);
  if (!map) return null;
  const [x, y] = map(polar(TWENTY_HINT_MM, -90 + degrees));
  return x >= 0 && y >= 0 ? [Math.round(x), Math.round(y)] : null;
}

/* ---------- Scoring helpers ---------- */

function segmentIndex(x, y) {
  const angle = Math.atan2(y, x) * 180 / Math.PI;
  return ((Math.round((angle + 90) / 18) % 20) + 20) % 20;
}

function scoreAt(x, y) {
  const radius = Math.hypot(x, y);
  if (radius <= R.bull) return "DB";
  if (radius <= R.outerBull) return "SB";
  if (radius > R.doubleOut) return "MISS";
  const number = SEGMENTS[segmentIndex(x, y)];
  if (radius >= R.doubleIn) return `D${number}`;
  if (radius >= R.tripleIn && radius <= R.tripleOut) return `T${number}`;
  return `S${number}`;
}

/* The board area a dart landed in, to highlight it; null for a miss. */
function hitArea(x, y) {
  const radius = Math.hypot(x, y);
  if (radius > R.doubleOut) return null;
  if (radius <= R.bull) return circlePoints(R.bull, 24);
  if (radius <= R.outerBull) return circlePoints(R.outerBull, 36);
  const bands = [[R.outerBull, R.tripleIn], [R.tripleIn, R.tripleOut], [R.tripleOut, R.doubleIn], [R.doubleIn, R.doubleOut]];
  const [inner, outer] = bands.find(([, end]) => radius <= end);
  const start = -99 + segmentIndex(x, y) * 18;
  return sectorPoints(inner, outer, start, start + 18);
}

/* Throws carry only a label, so darts are drawn in the middle of their area;
   null when the position is unknown. */
function labelPoint(label) {
  if (label === "SB") return [0, -10];
  if (label === "DB") return [2, 2];
  if (label === "MISS") return [210, -175];

  const match = /^([SDT])(\d{1,2})$/.exec(label);
  if (!match) return null;
  const [, ring, numberText] = match;
  const index = SEGMENTS.indexOf(Number(numberText));
  if (index < 0) return null;
  const radius = ring === "T" ? 103 : ring === "D" ? 166 : 65;
  return polar(radius, -90 + index * 18);
}

function svgPoint(svg, event) {
  const point = svg.createSVGPoint();
  point.x = event.clientX;
  point.y = event.clientY;
  return point.matrixTransform(svg.getScreenCTM().inverse());
}

/* ---------- Shared UI ---------- */

function toast(message) {
  let element = document.querySelector(".toast");
  if (!element) {
    element = document.createElement("div");
    element.className = "toast";
    element.setAttribute("role", "status");
    element.setAttribute("aria-live", "polite");
    document.body.appendChild(element);
  }
  element.textContent = message;
  element.classList.add("is-visible");
  window.clearTimeout(element.hideTimer);
  element.hideTimer = window.setTimeout(() => element.classList.remove("is-visible"), 4500);
}

const escapeHtml = text => String(text).replace(/[&<>"']/g, char => `&#${char.charCodeAt(0)};`);
const options = (list, selected) => list
  .map(([value, label]) => `<option value="${escapeHtml(value)}"${String(value) === String(selected) ? " selected" : ""}>${escapeHtml(label)}</option>`)
  .join("");
const sentence = text => text.charAt(0).toUpperCase() + text.slice(1);

/* ---------- Daemon API ---------- */

const api = {
  async request(path, { method = "GET", body } = {}) {
    let response;
    try {
      response = await fetch(`/api/v1${path}`, {
        method,
        cache: "no-store",
        headers: body === undefined ? {} : { "Content-Type": "application/json" },
        body: body === undefined ? undefined : JSON.stringify(body)
      });
    } catch {
      throw Object.assign(new Error("OcheCore is not reachable. Is the daemon running?"), { code: "offline" });
    }
    const data = await response.json().catch(() => null);
    if (!response.ok) {
      /* Daemon errors are {code, message}. */
      throw Object.assign(
        new Error(data?.message ? sentence(data.message) : `${method} ${path} failed (${response.status})`),
        { status: response.status, code: data?.code }
      );
    }
    return data;
  },
  status() {
    return this.request("/status");
  },
  recentEvents() {
    return this.request("/events/recent");
  },
  availableCameras() {
    return this.request("/cameras/available");
  },
  snapshotUrl(slot) {
    return `/api/v1/cameras/${slot}/snapshot?t=${Date.now()}`;
  },
  /* Commands are flat: {"command":"demo_throw","label":"T20"}. */
  async command(command, args = {}) {
    const snapshot = await this.request("/command", { method: "POST", body: { command, ...args } });
    daemon.accept(snapshot);
    return snapshot;
  },
  /* ConfigChange.value is a string on the daemon side, even for numbers. */
  async setConfig(key, value) {
    const snapshot = await this.request("/config", { method: "PUT", body: { key, value: String(value) } });
    daemon.accept(snapshot);
    return snapshot;
  },
  /* Applies [key, value] pairs in order and stops at the first refusal; each
     one is validated and saved atomically by the daemon. */
  async configure(changes) {
    let snapshot = daemon.snapshot;
    for (const [key, value] of changes) {
      try {
        snapshot = await this.setConfig(key, value);
      } catch (error) {
        error.key = key;
        throw error;
      }
    }
    return snapshot;
  }
};

/* Runs an API call and reports a refusal; resolves to null on failure. */
async function attempt(promise, success) {
  try {
    const result = await promise;
    if (success) toast(success);
    return result;
  } catch (error) {
    toast(error.message);
    return null;
  }
}

/* The daemon's state: the latest snapshot and the recent mutation events,
   kept current by the event stream, command replies and a status poll (camera
   FPS and motion change without an event). */
const daemon = {
  snapshot: null,
  events: [],
  connected: false,
  listeners: new Set(),

  subscribe(listener) {
    this.listeners.add(listener);
    if (this.snapshot) listener(this);
  },

  notify() {
    this.listeners.forEach(listener => listener(this));
  },

  /* Keeps the newest state; a new session ID means the daemon restarted. */
  accept(snapshot) {
    const current = this.snapshot;
    if (current && snapshot.session_id === current.session_id && snapshot.revision < current.revision) return;
    if (current && snapshot.session_id !== current.session_id) this.events = [];
    this.snapshot = snapshot;
    this.notify();
  },

  record(event) {
    if (event.kind === "snapshot" || event.kind === "resync") return;
    const last = this.events[this.events.length - 1];
    if (last && last.session_id === event.session_id && event.sequence <= last.sequence) return;
    this.events = [...this.events, event].slice(-64);
  },

  async connect() {
    let delay = 500;
    for (;;) {
      try {
        const response = await fetch("/api/v1/events", { cache: "no-store", headers: { Accept: "text/event-stream" } });
        if (!response.ok || !response.body) throw new Error(`events: ${response.status}`);
        await this.read(response.body, () => {
          delay = 500;
        });
      } catch {
        /* Reconnect below. */
      }
      if (this.connected) {
        this.connected = false;
        this.notify();
      }
      await sleep(delay);
      delay = Math.min(delay * 2, 5000);
    }
  },

  /* A minimal SSE reader: every event, whatever its name, carries a complete
     snapshot in its JSON data. */
  async read(body, onOpen) {
    const reader = body.pipeThrough(new TextDecoderStream()).getReader();
    let buffer = "";
    for (;;) {
      const { value, done } = await reader.read();
      if (done) return;
      buffer += value.replace(/\r\n?/g, "\n");
      let end;
      while ((end = buffer.indexOf("\n\n")) >= 0) {
        const block = buffer.slice(0, end);
        buffer = buffer.slice(end + 2);
        const data = block.split("\n").filter(line => line.startsWith("data:"))
          .map(line => line.slice(5).replace(/^ /, "")).join("\n");
        if (!data) continue;
        const event = JSON.parse(data);
        if (!this.connected) {
          this.connected = true;
          onOpen();
          await this.loadHistory(event.session_id);
        }
        this.record(event);
        this.accept(event.snapshot);
        this.notify();
      }
    }
  },

  async loadHistory(sessionId) {
    try {
      const recent = await api.recentEvents();
      const known = new Set(this.events.map(event => `${event.session_id}:${event.sequence}`));
      this.events = [...this.events, ...recent.filter(event => !known.has(`${event.session_id}:${event.sequence}`))]
        .filter(event => event.session_id === sessionId)
        .sort((a, b) => a.sequence - b.sequence)
        .slice(-64);
    } catch {
      /* History is a convenience; the stream continues without it. */
    }
  },

  async poll() {
    for (;;) {
      await sleep(2000);
      if (document.hidden || !this.connected) continue;
      try {
        this.accept(await api.status());
      } catch {
        /* The event stream reports the connection. */
      }
    }
  },

  start() {
    this.connect();
    this.poll();
  }
};

/* Calls `ready` once with the first snapshot. */
function whenReady(ready) {
  let started = false;
  daemon.subscribe(() => {
    if (started || !daemon.snapshot) return;
    started = true;
    ready(daemon.snapshot);
  });
}

/* ---------- Board state, derived from snapshots ---------- */

/* The calibration of a camera, if it was made with its current settings
   (CalibrationRecord::for_camera). */
function calibrationFor(record, camera) {
  return record?.cameras.find(c =>
    c.slot === camera.slot &&
    c.device === camera.device &&
    c.width === camera.width &&
    c.height === camera.height &&
    c.rotation === camera.rotation) || null;
}

const assignedCameras = snapshot => snapshot.config.cameras.filter(camera => camera.device);
const cameraStatus = (snapshot, slot) => snapshot.cameras.find(status => status.slot === slot);
const isStreaming = (snapshot, slot) => cameraStatus(snapshot, slot)?.state === "streaming";

/* Cameras with a device but no way to find the 20
   (Snapshot::cameras_needing_orientation). */
function camerasNeedingOrientation(snapshot) {
  return assignedCameras(snapshot)
    .filter(camera => camera.hint == null && !calibrationFor(snapshot.calibration, camera))
    .map(camera => camera.slot);
}

/* The pixel the daemon will use as the 20 for a camera: its hint, or the 20
   of its current calibration. */
function currentHint(snapshot, camera) {
  if (camera.hint) return camera.hint;
  const calibration = calibrationFor(snapshot.calibration, camera);
  return calibration ? hintAt(calibration, 0) : null;
}

/* What the board is doing, like Snapshot::activity in oche-domain. */
function boardActivity(snapshot) {
  const make = (level, label, detail = "") => ({ level, label, detail });
  if (snapshot.mode === "control_only") return make("idle", "Controls only", "cameras are not used in this mode");
  if (snapshot.state === "stopped") return make("idle", "Stopped", "press Start to open the cameras");
  if (snapshot.state === "standby") return make("idle", "Standby", "cameras are closed; press Start to wake them");
  if (snapshot.mode === "live") {
    const assigned = assignedCameras(snapshot);
    if (!assigned.length) return make("attention", "Needs setup", "assign a device to each camera");
    for (const camera of assigned) {
      const status = cameraStatus(snapshot, camera.slot);
      if (status?.state === "error") {
        return make("problem", "Error", `camera ${camera.slot}: ${status.error || "unknown error"}`);
      }
    }
    const waiting = assigned.find(camera => !isStreaming(snapshot, camera.slot));
    if (waiting) return make("busy", "Starting", `waiting for camera ${waiting.slot}`);
    if (camerasNeedingOrientation(snapshot).length) return make("attention", "Needs setup", "find the 20 on each camera in Setup → Calibrate");
    if (!snapshot.capabilities.calibration) return make("attention", "Needs calibration", "empty the board and calibrate");
    if (!snapshot.capabilities.vision) return make("attention", "Needs calibration", "at least two calibrated cameras are needed to score");
    if (snapshot.motion) return make("busy", "Motion", "waiting for the scene to settle");
  }
  switch (snapshot.scene) {
    case "partial_takeout":
      return make("busy", "Taking out", "darts are being removed; finish clearing the board");
    case "darts_present":
      if (snapshot.throws.length >= 3) return make("attention", "Take out darts", "the visit is complete; remove the darts");
      /* A reset clears the visit, not the board: the demo takes no fourth dart. */
      if (snapshot.mode === "demo" && snapshot.physical_darts >= 3) {
        return make("attention", "Take out darts", "three darts are still on the board; take them out");
      }
      return make("ready", "Ready", snapshot.physical_darts == null ? "" : `${snapshot.physical_darts} on the board`);
    case "empty":
      return make("ready", "Ready", "board empty");
    default:
      return make("busy", "Starting", "waiting for the first still scene");
  }
}

/* Status bar tones in style.css. */
const LEVEL_TONES = { idle: "stopped", busy: "busy", ready: "ready", attention: "setup", problem: "problem" };

/* ---------- Setup progress ---------- */

const STEP_PAGES = ["index.html", "cameras.html", "calibrate.html", "test.html"];
const testedKey = snapshot => snapshot.calibration?.created_unix_ms ?? snapshot.mode;

/* [board, cameras, calibration, test] completion. The board always has a
   valid name and address; calibration covers every assigned camera with its
   current settings. The test is a per-browser note for that calibration. */
function setupProgress(snapshot) {
  const cameras = snapshot.config.cameras;
  const camerasDone = cameras.length >= MIN_CAMERAS && cameras.every(camera => camera.device);
  const calibrated = camerasDone && cameras.every(camera => calibrationFor(snapshot.calibration, camera));
  return [true, camerasDone, calibrated, storage.get("tested", null) === testedKey(snapshot)];
}

/* Cameras and calibration only apply to a live daemon. */
const needsCameras = snapshot => snapshot.mode === "live";

/* The first step still to do (1–3), or 4 once the board can score. */
function firstOpenStep(snapshot) {
  if (!needsCameras(snapshot)) return 4;
  const open = setupProgress(snapshot).slice(0, 3).indexOf(false);
  return open === -1 ? 4 : open + 1;
}

const isSetUp = snapshot => firstOpenStep(snapshot) === 4;

/* A step opens once every step before it is done; otherwise this sends the
   browser to the first unfinished step and returns false. */
function enterStep(step, snapshot) {
  const open = firstOpenStep(snapshot);
  if (step > open) {
    window.location.replace(STEP_PAGES[open - 1]);
    return false;
  }
  setStepStates(step, snapshot);
  return true;
}

function setStepStates(currentStep, snapshot) {
  const done = setupProgress(snapshot);
  const open = firstOpenStep(snapshot);
  document.querySelectorAll("[data-setup-step]").forEach(step => {
    const index = Number(step.dataset.setupStep);
    const locked = index > open;
    const finished = done[index - 1] && (index === 1 || index === 4 || needsCameras(snapshot));
    step.classList.toggle("is-current", index === currentStep);
    step.classList.toggle("is-done", finished);
    step.classList.toggle("is-locked", locked);
    step.querySelector(".step-number").textContent = finished ? "✓" : String(index).padStart(2, "0");
    if (locked) {
      step.setAttribute("aria-disabled", "true");
      step.setAttribute("tabindex", "-1");
      step.title = `Finish step ${open} first`;
    } else {
      step.removeAttribute("aria-disabled");
      step.removeAttribute("tabindex");
      step.removeAttribute("title");
    }
    if (index === currentStep) step.setAttribute("aria-current", "step");
    else step.removeAttribute("aria-current");
  });
}

/* Locked steps and Next buttons do nothing but say what is missing. */
document.addEventListener("click", event => {
  const blocked = event.target.closest('[aria-disabled="true"]');
  if (!blocked) return;
  event.preventDefault();
  event.stopImmediatePropagation();
  toast(blocked.dataset.blockedMessage || blocked.title || "Finish this step first.");
}, true);

/* Any [data-command] button sends that native command; data-args adds JSON
   arguments. */
document.addEventListener("click", async event => {
  const button = event.target.closest("[data-command]");
  if (!button || button.disabled) return;
  const args = button.dataset.args ? JSON.parse(button.dataset.args) : {};
  button.disabled = true;
  await attempt(api.command(button.dataset.command, args));
  button.disabled = false;
});

function setNextEnabled(link, enabled, message) {
  if (enabled) {
    link.removeAttribute("aria-disabled");
  } else {
    link.setAttribute("aria-disabled", "true");
    link.dataset.blockedMessage = message;
  }
}

/* ---------- Status bar ---------- */

const MODE_NAMES = { live: "Live", demo: "Demo mode", control_only: "Controls only" };

/* Status-bar icons, 16 × 16, drawn in the text colour. */
const STATUS_ICONS = {
  board: '<g fill="none" stroke="currentColor" stroke-width="1.4"><circle cx="8" cy="8" r="6.3"/><circle cx="8" cy="8" r="3.4"/></g><circle cx="8" cy="8" r="1.1" fill="currentColor"/>',
  start: '<path d="M5 3.5v9l7-4.5z" fill="currentColor"/>',
  stop: '<rect x="4" y="4" width="8" height="8" rx="1.2" fill="currentColor"/>',
  reset: '<path d="M8 3.5 3.5 9h9z" fill="currentColor"/><rect x="3.5" y="10.5" width="9" height="2" rx=".6" fill="currentColor"/>',
  calibrate: '<g fill="none" stroke="currentColor" stroke-width="1.5"><circle cx="8" cy="8" r="4.5"/><path d="M8 1.5v3M8 11.5v3M1.5 8h3M11.5 8h3"/></g>',
  camera: '<g fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round"><path d="M2 5.5h2.6L6 3.5h4l1.4 2H14v7H2z"/><circle cx="8" cy="9" r="2.2"/></g>',
  cpu: '<g fill="none" stroke="currentColor" stroke-width="1.4"><rect x="4" y="4" width="8" height="8" rx="1"/><path d="M6.5 1.5V4M9.5 1.5V4M6.5 12v2.5M9.5 12v2.5M1.5 6.5H4M1.5 9.5H4M12 6.5h2.5M12 9.5h2.5"/></g>',
  memory: '<g fill="none" stroke="currentColor" stroke-width="1.4"><rect x="1.5" y="4.5" width="13" height="6" rx="1"/><path d="M4.5 10.5v2M8 10.5v2M11.5 10.5v2"/></g>'
};
const statusIcon = name =>
  `<svg class="status-icon" viewBox="0 0 16 16" aria-hidden="true">${STATUS_ICONS[name]}</svg>`;

/* Every page's footer gets the same controls and readouts: board controls
   beside the state, then camera FPS, CPU, memory and version on the right. */
function buildStatusBar(bar) {
  if (bar.dataset.built) return;
  bar.dataset.built = "true";
  const action = (attrs, name, label) =>
    `<button class="status-action" type="button" ${attrs} title="${label}" aria-label="${label}">${statusIcon(name)}</button>`;
  bar.querySelector("#status-state").insertAdjacentHTML("afterend", `
    <span class="status-actions" role="group" aria-label="Board controls">
      ${action('data-command="start" data-status-when="stopped" hidden', "start", "Start the board")}
      ${action('data-command="stop" data-status-when="running" hidden', "stop", "Stop the board")}
      ${action('data-command="reset"', "reset", "Reset the visit")}
      <a class="status-action" id="status-calibrate" href="/setup/calibrate.html" title="Calibrate the cameras" aria-label="Calibrate the cameras" hidden>${statusIcon("calibrate")}</a>
    </span>`);
  const right = document.createElement("span");
  right.className = "status-right";
  right.innerHTML = `
    <span class="status-item" id="status-cpu" hidden>${statusIcon("cpu")}<span></span></span>
    <span class="status-item" id="status-memory" hidden>${statusIcon("memory")}<span></span></span>`;
  right.prepend(bar.querySelector("#status-cameras"));
  right.append(bar.querySelector("#status-connection"));
  bar.append(right);
}

function renderStatusBar({ snapshot, connected }) {
  const state = document.getElementById("status-state");
  const cameraText = document.getElementById("status-cameras");
  const address = document.getElementById("status-address");
  const connection = document.getElementById("status-connection");
  if (!state) return;
  buildStatusBar(state.closest(".status-bar"));
  if (!snapshot) {
    state.textContent = "Connecting";
    state.dataset.tone = "busy";
    if (connection) connection.textContent = "Waiting for OcheCore…";
    return;
  }
  const running = snapshot.state === "running";
  document.querySelectorAll("[data-status-when]").forEach(button => {
    button.hidden = (button.dataset.statusWhen === "running") !== running;
  });
  document.getElementById("status-calibrate").hidden = snapshot.mode !== "live";
  const resources = snapshot.resources;
  const cpu = document.getElementById("status-cpu");
  cpu.hidden = resources?.cpu_percent == null;
  if (!cpu.hidden) {
    cpu.lastElementChild.textContent = `${resources.cpu_percent.toFixed(0)} %`;
    cpu.title = "OcheCore's CPU use over the last second, as a share of one core";
  }
  const memory = document.getElementById("status-memory");
  memory.hidden = !resources;
  if (resources) {
    memory.lastElementChild.textContent = `${(resources.memory_bytes / 1048576).toFixed(0)} MB`;
    memory.title = "OcheCore's resident memory";
  }
  const activity = boardActivity(snapshot);
  state.textContent = connected ? activity.label : "Offline";
  state.dataset.tone = connected ? LEVEL_TONES[activity.level] : "problem";
  state.title = connected ? sentence(activity.detail) : "The daemon is not reachable; retrying";
  if (cameraText) {
    /* Each camera's delivered FPS in slot order; a dash for a camera that is not streaming. */
    const count = snapshot.config.cameras.length;
    const fps = snapshot.config.cameras.map(camera => {
      const status = snapshot.cameras.find(item => item.slot === camera.slot);
      return status?.state === "streaming" ? status.fps.toFixed(0) : "–";
    });
    const live = snapshot.cameras.some(status => status.state === "streaming");
    cameraText.innerHTML = `${statusIcon("camera")}<span>${live ? `${fps.join(" ")} fps` : "off"}</span>`;
    cameraText.title = `${count} ${count === 1 ? "camera" : "cameras"}: ${live
      ? "frames per second each camera delivers"
      : "not capturing (stopped, standby or demo mode)"}`;
  }
  if (address) {
    const { bind, port } = snapshot.config.server;
    const saved = bind.includes(":") ? `[${bind}]:${port}` : `${bind}:${port}`;
    const listening = snapshot.restart_required
      ? `${snapshot.listening_on} · ${saved} after restart`
      : snapshot.listening_on;
    const name = snapshot.config.board.name;
    address.textContent = name ? `${name} · ${listening}` : listening;
    address.title = "Board name and the address OcheCore listens on";
  }
  if (connection) {
    const mode = MODE_NAMES[snapshot.mode] || snapshot.mode;
    connection.textContent = connected
      ? (snapshot.version ? `${mode} · v${snapshot.version}` : mode)
      : "Disconnected · retrying";
  }
}

/* ---------- Camera previews ---------- */

/* One camera's frame, turned upright by its configured rotation. Frames
   arrive unrotated, so drawings use raw image pixels in `frame`, the group
   that carries the rotation. A `still` preview keeps one picture until it is
   retaken, so drawings over it stay put; otherwise it follows the camera. */
function createCameraPreview(slot, still = false) {
  const element = document.createElement("div");
  element.className = "camera-preview";
  element.dataset.cameraPreview = slot;
  const svg = svgEl("svg", { role: "img", "aria-label": `Camera ${slot}`, preserveAspectRatio: "xMidYMid meet" });
  const frame = svgEl("g", {}, svg);
  const image = svgEl("image", { x: 0, y: 0 }, frame);
  const overlay = svgEl("g", {}, frame);
  const message = document.createElement("div");
  message.className = "preview-empty";
  const label = document.createElement("span");
  label.className = "preview-label";
  element.append(svg, message, label);
  let layoutKey = "";
  let url = null;

  return {
    slot,
    still,
    hasFrame: false,
    rotation: 0,
    element,
    svg,
    frame,
    overlay,
    label,
    layout({ width, height, rotation }) {
      const key = `${width}x${height}@${rotation}`;
      if (key === layoutKey) return;
      layoutKey = key;
      this.rotation = rotation;
      /* A frame of another size or orientation is a different picture. */
      this.hasFrame = false;
      const turned = rotation % 180 !== 0;
      svg.setAttribute("viewBox", `0 0 ${turned ? height : width} ${turned ? width : height}`);
      const shift = { 0: "", 90: `translate(${height} 0)`, 180: `translate(${width} ${height})`, 270: `translate(0 ${width})` };
      frame.setAttribute("transform", `${shift[rotation] || ""} rotate(${rotation})`);
      image.setAttribute("width", width);
      image.setAttribute("height", height);
    },
    show(blob) {
      const next = URL.createObjectURL(blob);
      image.setAttribute("href", next);
      if (url) URL.revokeObjectURL(url);
      url = next;
      this.hasFrame = true;
    },
    /* A message instead of the frame, with an optional Start button. */
    say(text, start = false) {
      const html = text
        ? `<p>${escapeHtml(text)}</p>${start ? '<button class="btn btn-small btn-lime" type="button" data-command="start">Start cameras</button>' : ""}`
        : "";
      if (message.innerHTML !== html) message.innerHTML = html;
      message.hidden = !text;
      svg.style.visibility = text ? "hidden" : "";
    }
  };
}

/* Why a camera shows no frame, or "" while it streams. */
function previewProblem(snapshot, camera) {
  if (snapshot.mode !== "live") return ["Previews need the live daemon: run oche-core run without --demo or --no-capture.", false];
  if (!camera.device) return ["Choose a device for this camera.", false];
  if (snapshot.state !== "running") return [`The board is ${snapshot.state}. Start it to see this camera.`, true];
  const status = cameraStatus(snapshot, camera.slot);
  if (status?.state === "error") return [`Camera error: ${status.error || "unknown"}`, false];
  if (status?.state !== "streaming") return ["Starting camera…", false];
  return ["", false];
}

/* Fetches each visible camera's latest JPEG, one request per camera at a
   time; a still preview only until it has a frame. */
const cameraFeed = {
  previews: new Map(),
  pending: new Set(),
  started: false,

  watch(preview) {
    this.previews.set(preview.slot, preview);
    if (!this.started) {
      this.started = true;
      window.setInterval(() => this.tick(), 700);
    }
  },

  forget(slot) {
    this.previews.delete(slot);
  },

  refresh(slot) {
    this.fetch(slot, true);
  },

  /* A new picture for a still preview. */
  retake(slot) {
    const preview = this.previews.get(slot);
    if (preview) preview.hasFrame = false;
    this.fetch(slot);
  },

  tick() {
    if (document.hidden || !daemon.snapshot || !daemon.connected) return;
    this.previews.forEach((_, slot) => this.fetch(slot));
  },

  async fetch(slot, force = false) {
    const preview = this.previews.get(slot);
    if (!preview || this.pending.has(slot)) return;
    if (!force && (!isStreaming(daemon.snapshot, slot) || (preview.still && preview.hasFrame))) return;
    this.pending.add(slot);
    try {
      const response = await fetch(api.snapshotUrl(slot), { cache: "no-store" });
      if (response.ok) preview.show(await response.blob());
    } catch {
      /* The next tick tries again. */
    } finally {
      this.pending.delete(slot);
    }
  }
};

function cameraCardHead(slot, count, tools) {
  return `
    <div class="camera-card-head">
      <span class="cam-number">${String(slot).padStart(2, "0")}</span>
      <div class="camera-title">
        <h3 id="camera-title-${slot}">Camera ${slot}</h3>
        <small>${VIEW_NAMES[count][slot - 1]}</small>
      </div>
      ${tools}
    </div>`;
}

/* Back / Next end where the camera row ends. */
function setContentWidth(cards, extra = 0) {
  document.body.style.setProperty("--content-width", `${cards * 24 + (cards - 1) + extra}rem`);
}

/* ---------- Step 1: board ---------- */

function isIpAddress(value) {
  const octet = "(25[0-5]|2[0-4]\\d|1\\d\\d|[1-9]?\\d)";
  if (new RegExp(`^${octet}(\\.${octet}){3}$`).test(value)) return true;
  if (!value.includes(":") || /[^0-9a-f:.]/i.test(value)) return false;
  try {
    new URL(`http://[${value}]/`);
    return true;
  } catch {
    return false;
  }
}

/* Same rules as Config::validate in oche-domain, checked before sending;
   only the fields given are checked. */
function validateBoard(values) {
  const errors = {};
  const name = values["board.name"];
  const bind = values["server.bind"];
  const port = values["server.port"];
  if (name === undefined) {
    /* Not on this form. */
  } else if (!name) errors["board.name"] = "Enter a name.";
  else if ([...name].length > 80) errors["board.name"] = "Use at most 80 characters.";
  else if (/[\u0000-\u001f\u007f-\u009f]/.test(name)) errors["board.name"] = "Use printable characters only.";
  if (bind !== undefined && !isIpAddress(bind)) {
    errors["server.bind"] = "Enter an IP address such as 127.0.0.1 or 0.0.0.0.";
  }
  if (port !== undefined && (!/^\d{1,5}$/.test(port) || Number(port) < 1 || Number(port) > 65535)) {
    errors["server.port"] = "Enter a port between 1 and 65535.";
  }
  return errors;
}

function setFieldError(input, message) {
  const field = input.closest(".field");
  const errorId = `${input.id}-error`;
  let error = document.getElementById(errorId);
  if (message && !error) {
    error = document.createElement("span");
    error.className = "field-error";
    error.id = errorId;
    field.appendChild(error);
  }
  if (error) {
    error.textContent = message || "";
    error.hidden = !message;
  }
  field.classList.toggle("has-error", Boolean(message));
  const described = (input.getAttribute("aria-describedby") || "").split(" ").filter(id => id && id !== errorId);
  if (message) described.push(errorId);
  if (described.length) input.setAttribute("aria-describedby", described.join(" "));
  else input.removeAttribute("aria-describedby");
  if (message) input.setAttribute("aria-invalid", "true");
  else input.removeAttribute("aria-invalid");
}

/* The board name and listen address fields, on step 1 and in Settings. */
function boardInputs() {
  return {
    "board.name": document.getElementById("board-name"),
    "server.bind": document.getElementById("host-ip"),
    "server.port": document.getElementById("host-port")
  };
}

const savedBoardValues = ({ board, server }) => ({
  "board.name": board.name,
  "server.bind": server.bind,
  "server.port": server.port
});

function fillBoardInputs(inputs, config) {
  const saved = savedBoardValues(config);
  Object.entries(inputs).forEach(([key, input]) => {
    input.value = saved[key];
  });
}

/* Greys the listen address where a container's port mapping decides it, and
   says when a saved address waits for a restart. */
function renderListener(snapshot, inputs, note, defaultNote) {
  const fixed = snapshot.listener_fixed;
  ["server.bind", "server.port"].forEach(key => {
    if (!inputs[key]) return;
    inputs[key].disabled = fixed;
    if (fixed) inputs[key].value = savedBoardValues(snapshot.config)[key];
  });
  const { bind, port } = snapshot.config.server;
  note.classList.toggle("is-warning", !fixed && snapshot.restart_required);
  note.textContent = fixed
    ? "OcheCore runs in a container here, so Docker's port mapping decides where it can be reached. To change it, set OCHECORE_BIND or OCHECORE_PORT next to compose.yaml and recreate the container."
    : snapshot.restart_required
      ? `Saved, not in use yet: OcheCore still listens on ${snapshot.listening_on}. Restart it to listen on ${bind.includes(":") ? `[${bind}]` : bind}:${port}.`
      : defaultNote;
}

/* Validates the form's fields and saves what changed; true when nothing was
   refused. Disabled fields are left alone. */
async function saveBoardInputs(inputs) {
  const saved = savedBoardValues(daemon.snapshot.config);
  const editable = Object.entries(inputs).filter(([, input]) => !input.disabled);
  const values = Object.fromEntries(editable.map(([key, input]) => [key, input.value.trim()]));
  const errors = validateBoard(values);
  editable.forEach(([key, input]) => setFieldError(input, errors[key]));
  const invalid = editable.map(([, input]) => input).find(input => input.hasAttribute("aria-invalid"));
  if (invalid) {
    invalid.focus();
    return false;
  }
  const changes = Object.entries(values)
    .map(([key, value]) => [key, key === "server.port" ? Number(value) : value])
    .filter(([key, value]) => String(value) !== String(saved[key]));
  try {
    const snapshot = await api.configure(changes);
    if (changes.some(([key]) => key.startsWith("server.")) && snapshot?.restart_required) {
      toast("Saved. Restart OcheCore to listen on the new address.");
    } else if (changes.length) {
      toast("Saved.");
    }
    return true;
  } catch (error) {
    if (inputs[error.key]) {
      setFieldError(inputs[error.key], error.message);
      inputs[error.key].focus();
    } else {
      toast(error.message);
    }
    return false;
  }
}

function initBoardPage() {
  const form = document.getElementById("board-form");
  const inputs = boardInputs();
  const next = document.querySelector("[data-save-next]");
  let saving = false;

  const note = document.getElementById("listen-note");
  const defaultNote = note.textContent;

  whenReady(snapshot => {
    enterStep(1, snapshot);
    fillBoardInputs(inputs, snapshot.config);
  });
  daemon.subscribe(({ snapshot }) => {
    setStepStates(1, snapshot);
    renderListener(snapshot, inputs, note, defaultNote);
  });

  async function saveAndContinue() {
    if (saving || !daemon.snapshot) return;
    saving = true;
    if (await saveBoardInputs(inputs)) window.location.href = next.href;
    saving = false;
  }

  /* Enter in a field behaves like the Next button. */
  form.addEventListener("submit", event => {
    event.preventDefault();
    saveAndContinue();
  });
  form.addEventListener("input", event => setFieldError(event.target, ""));
  next.addEventListener("click", event => {
    event.preventDefault();
    saveAndContinue();
  });
}

/* ---------- Step 2: cameras ---------- */

/* Resolutions and frame rates the discovered devices offer, across pixel
   formats, plus the current setting; nothing is guessed. */
function cameraChoices(inventory, cameras) {
  const modes = inventory.devices.flatMap(device => device.modes || []);
  const sizes = new Map();
  modes.forEach(mode => sizes.set(`${mode.width}x${mode.height}`, mode.width * mode.height));
  const resolutions = [...sizes.entries()].sort((a, b) => b[1] - a[1]).map(([size]) => size);
  const current = `${cameras[0].width}x${cameras[0].height}`;
  if (!resolutions.includes(current)) resolutions.unshift(current);
  const frameRates = [...new Set(modes
    .filter(mode => `${mode.width}x${mode.height}` === current)
    .flatMap(mode => mode.fps))];
  if (!frameRates.includes(cameras[0].fps)) frameRates.push(cameras[0].fps);
  return { resolutions, frameRates: frameRates.sort((a, b) => a - b) };
}

/* Fills the settings that apply to every camera: #camera-resolution,
   #camera-fps, #detection-speed and #detection-standby. */
function renderDetectionSettings(snapshot, inventory) {
  const { cameras, detection } = snapshot.config;
  const choices = cameraChoices(inventory, cameras);
  document.getElementById("camera-resolution").innerHTML =
    options(choices.resolutions.map(size => [size, size]), `${cameras[0].width}x${cameras[0].height}`);
  document.getElementById("camera-fps").innerHTML =
    options(choices.frameRates.map(fps => [fps, `${fps} fps`]), cameras[0].fps);
  document.getElementById("detection-speed").innerHTML = options(SPEEDS, detection.speed);
  document.getElementById("detection-standby").innerHTML = options(STANDBY, detection.standby_seconds);
}

/* Config changes for one of those fields. Every camera gets the same
   resolution and FPS, as in the terminal UI. */
function detectionChanges(snapshot, id, value) {
  const { cameras } = snapshot.config;
  if (id === "camera-resolution") {
    const [width, height] = value.split("x").map(Number);
    return cameras.flatMap(camera => {
      const sameShape = Math.abs(width / height - camera.width / camera.height) < 0.01;
      return [
        [`cameras.${camera.slot}.resolution`, value],
        ...hintChange(snapshot, camera, sameShape ? [width / camera.width, height / camera.height] : null)
      ];
    });
  }
  if (id === "camera-fps") return cameras.map(camera => [`cameras.${camera.slot}.fps`, value]);
  if (id === "detection-speed") return [["detection.speed", value]];
  if (id === "detection-standby") return [["detection.standby_seconds", value]];
  return [];
}

const deviceMatches = (device, path) => device.path === path || (device.aliases || []).includes(path);

/* The `0:1.1` of `.../platform-…-usb-0:1.1:1.0-video-index0`, as usb_port in
   the terminal UI. */
function usbPort(path) {
  const rest = path.split("/").pop().split("-usb-")[1];
  if (rest === undefined) return null;
  const ports = rest.split("-video")[0];
  return ports.endsWith(":1.0") ? ports.slice(0, -4) : ports;
}

/* A camera as the user knows it, named as in the terminal UI: its name and
   the USB port it sits in. */
function deviceLabel(device) {
  const port = [...(device.aliases || []), device.path].map(usbPort).find(Boolean);
  return `${device.name || "Camera"} · ${port ? `USB ${port}` : device.path}`;
}

/* Config changes that keep a camera's 20 when its pixels move: rotation only
   relabels frames, so the hint stays; a resolution with the same aspect ratio
   scales it; anything else clears it, and the camera needs orienting again. */
function hintChange(snapshot, camera, scale) {
  const hint = currentHint(snapshot, camera);
  const key = `cameras.${camera.slot}.hint`;
  if (!hint) return [];
  if (!scale) return [[key, ""]];
  return [[key, `${Math.round(hint[0] * scale[0])},${Math.round(hint[1] * scale[1])}`]];
}

function initCamerasPage() {
  const grid = document.getElementById("camera-grid");
  const settings = document.getElementById("detection-settings");
  const next = document.querySelector("[data-save-next]");
  const lede = document.querySelector(".page-head .lede");
  const ledeText = lede.textContent;
  const previews = new Map();
  let inventory = { devices: [], warnings: [] };
  let renderedKey = "";
  let busy = false;

  async function discover() {
    try {
      inventory = await api.availableCameras();
    } catch (error) {
      inventory = { devices: [], warnings: [error.message] };
    }
    renderedKey = "";
    if (daemon.snapshot) render(daemon);
  }

  /* Sends changes, then re-renders from the daemon's reply; resolves to the
     new snapshot, or null when nothing was saved. */
  async function apply(changes, success) {
    if (busy || !changes.length) return null;
    busy = true;
    grid.classList.add("is-busy");
    const snapshot = await attempt(api.configure(changes), success);
    busy = false;
    grid.classList.remove("is-busy");
    renderedKey = "";
    render(daemon);
    return snapshot;
  }

  function deviceOptions(camera) {
    const known = inventory.devices.find(device => deviceMatches(device, camera.device));
    const list = [["", "Choose a device"]];
    inventory.devices.forEach(device => {
      const value = known === device ? camera.device : device.path;
      list.push([value, `${deviceLabel(device)}${device.error ? " (error)" : ""}`]);
    });
    if (camera.device && !known) {
      const port = usbPort(camera.device);
      list.push([camera.device, `${port ? `USB ${port}` : camera.device} (not found)`]);
    }
    return options(list, camera.device);
  }

  function cardMarkup(snapshot, camera) {
    const { slot } = camera;
    const count = snapshot.config.cameras.length;
    const tools = `
      <div class="camera-tool-buttons">
        <button class="icon-btn" type="button" data-rotate="${slot}" aria-label="Rotate camera ${slot}" title="Rotate 90°">${icon("rotate")}</button>
        <button class="icon-btn" type="button" data-refresh="${slot}" aria-label="Refresh camera ${slot}" title="Refresh preview and device list">${icon("refresh")}</button>
        ${count > MIN_CAMERAS ? `<button class="icon-btn" type="button" data-remove="${slot}" aria-label="Remove camera ${slot}" title="Remove camera">${icon("remove")}</button>` : ""}
      </div>`;
    return `
      <section class="panel camera-card" data-camera-card="${slot}" aria-labelledby="camera-title-${slot}">
        ${cameraCardHead(slot, count, tools)}
        <div data-preview-slot="${slot}"></div>
        <div class="camera-device field">
          <label class="sr-only" for="device-${slot}">Camera ${slot} device</label>
          <select id="device-${slot}" data-device="${slot}">${deviceOptions(camera)}</select>
        </div>
      </section>`;
  }

  function preview(slot) {
    if (!previews.has(slot)) {
      const created = createCameraPreview(slot);
      previews.set(slot, created);
      cameraFeed.watch(created);
    }
    return previews.get(slot);
  }

  function render({ snapshot }) {
    const { cameras, detection } = snapshot.config;
    const key = JSON.stringify([cameras, detection, inventory.devices.length]);
    if (key !== renderedKey && !busy) {
      renderedKey = key;
      const canAdd = cameras.length < MAX_CAMERAS;
      grid.innerHTML = cameras.map(camera => cardMarkup(snapshot, camera)).join("") +
        (canAdd ? `<button class="add-camera" type="button" id="add-camera"><span aria-hidden="true">+</span>Add camera</button>` : "");
      grid.style.setProperty("--cols", cameras.length);
      grid.classList.toggle("can-add", canAdd);
      setContentWidth(cameras.length, canAdd ? 5.5 : 0);
      cameras.forEach(camera => {
        grid.querySelector(`[data-preview-slot="${camera.slot}"]`).replaceWith(preview(camera.slot).element);
        preview(camera.slot).layout(camera);
      });
      [...previews.keys()].filter(slot => slot > cameras.length).forEach(slot => {
        previews.delete(slot);
        cameraFeed.forget(slot);
      });

      renderDetectionSettings(snapshot, inventory);
    }

    cameras.forEach(camera => {
      const view = preview(camera.slot);
      const [problem, start] = previewProblem(snapshot, camera);
      view.say(problem, start);
      const status = cameraStatus(snapshot, camera.slot);
      const delivered = status?.state === "streaming" ? ` · ${status.fps.toFixed(0)}/${camera.fps} fps` : ` · ${camera.fps} fps`;
      view.label.textContent = `${camera.height}p${delivered} · ${camera.rotation}°`;
    });

    const notes = [];
    if (snapshot.mode === "live" && cameras.length < 2) notes.push("Scoring needs at least two cameras.");
    if (snapshot.mode === "live" && inventory.warnings.length) notes.push(inventory.warnings.join(" "));
    lede.textContent = [ledeText, ...notes].join(" ");

    const blocked = !needsCameras(snapshot) ? ""
      : cameras.length < MIN_CAMERAS ? "Add a second camera: scoring needs at least two."
      : cameras.filter(camera => !camera.device).map(camera => `Choose a device for camera ${camera.slot}.`)[0] || "";
    setNextEnabled(next, !blocked, blocked);
    setStepStates(2, snapshot);
  }

  grid.addEventListener("click", event => {
    const snapshot = daemon.snapshot;
    if (!snapshot) return;
    const { cameras } = snapshot.config;
    const rotate = event.target.closest("[data-rotate]");
    const refresh = event.target.closest("[data-refresh]");
    const remove = event.target.closest("[data-remove]");
    if (rotate) {
      const camera = cameras[Number(rotate.dataset.rotate) - 1];
      /* Keep the 20 a calibration found: rotation does not move pixels. */
      const keep = camera.hint ? [] : hintChange(snapshot, camera, [1, 1]);
      apply([...keep, [`cameras.${camera.slot}.rotation`, (camera.rotation + 90) % 360]]);
    } else if (refresh) {
      cameraFeed.refresh(Number(refresh.dataset.refresh));
      discover();
    } else if (remove && cameras.length <= MIN_CAMERAS) {
      toast("Scoring needs at least two cameras.");
    } else if (remove) {
      /* Slots stay contiguous: later cameras move up one place, with their
         settings and their 20. */
      const removed = Number(remove.dataset.remove);
      const changes = [[`cameras.${removed}.device`, ""]];
      for (let slot = removed; slot < cameras.length; slot += 1) {
        const moving = cameras[slot];
        const hint = currentHint(snapshot, moving);
        changes.push(
          [`cameras.${slot + 1}.device`, ""],
          [`cameras.${slot}.device`, moving.device],
          [`cameras.${slot}.resolution`, `${moving.width}x${moving.height}`],
          [`cameras.${slot}.fps`, moving.fps],
          [`cameras.${slot}.rotation`, moving.rotation],
          [`cameras.${slot}.exposure`, moving.exposure ?? "auto"],
          [`cameras.${slot}.hint`, hint ? hint.join(",") : ""]
        );
      }
      changes.push(["cameras.count", cameras.length - 1]);
      apply(changes, `Camera ${removed} removed.`);
    } else if (event.target.closest("#add-camera")) {
      const first = cameras[0];
      const slot = cameras.length + 1;
      apply([
        ["cameras.count", slot],
        [`cameras.${slot}.resolution`, `${first.width}x${first.height}`],
        [`cameras.${slot}.fps`, first.fps]
      ], `Camera ${slot} added. Choose its device.`);
    }
  });

  grid.addEventListener("change", async event => {
    const slot = Number(event.target.dataset.device);
    const snapshot = daemon.snapshot;
    if (!slot || !snapshot) return;
    const path = event.target.value;
    const owner = snapshot.config.cameras.find(camera => camera.slot !== slot && path && camera.device === path);
    if (owner) {
      setFieldError(event.target, `Camera ${owner.slot} already uses this device.`);
      return;
    }
    setFieldError(event.target, "");
    /* A different device sees the board from somewhere else: its 20 is unknown. */
    const saved = await apply([[`cameras.${slot}.device`, path], [`cameras.${slot}.hint`, ""]]);
    /* Cameras only stream while the board runs: start it to show the preview. */
    if (saved && path && saved.mode === "live" && saved.state !== "running") {
      await attempt(api.command("start"));
    }
  });

  settings.addEventListener("change", event => {
    if (daemon.snapshot) apply(detectionChanges(daemon.snapshot, event.target.id, event.target.value));
  });

  whenReady(snapshot => {
    if (!enterStep(2, snapshot)) return;
    daemon.subscribe(render);
    discover();
  });
}

/* ---------- Step 3: calibrate ---------- */

/* Segments painted over each camera so the fitted orientation can be checked
   against the board's printed numbers at a glance. */
const ORIENTATION_MARKS = [
  { number: 20, color: "#ff3b30", name: "red" },
  { number: 6, color: "#2f7bff", name: "blue" },
  { number: 3, color: "#ff9500", name: "orange" },
  { number: 11, color: "#b44dff", name: "purple" }
];
const MARKS_TEXT = "red on 20, blue on 6, orange on 3, purple on 11";

/* The daemon takes the 20 to be the red double nearest the camera's hint
   (detect in oche-calibration). The real 20 has a red double too, so only
   every other segment can be it: the arrows turn two segments at a time. */
const TURN_SEGMENTS = 2;

/* Two ways to find the 20 on each camera:
   - "pick", on screen (the default): fit the wires, starting from a guessed
     hint where a camera has none. The fitted wires are drawn over a still
     picture with the 20, 6, 3 and 11 in colour; the arrows turn the colours
     here, and Save moves each turned camera's hint to its new 20 and refits
     that camera.
   - "dart", dart at D20, as in the terminal: orient_begin, one dart at D20,
     orient_finish, then calibrate with the board empty. */
const CALIBRATION_MODES = {
  pick: {
    lede: `OcheCore fits the wires on every camera. Then turn each camera's colours onto the board's numbers: ${MARKS_TEXT}.`,
    blocked: "Fit the wires and save the colours on every camera first."
  },
  dart: {
    lede: "Throw one dart at D20, like in the terminal: OcheCore finds the 20 on every camera, then fits the wires.",
    blocked: "Calibrate every camera first."
  }
};

/* The fitted wires, high-contrast, with the orientation marks turned by
   `offset` segments. Labels stay upright on rotated cameras. */
function drawCalibration(overlay, calibration, record, camera, offset) {
  const map = boardToImage(calibration);
  if (!map) return;
  const b = record.board;
  const radii = [b.inner_bull, b.outer_bull, b.treble_inner, b.treble_outer, b.double_inner, b.double_outer];
  const scale = Math.max(camera.width, camera.height) / 640;
  ORIENTATION_MARKS.forEach(({ number, color }) => {
    const index = SEGMENTS.indexOf(number) + offset;
    const start = -99 + index * 18;
    svgEl("polygon", {
      points: pointsAttr(sectorPoints(b.outer_bull, b.double_outer, start, start + 18).map(map)),
      fill: color,
      "fill-opacity": 0.55
    }, overlay);
  });
  drawWires(overlay, map, { stroke: "#000", "stroke-width": 3.4 * scale, opacity: 0.55 }, radii);
  drawWires(overlay, map, { stroke: "#fff", "stroke-width": 1.4 * scale, opacity: 1 }, radii);
  ORIENTATION_MARKS.forEach(({ number, color }) => {
    const [x, y] = map(polar(b.double_outer + 24, -90 + (SEGMENTS.indexOf(number) + offset) * 18));
    const group = svgEl("g", { transform: `translate(${round1(x)} ${round1(y)}) rotate(${-camera.rotation})` }, overlay);
    svgEl("circle", { r: 15 * scale, fill: color, stroke: "#fff", "stroke-width": 2.5 * scale }, group);
    const label = svgEl("text", {
      "text-anchor": "middle",
      "dominant-baseline": "central",
      fill: "#fff",
      "font-size": 15 * scale,
      "font-weight": 900,
      "font-family": "Arial Rounded MT Bold, Trebuchet MS, sans-serif"
    }, group);
    label.textContent = number;
  });
}

function initCalibrationPage() {
  const grid = document.getElementById("calibration-grid");
  const lede = document.getElementById("calibrate-lede");
  const modeButtons = [...document.querySelectorAll("[data-calibration-mode]")];
  const missions = {
    pick: document.getElementById("missions-pick"),
    dart: document.getElementById("missions-dart")
  };
  const buttons = {
    fit: document.getElementById("fit-wires"),
    save: document.getElementById("save-turns"),
    begin: document.getElementById("start-orientation"),
    find: document.getElementById("lock-d20"),
    calibrate: document.getElementById("calibrate-cameras")
  };
  const resetButton = document.getElementById("reset-calibration");
  const next = document.querySelector("[data-step-next]");
  const previews = new Map();
  const ui = {
    mode: storage.get("calibration-mode", "pick") === "dart" ? "dart" : "pick",
    /* The daemon keeps orient_begin's empty frames in memory only. */
    orienting: session.get("orienting", null),
    /* Start over: show the steps again; the saved calibration stays until a
       new one succeeds. */
    redo: false,
    /* orient_finish stored a 20 for every camera since the last calibration. */
    found: false,
    /* Unsaved turns of the colours per camera slot, in segments. */
    turns: {},
    /* The camera being measured, "all", or null. */
    busy: null
  };
  let renderedKey = "";

  const setOrienting = value => {
    ui.orienting = value;
    if (value) session.set("orienting", value);
    else session.remove("orienting");
  };
  const turned = () => Object.values(ui.turns).some(Boolean);

  /* Where the procedure is, from the daemon's state and the steps run here. */
  function progress(snapshot) {
    const cameras = assignedCameras(snapshot);
    const calibrated = cameras.length > 0 &&
      snapshot.config.cameras.every(camera => calibrationFor(snapshot.calibration, camera));
    if (ui.mode === "pick") {
      if (!calibrated || ui.redo) return { calibrated, stage: 0 };
      return { calibrated, stage: turned() ? 1 : 2 };
    }
    const oriented = camerasNeedingOrientation(snapshot).length === 0;
    if (calibrated && !ui.redo) return { calibrated, stage: 3 };
    if (ui.orienting === snapshot.session_id) return { calibrated, stage: 1 };
    return { calibrated, stage: (oriented && !ui.redo) || ui.found ? 2 : 0 };
  }

  function chip(snapshot, camera, state) {
    if (!camera.device) return ["No device", "wait"];
    if (ui.busy === "all" || ui.busy === camera.slot) return ["Measuring…", "busy"];
    const calibration = calibrationFor(snapshot.calibration, camera);
    if (calibration && !ui.redo) {
      if (ui.turns[camera.slot]) return ["Turned, not saved", "found"];
      return [`Wire fit ✓ ${calibration.wire_rms_mm.toFixed(2)} mm`, "done"];
    }
    if (state.stage === 1 && ui.mode === "dart") return ["Waiting for dart", "wait"];
    return ["Waiting", "wait"];
  }

  function preview(slot) {
    if (!previews.has(slot)) {
      const created = createCameraPreview(slot, true);
      previews.set(slot, created);
      cameraFeed.watch(created);
    }
    return previews.get(slot);
  }

  function drawOverlay(snapshot, camera) {
    const view = preview(camera.slot);
    view.overlay.replaceChildren();
    const calibration = calibrationFor(snapshot.calibration, camera);
    if (calibration && !ui.redo) {
      drawCalibration(view.overlay, calibration, snapshot.calibration, camera, ui.turns[camera.slot] || 0);
    }
  }

  const retakeAll = () => previews.forEach((_, slot) => cameraFeed.retake(slot));

  function cardMarkup(snapshot, camera, state) {
    const { slot } = camera;
    const count = snapshot.config.cameras.length;
    const canTurn = ui.mode === "pick" && state.stage > 0 && calibrationFor(snapshot.calibration, camera);
    const tools = `
      <span class="cal-status" data-cal-status></span>
      <button class="icon-btn" type="button" data-retake="${slot}" aria-label="Take a new picture with camera ${slot}" title="New picture">${icon("refresh")}</button>`;
    return `
      <section class="panel camera-card" aria-labelledby="camera-title-${slot}">
        ${cameraCardHead(slot, count, `<div class="camera-tool-buttons">${tools}</div>`)}
        <div data-preview-slot="${slot}"></div>
        ${canTurn ? `
          <div class="orient-controls">
            <span>Turn the colours</span>
            <div class="turn-buttons" role="group" aria-label="Turn the colours on camera ${slot}">
              <button type="button" data-turn="-1" data-camera="${slot}" aria-label="Turn anticlockwise" title="Turn anticlockwise">${icon("anticlockwise")}</button>
              <button type="button" data-turn="1" data-camera="${slot}" aria-label="Turn clockwise" title="Turn clockwise">${icon("clockwise")}</button>
            </div>
          </div>` : ""}
      </section>`;
  }

  function renderSteps(snapshot, state) {
    const live = snapshot.mode === "live";
    const running = snapshot.state === "running";
    lede.textContent = live
      ? CALIBRATION_MODES[ui.mode].lede
      : "Calibration needs cameras, but this daemon runs without them. Restart it with oche-core run (no --demo or --no-capture) to calibrate.";
    modeButtons.forEach(button => {
      button.setAttribute("aria-checked", String(button.dataset.calibrationMode === ui.mode));
      button.disabled = Boolean(ui.busy) || !live;
    });
    Object.entries(missions).forEach(([mode, list]) => {
      list.hidden = mode !== ui.mode;
      [...list.querySelectorAll(".mission")].forEach((mission, index) => {
        mission.classList.toggle("is-done", state.stage > index);
        mission.classList.toggle("is-active", state.stage === index);
      });
    });
    const blocked = !live || !running || Boolean(ui.busy);
    buttons.fit.disabled = blocked || state.stage !== 0;
    buttons.save.disabled = blocked || state.stage !== 1;
    buttons.begin.disabled = blocked || state.stage !== 0;
    buttons.find.disabled = blocked || state.stage !== 1;
    buttons.calibrate.disabled = blocked || state.stage !== 2;
    resetButton.disabled = Boolean(ui.busy) || !live;
    setStepStates(3, snapshot);
    setNextEnabled(next, !live || (state.calibrated && !ui.busy && !turned()), CALIBRATION_MODES[ui.mode].blocked);
  }

  function render({ snapshot }) {
    const state = progress(snapshot);
    const { cameras } = snapshot.config;
    const key = JSON.stringify([cameras, snapshot.calibration?.created_unix_ms, ui.mode, ui.redo, state.stage, ui.busy]);
    if (key !== renderedKey) {
      renderedKey = key;
      grid.style.setProperty("--cols", cameras.length);
      setContentWidth(cameras.length);
      grid.innerHTML = cameras.map(camera => cardMarkup(snapshot, camera, state)).join("");
      cameras.forEach(camera => {
        const view = preview(camera.slot);
        grid.querySelector(`[data-preview-slot="${camera.slot}"]`).replaceWith(view.element);
        view.layout(camera);
        drawOverlay(snapshot, camera);
      });
    }
    cameras.forEach((camera, index) => {
      const view = preview(camera.slot);
      const [problem, start] = previewProblem(snapshot, camera);
      view.say(problem, start);
      const [text, tone] = chip(snapshot, camera, state);
      const element = grid.querySelectorAll("[data-cal-status]")[index];
      if (element) {
        element.textContent = text;
        element.dataset.tone = tone;
      }
      const calibration = calibrationFor(snapshot.calibration, camera);
      view.label.textContent = calibration ? `${calibration.wire_points} wire points` : `${camera.height}p · ${camera.rotation}°`;
    });
    renderSteps(snapshot, state);
  }

  /* Runs one daemon step with every control locked, then shows new pictures. */
  async function step(busy, run) {
    ui.busy = busy;
    render(daemon);
    const result = await run();
    ui.busy = null;
    renderedKey = "";
    render(daemon);
    retakeAll();
    return result;
  }

  async function measure(camera, success) {
    const result = await step(camera ?? "all", () =>
      attempt(api.command("calibrate", camera ? { camera } : {}), success));
    if (result) {
      ui.redo = false;
      ui.found = false;
      renderedKey = "";
      render(daemon);
    }
    return result;
  }

  modeButtons.forEach(button => {
    button.addEventListener("click", () => {
      if (button.dataset.calibrationMode === ui.mode || ui.busy) return;
      ui.mode = button.dataset.calibrationMode;
      storage.set("calibration-mode", ui.mode);
      setOrienting(null);
      ui.found = false;
      ui.turns = {};
      render(daemon);
    });
  });

  /* A camera with neither a hint nor a calibration starts from a guess near
     the top of its picture; the colours show where that put the 20. */
  buttons.fit.addEventListener("click", async () => {
    const guesses = assignedCameras(daemon.snapshot)
      .filter(camera => camera.hint == null && !calibrationFor(daemon.snapshot.calibration, camera))
      .map(camera => [`cameras.${camera.slot}.hint`, `${Math.round(camera.width / 2)},${Math.round(camera.height * 0.08)}`]);
    if (guesses.length && !(await step("all", () => attempt(api.configure(guesses))))) return;
    ui.turns = {};
    await measure(null, `Wires fitted. Turn each camera until ${MARKS_TEXT}.`);
  });

  /* Moves each turned camera's hint to the segment now shown as the 20 and
     refits that camera against the board already measured. */
  buttons.save.addEventListener("click", async () => {
    for (const [slot, turn] of Object.entries(ui.turns).filter(([, turn]) => turn)) {
      const snapshot = daemon.snapshot;
      const camera = snapshot.config.cameras[slot - 1];
      const calibration = camera && calibrationFor(snapshot.calibration, camera);
      const hint = calibration && hintAt(calibration, turn * 18);
      if (!hint || hint[0] >= camera.width || hint[1] >= camera.height) {
        toast(`Camera ${slot}: that 20 is outside the picture.`);
        return;
      }
      const saved = await step(Number(slot), () => attempt(api.setConfig(`cameras.${slot}.hint`, hint.join(","))));
      if (!saved || !(await measure(Number(slot)))) return;
      delete ui.turns[slot];
    }
    renderedKey = "";
    render(daemon);
    toast("Saved. Every camera now uses the 20 you set.");
  });

  buttons.begin.addEventListener("click", async () => {
    const result = await step("all", () =>
      attempt(api.command("orient_begin"), "Now throw one dart at D20 and leave it in."));
    if (result) setOrienting(result.session_id);
    render(daemon);
  });

  buttons.find.addEventListener("click", () => step("all", async () => {
    try {
      await api.command("orient_finish");
      setOrienting(null);
      ui.found = true;
      toast("Found the 20 on every camera. Take the dart out, then calibrate.");
    } catch (error) {
      /* 409: the daemon restarted or never began; begin again. */
      if (error.status === 409) setOrienting(null);
      toast(error.message);
    }
  }));

  buttons.calibrate.addEventListener("click", () =>
    measure(null, `Calibrated. Check ${MARKS_TEXT} on every camera.`));

  grid.addEventListener("click", event => {
    const snapshot = daemon.snapshot;
    if (!snapshot || ui.busy) return;
    const retake = event.target.closest("[data-retake]");
    if (retake) {
      cameraFeed.retake(Number(retake.dataset.retake));
      return;
    }
    const turn = event.target.closest("[data-turn]");
    if (!turn) return;
    /* Only the drawing turns; Save sends it to the daemon. */
    const slot = Number(turn.dataset.camera);
    ui.turns[slot] = ((ui.turns[slot] || 0) + Number(turn.dataset.turn) * TURN_SEGMENTS + 30) % 20 - 10;
    if (!ui.turns[slot]) delete ui.turns[slot];
    drawOverlay(snapshot, snapshot.config.cameras[slot - 1]);
    render(daemon);
  });

  resetButton.addEventListener("click", () => {
    ui.redo = true;
    ui.found = false;
    ui.turns = {};
    setOrienting(null);
    renderedKey = "";
    render(daemon);
    toast("Starting over. The saved calibration stays until a new one succeeds.");
  });

  whenReady(snapshot => {
    if (!enterStep(3, snapshot)) return;
    daemon.subscribe(render);
  });
}

/* ---------- Play view: the strip and the board's darts ---------- */

const visitTotal = throws => throws.reduce((sum, dart) => sum + dart.score, 0);

/* The strip's words for the board's activity. */
function stripState(snapshot) {
  const activity = boardActivity(snapshot);
  if (activity.label === "Ready") return ["Throw", "ready"];
  if (activity.label === "Take out darts") return ["Take out", "takeout"];
  return [activity.label, { idle: "stopped", problem: "problem", attention: "takeout" }[activity.level] || "busy"];
}

/* Where a dart is drawn: its position (board mm, +y to the 20) in SVG
   coordinates, or the middle of its segment when only the label is known. */
const dartPoint = dart => dart.position ? [dart.position[0], -dart.position[1]] : labelPoint(dart.label);

/* Shared by the test step, the board view and debug; `interactive` boards
   take demo throws. Clicking a dart in the strip, then where it really is on
   the board, corrects it (correct_throw with that position). */
function createPlayView(interactive = false) {
  const board = document.getElementById("throw-board");
  let drawn = drawBoard(board, identity, { numberSize: 22 });
  /* A flattened camera picture shown instead of the drawn board, cut to the
     drawn board's outline. */
  const clip = svgEl("clipPath", { id: "photo-clip" }, svgEl("defs", {}, board));
  svgEl("circle", { r: SURROUND }, clip);
  const photo = svgEl("image", {
    x: -235, y: -235, width: 470, height: 470, hidden: "", "clip-path": "url(#photo-clip)"
  }, board);
  const hitLayer = svgEl("g", { "pointer-events": "none" }, board);
  const dartLayer = svgEl("g", { "pointer-events": "none" }, board);
  const slots = document.getElementById("dart-slots");
  const total = document.getElementById("visit-total");
  const boardState = document.getElementById("board-state");
  /* The dart being corrected (0-2), or null. */
  let correcting = null;

  function render(snapshot) {
    const { throws } = snapshot;
    if (useBoard(snapshot.calibration?.board)) {
      const fresh = drawBoard(board, identity, { numberSize: 22 });
      fresh.setAttribute("visibility", drawn.getAttribute("visibility") || "visible");
      drawn.replaceWith(fresh);
      drawn = fresh;
    }
    if (correcting !== null && !throws[correcting]) correcting = null;
    board.classList.toggle("is-correcting", correcting !== null);
    slots.innerHTML = [0, 1, 2].map(index => {
      const dart = throws[index];
      if (!dart) return '<span class="dart-slot">–</span>';
      const why = dart.uncertain ? `${dart.uncertainty || "Position uncertain"}. ` : "";
      const classes = `dart-slot is-filled${dart.uncertain ? " is-uncertain" : ""}${dart.corrected ? " is-corrected" : ""}${correcting === index ? " is-correcting" : ""}`;
      return `<button type="button" class="${classes}" data-correct="${index}" title="${escapeHtml(`${why}Click to correct dart ${index + 1}`)}" aria-pressed="${correcting === index}">${escapeHtml(dart.label)}${dart.corrected ? "*" : ""}</button>`;
    }).join("");
    total.textContent = visitTotal(throws);
    if (boardState) {
      const [text, tone] = stripState(snapshot);
      boardState.textContent = text;
      boardState.dataset.tone = tone;
      boardState.title = sentence(boardActivity(snapshot).detail);
    }
    board.classList.toggle("is-stopped", snapshot.state !== "running");
    board.classList.toggle("is-display", !(interactive && snapshot.capabilities.synthetic_input));

    hitLayer.replaceChildren();
    dartLayer.replaceChildren();
    throws.slice(0, 3).forEach((dart, index) => {
      const point = dartPoint(dart);
      if (!point) return;
      const area = hitArea(...point);
      if (area) svgEl("polygon", { points: pointsAttr(area), fill: COLORS.yellow, opacity: 0.85 }, hitLayer);
      const group = svgEl("g", { transform: `translate(${round1(point[0])} ${round1(point[1])})` }, dartLayer);
      // About the size of a dart's barrel, so it marks a spot within a ring.
      svgEl("circle", { r: 3.6, fill: DART_COLORS[index], stroke: COLORS.ink, "stroke-width": 1.4 }, group);
      const number = svgEl("text", {
        y: -9,
        "text-anchor": "middle",
        fill: "#fffdf7",
        "font-size": 11,
        "font-weight": 900,
        "font-family": "Arial Rounded MT Bold, Trebuchet MS, sans-serif",
        "paint-order": "stroke",
        stroke: COLORS.ink,
        "stroke-width": 3
      }, group);
      number.textContent = index + 1;
    });
  }

  /* Shows a picture of the board in board coordinates, or the drawn board
     again when `url` is null. Hit areas stay faint over a picture, where the
     real darts are visible. */
  function showPhoto(url) {
    if (url) photo.setAttribute("href", url);
    photo.toggleAttribute("hidden", !url);
    drawn.setAttribute("visibility", url ? "hidden" : "visible");
    hitLayer.setAttribute("opacity", url ? 0.45 : 1);
  }

  slots.addEventListener("click", event => {
    const button = event.target.closest("[data-correct]");
    if (!button || !daemon.snapshot) return;
    const index = Number(button.dataset.correct);
    correcting = correcting === index ? null : index;
    if (correcting !== null) toast(`Click where dart ${index + 1} really is on the board.`);
    render(daemon.snapshot);
  });

  /* While correcting, a click on the board places the dart instead of
     throwing one. */
  board.addEventListener("click", async event => {
    if (correcting === null || !daemon.snapshot) return;
    event.stopImmediatePropagation();
    const point = svgPoint(board, event);
    const position = [Math.round(point.x * 10) / 10, Math.round(-point.y * 10) / 10];
    const label = scoreAt(point.x, point.y);
    const { revision, session_id: session } = daemon.snapshot;
    const dart = correcting + 1;
    correcting = null;
    await attempt(api.command("correct_throw", {
      dart,
      label,
      position,
      expected_revision: revision,
      expected_session_id: session
    }), `Dart ${dart} corrected to ${label}.`);
    render(daemon.snapshot);
  }, true);

  return { board, render, showPhoto };
}

/* Output size of a flattened picture: 470 mm of board at this many px. */
const FLAT_SIZE = 600;

/* Warps one camera's frame onto the board plane, face-on with the 20 up,
   covering the drawn board's -235..235 mm. Resolves to a PNG blob. */
async function flattenFrame(blob, calibration) {
  const toImage = invert3(calibration.to_board);
  if (!toImage) return null;
  const bitmap = await createImageBitmap(blob);
  const source = new OffscreenCanvas(bitmap.width, bitmap.height);
  const sourceContext = source.getContext("2d");
  sourceContext.drawImage(bitmap, 0, 0);
  const pixels = sourceContext.getImageData(0, 0, bitmap.width, bitmap.height).data;
  const output = new OffscreenCanvas(FLAT_SIZE, FLAT_SIZE);
  const outputContext = output.getContext("2d");
  const flat = outputContext.createImageData(FLAT_SIZE, FLAT_SIZE);
  const mmPerPx = 470 / FLAT_SIZE;
  /* A homography's sign is arbitrary: points in front of the camera share the
     board centre's w sign, whichever it is (as in oche-calibration). */
  const centreW = toImage[2][2];
  for (let row = 0; row < FLAT_SIZE; row += 1) {
    /* Board y points to the 20; image rows point down. */
    const y = 235 - (row + 0.5) * mmPerPx;
    for (let column = 0; column < FLAT_SIZE; column += 1) {
      const x = (column + 0.5) * mmPerPx - 235;
      const w = toImage[2][0] * x + toImage[2][1] * y + toImage[2][2];
      if (w * centreW <= 0) continue;
      const u = Math.round((toImage[0][0] * x + toImage[0][1] * y + toImage[0][2]) / w);
      const v = Math.round((toImage[1][0] * x + toImage[1][1] * y + toImage[1][2]) / w);
      if (u < 0 || v < 0 || u >= bitmap.width || v >= bitmap.height) continue;
      const from = (v * bitmap.width + u) * 4;
      const to = (row * FLAT_SIZE + column) * 4;
      flat.data[to] = pixels[from];
      flat.data[to + 1] = pixels[from + 1];
      flat.data[to + 2] = pixels[from + 2];
      flat.data[to + 3] = 255;
    }
  }
  outputContext.putImageData(flat, 0, 0);
  return output.convertToBlob({ type: "image/png" });
}

/* The Live board's view: the drawn board, or one calibrated camera's
   flattened picture. A picture is fetched only when the still scene changes
   (the board empties, or a dart arrives), not streamed, to spare the
   daemon's CPU and the network. */
function createBoardViewSwitch(view) {
  const container = document.getElementById("board-view");
  let choice = storage.get("board-view", "drawn");
  let url = null;
  let busy = false;
  let shownKey = "";
  /* The scene the shown picture was taken in. */
  let pictureKey = "";

  /* The still scene worth a picture, or null while it is moving or partly
     emptied: an empty board, or the darts on it. */
  const sceneKey = snapshot => snapshot.scene === "empty" || snapshot.scene === "darts_present"
    ? JSON.stringify([choice, snapshot.session_id, snapshot.calibration?.created_unix_ms,
      snapshot.scene, snapshot.physical_darts, snapshot.throws.length])
    : null;

  const cameras = snapshot => snapshot.mode !== "live" ? [] : snapshot.config.cameras
    .filter(camera => calibrationFor(snapshot.calibration, camera) && isStreaming(snapshot, camera.slot));
  /* Why a camera cannot be shown, or "" when it can. */
  const unavailable = (snapshot, camera) =>
    snapshot.mode !== "live" ? "Needs the live daemon with cameras (not --demo)"
      : !camera.device ? "No device assigned"
      : !calibrationFor(snapshot.calibration, camera) ? "Not calibrated with its current settings"
      : !isStreaming(snapshot, camera.slot) ? "Not streaming: start the board"
      : "";

  async function refresh() {
    const snapshot = daemon.snapshot;
    const camera = snapshot && cameras(snapshot).find(candidate => String(candidate.slot) === choice);
    if (!camera) {
      view.showPhoto(null);
      pictureKey = "";
      return;
    }
    const key = sceneKey(snapshot);
    if (!key || key === pictureKey || busy || document.hidden) return;
    busy = true;
    try {
      const response = await fetch(api.snapshotUrl(camera.slot), { cache: "no-store" });
      const flat = response.ok && await flattenFrame(await response.blob(), calibrationFor(snapshot.calibration, camera));
      if (flat && choice === String(camera.slot)) {
        const next = URL.createObjectURL(flat);
        /* Decoded before the swap, so the old picture stays until the new
           one can be drawn instead of flashing blank. */
        const decoded = new Image();
        decoded.src = next;
        await decoded.decode();
        view.showPhoto(next);
        if (url) URL.revokeObjectURL(url);
        url = next;
        pictureKey = key;
      }
    } catch {
      /* The next daemon update tries again. */
    } finally {
      busy = false;
    }
    /* The scene or the choice moved on while this picture loaded. */
    if (daemon.snapshot && sceneKey(daemon.snapshot) !== key) refresh();
  }

  function render(snapshot) {
    const available = cameras(snapshot);
    if (choice !== "drawn" && !available.some(camera => String(camera.slot) === choice)) {
      view.showPhoto(null);
    }
    const items = [["drawn", statusIcon("board"), ""], ...snapshot.config.cameras.map(camera =>
      [String(camera.slot), `${statusIcon("camera")}${camera.slot}`, unavailable(snapshot, camera)])];
    const key = JSON.stringify([items, choice, available.length]);
    if (key === shownKey) return;
    shownKey = key;
    container.hidden = false;
    const shown = available.some(camera => String(camera.slot) === choice) ? choice : "drawn";
    container.innerHTML = items.map(([value, label, why]) =>
      `<button type="button" role="radio" aria-checked="${value === shown}" data-view="${value}" aria-label="${value === "drawn" ? "Drawn board" : `Camera ${value}`}"${why ? ` disabled title="${escapeHtml(`Camera ${value}: ${why}`)}"` : ` title="${value === "drawn" ? "The drawn board" : `Camera ${value}'s picture, flattened with the 20 up`}"`}>${label}</button>`).join("");
  }

  /* Collapsed to the view shown; hovering lists the others. A tap on the shown
     view lists them too, for touch screens. */
  container.addEventListener("click", event => {
    const button = event.target.closest("[data-view]");
    if (!button || button.disabled) return;
    if (button.getAttribute("aria-checked") === "true") {
      container.classList.toggle("is-open");
      return;
    }
    container.classList.remove("is-open");
    choice = button.dataset.view;
    storage.set("board-view", choice);
    shownKey = "";
    /* The previous camera's picture is not this one's: the drawn board shows
       until this camera's picture arrives. */
    view.showPhoto(null);
    pictureKey = "";
    if (daemon.snapshot) render(daemon.snapshot);
    refresh();
  });
  document.addEventListener("click", event => {
    if (!container.contains(event.target)) container.classList.remove("is-open");
  });
  return { render, refresh };
}

/* In demo mode, clicking the board or a quick-throw button injects a throw;
   a click also gives the dart its position, so it is drawn where clicked. */
function bindDemoThrows(view, onThrow = () => {}) {
  const send = async (label, position) => {
    if (!daemon.snapshot?.capabilities.synthetic_input) return;
    const args = position ? { label, position } : { label };
    if (await attempt(api.command("demo_throw", args))) onThrow();
  };
  view.board.addEventListener("click", event => {
    const point = svgPoint(view.board, event);
    /* Beyond 500 mm is no position the daemon takes; it is a miss anyway. */
    const position = Math.hypot(point.x, point.y) <= 500
      ? [Math.round(point.x * 10) / 10, Math.round(-point.y * 10) / 10]
      : null;
    send(scoreAt(point.x, point.y), position);
  });
  document.querySelectorAll("[data-throw]").forEach(button => {
    button.addEventListener("click", () => send(button.dataset.throw));
  });
}

/* Start/Stop buttons and demo-only controls follow the daemon. */
function renderControls(snapshot) {
  const running = snapshot.state === "running";
  document.querySelectorAll('[data-command="start"][data-when-stopped]').forEach(button => {
    button.hidden = running;
  });
  document.querySelectorAll('[data-command="stop"][data-when-running]').forEach(button => {
    button.hidden = !running;
  });
  document.querySelectorAll("[data-demo-only]").forEach(element => {
    element.hidden = !snapshot.capabilities.synthetic_input;
  });
}

/* ---------- Step 4: test ---------- */

function initTestPage() {
  const finish = document.querySelector("[data-step-next]");
  const lede = document.querySelector(".page-head .lede");
  whenReady(snapshot => {
    if (!enterStep(4, snapshot)) return;
    const view = createPlayView(true);
    bindDemoThrows(view);
    /* Visits already on the board when this page opened do not count. */
    const seen = new Set(daemon.events.map(event => `${event.session_id}:${event.sequence}`));
    daemon.subscribe(({ snapshot: current, events }) => {
      if (events.some(event => event.kind === "throw_accepted" && !seen.has(`${event.session_id}:${event.sequence}`))) {
        storage.set("tested", testedKey(current));
      }
      view.render(current);
      renderControls(current);
      lede.textContent = current.capabilities.synthetic_input
        ? "Demo mode: click the board or a demo button to throw. After three darts, take them out."
        : "Throw three darts. When it says Take out, pull them out; Reset clears the visit early.";
      setStepStates(4, current);
      setNextEnabled(finish, setupProgress(current)[3], "Throw at least one dart first.");
    });
  });
}

/* ---------- Settings: the terminal UI's settings, in the browser ---------- */

/* The configuration as the keys `PUT /api/v1/config` accepts. */
function configEntries(config) {
  const entries = [
    ["board.id", config.board.id],
    ["board.name", config.board.name],
    ["server.bind", config.server.bind],
    ["server.port", config.server.port],
    ["detection.speed", config.detection.speed],
    ["detection.standby_seconds", config.detection.standby_seconds],
    ["debug.save_corrections", Boolean(config.debug?.save_corrections)],
    ["debug.keep_corrections", config.debug?.keep_corrections ?? 200],
    ["cameras.count", config.cameras.length]
  ];
  config.cameras.forEach(camera => {
    const prefix = `cameras.${camera.slot}`;
    entries.push(
      [`${prefix}.device`, camera.device],
      [`${prefix}.resolution`, `${camera.width}x${camera.height}`],
      [`${prefix}.fps`, camera.fps],
      [`${prefix}.rotation`, camera.rotation],
      [`${prefix}.exposure`, camera.exposure ?? "auto"],
      [`${prefix}.hint`, camera.hint ? camera.hint.join(",") : ""]
    );
  });
  return entries;
}

/* Config changes that turn the current configuration into `imported` (the
   shape of `oche-core config export --json`). The board keeps its identity;
   the listen address is skipped where a container's port mapping decides it.
   Devices are cleared before they are reassigned, so moving one between
   slots never holds it twice. */
function importChanges(snapshot, imported) {
  if (!imported || typeof imported !== "object" || !imported.board || !imported.server ||
      !imported.detection || !Array.isArray(imported.cameras) ||
      imported.cameras.length < 1 || imported.cameras.length > MAX_CAMERAS) {
    throw new Error("This is not an OcheCore settings file.");
  }
  const target = configEntries({ debug: {}, ...imported });
  const current = new Map(configEntries(snapshot.config));
  const skip = key => key === "board.id" || key === "cameras.count" ||
    (!imported.debug && key.startsWith("debug.")) ||
    (snapshot.listener_fixed && key.startsWith("server.")) || /^cameras\.\d+\.device$/.test(key);
  const changes = [];
  if (imported.cameras.length !== snapshot.config.cameras.length) {
    changes.push(["cameras.count", imported.cameras.length]);
  }
  const devices = target.filter(([key]) => /^cameras\.\d+\.device$/.test(key));
  devices.forEach(([key, value]) => {
    if (current.has(key) && String(current.get(key)) !== String(value)) changes.push([key, ""]);
  });
  devices.forEach(([key, value]) => {
    if (String(current.get(key) ?? "") !== String(value) || changes.some(([k]) => k === key)) {
      changes.push([key, value]);
    }
  });
  target.forEach(([key, value]) => {
    if (!skip(key) && String(current.get(key) ?? "") !== String(value)) changes.push([key, value]);
  });
  return changes;
}


/* Frames analysed per second per camera (Speed::analysis_hz in oche-domain). */
const ANALYSIS_HZ = { very_low: 3, low: 5, default: 10, high: 15, very_high: 30 };
/* Seconds of stillness before a dart is scored, like Speed::still_seconds. */
const STILL_SECONDS = { very_low: 1, low: 0.8, default: 0.5, high: 0.4, very_high: 0.3 };

/* What detection is doing, like the terminal UI's detection line. */
function detectionSummary(snapshot) {
  if (snapshot.mode === "demo") return "Demo mode: throws are injected from the Debug page.";
  if (snapshot.mode === "control_only") return "Off: this daemon runs without cameras (--no-capture).";
  if (snapshot.state !== "running") return "Cameras are off. Start the board on the Live page.";
  if (!snapshot.capabilities.calibration) return "Needs calibration: empty the board, then calibrate.";
  if (!snapshot.capabilities.vision) return "Needs at least two calibrated cameras to score.";
  const speed = snapshot.config.detection.speed;
  return `Live · ${ANALYSIS_HZ[speed]} analyses per second per camera; a dart is scored ${STILL_SECONDS[speed]} s after it stops moving.`;
}

/* One camera as the terminal UI lists it: device, state and calibration. */
function cameraSummary(snapshot, camera, inventory) {
  const device = inventory.devices.find(candidate => deviceMatches(candidate, camera.device));
  const name = device ? deviceLabel(device) : camera.device || "No device";
  const status = cameraStatus(snapshot, camera.slot);
  const state = {
    streaming: () => `streaming ${status.fps.toFixed(1)} fps`,
    starting: () => "starting",
    error: () => `error: ${status.error || "unknown"}`
  }[status?.state]?.() ?? "off";
  const calibration = calibrationFor(snapshot.calibration, camera);
  return {
    name,
    detail: `${state} · ${calibration ? `calibrated, wire fit ${calibration.wire_rms_mm.toFixed(2)} mm` : "not calibrated"}`,
    problem: snapshot.mode === "live" && (status?.state === "error" || !camera.device)
  };
}

function initBackup() {
  const exportButton = document.getElementById("export-settings");
  const importInput = document.getElementById("import-settings");
  exportButton.addEventListener("click", () => {
    const snapshot = daemon.snapshot;
    if (!snapshot) return;
    const blob = new Blob([JSON.stringify(snapshot.config, null, 2)], { type: "application/json" });
    const link = document.createElement("a");
    link.href = URL.createObjectURL(blob);
    link.download = `ochecore-${snapshot.config.board.name.replace(/[^\w-]+/g, "-").toLowerCase()}-settings.json`;
    link.click();
    window.setTimeout(() => URL.revokeObjectURL(link.href), 1000);
  });
  importInput.addEventListener("change", async () => {
    const [file] = importInput.files;
    importInput.value = "";
    if (!file || !daemon.snapshot) return;
    let changes;
    try {
      changes = importChanges(daemon.snapshot, JSON.parse(await file.text()));
    } catch (error) {
      toast(error instanceof SyntaxError ? "This file is not JSON." : error.message);
      return;
    }
    if (!changes.length) {
      toast("These settings are already in use.");
      return;
    }
    const list = changes.map(([key, value]) => `${key} = ${value === "" ? "(cleared)" : value}`).join("\n");
    if (!window.confirm(`Apply ${changes.length} changes from ${file.name}?\n\n${list}`)) return;
    await attempt(api.configure(changes), `Imported ${file.name}.`);
  });
}

/* The detector's accuracy, as `Accuracy::summary` words it in the terminal:
   right darts out of all it reported, halves rounded up. */
function accuracyPercent({ darts = 0, corrected = 0, uncertain = 0 } = {}) {
  if (!darts) return null;
  return Math.round(Math.max(0, darts - corrected - uncertain) * 100 / darts);
}

function accuracySummary(accuracy = {}) {
  const { darts = 0, corrected = 0, uncertain = 0 } = accuracy;
  const percent = accuracyPercent(accuracy);
  if (percent === null) return "No detected darts yet";
  const right = Math.max(0, darts - corrected - uncertain);
  return `${percent}% · ${right} of ${darts} right · ${corrected} corrected · ${uncertain} uncertain`;
}

function initSettingsPage() {
  initBackup();
  const accuracy = document.getElementById("accuracy-summary");
  const resetAccuracy = document.getElementById("reset-accuracy");
  resetAccuracy.addEventListener("click", async () => {
    if (!window.confirm("Start the accuracy count again from zero? This cannot be undone.")) return;
    resetAccuracy.disabled = true;
    await attempt(api.command("reset_accuracy"), "Accuracy count reset.");
    resetAccuracy.disabled = false;
  });
  const all = boardInputs();
  const forms = [
    [document.getElementById("board-form"), { "board.name": all["board.name"] }],
    [document.getElementById("network-form"), { "server.bind": all["server.bind"], "server.port": all["server.port"] }]
  ];
  const network = forms[1][1];
  const networkSave = document.querySelector("#network-form [type=submit]");
  const note = document.getElementById("listen-note");
  const defaultNote = note.textContent;
  const list = document.getElementById("camera-list");
  const summary = document.getElementById("detection-summary");
  const saveCorrections = document.getElementById("debug-save-corrections");
  const keepCorrections = document.getElementById("debug-keep-corrections");
  const settings = document.getElementById("detection-settings");
  let inventory = { devices: [], warnings: [] };
  let settingsKey = "";
  let saving = false;

  function render({ snapshot }) {
    list.innerHTML = snapshot.config.cameras.map(camera => {
      const { name, detail, problem } = cameraSummary(snapshot, camera, inventory);
      return `
        <li${problem ? ' class="is-problem"' : ""}>
          <span class="cam-number">${String(camera.slot).padStart(2, "0")}</span>
          <div>
            <strong>${escapeHtml(name)}</strong>
            <small>${escapeHtml(detail)}</small>
          </div>
        </li>`;
    }).join("");
    summary.textContent = detectionSummary(snapshot);
    const since = snapshot.accuracy?.since_unix_ms;
    accuracy.textContent = accuracySummary(snapshot.accuracy) +
      (since ? ` · since ${new Date(since).toLocaleDateString([], { day: "numeric", month: "short", year: "numeric" })}` : "");
    if (document.activeElement !== saveCorrections) {
      saveCorrections.value = String(Boolean(snapshot.config.debug?.save_corrections));
    }
    if (document.activeElement !== keepCorrections) {
      keepCorrections.value = String(snapshot.config.debug?.keep_corrections ?? 200);
    }
    renderListener(snapshot, network, note, defaultNote);
    networkSave.disabled = snapshot.listener_fixed;
    /* Leave a list alone while it is being used. */
    const key = JSON.stringify([snapshot.config.cameras, snapshot.config.detection, inventory.devices.length]);
    if (key !== settingsKey && !settings.contains(document.activeElement)) {
      settingsKey = key;
      renderDetectionSettings(snapshot, inventory);
    }
  }

  forms.forEach(([form, inputs]) => {
    form.addEventListener("submit", async event => {
      event.preventDefault();
      if (saving || !daemon.snapshot) return;
      saving = true;
      await saveBoardInputs(inputs);
      saving = false;
    });
    form.addEventListener("input", event => setFieldError(event.target, ""));
  });

  [[saveCorrections, "debug.save_corrections"], [keepCorrections, "debug.keep_corrections"]].forEach(([select, key]) => {
    select.addEventListener("change", async () => {
      await attempt(api.setConfig(key, select.value), "Saved.");
      select.blur();
      render(daemon);
    });
  });

  settings.addEventListener("change", async event => {
    if (!daemon.snapshot) return;
    await attempt(api.configure(detectionChanges(daemon.snapshot, event.target.id, event.target.value)), "Saved.");
    settingsKey = "";
    event.target.blur();
    render(daemon);
  });

  whenReady(async snapshot => {
    fillBoardInputs(all, snapshot.config);
    daemon.subscribe(render);
    try {
      inventory = await api.availableCameras();
    } catch {
      /* Devices show by path until discovery answers. */
    }
    settingsKey = "";
    render(daemon);
  });
}

/* Setup pages have no section links; once the board is set up, an Exit link
   leads back to Settings. */
function initSetupExit() {
  const exit = document.querySelector("[data-exit-setup]");
  if (exit) daemon.subscribe(({ snapshot }) => {
    exit.hidden = !isSetUp(snapshot);
  });
}

/* ---------- Board view: the default page once the board is set up ---------- */

const timeOf = event => new Date(event.unix_time_ms).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" });

/* Game events as players see them, from an event and the snapshot before it;
   service and debug events stay in the debug log. */
const GAME_EVENTS = {
  throw_accepted: ({ snapshot }) => {
    const dart = snapshot.throws[snapshot.throws.length - 1];
    return [`Dart ${snapshot.throws.length}`, dart?.label ?? "", dart?.score ?? ""];
  },
  throw_uncertain: ({ snapshot }) => [`Dart ${snapshot.throws.length}`, "Uncertain, correct it", 0],
  throw_corrected: ({ snapshot }, before) => {
    const index = snapshot.throws.findIndex((dart, i) => dart.label !== before?.throws[i]?.label);
    const dart = snapshot.throws[index];
    return [index >= 0 ? `Dart ${index + 1} corrected` : "Dart corrected", dart?.label ?? "", dart?.score ?? ""];
  },
  takeout_completed: (event, before) => ["Darts out", "Visit", before ? visitTotal(before.throws) : ""],
  partial_takeout: () => ["Taking darts out", "", ""],
  visit_reset: () => ["Visit reset", "", ""],
  arrival_ignored: () => ["Extra dart ignored", "Take out first", ""],
  started: () => ["Board started", "", ""],
  stopped: () => ["Board stopped", "", ""],
  standby: () => ["Standby", "Cameras closed", ""],
  board_ready: () => ["Board ready", "", ""],
  calibration_updated: () => ["Cameras recalibrated", "", ""]
};

/* Hiding the game events gives the board the whole width; each browser
   remembers its choice. */
/* The events panel collapses to a button in the top right corner, which
   expands it again; the panel's own Hide button collapses it. */
function initEventsToggle() {
  const layout = document.querySelector(".home-layout");
  const panel = document.getElementById("event-panel");
  const show = document.getElementById("events-show");
  const hide = document.getElementById("events-hide");
  const apply = hidden => {
    panel.hidden = hidden;
    show.hidden = !hidden;
    layout.classList.toggle("is-events-hidden", hidden);
  };
  const choose = hidden => {
    storage.set("events-hidden", hidden);
    apply(hidden);
    (hidden ? show : hide).focus();
  };
  apply(storage.get("events-hidden", false) === true);
  show.addEventListener("click", () => choose(false));
  hide.addEventListener("click", () => choose(true));
}

function initHomePage() {
  const feed = document.getElementById("event-feed");
  const empty = document.getElementById("event-empty");
  initEventsToggle();
  whenReady(snapshot => {
    if (!isSetUp(snapshot)) {
      window.location.replace(`setup/${STEP_PAGES[firstOpenStep(snapshot) - 1]}`);
      return;
    }
    document.body.classList.add("is-ready");
    const view = createPlayView();
    const boardView = createBoardViewSwitch(view);
    const accuracy = document.getElementById("home-accuracy");
    daemon.subscribe(({ snapshot: current, events }) => {
      const percent = accuracyPercent(current.accuracy);
      accuracy.textContent = percent === null ? "Accuracy —" : `Accuracy ${percent}%`;
      accuracy.title = `${accuracySummary(current.accuracy)}. Reset it in Settings.`;
      accuracy.hidden = false;
      view.render(current);
      boardView.render(current);
      /* A new picture only when the still scene changes. */
      boardView.refresh();
      renderControls(current);
      const items = events.map((event, index) => [event, events[index - 1]?.snapshot])
        .filter(([event]) => GAME_EVENTS[event.kind])
        .slice(-40)
        .reverse();
      feed.innerHTML = items.map(([event, before]) => {
        const [title, detail, value] = GAME_EVENTS[event.kind](event, before);
        return `
          <li class="event-item" data-kind="${escapeHtml(event.kind)}">
            <time>${timeOf(event)}</time>
            <span class="event-title">${escapeHtml(title)}</span>
            ${detail ? `<span class="event-detail">${escapeHtml(detail)}</span>` : ""}
            <strong class="event-value">${escapeHtml(value)}</strong>
          </li>`;
      }).join("");
      empty.hidden = items.length > 0;
    });
  });
}

document.addEventListener("DOMContentLoaded", () => {
  daemon.subscribe(renderStatusBar);
  renderStatusBar(daemon);
  const page = document.body.dataset.page;
  if (page === "home") initHomePage();
  if (page === "board") initBoardPage();
  if (page === "cameras") initCamerasPage();
  if (page === "calibrate") initCalibrationPage();
  if (page === "test") initTestPage();
  if (page === "settings") initSettingsPage();
  if (["board", "cameras", "calibrate", "test"].includes(page)) initSetupExit();
  daemon.start();
});
